//! Shared helpers: a generated test clip, headless mpv, raw IPC access.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use watchsync::engine::{self, Options, Snapshot, Timing, UiCmd};

pub const CLIP_SECS: f64 = 300.0;

/// A 5-minute 24fps test clip with audio, generated once with ffmpeg.
pub fn clip() -> PathBuf {
    static CLIP: OnceLock<PathBuf> = OnceLock::new();
    CLIP.get_or_init(|| {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clip.mkv");
        if !path.exists() {
            let tmp = path.with_extension("tmp.mkv");
            let status = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i", "testsrc=duration=300:size=320x180:rate=24"])
                .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=300"])
                .args(["-c:v", "libx264", "-preset", "ultrafast", "-g", "48"])
                .args(["-c:a", "aac", "-shortest"])
                .arg(&tmp)
                .status()
                .expect("ffmpeg is needed for tests: brew install ffmpeg");
            assert!(status.success(), "ffmpeg failed");
            std::fs::rename(&tmp, &path).unwrap();
        }
        path
    })
    .clone()
}

/// Run every mpv in this test process without windows or sound.
pub fn headless() {
    watchsync::mpv::set_extra_args(
        ["--vo=null", "--ao=null", "--no-force-window", "--no-config"]
            .map(String::from)
            .to_vec(),
    );
}

/// Talk to an mpv socket directly, the way a user pressing keys would.
pub fn ipc(socket: &Path, command: Value) -> Value {
    let mut stream = UnixStream::connect(socket).expect("connect to mpv");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let msg = json!({ "command": command, "request_id": 4242 });
    writeln!(stream, "{msg}").unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        reader.read_line(&mut line).expect("mpv reply");
        let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        if v["request_id"] == 4242 {
            return v["data"].clone();
        }
    }
}

pub fn time_pos(socket: &Path) -> f64 {
    ipc(socket, json!(["get_property", "time-pos"]))
        .as_f64()
        .expect("time-pos")
}

pub fn paused(socket: &Path) -> bool {
    ipc(socket, json!(["get_property", "pause"]))
        .as_bool()
        .expect("pause")
}

