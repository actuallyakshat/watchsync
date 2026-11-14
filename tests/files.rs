//! Sending a movie to a friend with croc, end to end: real engines, real
//! croc, real caffeinate. Needs `brew install croc`.

mod common;

use common::*;
use std::time::{Duration, Instant};
use watchsync::engine::{Direction, FileStatus, Options, Phase, Timing, TransferState, UiCmd};

fn crocs() -> usize {
    count_procs("croc", "--ignore-stdin")
}
fn awake() -> usize {
    count_procs("caffeinate", "-i -s -w")
}

#[test]
fn send_a_movie_to_a_friend_while_keeping_the_mac_awake() {
    assert!(watchsync::share::is_installed(), "brew install croc");
    headless();
    let movie = big_clip();
    let size = std::fs::metadata(&movie).unwrap().len();
    let opts = |name: &str| Options {
        timing: Timing::default(),
        downloads: test_downloads(name),
        send_start_timeout: Duration::from_secs(15),
        request_timeout: Duration::from_secs(10),
    };
    let mut host = Peer::start_opts("Host", opts("Host"));
    let riya_downloads = test_downloads("Riya");
    let mut riya = Peer::start_opts("Riya", Options { downloads: riya_downloads.clone(), ..opts("Riya") });
    host.send(UiCmd::Host);
    let code = host.wait("room", 30, |s| s.code.is_some()).code.unwrap();
    riya.send(UiCmd::Join { code });
    riya.wait("joined", 30, |s| s.phase == Phase::Room);

    // The host shares their copy; Riya, who has no movie, sees the offer.
    host.send(UiCmd::OpenFile { path: movie.clone() });
    host.socket();
    host.send(UiCmd::ShareFile);
    let s = riya.wait("offer visible", 10, |s| !s.offers.is_empty());
    let offer = s.offers[0].clone();
    assert_eq!((offer.from.as_str(), offer.name.as_str(), offer.size), ("Host", "big.mkv", size));
    assert!(host.now().offers.is_empty(), "you don't see your own offer");

    // Riya downloads it. While bytes flow, both Macs are kept awake.
    riya.send(UiCmd::GetFile { offer_id: offer.offer_id.clone() });
    let start = Instant::now();
    let mut max_awake = 0;
    let mut saw_progress = false;
    loop {
        let s = riya.now();
        let t = s.transfers.first().cloned();
        if let Some(t) = &t {
            if t.state == TransferState::Running {
                max_awake = max_awake.max(awake());
                saw_progress |= t.progress.percent > 0 && t.progress.percent < 100;
            }
            assert!(t.state != TransferState::Failed, "failed: {:?}", t.message);
            if t.state == TransferState::Done {
                break;
            }
        }
        assert!(start.elapsed() < Duration::from_secs(120), "transfer took too long: {t:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("300 MB in {:.1}s, caffeinate seen: {max_awake}", start.elapsed().as_secs_f64());
    assert!(max_awake >= 1, "the Mac should be kept awake during the transfer");
    assert!(saw_progress, "progress should be reported");

    // Exactly the same file, saved where it should be, and now playing.
    let got = riya_downloads.join("big.mkv");
    assert_eq!(std::fs::read(&got).unwrap().len() as u64, size);
    let s = riya.wait("received copy opened", 20, |s| s.player_open && s.my_file == FileStatus::Match);
    assert_eq!(s.transfers[0].path.as_deref(), Some(got.as_path()));
    let h = host.wait("send done", 10, |s| s.transfers.first().is_some_and(|t| t.state == TransferState::Done));
    assert_eq!(h.transfers[0].direction, Direction::Send);
    assert_eq!(h.transfers[0].peer, "Riya");

    // Afterwards nothing keeps the Mac awake and no croc is left running.
    assert!(eventually(Duration::from_secs(5), || awake() == 0 && crocs() == 0));

    // A send nobody connects to is stopped, and never kept the Mac awake.
    let mut aman = Peer::start_opts("Aman", opts("Aman"));
    let code = host.now().code.unwrap();
    aman.send(UiCmd::Join { code });
    aman.wait("joined", 30, |s| s.phase == Phase::Room);
    let s = aman.wait("offer", 10, |s| !s.offers.is_empty());
    aman.send(UiCmd::GetFile { offer_id: s.offers[0].offer_id.clone() });
    host.wait("second send waiting", 10, |s| s.transfers.len() == 2);
    // Aman changes their mind before croc connects.
    aman.wait("asked", 10, |s| s.transfers.len() == 1);
    let id = aman.now().transfers[0].id;
    aman.send(UiCmd::CancelTransfer { id });
    let h = host.wait("abandoned send stopped", 30, |s| {
        s.transfers.len() == 2 && matches!(s.transfers[1].state, TransferState::Failed | TransferState::Done)
    });
    println!("abandoned send: {:?} {:?}", h.transfers[1].state, h.transfers[1].message);
    assert!(eventually(Duration::from_secs(5), || awake() == 0 && crocs() == 0));

    // Requests for a copy that's no longer offered are refused.
    host.send(UiCmd::StopSharing);
    aman.wait("offer gone", 10, |s| s.offers.is_empty());
}
