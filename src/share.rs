//! Sending movie files to friends, two ways (the sender picks):
//!
//! - **Direct**: over the room's own iroh connection, straight from Mac to
//!   Mac when iroh can punch through both routers (most homes), else through
//!   iroh's relay. Only the friend who asked can connect, checked by their
//!   endpoint id. A dropped download picks up where it stopped.
//! - **croc** (https://github.com/schollz/croc): one croc session per
//!   download, each with its own secret code, through croc's public relay.
//!
//! While bytes are actually flowing, `caffeinate` keeps the Mac awake, and
//! it ends the moment the transfer does.

use anyhow::{Result, anyhow, bail};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

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
    /// Direct transfers: "direct" (Mac to Mac) or "relayed" (through
    /// iroh's relay, slower).
    pub via: Option<String>,
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

/// A transfer in progress, of either kind. Dropping it stops the transfer.
pub enum Running {
    Croc(Croc),
    Direct(Direct),
}

#[derive(Debug)]
pub enum TransferEvent {
    Started,
    Progress(Progress),
    /// It ended; `Ok` if it succeeded, otherwise why not.
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
pub fn start(id: u64, job: Job, code: &str, events: mpsc::UnboundedSender<(u64, TransferEvent)>) -> Result<Croc> {
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
            // Movies are already compressed, and hashing only the samples
            // imohash reads lets sending start at once, not after reading
            // the whole file.
            cmd.args(["--no-compress", "send", "--hash", "imohash"]).arg(file);
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
                    let _ = events.send((id, TransferEvent::Started));
                }
                Line::Progress(p) => {
                    let _ = events.send((id, TransferEvent::Progress(p)));
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
        let _ = events.send((id, TransferEvent::Finished(result)));
    });

    // The child is owned by the waiting task; stop it by pid.
    Ok(Croc { pid })
}

/// `caffeinate -i -s -w <pid>`: no idle or system sleep while `pid` runs.
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

/// Keeps the Mac awake (and stops caffeinate when dropped).
struct Awake(Option<Child>);

impl Awake {
    /// For as long as this app runs, at most: if it crashes, caffeinate ends too.
    fn new() -> Self {
        Self(keep_awake(std::process::id()))
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            if let Some(p) = c.id() {
                track(p, false);
            }
            let _ = c.start_kill();
        }
    }
}

/// Stop a croc by pid (dropping `Croc` does the same).
pub fn stop(pid: u32) {
    if pid != 0 {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    }
    track(pid, false);
}

// ------------------------------------------------------------------ direct

type Events = mpsc::UnboundedSender<(u64, TransferEvent)>;

/// Bytes per read and write.
const CHUNK: usize = 1024 * 1024;
/// How long the receiver may take to connect and ask.
const DIRECT_CONNECT: Duration = Duration::from_secs(30);

/// What the receiver asks for: an offer, from byte `from` (resuming).
#[derive(Serialize, Deserialize, Debug)]
struct Request {
    offer_id: String,
    from: u64,
}

/// A direct send or receive. Dropping it stops the transfer.
pub struct Direct {
    _stop: oneshot::Sender<()>,
    /// Sender: the ticket to withdraw if nobody has used it yet.
    ticket: Option<(Outbox, (String, String))>,
}

impl Drop for Direct {
    fn drop(&mut self) {
        if let Some((outbox, key)) = self.ticket.take() {
            outbox.0.lock().unwrap().remove(&key);
        }
    }
}

struct Ticket {
    id: u64,
    path: PathBuf,
    size: u64,
    stop: oneshot::Receiver<()>,
    events: Events,
}

/// The sends waiting for their friend to connect, by (friend's endpoint id,
/// offer id). Each lets exactly one connection in.
#[derive(Clone, Default)]
pub struct Outbox(Arc<Mutex<HashMap<(String, String), Ticket>>>);

impl Outbox {
    /// Let the friend with endpoint id `requester` download `path`, once.
    pub fn allow(&self, id: u64, requester: &str, offer_id: &str, path: &Path, size: u64, events: Events) -> Direct {
        let (stop_tx, stop) = oneshot::channel();
        let key = (requester.to_string(), offer_id.to_string());
        let ticket = Ticket {
            id,
            path: path.to_path_buf(),
            size,
            stop,
            events,
        };
        self.0.lock().unwrap().insert(key.clone(), ticket);
        Direct {
            _stop: stop_tx,
            ticket: Some((self.clone(), key)),
        }
    }

    /// A friend connected to download (`ALPN_FILE`).
    pub fn accept(&self, conn: Connection) {
        tokio::spawn(serve(self.clone(), conn));
    }
}

