//! Rooms that survive trouble: a guest's connection blinking, the host
//! leaving, the host vanishing. Real engines, real (headless) mpv, real
//! network; short timings so the tests don't take minutes.

mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use watchsync::engine::{Phase, Timing, UiCmd};

fn fast() -> Timing {
    Timing {
        silence: Duration::from_secs(2),
        old_host_grace: Duration::from_secs(2),
        successor_window: Duration::from_secs(6),
        give_up: Duration::from_secs(30),
        peer_timeout: Duration::from_secs(4),
        attempt: Duration::from_secs(4),
    }
}

/// A room of `names` (first is the host), everyone with the clip open and playing.
fn playing_room(names: &[&str]) -> (Vec<Peer>, Vec<PathBuf>) {
    headless();
    let clip = clip();
    let mut peers: Vec<Peer> = names.iter().map(|n| Peer::start_with(n, fast())).collect();
    peers[0].send(UiCmd::Host);
    let code = peers[0]
        .wait("host room", 30, |s| s.phase == Phase::Room && s.code.is_some())
        .code
        .unwrap();
    for i in 1..peers.len() {
        peers[i].send(UiCmd::Join { code: code.clone() });
        peers[i].wait("joined", 30, |s| s.phase == Phase::Room);
        // One at a time: join order is the order of succession.
        peers[0].wait("member list", 10, |s| s.peers.len() == i + 1);
    }
    let mut socks = Vec::new();
    for p in peers.iter_mut() {
        p.send(UiCmd::OpenFile { path: clip.clone() });
    }
    for p in peers.iter_mut() {
        socks.push(p.socket());
    }
    peers[0].send(UiCmd::TogglePlay);
    assert!(eventually(Duration::from_secs(5), || socks.iter().all(|s| !paused(s))));
    std::thread::sleep(Duration::from_secs(2));
    (peers, socks)
}

/// Watch players for `secs` while `step` runs each tick. Fails if any player
/// pauses or jumps backwards: what a viewer would notice.
fn watch_smoothly(socks: &[PathBuf], secs: u64, mut done: impl FnMut() -> bool) {
    let mut last: Vec<f64> = socks.iter().map(|s| time_pos(s)).collect();
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        std::thread::sleep(Duration::from_millis(150));
        for (i, s) in socks.iter().enumerate() {
            assert!(!paused(s), "player {i} paused");
            let p = time_pos(s);
            assert!(p >= last[i] - 0.05, "player {i} jumped back: {} -> {p}", last[i]);
            assert!(p - last[i] < 1.0, "player {i} jumped forward: {} -> {p}", last[i]);
            last[i] = p;
        }
        if done() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out");
    }
}

fn in_sync(a: &Path, b: &Path) -> bool {
    (time_pos(a) - time_pos(b)).abs() < 0.1
}

#[test]
fn a_guest_whose_connection_blinks_rejoins_without_a_hiccup() {
    let (mut peers, socks) = playing_room(&["Host", "Riya"]);
    peers[1].send(UiCmd::TestDropHostLink);
    {
        let guest = &mut peers[1];
        watch_smoothly(&socks, 20, || {
            let s = guest.now();
            s.reconnects == 1 && !s.reconnecting && s.phase == Phase::Room
        });
    }
    // No duplicate "Riya" in the host's list.
    peers[0].wait("one Riya", 10, |s| s.peers.iter().filter(|p| p.name == "Riya").count() == 1 && s.peers.len() == 2);
    assert!(eventually(Duration::from_secs(10), || in_sync(&socks[0], &socks[1])));
    // Still a working room: the guest pauses everyone.
    peers[1].send(UiCmd::TogglePlay);
    assert!(eventually(Duration::from_secs(4), || paused(&socks[0])));
}

#[test]
fn when_the_host_leaves_the_next_person_takes_over() {
    let (mut peers, socks) = playing_room(&["Host", "Riya", "Aman"]);
    let watchers = vec![socks[1].clone(), socks[2].clone()];
    peers[0].send(UiCmd::Leave);
    {
        let (a, b) = peers.split_at_mut(2);
        let riya = &mut a[1];
        let aman = &mut b[0];
        watch_smoothly(&watchers, 25, || {
            let r = riya.now();
            let m = aman.now();
            r.is_host && !m.reconnecting && m.peers.iter().any(|p| p.is_host && p.name == "Riya")
        });
    }
    let s = peers[1].wait("Riya hosts both", 10, |s| s.peers.len() == 2);
    assert!(s.code.is_some(), "the new host has an invite code");
    assert!(s.last_action.unwrap_or_default().contains("is now the host"));
    assert!(eventually(Duration::from_secs(10), || in_sync(&watchers[0], &watchers[1])));
    // The room works under the new host.
    peers[2].send(UiCmd::TogglePlay);
    assert!(eventually(Duration::from_secs(4), || paused(&watchers[0]) && paused(&watchers[1])));
    let (a, b) = (time_pos(&watchers[0]), time_pos(&watchers[1]));
    assert!((a - b).abs() < 0.05, "paused on different frames: {a} vs {b}");
}

#[test]
fn when_the_host_vanishes_the_next_person_takes_over() {
    let (mut peers, socks) = playing_room(&["Host", "Riya", "Aman"]);
    let watchers = vec![socks[1].clone(), socks[2].clone()];
    peers[0].send(UiCmd::TestCrash);
    {
        let (a, b) = peers.split_at_mut(2);
        let riya = &mut a[1];
        let aman = &mut b[0];
        watch_smoothly(&watchers, 30, || {
            let r = riya.now();
            let m = aman.now();
            r.is_host && !m.reconnecting && m.peers.iter().any(|p| p.is_host && p.name == "Riya")
        });
    }
    assert!(eventually(Duration::from_secs(10), || in_sync(&watchers[0], &watchers[1])));
    peers[1].send(UiCmd::SeekBy { seconds: 30.0 });
    let target = time_pos(&watchers[0]) + 30.0;
    assert!(eventually(Duration::from_secs(5), || (time_pos(&watchers[1]) - target).abs() < 2.0));
}
