//! Sending a movie to a friend, both ways (directly over iroh, and with
//! croc), end to end: real engines, real connections, real caffeinate. The
//! croc test needs `brew install croc`.

mod common;

use common::*;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use watchsync::engine::{Direction, FileStatus, Options, Phase, Timing, TransferState, UiCmd};
use watchsync::net::Method;

/// One transfer test at a time: they count caffeinate processes.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn crocs() -> usize {
    count_procs("croc", "--ignore-stdin")
}
fn awake() -> usize {
    count_procs("caffeinate", "-i -s -w")
}

#[test]
fn send_a_movie_with_croc_while_keeping_the_mac_awake() {
    assert!(watchsync::share::is_installed(), "brew install croc");
    send_a_movie(Method::Croc);
}

#[test]
fn send_a_movie_directly_while_keeping_the_mac_awake() {
    send_a_movie(Method::Direct);
}

fn send_a_movie(method: Method) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    headless();
    let movie = big_clip();
    let size = std::fs::metadata(&movie).unwrap().len();
    let opts = |name: &str| Options {
        timing: Timing::default(),
        downloads: test_downloads(name),
        send_start_timeout: Duration::from_secs(15),
        request_timeout: Duration::from_secs(10),
        signals: None,
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
    host.send(UiCmd::ShareFile { method });
    let s = riya.wait("offer visible", 10, |s| !s.offers.is_empty());
    let offer = s.offers[0].clone();
    assert_eq!((offer.from.as_str(), offer.name.as_str(), offer.size), ("Host", "big.mkv", size));
    assert_eq!(offer.method, method);
    assert_eq!(host.now().sharing_method, Some(method));
    assert!(host.now().offers.is_empty(), "you don't see your own offer");

    if method == Method::Direct {
        // A stranger who isn't in the room can't download the copy, even
        // knowing the offer and where the host is.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let host_code = host.now().code.unwrap();
        let err = rt.block_on(async {
            let stranger = watchsync::net::bind().await.unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let dir = test_downloads("Stranger");
            let _job = watchsync::share::receive(
                1,
                stranger,
                watchsync::net::decode_code(&host_code).unwrap(),
                offer.offer_id.clone(),
                size,
                dir.join(".part"),
                dir.join("big.mkv"),
                tx,
            );
            loop {
                match tokio::time::timeout(Duration::from_secs(40), rx.recv()).await.unwrap().unwrap().1 {
                    watchsync::share::TransferEvent::Finished(r) => break r.unwrap_err(),
                    _ => continue,
                }
            }
        });
        assert!(err.contains("aren't sending you"), "{err}");

        // Riya already has the first 100 MB from an earlier try: only the
        // rest is sent.
        std::fs::create_dir_all(&riya_downloads).unwrap();
        let head = &std::fs::read(&movie).unwrap()[..100_000_000];
        std::fs::write(
            watchsync::share::partial_path(&riya_downloads, "big.mkv", &offer.fingerprint),
            head,
        )
        .unwrap();
    }

    // Riya downloads it. While bytes flow, both Macs are kept awake.
    riya.send(UiCmd::GetFile { offer_id: offer.offer_id.clone() });
    let start = Instant::now();
    let mut max_awake = 0;
    let mut saw_progress = false;
    let mut first_percent = None;
    let mut via = None;
    loop {
        let s = riya.now();
        let t = s.transfers.first().cloned();
        if let Some(t) = &t {
            if t.state == TransferState::Running {
                max_awake = max_awake.max(awake());
                saw_progress |= t.progress.percent > 0 && t.progress.percent < 100;
                first_percent = first_percent.or(t.progress.amount.as_ref().map(|_| t.progress.percent));
                via = via.or(t.progress.via.clone());
            }
            assert!(t.state != TransferState::Failed, "failed: {:?} / sender: {:?}", t.message, host.now().transfers);
            if t.state == TransferState::Done {
                break;
            }
        }
        assert!(start.elapsed() < Duration::from_secs(120), "transfer took too long: {t:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    println!(
        "{method:?}: 300 MB in {:.1}s, caffeinate seen: {max_awake}, via {via:?}",
        start.elapsed().as_secs_f64()
    );
    if method == Method::Direct {
        assert!(first_percent.is_some_and(|p| p >= 30), "resumed, not restarted: {first_percent:?}");
        assert_eq!(via.as_deref(), Some("direct"), "two apps on one Mac connect directly");
        let leftovers: Vec<_> = std::fs::read_dir(&riya_downloads).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers, ["big.mkv"], "the partial file became the movie");
    }
    assert!(max_awake >= 1, "the Mac should be kept awake during the transfer");
    assert!(saw_progress, "progress should be reported");

    // Exactly the same file, saved where it should be, and now playing.
    let got = riya_downloads.join("big.mkv");
    assert!(std::fs::read(&got).unwrap() == std::fs::read(&movie).unwrap(), "byte for byte");
    let s = riya.wait("received copy opened", 20, |s| s.player_open && s.my_file == FileStatus::Match);
    assert_eq!(s.transfers[0].path.as_deref(), Some(got.as_path()));
    let h = host.wait("send done", 10, |s| s.transfers.first().is_some_and(|t| t.state == TransferState::Done));
    assert_eq!(h.transfers[0].direction, Direction::Send);
    assert_eq!(h.transfers[0].peer, "Riya");

    // Afterwards nothing keeps the Mac awake and no croc is left running.
    assert!(eventually(Duration::from_secs(5), || awake() == 0 && crocs() == 0));

    // A send nobody connects to is stopped, and never kept the Mac awake.
    let aman_downloads = test_downloads("Aman");
    let mut aman = Peer::start_opts("Aman", Options { downloads: aman_downloads.clone(), ..opts("Aman") });
    let code = host.now().code.unwrap();
    aman.send(UiCmd::Join { code });
    aman.wait("joined", 30, |s| s.phase == Phase::Room);
    let s = aman.wait("offer", 10, |s| !s.offers.is_empty());
    aman.send(UiCmd::GetFile { offer_id: s.offers[0].offer_id.clone() });
    host.wait("second send waiting", 10, |s| s.transfers.len() == 2);
    // Aman changes their mind before connecting.
    aman.wait("asked", 10, |s| s.transfers.len() == 1);
    let id = aman.now().transfers[0].id;
    aman.send(UiCmd::CancelTransfer { id });
    let h = host.wait("abandoned send stopped", 30, |s| {
        s.transfers.len() == 2 && matches!(s.transfers[1].state, TransferState::Failed | TransferState::Done)
    });
    println!("abandoned send: {:?} {:?}", h.transfers[1].state, h.transfers[1].message);
    assert!(eventually(Duration::from_secs(5), || awake() == 0 && crocs() == 0));

    let parts = || {
        std::fs::read_dir(&aman_downloads)
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count())
            .unwrap_or(0)
    };
    assert!(eventually(Duration::from_secs(5), || parts() == 0), "cancelling leaves no partial file");

    // Requests for a copy that's no longer offered are refused.
    host.send(UiCmd::StopSharing);
    aman.wait("offer gone", 10, |s| s.offers.is_empty());

    if method == Method::Direct {
        // Guest to guest: Riya shares the copy she just got, and Aman gets
        // it from her (the host only passes the messages on).
        riya.send(UiCmd::ShareFile { method });
        let s = aman.wait("Riya's offer", 10, |s| s.offers.iter().any(|o| o.from == "Riya"));
        aman.send(UiCmd::GetFile { offer_id: s.offers[0].offer_id.clone() });
        let s = aman.wait("got Riya's copy", 120, |s| {
            let t = s.transfers.last().unwrap();
            assert!(t.state != TransferState::Failed, "failed: {:?}", t.message);
            s.transfers.len() == 2 && t.state == TransferState::Done
        });
        assert_eq!(s.transfers[1].peer, "Riya");
        assert!(std::fs::read(aman_downloads.join("big.mkv")).unwrap() == std::fs::read(&movie).unwrap());
        assert_eq!(parts(), 0);
    }
}
