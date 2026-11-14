//! Shared helpers: a generated test clip, headless mpv, raw IPC access.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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
