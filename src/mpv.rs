//! Launches mpv and controls it over its JSON IPC socket.

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum MpvEvent {
    /// The `pause` property changed (by us or by the user).
    Pause(bool),
    /// Playback resumed after a seek or file load.
    PlaybackRestart,
    /// mpv quit (window closed).
    Exited,
}

/// PID of the running player, so the app can close it on quit
/// (tokio's kill_on_drop doesn't run when the GUI exits the process).
static PLAYER_PID: AtomicU32 = AtomicU32::new(0);

/// Close the player window, if one is open. Safe to call from any thread.
pub fn kill_player() {
    let pid = PLAYER_PID.swap(0, Ordering::SeqCst);
    if pid != 0 {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    }
}

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Extra mpv arguments for every launch. Tests use this to run headless.
static EXTRA_ARGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

pub fn set_extra_args(args: Vec<String>) {
    let _ = EXTRA_ARGS.set(args);
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

pub struct Mpv {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    pid: u32,
    socket: PathBuf,
    _child: Child,
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // kill_on_drop closes the process; forget its PID if it's still ours.
        let _ = PLAYER_PID.compare_exchange(self.pid, 0, Ordering::SeqCst, Ordering::SeqCst);
        // A killed mpv can't remove its own socket file.
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Find a program in Homebrew's folders or on PATH.
/// Finder-launched apps don't get the shell's PATH, so check Homebrew first.
fn find_program(name: &str) -> Option<PathBuf> {
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let p = Path::new(dir).join(name);
        if p.exists() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.exists())
}

/// Is mpv installed?
pub fn is_installed() -> bool {
    find_program("mpv").is_some()
}

/// Homebrew, if installed.
pub fn find_brew() -> Option<PathBuf> {
    find_program("brew")
}

/// Run `brew install mpv` (blocking; takes a minute or two).
pub fn install_with_brew() -> Result<()> {
    let brew = find_brew().ok_or_else(|| anyhow!("Homebrew is not installed"))?;
    let out = std::process::Command::new(brew)
        .args(["install", "mpv"])
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .output()
        .context("couldn't run brew")?;
    if out.status.success() && is_installed() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        bail!("brew install mpv failed: {last}")
    }
}

fn find_mpv() -> Result<PathBuf> {
    find_program("mpv").ok_or_else(|| anyhow!("mpv is not installed. Run: brew install mpv"))
}

impl Mpv {
    /// Start mpv paused at `start` seconds.
    pub async fn launch(
        file: &Path,
        start: f64,
        events: mpsc::UnboundedSender<MpvEvent>,
    ) -> Result<Self> {
        let bin = find_mpv()?;
        // Unique per player, so we never touch someone's own mpv session.
        let n = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let socket = std::env::temp_dir()
            .join(format!("watchsync-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);

        let child = Command::new(bin)
            .arg(format!("--input-ipc-server={}", socket.display()))
            .arg("--pause")
            .arg(format!("--start={start:.3}"))
            .arg("--keep-open=yes")
            .arg("--force-window=yes")
            .arg("--osd-bar=yes")
            .arg("--sub-auto=fuzzy")
            .arg("--hr-seek=yes")
            .arg("--title=WatchSync — ${filename}")
            .args(EXTRA_ARGS.get().into_iter().flatten())
            .arg("--")
            .arg(file)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start mpv")?;
        let pid = child.id().unwrap_or(0);
        PLAYER_PID.store(pid, Ordering::SeqCst);

        // Wait for mpv to create its socket.
        let mut child = child;
        let mut stream = None;
        for _ in 0..100 {
            if let Ok(s) = UnixStream::connect(&socket).await {
                stream = Some(s);
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                bail!(
                    "The video player (mpv) quit right away ({status}). \
                     Try reinstalling it: brew reinstall mpv"
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let stream = stream.ok_or_else(|| anyhow!("mpv did not open its control socket"))?;
        let (read_half, mut write_half) = stream.into_split();

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if write_half.write_all(line.as_bytes()).await.is_err()
                    || write_half.write_all(b"\n").await.is_err()
                {
                    break;
                }
            }
        });

        let pending: Pending = Arc::default();
        let pending_r = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read_half).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(event) = v.get("event").and_then(Value::as_str) {
                    let ev = match event {
                        "property-change" if v["name"] == "pause" => {
                            v["data"].as_bool().map(MpvEvent::Pause)
                        }
                        "playback-restart" => Some(MpvEvent::PlaybackRestart),
                        _ => None,
                    };
                    if let Some(ev) = ev {
                        let _ = events.send(ev);
                    }
                } else if let Some(id) = v.get("request_id").and_then(Value::as_u64) {
                    let reply = if v["error"] == "success" {
                        Ok(v.get("data").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(anyhow!("mpv: {}", v["error"]))
                    };
                    if let Some(waiter) = pending_r.lock().unwrap().remove(&id) {
                        let _ = waiter.send(reply);
                    }
                }
            }
            let _ = events.send(MpvEvent::Exited);
        });

        let mpv = Self {
            tx,
            pending,
            next_id: AtomicU64::new(1),
            pid,
            socket: socket.clone(),
            _child: child,
        };
        mpv.command(json!(["observe_property", 1, "pause"])).await?;
        Ok(mpv)
    }

    /// The IPC socket path (lets tests act like a user pressing keys).
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    async fn command(&self, args: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({ "command": args, "request_id": id });
        self.tx
            .send(msg.to_string())
            .map_err(|_| anyhow!("mpv is not running"))?;
        match tokio::time::timeout(Duration::from_secs(2), rx).await {
            Ok(Ok(reply)) => reply,
            _ => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("mpv did not reply"))
            }
        }
    }

    pub async fn time_pos(&self) -> Result<f64> {
        self.command(json!(["get_property", "time-pos"]))
            .await?
            .as_f64()
            .ok_or_else(|| anyhow!("time-pos unavailable"))
    }

    pub async fn duration(&self) -> Result<f64> {
        self.command(json!(["get_property", "duration"]))
            .await?
            .as_f64()
            .ok_or_else(|| anyhow!("duration unavailable"))
    }

    pub async fn set_pause(&self, paused: bool) -> Result<()> {
        self.command(json!(["set_property", "pause", paused])).await?;
        Ok(())
    }

    pub async fn set_speed(&self, speed: f64) -> Result<()> {
        self.command(json!(["set_property", "speed", speed])).await?;
        Ok(())
    }

    pub async fn seek(&self, position: f64) -> Result<()> {
        self.command(json!(["seek", position.max(0.0), "absolute+exact"]))
            .await?;
        Ok(())
    }

    pub async fn show_text(&self, text: &str) {
        let _ = self.command(json!(["show-text", text, 2500])).await;
    }
}
