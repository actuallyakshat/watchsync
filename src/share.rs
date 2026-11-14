//! Sending movie files to friends with croc (https://github.com/schollz/croc).
//!
//! One croc session per download, each with its own secret code. While bytes
//! are actually flowing, `caffeinate` keeps the Mac awake; it's tied to the
//! croc process, so it ends the moment the transfer does.

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use tokio::sync::mpsc;

pub fn find_croc() -> Option<PathBuf> {
    crate::mpv::find_program("croc")
}

pub fn is_installed() -> bool {
    find_croc().is_some()
}

/// `brew install croc` (blocking).
pub fn install_with_brew() -> Result<()> {
    crate::mpv::brew_install("croc")
}

/// An unguessable croc code: whoever has it gets the file.
pub fn new_code() -> Result<String> {
    Ok(format!("ws-{}", random_hex(16)?))
}

/// `n` random bytes from the OS, as hex.
pub fn random_hex(n: usize) -> Result<String> {
    let mut bytes = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex::encode(bytes))
}

/// Only the last path component: a file name from the network can't point
/// anywhere else.
pub fn safe_file_name(name: &str) -> Option<String> {
    let n = Path::new(name).file_name()?.to_str()?.trim();
    (!n.is_empty() && n != "." && n != "..").then(|| n.to_string())
}

/// Free space (bytes) on the disk holding `dir`, from `df`.
pub fn free_space(dir: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-k").arg(dir).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1)?;
    let kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb * 1024)
}

/// The default download folder: ~/Movies/WatchSync.
pub fn default_downloads() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Movies")
        .join("WatchSync")
}

// ------------------------------------------------------------------ progress

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Progress {
    pub percent: u8,
    /// "353/400 MB"
    pub amount: Option<String>,
    /// "107 MB/s"
    pub speed: Option<String>,
    /// Time left, "1m2s"
    pub eta: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// Connected to the other side; bytes are about to flow.
    Started,
    Progress(Progress),
    Other,
}

/// Understand one line of croc output (lines end in `\r` or `\n`).
pub fn parse_line(line: &str) -> Line {
    let t = line.trim();
    if t.starts_with("Sending (") || t.starts_with("Receiving (") {
        return Line::Started;
    }
    // "big.bin  88% |█████   | (353/400 MB, 107 MB/s) [3s:0s]"
    // "Hashing big.bin 0% ..." is croc checking the file, not the transfer.
    if t.starts_with("Hashing") {
        return Line::Other;
    }
    let Some(pct_at) = t.find("% |") else {
        return Line::Other;
    };
    let digits: String = t[..pct_at]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let Ok(percent) = digits.parse::<u8>() else {
        return Line::Other;
    };
    let mut p = Progress {
        percent: percent.min(100),
        ..Default::default()
    };
    let rest = &t[pct_at..];
    if let (Some(a), Some(b)) = (rest.find('('), rest.find(')')) {
        let mut parts = rest[a + 1..b].split(", ");
        p.amount = parts.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        p.speed = parts.next().map(|s| s.trim().to_string());
        if let (Some(c), Some(d)) = (rest.find('['), rest.find(']'))
            && let Some((_, left)) = rest[c + 1..d].split_once(':')
        {
            p.eta = Some(left.to_string());
        }
    }
    Line::Progress(p)
}

/// Split raw croc output (lenient UTF-8, `\r` or `\n` endings) into lines.
#[derive(Default)]
pub struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in bytes {
            if b == b'\r' || b == b'\n' {
                if !self.buf.is_empty() {
                    lines.push(String::from_utf8_lossy(&self.buf).into_owned());
                    self.buf.clear();
                }
            } else {
                self.buf.push(b);
            }
        }
        lines
    }
}

// ------------------------------------------------------------------ processes

/// PIDs of running croc and caffeinate processes, so quitting the app can
/// stop them (tokio's kill_on_drop doesn't run when the GUI exits).
static PIDS: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

fn track(pid: u32, add: bool) {
    let mut g = PIDS.lock().unwrap();
    let set = g.get_or_insert_with(HashSet::new);
    if add {
        set.insert(pid);
    } else {
        set.remove(&pid);
    }
}

/// Stop every transfer (on quit).
pub fn kill_all() {
    let pids: Vec<u32> = PIDS.lock().unwrap().as_ref().map(|s| s.iter().copied().collect()).unwrap_or_default();
    for pid in pids {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    }
}

#[derive(Debug)]
pub enum CrocEvent {
    Started,
    Progress(Progress),
    /// croc exited; `Ok` if it succeeded, otherwise its last words.
    Finished(Result<(), String>),
}

/// A running croc. Dropping it stops the transfer (if it's still running).
pub struct Croc {
    pid: u32,
}

