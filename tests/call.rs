//! The call's plumbing in the engine: who has their mic and camera on, and
//! WebRTC signalling passed between guests through the host. The media
//! itself runs in the call window, so it isn't tested here.

mod common;

use common::*;
use serde_json::json;
use std::time::Duration;
use watchsync::engine::{Options, Phase, SignalIn, Timing, UiCmd};

/// A person whose incoming signalling we can read.
fn person(name: &str) -> (Peer, async_channel::Receiver<SignalIn>) {
    let (tx, rx) = async_channel::unbounded();
    let options = Options {
        timing: Timing::default(),
        downloads: test_downloads(name),
        signals: Some(tx),
        ..Options::default()
    };
    (Peer::start_opts(name, options), rx)
}

fn next(rx: &async_channel::Receiver<SignalIn>) -> SignalIn {
    let mut got = None;
    assert!(
        eventually(Duration::from_secs(5), || {
            got = rx.try_recv().ok();
            got.is_some()
        }),
        "no signal arrived"
    );
    got.unwrap()
}

#[test]
fn signalling_reaches_the_right_person_with_a_sender_they_can_trust() {
    let (mut host, host_rx) = person("Host");
    host.send(UiCmd::Host);
    let code = host.wait("room", 30, |s| s.code.is_some()).code.unwrap();
    let (mut riya, riya_rx) = person("Riya");
    let (mut aman, aman_rx) = person("Aman");
    for p in [&mut riya, &mut aman] {
        p.send(UiCmd::Join { code: code.clone() });
        p.wait("joined", 30, |s| s.phase == Phase::Room);
    }
    let all = |s: &watchsync::engine::Snapshot| s.peers.len() == 3 && s.peers.iter().all(|p| !p.id.is_empty());
    let host_id = host.wait("all here", 10, all).my_id;
    let riya_id = riya.wait("all listed", 10, all).my_id;
    let aman_id = aman.wait("all listed", 10, all).my_id;
    assert!(!host_id.is_empty() && riya_id != aman_id);

    // Guest to guest, through the host.
    riya.send(UiCmd::Signal { to: aman_id.clone(), data: json!({ "description": "offer" }) });
    let s = next(&aman_rx);
    assert_eq!(s.from, riya_id);
    assert_eq!(s.data, json!({ "description": "offer" }));

    // Guest to host, and host to guest.
    aman.send(UiCmd::Signal { to: host_id.clone(), data: json!(1) });
    assert_eq!(next(&host_rx).from, aman_id);
    host.send(UiCmd::Signal { to: riya_id.clone(), data: json!(2) });
    let s = next(&riya_rx);
    assert_eq!((s.from.as_str(), s.data), (host_id.as_str(), json!(2)));

    // Nobody else saw any of it.
    std::thread::sleep(Duration::from_millis(300));
    assert!(host_rx.try_recv().is_err() && riya_rx.try_recv().is_err() && aman_rx.try_recv().is_err());
}

#[test]
fn everyone_sees_whose_mic_and_camera_are_on() {
    let (mut host, _h) = person("Host");
    host.send(UiCmd::Host);
    let code = host.wait("room", 30, |s| s.code.is_some()).code.unwrap();
    let (mut riya, _r) = person("Riya");
    riya.send(UiCmd::Join { code });
    riya.wait("joined", 30, |s| s.phase == Phase::Room);

    riya.send(UiCmd::SetCall { mic: true, cam: false, screen: true });
    host.send(UiCmd::SetCall { mic: false, cam: true, screen: false });
    let find = |s: &watchsync::engine::Snapshot, name: &str| s.peers.iter().find(|p| p.name == name).map(|p| (p.mic, p.cam, p.screen));
    for p in [&mut host, &mut riya] {
        p.wait("call state", 10, |s| find(s, "Riya") == Some((true, false, true)) && find(s, "Host") == Some((false, true, false)));
    }
    assert!(riya.now().screen && !host.now().screen);

    // Stopping the share is seen too.
    riya.send(UiCmd::SetCall { mic: true, cam: false, screen: false });
    host.wait("share stopped", 10, |s| find(s, "Riya") == Some((true, false, false)));
}

#[test]
fn the_call_window_knows_when_the_movie_goes_fullscreen() {
    headless();
    let (mut host, _h) = person("Host");
    host.send(UiCmd::Host);
    host.wait("room", 30, |s| s.code.is_some());
    host.send(UiCmd::OpenFile { path: clip() });
    let socket = host.wait("player", 20, |s| s.player_socket.is_some()).player_socket.unwrap();
    assert!(!host.now().player_fullscreen);

    host.send(UiCmd::ToggleFullscreen);
    host.wait("fullscreen", 5, |s| s.player_fullscreen);
    // Leaving fullscreen in the movie window itself (f, Esc) is seen too.
    ipc(&socket, json!(["set_property", "fullscreen", false]));
    host.wait("windowed", 5, |s| !s.player_fullscreen);
}

#[test]
fn each_room_gets_its_own_relay_for_the_call() {
    let (mut host, _h) = person("Host");
    assert!(host.now().turn.is_none());
    host.send(UiCmd::Host);
    let first = host.wait("relay", 30, |s| s.turn.is_some()).turn.unwrap();
    assert!(first.urls.starts_with("turn:127.0.0.1:") && !first.credential.is_empty());
    host.send(UiCmd::Leave);
    host.wait("left", 10, |s| s.turn.is_none());
    host.send(UiCmd::Host);
    let second = host.wait("new relay", 30, |s| s.turn.is_some()).turn.unwrap();
    assert_ne!(first.credential, second.credential);
}
