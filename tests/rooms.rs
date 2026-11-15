//! Rooms that survive trouble: a guest's connection blinking, the host
//! leaving, the host vanishing. Real engines, real (headless) mpv, real
//! network; short timings so the tests don't take minutes.

mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use watchsync::engine::{Phase, Timing, UiCmd};
use watchsync::mpv::{Comfort, TrackKind};
use watchsync::net::Reaction;

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
fn when_the_named_successor_is_gone_too_the_next_in_line_takes_over() {
    let (mut peers, socks) = playing_room(&["Host", "Riya", "Aman"]);
    // The host hands over to Riya just as Riya's Mac drops off.
    peers[1].send(UiCmd::TestCrash);
    peers[0].send(UiCmd::Leave);
    let s = peers[2].wait("Aman takes over", 30, |s| s.is_host);
    assert!(s.last_action.unwrap_or_default().contains("is now the host"));
    assert!(eventually(Duration::from_secs(4), || !paused(&socks[2])), "Aman keeps watching");
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

#[test]
fn chat_and_reactions_reach_everyone_with_the_right_names() {
    headless();
    let mut host = Peer::start_with("Host", fast());
    host.send(UiCmd::Host);
    let code = host.wait("room", 30, |s| s.code.is_some()).code.unwrap();
    let mut riya = Peer::start_with("Riya", fast());
    let mut aman = Peer::start_with("Aman", fast());
    for p in [&mut riya, &mut aman] {
        p.send(UiCmd::Join { code: code.clone() });
        p.wait("joined", 30, |s| s.phase == Phase::Room);
    }
    host.wait("all here", 10, |s| s.peers.len() == 3);

    riya.send(UiCmd::Chat { text: "  popcorn ready?\n${path} ".into() });
    for p in [&mut host, &mut aman] {
        let s = p.wait("Riya's message", 5, |s| !s.chat.is_empty());
        let line = &s.chat[0];
        assert_eq!(line.from, "Riya");
        assert_eq!(line.text.as_deref(), Some("popcorn ready? ${path}"));
        assert!(!line.mine);
    }
    let s = riya.wait("own line", 5, |s| !s.chat.is_empty());
    assert!(s.chat[0].mine && s.chat.len() == 1, "the sender sees it once");

    aman.send(UiCmd::React { reaction: Reaction::Laugh });
    host.send(UiCmd::Chat { text: "starting now".into() });
    for p in [&mut riya, &mut aman, &mut host] {
        p.wait("everything", 5, |s| s.chat.len() == 3);
    }
    let s = riya.now();
    assert!(s.chat.iter().any(|l| l.from == "Aman" && l.reaction == Some(Reaction::Laugh)));
    assert!(s.chat.iter().any(|l| l.from == "Host" && l.text.as_deref() == Some("starting now")));
    assert_eq!(aman.now().chat.iter().filter(|l| l.reaction.is_some()).count(), 1);
}

#[test]
fn comfort_settings_and_tracks_are_personal() {
    headless();
    let clip = rich_clip();
    let mut host = Peer::start_with("Host", fast());
    host.send(UiCmd::Host);
    let code = host.wait("room", 30, |s| s.code.is_some()).code.unwrap();
    let mut riya = Peer::start_with("Riya", fast());
    riya.send(UiCmd::Join { code });
    riya.wait("joined", 30, |s| s.phase == Phase::Room);
    host.send(UiCmd::OpenFile { path: clip.clone() });
    riya.send(UiCmd::OpenFile { path: clip });
    let (hs, rs) = (host.socket(), riya.socket());

    // Riya sees her file's tracks...
    let s = riya.wait("tracks listed", 10, |s| s.tracks.len() == 4);
    let french = s
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Subtitle && t.lang.as_deref() == Some("fre"))
        .unwrap()
        .id;
    // ...and picks French subtitles and both comfort options.
    riya.send(UiCmd::SetTrack { kind: TrackKind::Subtitle, id: Some(french) });
    riya.send(UiCmd::SetComfort {
        comfort: Comfort { level_volume: true, boost_dialogue: true },
    });
    riya.wait("applied", 5, |s| {
        s.comfort.level_volume
            && s.tracks.iter().any(|t| t.kind == TrackKind::Subtitle && t.id == french && t.selected)
    });
    let filters = |sock: &Path| {
        ipc(sock, serde_json::json!(["get_property", "af"]))
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0)
    };
    assert_eq!(filters(&rs), 2);
    // The host's player is untouched.
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(filters(&hs), 0);
    let host_sid = ipc(&hs, serde_json::json!(["get_property", "sid"]));
    assert_ne!(host_sid.as_i64(), Some(french), "host's subtitles unchanged");

    // Comfort survives changing the movie.
    riya.send(UiCmd::OpenFile { path: rich_clip() });
    std::thread::sleep(Duration::from_secs(2));
    let rs = riya.socket();
    assert!(eventually(Duration::from_secs(5), || filters(&rs) == 2));
}