async fn serve(outbox: Outbox, conn: Connection) {
    let opened = async {
        let (send, mut recv) = conn.accept_bi().await?;
        let len = recv.read_u32().await? as usize;
        anyhow::ensure!(len <= 4096, "request too long");
        let mut buf = vec![0u8; len];
        recv.read_exact(&mut buf).await?;
        let req: Request = serde_json::from_slice(&buf)?;
        anyhow::Ok((send, recv, req))
    };
    let Ok(Ok((send, recv, req))) = timeout(DIRECT_CONNECT, opened).await else {
        conn.close(1u32.into(), b"bad request");
        return;
    };
    // Only the friend this copy was offered to: iroh has already checked
    // that the connection really comes from that endpoint id.
    let key = (conn.remote_id().to_string(), req.offer_id.clone());
    let Some(ticket) = outbox.0.lock().unwrap().remove(&key) else {
        conn.close(1u32.into(), b"not offered to you");
        return;
    };
    let Ticket { id, path, size, stop, events } = ticket;
    let _ = events.send((id, TransferEvent::Started));
    let _awake = Awake::new();
    let result = tokio::select! {
        r = send_file(&conn, send, recv, &path, size, req.from, id, &events) => r.map_err(|e| e.to_string()),
        _ = stop => {
            conn.close(2u32.into(), b"cancelled");
            Err("stopped".into())
        }
    };
    let _ = events.send((id, TransferEvent::Finished(result)));
}

#[allow(clippy::too_many_arguments)]
async fn send_file(
    conn: &Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    path: &Path,
    size: u64,
    from: u64,
    id: u64,
    events: &Events,
) -> Result<()> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| anyhow!("the movie file isn't there anymore"))?;
    if file.metadata().await?.len() != size {
        bail!("the movie file changed since it was shared");
    }
    if from > size {
        bail!("asked for more than the whole file");
    }
    file.seek(std::io::SeekFrom::Start(from)).await?;
    let mut meter = Meter::new(size, from);
    let mut buf = vec![0u8; CHUNK];
    let mut sent = from;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await.map_err(|_| anyhow!("the connection dropped"))?;
        sent += n as u64;
        if let Some(p) = meter.tick(sent, conn, false) {
            let _ = events.send((id, TransferEvent::Progress(p)));
        }
    }
    if sent != size {
        bail!("the movie file changed while sending");
    }
    send.finish()?;
    // The friend says "got it all" once it's safely on their disk.
    recv.read_u8().await.map_err(|_| anyhow!("the connection dropped near the end"))?;
    if let Some(p) = meter.tick(sent, conn, true) {
        let _ = events.send((id, TransferEvent::Progress(p)));
    }
    conn.close(0u32.into(), b"done");
    Ok(())
}

/// Where a direct download collects its bytes until complete: hidden, and
/// named after the copy, so a later try resumes only the same copy.
pub fn partial_path(downloads: &Path, name: &str, fingerprint: &str) -> PathBuf {
    downloads.join(format!(".{name}.{fingerprint}.part"))
}

