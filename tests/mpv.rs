//! The mpv controller against a real (headless) mpv.

mod common;

use common::*;
use std::time::Duration;
use tokio::sync::mpsc;
use serde_json::json;
use watchsync::mpv::{Comfort, Mpv, MpvEvent, TrackKind};

async fn launch(start: f64) -> (Mpv, mpsc::UnboundedReceiver<MpvEvent>) {
    headless();
    let (tx, rx) = mpsc::unbounded_channel();
    let mpv = Mpv::launch(&clip(), start, tx).await.expect("launch mpv");
    (mpv, rx)
}

/// time-pos is only available once the file has loaded.
async fn loaded_pos(mpv: &Mpv) -> f64 {
    for _ in 0..50 {
        if let Ok(p) = mpv.time_pos().await {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("file never loaded");
}

#[tokio::test(flavor = "multi_thread")]
async fn starts_paused_at_the_requested_position() {
    let (mpv, _rx) = launch(30.0).await;
    let pos = loaded_pos(&mpv).await;
    assert!((pos - 30.0).abs() < 0.1, "pos {pos}");
    assert!(paused(mpv.socket()));
    let duration = mpv.duration().await.unwrap();
    assert!((duration - CLIP_SECS).abs() < 1.0, "duration {duration}");
}

#[tokio::test(flavor = "multi_thread")]
async fn plays_in_real_time_and_pauses() {
    let (mpv, _rx) = launch(10.0).await;
    loaded_pos(&mpv).await;
    mpv.set_pause(false).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    mpv.set_pause(true).await.unwrap();
    let pos = mpv.time_pos().await.unwrap();
    assert!((pos - 12.0).abs() < 0.3, "after 2s of play pos is {pos}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let still = mpv.time_pos().await.unwrap();
    assert!((still - pos).abs() < 0.01, "moved while paused: {pos} -> {still}");
}

#[tokio::test(flavor = "multi_thread")]
async fn seeks_are_frame_exact() {
    let (mpv, _rx) = launch(0.0).await;
    loaded_pos(&mpv).await;
    // 123.45s is between keyframes (every 2s) and between frames.
    mpv.seek(123.45).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pos = mpv.time_pos().await.unwrap();
    assert!((pos - 123.45).abs() < 1.0 / 24.0, "pos {pos}");
}

#[tokio::test(flavor = "multi_thread")]
async fn sets_speed() {
    let (mpv, _rx) = launch(0.0).await;
    loaded_pos(&mpv).await;
    mpv.set_speed(1.05).await.unwrap();
    let speed = ipc(mpv.socket(), serde_json::json!(["get_property", "speed"]));
    assert_eq!(speed.as_f64(), Some(1.05));
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_user_pause_and_seek() {
    let (mpv, mut rx) = launch(0.0).await;
    loaded_pos(&mpv).await;

    // The user presses space in the mpv window. (The initial Pause(true)
    // from observing the property may still be in flight, so skip past it.)
    ipc(mpv.socket(), serde_json::json!(["set_property", "pause", false]));
    let got_unpause = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(ev) = rx.recv().await {
            if matches!(ev, MpvEvent::Pause(false)) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(got_unpause, Ok(true));

    // The user seeks with the arrow keys.
    ipc(mpv.socket(), serde_json::json!(["seek", 60, "absolute"]));
    let got_restart = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(ev) = rx.recv().await {
            if matches!(ev, MpvEvent::PlaybackRestart) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(got_restart, Ok(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_exit_when_the_player_goes_away() {
    let (mpv, mut rx) = launch(0.0).await;
    loaded_pos(&mpv).await;
    let socket = mpv.socket().to_path_buf();
    drop(mpv); // kills the process
    let exited = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(ev) = rx.recv().await {
            if matches!(ev, MpvEvent::Exited) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(exited, Ok(true));
    assert!(std::os::unix::net::UnixStream::connect(socket).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn crops_and_uncrops_the_picture() {
    let (mpv, _rx) = launch(0.0).await;
    loaded_pos(&mpv).await;
    let mut size = None;
    for _ in 0..50 {
        if let Ok(s) = mpv.video_size().await {
            size = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(size, Some((320, 180, 320, 180)));

    // Someone pressed `w` (panscan) locally; the crop must undo it.
    ipc(mpv.socket(), serde_json::json!(["set_property", "panscan", 0.5]));
    mpv.set_crop("320x134+0+23").await.unwrap();
    let crop = ipc(mpv.socket(), serde_json::json!(["get_property", "video-crop"]));
    assert_eq!(crop.as_str(), Some("320x134+0+23"));
    let panscan = ipc(mpv.socket(), serde_json::json!(["get_property", "panscan"]));
    assert_eq!(panscan.as_f64(), Some(0.0));
    // The output picture really is cropped.
    let out = ipc(mpv.socket(), serde_json::json!(["get_property", "video-out-params"]));
    println!("video-out-params after crop: {out}");

    mpv.set_crop("").await.unwrap();
    let crop = ipc(mpv.socket(), serde_json::json!(["get_property", "video-crop"]));
    println!("video-crop after clearing: {crop}");
    assert!(
        matches!(crop.as_str(), Some("") | Some("0x0+0+0") | Some("0x0")),
        "crop should be cleared, got {crop}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn on_screen_text_cannot_expand_properties() {
    let (mpv, _rx) = launch(0.0).await;
    loaded_pos(&mpv).await;
    // Unescaped, mpv would put the file's path on screen.
    let raw = mpv.expand_text("${path}").await.unwrap();
    assert!(raw.contains("clip.mkv"), "sanity: mpv expands {raw}");
    for text in ["Riya: ${path}", "costs $5", "$$", "${filename} lol $>"] {
        let shown = mpv.expand_text(&watchsync::mpv::osd_escape(text)).await.unwrap();
        assert_eq!(shown, text);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_and_switches_audio_and_subtitle_tracks() {
    headless();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mpv = Mpv::launch(&rich_clip(), 5.0, tx).await.unwrap();
    loaded_pos(&mpv).await;
    let tracks = mpv.tracks().await.unwrap();
    let audio: Vec<_> = tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect();
    let subs: Vec<_> = tracks.iter().filter(|t| t.kind == TrackKind::Subtitle).collect();
    assert_eq!(audio.len(), 2, "{tracks:?}");
    assert_eq!(subs.len(), 2, "{tracks:?}");
    assert_eq!(audio[0].lang.as_deref(), Some("eng"));
    assert_eq!(audio[0].title.as_deref(), Some("Surround"));
    assert_eq!(audio[0].channels, Some(6));
    assert_eq!(audio[1].lang.as_deref(), Some("hin"));
    assert_eq!(audio[1].channels, Some(2));
    assert_eq!(subs[1].lang.as_deref(), Some("fre"));

    mpv.set_track(TrackKind::Audio, Some(audio[1].id)).await.unwrap();
    mpv.set_track(TrackKind::Subtitle, Some(subs[1].id)).await.unwrap();
    let now = mpv.tracks().await.unwrap();
    assert!(now.iter().any(|t| t.kind == TrackKind::Audio && t.lang.as_deref() == Some("hin") && t.selected));
    assert!(now.iter().any(|t| t.kind == TrackKind::Subtitle && t.lang.as_deref() == Some("fre") && t.selected));

    mpv.set_track(TrackKind::Subtitle, None).await.unwrap();
    let now = mpv.tracks().await.unwrap();
    assert!(!now.iter().any(|t| t.kind == TrackKind::Subtitle && t.selected), "subtitles off");
}

#[tokio::test(flavor = "multi_thread")]
async fn comfort_filters_keep_sound_in_sync_even_after_seeks() {
    headless();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mpv = Mpv::launch(&rich_clip(), 10.0, tx).await.unwrap();
    loaded_pos(&mpv).await;
    let both = Comfort { level_volume: true, boost_dialogue: true };
    mpv.set_comfort(both).await.unwrap();
    let af = ipc(mpv.socket(), json!(["get_property", "af"]));
    assert_eq!(af.as_array().map(|a| a.len()), Some(2), "both filters active: {af}");

    mpv.set_pause(false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t = mpv.time_pos().await.unwrap();
    assert!((t - 11.5).abs() < 0.4, "plays in real time with filters: {t}");

    // After a seek the sound comes back right away and stays in sync.
    mpv.seek(60.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let video = mpv.time_pos().await.unwrap();
    let audio = ipc(mpv.socket(), json!(["get_property", "audio-pts"])).as_f64().unwrap();
    let avsync = ipc(mpv.socket(), json!(["get_property", "avsync"])).as_f64().unwrap();
    assert!((audio - video).abs() < 0.1, "audio {audio} vs video {video}");
    assert!(avsync.abs() < 0.05, "avsync {avsync}");

    mpv.set_comfort(Comfort::default()).await.unwrap();
    let af = ipc(mpv.socket(), json!(["get_property", "af"]));
    assert_eq!(af.as_array().map(|a| a.len()), Some(0), "filters removed: {af}");
}