#[test]
fn a_guest_without_the_movie_is_offered_their_copy_found_by_name() {
    headless();
    let clip = clip();
    // Spotlight must already know the test clip (it indexes it in the background).
    if watchsync::find::find_by_name("clip.mkv").as_deref() != Some(clip.as_path()) {
        eprintln!("skipping: Spotlight hasn't indexed {}", clip.display());
        return;
    }
    let mut host = Peer::start_with("Host", fast());
    host.send(UiCmd::Host);
    let code = host.wait("room", 30, |s| s.code.is_some()).code.unwrap();
    let mut riya = Peer::start_with("Riya", fast());
    riya.send(UiCmd::Join { code });
    riya.wait("joined", 30, |s| s.phase == Phase::Room);
    assert!(riya.now().found.is_none(), "nothing to look for yet");

    host.send(UiCmd::OpenFile { path: clip.clone() });
    let s = riya.wait("found by name", 10, |s| s.found.is_some());
    assert_eq!(s.found.as_ref().unwrap().path, clip);
    assert!(!s.player_open, "only suggested, not opened");

    // Opening it gives the same file as the host.
    riya.send(UiCmd::OpenFile { path: s.found.unwrap().path });
    let s = riya.wait("opened", 15, |s| s.player_open && s.my_file == watchsync::engine::FileStatus::Match);
    assert!(s.found.is_none(), "suggestion goes away once a movie is open");
}

#[test]
fn subtitle_size_is_the_same_for_everyone() {
    let (mut peers, socks) = playing_room(&["Host", "Riya"]);
    let sub_scale = |s: &Path| common::ipc(s, serde_json::json!(["get_property", "sub-scale"])).as_f64().unwrap();
    // Everyone starts at the room's size, whatever their own mpv had.
    assert!(eventually(Duration::from_secs(3), || socks.iter().all(|s| (sub_scale(s) - 1.0).abs() < 1e-6)));

    // Set from the app by the host.
    peers[0].send(UiCmd::SetSubScale { scale: 1.4 });
    peers[1].wait("host's size", 5, |s| (s.sub_scale - 1.4).abs() < 1e-6 && s.sub_scale_by.as_deref() == Some("Host"));
    assert!(eventually(Duration::from_secs(3), || socks.iter().all(|s| (sub_scale(s) - 1.4).abs() < 1e-6)));

    // Riya presses G in her movie window (mpv's "bigger subtitles" key).
    common::ipc(&socks[1], serde_json::json!(["add", "sub-scale", 0.1]));
    peers[0].wait("Riya's size", 5, |s| (s.sub_scale - 1.5).abs() < 1e-6 && s.sub_scale_by.as_deref() == Some("Riya"));
    assert!(eventually(Duration::from_secs(3), || socks.iter().all(|s| (sub_scale(s) - 1.5).abs() < 1e-6)));

    // Out of range is kept sensible.
    peers[1].send(UiCmd::SetSubScale { scale: 40.0 });
    assert!(eventually(Duration::from_secs(3), || socks.iter().all(|s| (sub_scale(s) - 3.0).abs() < 1e-6)));
}