/// Poll `check` until it returns true or `timeout` passes.
pub fn eventually(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// One person in a test room: an engine running exactly as in the app.
pub struct Peer {
    pub cmd: async_channel::Sender<UiCmd>,
    pub snaps: async_channel::Receiver<Snapshot>,
    pub last: Snapshot,
}

impl Peer {
    /// Start an engine on its own runtime thread, exactly like the app does.
    pub fn start(name: &str) -> Self {
        Self::start_with(name, Timing::default())
    }


    /// Same, with custom connection timings (tests use short ones).
    pub fn start_with(name: &str, timing: Timing) -> Self {
        Self::start_opts(name, Options { timing, downloads: test_downloads(name), ..Options::default() })
    }

    /// Same, with all options.
    pub fn start_opts(name: &str, options: Options) -> Self {
        let (snap_tx, snaps) = async_channel::bounded(1);
        let (cmd, cmd_rx) = async_channel::unbounded();
        let name = name.to_string();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(engine::run_with(name, snap_tx, cmd_rx, options));
        });
        Self {
            cmd,
            snaps,
            last: Snapshot::default(),
        }
    }

    pub fn send(&self, cmd: UiCmd) {
        self.cmd.send_blocking(cmd).unwrap();
    }

    /// Wait until a snapshot satisfies `pred`.
    pub fn wait(&mut self, what: &str, secs: u64, pred: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            while let Ok(s) = self.snaps.try_recv() {
                self.last = s;
            }
            if pred(&self.last) {
                return self.last.clone();
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for: {what} (error: {:?})",
                self.last.error
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Latest snapshot without waiting.
    pub fn now(&mut self) -> Snapshot {
        while let Ok(s) = self.snaps.try_recv() {
            self.last = s;
        }
        self.last.clone()
    }

    pub fn socket(&mut self) -> PathBuf {
        self.wait("player open", 15, |s| s.player_socket.is_some())
            .player_socket
            .unwrap()
    }
}

impl Drop for Peer {
    /// Leave the room so the engine closes its mpv. Otherwise the headless
    /// players outlive the test process.
    fn drop(&mut self) {
        let _ = self.cmd.send_blocking(UiCmd::Leave);
        let _ = eventually(Duration::from_secs(3), || {
            while let Ok(s) = self.snaps.try_recv() {
                self.last = s;
            }
            !self.last.player_open
        });
    }
}


/// A 2-minute clip like a real movie file: 5.1 English audio, a stereo Hindi
/// track, English and French subtitles.
pub fn rich_clip() -> PathBuf {
    static CLIP: OnceLock<PathBuf> = OnceLock::new();
    CLIP.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let path = dir.join("rich.mkv");
        if !path.exists() {
            let srt = |lang: &str, line: &str| {
                let p = dir.join(format!("subs-{lang}.srt"));
                std::fs::write(&p, format!("1\n00:00:01,000 --> 00:01:50,000\n{line}\n")).unwrap();
                p
            };
            let (en, fr) = (srt("en", "Hello there"), srt("fr", "Bonjour"));
            let tmp = path.with_extension("tmp.mkv");
            let status = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i", "testsrc=duration=120:size=320x180:rate=24"])
                .args(["-f", "lavfi", "-i", "sine=frequency=300:duration=120"])
                .args(["-f", "lavfi", "-i", "sine=frequency=500:duration=120"])
                .arg("-i").arg(&en)
                .arg("-i").arg(&fr)
                .args(["-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3", "-map", "4"])
                .args(["-c:v", "libx264", "-preset", "ultrafast", "-g", "48"])
                .args(["-c:a:0", "aac", "-ac:a:0", "6", "-c:a:1", "aac", "-ac:a:1", "2"])
                .args(["-c:s", "srt"])
                .args(["-metadata:s:a:0", "language=eng", "-metadata:s:a:0", "title=Surround"])
                .args(["-metadata:s:a:1", "language=hin"])
                .args(["-metadata:s:s:0", "language=eng", "-metadata:s:s:1", "language=fre"])
                .arg(&tmp)
                .status()
                .expect("ffmpeg is needed for tests: brew install ffmpeg");
            assert!(status.success(), "ffmpeg failed");
            std::fs::rename(&tmp, &path).unwrap();
        }
        path
    })
    .clone()
}

/// A fresh download folder per test person, never the real ~/Movies/WatchSync.
pub fn test_downloads(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("downloads")
        .join(format!("{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A ~300 MB real video, big enough that a transfer takes a few seconds.
pub fn big_clip() -> PathBuf {
    static CLIP: OnceLock<PathBuf> = OnceLock::new();
    CLIP.get_or_init(|| {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("big.mkv");
        if !path.exists() {
            let tmp = path.with_extension("tmp.mkv");
            let status = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i", "testsrc2=duration=60:size=1920x1080:rate=24"])
                .args(["-c:v", "libx264", "-preset", "ultrafast", "-b:v", "40M", "-maxrate", "40M", "-bufsize", "80M"])
                .arg(&tmp)
                .status()
                .expect("ffmpeg is needed for tests: brew install ffmpeg");
            assert!(status.success(), "ffmpeg failed");
            std::fs::rename(&tmp, &path).unwrap();
        }
        path
    })
    .clone()
}

/// Running `program` processes whose arguments contain `args`. Matches the
/// program's own file name (not `comm`, which ps cuts to 16 characters), so
/// shells that merely mention it don't count.
pub fn count_procs(program: &str, args: &str) -> usize {
    let out = Command::new("ps").args(["-axo", "args="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| {
            let exe = l.split_whitespace().next().unwrap_or("");
            Path::new(exe).file_name().and_then(|n| n.to_str()) == Some(program) && l.contains(args)
        })
        .count()
}
