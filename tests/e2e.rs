//! End to end: real engines, real (headless) mpv players, real iroh
//! connection. Checks the things a viewer would notice.
//! Needs internet (iroh's relay) plus mpv and ffmpeg installed.

mod common;

use common::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use watchsync::engine::{FileStatus, Phase, UiCmd};
use watchsync::sync::Crop;

/// Max allowed position difference between two players while playing.
/// 24fps → a frame is ~42ms; we demand < 2.5 frames.
const IN_SYNC: f64 = 0.1;

/// Positions of two players, sampled back to back.
fn positions(a: &Path, b: &Path) -> (f64, f64) {
    (time_pos(a), time_pos(b))
}

fn assert_in_sync(label: &str, a: &Path, b: &Path, tolerance: f64) {
    let (pa, pb) = positions(a, b);
    println!("{label}: host {pa:.3}s  guest {pb:.3}s  diff {:+.0}ms", (pb - pa) * 1000.0);
    assert!(
        (pa - pb).abs() < tolerance,
        "{label}: out of sync, host {pa:.3} vs guest {pb:.3}"
    );
}

#[test]
fn watch_party() {
    headless();
    let clip = clip();

    // --- Room ------------------------------------------------------------
    let mut host = Peer::start("Host");
    host.send(UiCmd::Host);
    let code = host
        .wait("host room", 30, |s| s.phase == Phase::Room && s.code.is_some())
        .code
        .unwrap();

    let mut guest = Peer::start("Riya");
    guest.send(UiCmd::Join { code: code.clone() });
    guest.wait("guest joined", 30, |s| s.phase == Phase::Room);
    host.wait("host sees Riya by name", 10, |s| {
        s.peers.iter().any(|p| p.name == "Riya")
    });

    // --- Same file ---------------------------------------------------------
    host.send(UiCmd::OpenFile { path: clip.clone() });
    guest.send(UiCmd::OpenFile { path: clip.clone() });
    let hs = host.socket();
    let gs = guest.socket();
    host.wait("file match shown", 10, |s| {
        s.peers.len() == 2 && s.peers.iter().all(|p| p.file == FileStatus::Match)
    });
    assert!(paused(&hs) && paused(&gs), "should start paused");
    let s = host.wait("no version warnings", 5, |s| s.peers.len() == 2);
    assert!(s.peers.iter().all(|p| !p.outdated), "same build, no warnings");

    // --- Guest picks a 2.39:1 crop: everyone gets the same picture ----------
    guest.send(UiCmd::SetCrop { crop: Crop::Scope });
    let crop_of = |sock: &PathBuf| {
        ipc(sock, json!(["get_property", "video-crop"]))
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        eventually(Duration::from_secs(5), || crop_of(&hs) == "320x134+0+23"
            && crop_of(&gs) == "320x134+0+23"),
        "both should crop: host {:?}, guest {:?}",
        crop_of(&hs),
        crop_of(&gs)
    );
    let s = host.wait("host shows crop", 5, |s| s.crop == Crop::Scope && s.crop_applied);
    assert_eq!(s.crop_by.as_deref(), Some("Riya"));

    // --- Host presses Play --------------------------------------------------
    host.send(UiCmd::TogglePlay);
    assert!(eventually(Duration::from_secs(3), || !paused(&hs) && !paused(&gs)));
    std::thread::sleep(Duration::from_secs(4));
    assert_in_sync("after play", &hs, &gs, IN_SYNC);

    // --- Guest presses space in the mpv window (global pause) ---------------
    ipc(&gs, json!(["set_property", "pause", true]));
    host.wait("host sees pause", 3, |s| !s.playing);
    assert!(eventually(Duration::from_secs(3), || paused(&hs)), "host should pause");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(paused(&gs), "guest should stay paused");
    // Paused on the same frame.
    assert_in_sync("after pause", &hs, &gs, 0.02);
    let s = host.wait("pause label", 3, |s| s.last_action.is_some());
    let label = s.last_action.unwrap();
    assert!(label.starts_with("Riya paused"), "label: {label}");

    // --- Guest seeks with the arrow keys / seek bar -------------------------
    ipc(&gs, json!(["seek", 200, "absolute"]));
    assert!(
        eventually(Duration::from_secs(4), || (time_pos(&hs) - 200.0).abs() < 0.5),
        "host should follow the guest's seek, is at {}",
        time_pos(&hs)
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert_in_sync("after guest seek", &hs, &gs, 0.05);

    // --- Host seeks back with the app's button -------------------------------
    host.send(UiCmd::SeekBy { seconds: -100.0 });
    assert!(
        eventually(Duration::from_secs(4), || (time_pos(&gs) - 100.0).abs() < 0.5),
        "guest should follow the host's seek, is at {}",
        time_pos(&gs)
    );

    // --- Guest resumes from the app ------------------------------------------
    guest.send(UiCmd::TogglePlay);
    assert!(eventually(Duration::from_secs(3), || !paused(&hs) && !paused(&gs)));
    std::thread::sleep(Duration::from_secs(3));
    assert_in_sync("after guest play", &hs, &gs, IN_SYNC);

    // --- Drift: knock the guest 0.5s ahead; it must slide back --------------
    let target = time_pos(&gs) + 0.5;
    ipc(&gs, json!(["seek", target, "absolute+exact"]));
    let start = Instant::now();
    let recovered = eventually(Duration::from_secs(20), || {
        let (pa, pb) = positions(&hs, &gs);
        (pa - pb).abs() < 0.06
    });
    println!("drift of 500ms recovered in {:.1}s", start.elapsed().as_secs_f64());
    assert!(recovered, "drift was not corrected");
    assert!(!paused(&hs) && !paused(&gs), "drift fix must not pause anyone");
    std::thread::sleep(Duration::from_secs(2));
    assert_in_sync("after drift fix", &hs, &gs, IN_SYNC);
    let speed = ipc(&gs, json!(["get_property", "speed"])).as_f64().unwrap();
    assert!((speed - 1.0).abs() < 0.06, "speed should be (back) near 1.0: {speed}");

    // --- Late joiner lands where the room is ----------------------------------
    let mut late = Peer::start("Aman");
    late.send(UiCmd::Join { code });
    late.wait("late joined", 30, |s| s.phase == Phase::Room);
    late.send(UiCmd::OpenFile { path: clip.clone() });
    let ls = late.socket();
    assert!(
        eventually(Duration::from_secs(10), || {
            !paused(&ls) && (time_pos(&ls) - time_pos(&hs)).abs() < IN_SYNC
        }),
        "late joiner should catch up: host {} vs late {}",
        time_pos(&hs),
        time_pos(&ls)
    );
    assert_in_sync("late joiner", &hs, &ls, IN_SYNC);
    assert!(
        eventually(Duration::from_secs(5), || crop_of(&ls) == "320x134+0+23"),
        "late joiner should get the room's crop, has {:?}",
        crop_of(&ls)
    );

    // --- Back to the original picture for everyone ---------------------------
    host.send(UiCmd::SetCrop { crop: Crop::Original });
    assert!(
        eventually(Duration::from_secs(5), || crop_of(&hs).is_empty()
            && crop_of(&gs).is_empty()
            && crop_of(&ls).is_empty()),
        "crop should be cleared everywhere"
    );
    host.wait("three people listed", 5, |s| s.peers.len() == 3);

    // --- Renames show up for everyone, live ---------------------------------
    guest.send(UiCmd::SetName { name: "  Riya K  ".into() });
    host.wait("host sees guest's new name", 5, |s| {
        s.peers.iter().any(|p| p.name == "Riya K") && !s.peers.iter().any(|p| p.name == "Riya")
    });
    host.send(UiCmd::SetName { name: "Akshat".into() });
    late.wait("guest sees host's new name", 5, |s| {
        s.peers.iter().any(|p| p.is_host && p.name == "Akshat")
    });
    assert!(!paused(&hs) && !paused(&gs), "renaming must not interrupt playback");

    // --- Leaving: the host notices quickly ----------------------------------
    guest.send(UiCmd::Leave);
    let s = guest.wait("guest back home", 5, |s| s.phase == Phase::Home && !s.player_open);
    assert!(s.peers.is_empty());
    host.wait("host sees guest leave", 10, |s| s.peers.len() == 2);
}