impl Croc {
    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for Croc {
    fn drop(&mut self) {
        let running = PIDS.lock().unwrap().as_ref().is_some_and(|s| s.contains(&self.pid));
        if running {
            stop(self.pid);
        }
    }
}

pub enum Job<'a> {
    Send { file: &'a Path },
    Receive { into: &'a Path },
}

/// Start croc. Events arrive tagged with `id`.
pub fn start(id: u64, job: Job, code: &str, events: mpsc::UnboundedSender<(u64, CrocEvent)>) -> Result<Croc> {
    let croc = find_croc().ok_or_else(|| anyhow!("croc isn't installed. Install it with: brew install croc"))?;
    let mut cmd = Command::new(croc);
    // The code goes in the environment (croc's secure mode on macOS), never
    // on the command line where other users could see it.
    cmd.env("CROC_SECRET", code)
        .args(["--yes", "--ignore-stdin", "--no-update-check", "--disable-clipboard"]);
    match job {
        Job::Send { file } => {
            if !file.is_file() {
                bail!("the movie file isn't there anymore");
            }
            cmd.arg("send").arg(file);
        }
        Job::Receive { into } => {
            std::fs::create_dir_all(into)?;
            cmd.arg("--overwrite").arg("--out").arg(into);
        }
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("couldn't start croc: {e}"))?;
    let pid = child.id().unwrap_or(0);
    track(pid, true);

    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();
    for stream in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let mut stream = stream;
            let mut split = LineSplitter::default();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                for line in split.push(&buf[..n]) {
                    let _ = tx.send(line);
                }
            }
        });
    }
    drop(line_tx);

    // Watch the output; keep the Mac awake once bytes flow; report the end.
    let mut child_for_wait = child;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<std::process::ExitStatus>();
    tokio::spawn(async move {
        let status = child_for_wait.wait().await;
        if let Ok(s) = status {
            let _ = done_tx.send(s);
        }
    });
    tokio::spawn(async move {
        let mut awake: Option<Child> = None;
        let mut last = String::new();
        while let Some(line) = line_rx.recv().await {
            match parse_line(&line) {
                Line::Started => {
                    if awake.is_none() {
                        awake = keep_awake(pid);
                    }
                    let _ = events.send((id, CrocEvent::Started));
                }
                Line::Progress(p) => {
                    let _ = events.send((id, CrocEvent::Progress(p)));
                }
                Line::Other => {
                    if !line.trim().is_empty() {
                        last = line.trim().to_string();
                    }
                }
            }
        }
        let result = match done_rx.await {
            Ok(s) if s.success() => Ok(()),
            _ => Err(if last.is_empty() { "croc stopped".into() } else { last }),
        };
        if let Some(mut a) = awake.take() {
            let _ = a.start_kill();
            if let Some(p) = a.id() {
                track(p, false);
            }
        }
        track(pid, false);
        let _ = events.send((id, CrocEvent::Finished(result)));
    });

    // The child is owned by the waiting task; stop it by pid.
    Ok(Croc { pid })
}

/// `caffeinate -i -s -w <pid>`: no idle or system sleep while croc runs.
/// (-s only applies on mains power; closing a laptop's lid still sleeps it.)
fn keep_awake(pid: u32) -> Option<Child> {
    let child = Command::new("/usr/bin/caffeinate")
        .args(["-i", "-s", "-w", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    if let Some(p) = child.id() {
        track(p, true);
    }
    Some(child)
}

/// Stop a croc by pid (dropping `Croc` does the same).
pub fn stop(pid: u32) {
    if pid != 0 {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    }
    track(pid, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(raw: &[u8]) -> Vec<Line> {
        let mut s = LineSplitter::default();
        let mut out: Vec<Line> = s.push(raw).iter().map(|l| parse_line(l)).collect();
        out.retain(|l| *l != Line::Other);
        out
    }

    #[test]
    fn reads_real_croc_receive_output() {
        let lines = lines_of(include_bytes!("../tests/fixtures/croc-receive.log"));
        assert_eq!(lines[0], Line::Started);
        let progress: Vec<&Progress> = lines
            .iter()
            .filter_map(|l| if let Line::Progress(p) = l { Some(p) } else { None })
            .collect();
        assert!(progress.len() > 10, "{}", progress.len());
        assert!(progress.windows(2).all(|w| w[0].percent <= w[1].percent), "never goes backwards (Hashing lines skipped)");
        assert_eq!(progress.last().unwrap().percent, 100);
        let mid = progress.iter().find(|p| p.percent == 88).unwrap();
        assert_eq!(mid.amount.as_deref(), Some("353/400 MB"));
        assert_eq!(mid.speed.as_deref(), Some("107 MB/s"));
        assert_eq!(mid.eta.as_deref(), Some("0s"));
    }

    #[test]
    fn reads_real_croc_send_output() {
        let lines = lines_of(include_bytes!("../tests/fixtures/croc-send.log"));
        // "Hashing" comes before "Sending (": starting means connected, not hashing.
        assert_eq!(lines[0], Line::Started);
        assert!(matches!(lines.last(), Some(Line::Progress(p)) if p.percent == 100));
    }

    #[test]
    fn splits_on_carriage_returns_and_survives_bad_bytes() {
        let mut s = LineSplitter::default();
        let mut got = s.push(b"a 1% |  | (1/9 MB, 1 MB/s) [0s:9s]\r\xff\xfeb 2");
        got.extend(s.push(b"% |  | (2/9 MB, 1 MB/s) [1s:8s]\n"));
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(matches!(parse_line(&got[0]), Line::Progress(Progress { percent: 1, .. })));
        // The second line starts with invalid bytes and still parses.
        assert!(got[1].starts_with('\u{fffd}'));
        assert!(matches!(parse_line(&got[1]), Line::Progress(Progress { percent: 2, .. })));
        assert_eq!(parse_line("connecting..."), Line::Other);
    }

    #[test]
    fn file_names_from_the_network_stay_in_the_folder() {
        assert_eq!(safe_file_name("Movie (2026).mkv").as_deref(), Some("Movie (2026).mkv"));
        assert_eq!(safe_file_name("../../.ssh/id_rsa").as_deref(), Some("id_rsa"));
        assert_eq!(safe_file_name("/etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(safe_file_name(".."), None);
        assert_eq!(safe_file_name(""), None);
    }

    #[test]
    fn codes_are_long_and_unique() {
        let (a, b) = (new_code().unwrap(), new_code().unwrap());
        assert_ne!(a, b);
        assert_eq!(a.len(), 3 + 32);
    }
}