/// Download offer `offer_id` (`size` bytes) from the Mac at `from`, into
/// `part` (resuming whatever's there), then move it to `dest`.
#[allow(clippy::too_many_arguments)]
pub fn receive(
    id: u64,
    endpoint: Endpoint,
    from: EndpointAddr,
    offer_id: String,
    size: u64,
    part: PathBuf,
    dest: PathBuf,
    events: Events,
) -> Direct {
    let (stop_tx, stop) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let result = tokio::select! {
            r = receive_file(id, &endpoint, from, &offer_id, size, &part, &dest, &events) => r.map_err(|e| e.to_string()),
            _ = stop => Err("stopped".into()),
        };
        let _ = events.send((id, TransferEvent::Finished(result)));
    });
    Direct {
        _stop: stop_tx,
        ticket: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_file(
    id: u64,
    endpoint: &Endpoint,
    from: EndpointAddr,
    offer_id: &str,
    size: u64,
    part: &Path,
    dest: &Path,
    events: &Events,
) -> Result<()> {
    let conn = timeout(DIRECT_CONNECT, endpoint.connect(from, crate::net::ALPN_FILE))
        .await
        .map_err(|_| anyhow!("couldn't connect (timed out)"))?
        .map_err(|e| anyhow!("couldn't connect: {e}"))?;
    let result = receive_on(&conn, id, offer_id, size, part, dest, events).await;
    if result.is_err() {
        conn.close(2u32.into(), b"stopped");
    }
    result
}

async fn receive_on(
    conn: &Connection,
    id: u64,
    offer_id: &str,
    size: u64,
    part: &Path,
    dest: &Path,
    events: &Events,
) -> Result<()> {
    if let Some(dir) = part.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(part).await?;
    let mut have = file.metadata().await?.len();
    if have > size {
        file.set_len(0).await?;
        have = 0;
    }
    let (mut send, mut recv) = conn.open_bi().await?;
    let req = serde_json::to_vec(&Request {
        offer_id: offer_id.to_string(),
        from: have,
    })?;
    send.write_u32(req.len() as u32).await?;
    send.write_all(&req).await?;
    let _ = events.send((id, TransferEvent::Started));
    let _awake = Awake::new();
    let mut meter = Meter::new(size, have);
    let dropped = |e: iroh::endpoint::ReadError| match e {
        iroh::endpoint::ReadError::ConnectionLost(iroh::endpoint::ConnectionError::ApplicationClosed(c))
            if c.reason.as_ref() == b"not offered to you" =>
        {
            anyhow!("they aren't sending you this copy anymore")
        }
        _ => anyhow!("the connection dropped. Try again to pick up where it stopped"),
    };
    while let Some(chunk) = recv.read_chunk(CHUNK).await.map_err(dropped)? {
        have += chunk.len() as u64;
        if have > size {
            bail!("more arrived than the offered file");
        }
        file.write_all(&chunk).await?;
        if let Some(p) = meter.tick(have, conn, false) {
            let _ = events.send((id, TransferEvent::Progress(p)));
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if have != size {
        bail!("the transfer ended early. Try again to pick up where it stopped");
    }
    tokio::fs::rename(part, dest).await?;
    if let Some(p) = meter.tick(have, conn, true) {
        let _ = events.send((id, TransferEvent::Progress(p)));
    }
    // "Got it all", then give the sender a moment to hear it and hang up.
    send.write_u8(1).await?;
    let _ = send.finish();
    let _ = timeout(Duration::from_secs(5), conn.closed()).await;
    Ok(())
}

/// "direct" or "relayed": how the connection's bytes travel right now.
fn route(conn: &Connection) -> Option<String> {
    let paths = conn.paths();
    let selected = paths.iter().find(|p| p.is_selected())?;
    Some(if selected.is_ip() { "direct" } else { "relayed" }.to_string())
}

/// Progress, speed and time left, a few times a second.
struct Meter {
    size: u64,
    last: Option<Instant>,
    /// (when, bytes done) over the last few seconds, for the speed.
    samples: VecDeque<(Instant, u64)>,
}

impl Meter {
    fn new(size: u64, done: u64) -> Self {
        Self {
            size,
            last: None,
            samples: VecDeque::from([(Instant::now(), done)]),
        }
    }

    fn tick(&mut self, done: u64, conn: &Connection, force: bool) -> Option<Progress> {
        let now = Instant::now();
        if !force && self.last.is_some_and(|l| now - l < Duration::from_millis(250)) {
            return None;
        }
        self.last = Some(now);
        self.samples.push_back((now, done));
        while self.samples.len() > 2 && now - self.samples[0].0 > Duration::from_secs(3) {
            self.samples.pop_front();
        }
        let (t0, d0) = self.samples[0];
        let secs = (now - t0).as_secs_f64();
        let rate = if secs > 0.2 { (done - d0) as f64 / secs } else { 0.0 };
        Some(progress(done, self.size, rate, route(conn)))
    }
}

/// The progress line for `done` of `size` bytes at `rate` bytes/s.
pub fn progress(done: u64, size: u64, rate: f64, via: Option<String>) -> Progress {
    let percent = (done.min(size) * 100).checked_div(size).unwrap_or(100) as u8;
    let gb = size >= 1_000_000_000;
    let amount = if gb {
        format!("{:.1}/{:.1} GB", done as f64 / 1e9, size as f64 / 1e9)
    } else {
        format!("{}/{} MB", done / 1_000_000, size / 1_000_000)
    };
    let (speed, eta) = if rate >= 1.0 {
        let left = ((size.saturating_sub(done)) as f64 / rate).round() as u64;
        (Some(format!("{:.1} MB/s", rate / 1e6)), Some(duration_words(left)))
    } else {
        (None, None)
    };
    Progress {
        percent,
        amount: Some(amount),
        speed,
        eta,
        via,
    }
}

/// "45s", "3m5s", "1h2m".
fn duration_words(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        s => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
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
    fn direct_progress_reads_like_crocs() {
        let p = progress(353_000_000, 400_000_000, 107e6, Some("direct".into()));
        assert_eq!(p.percent, 88);
        assert_eq!(p.amount.as_deref(), Some("353/400 MB"));
        assert_eq!(p.speed.as_deref(), Some("107.0 MB/s"));
        assert_eq!(p.eta.as_deref(), Some("0s"));
        let p = progress(1_000_000_000, 4_200_000_000, 10e6, None);
        assert_eq!((p.percent, p.amount.as_deref()), (23, Some("1.0/4.2 GB")));
        assert_eq!(p.eta.as_deref(), Some("5m20s"));
        let p = progress(0, 9_000_000_000, 1e6, None);
        assert_eq!(p.eta.as_deref(), Some("2h30m"));
        // No speed yet: no time left either.
        assert_eq!(progress(5, 10, 0.0, None).speed, None);
    }

    #[test]
    fn partial_downloads_are_hidden_and_tied_to_the_copy() {
        let p = partial_path(Path::new("/d"), "Movie.mkv", "ab12");
        assert_eq!(p, Path::new("/d/.Movie.mkv.ab12.part"));
    }

    #[test]
    fn codes_are_long_and_unique() {
        let (a, b) = (new_code().unwrap(), new_code().unwrap());
        assert_ne!(a, b);
        assert_eq!(a.len(), 3 + 32);
    }
}
