//! The WatchSync window: a web UI over the sync engine in the `watchsync`
//! crate. The engine runs on its own tokio runtime; the UI sends it commands
//! and receives a `snapshot` event whenever something changes.

mod hotkeys;
mod output;
mod screen;
mod share_sound;

use serde::Serialize;
use tauri::{Emitter, Manager, RunEvent, State};
use watchsync::engine::{self, Options, Phase, SignalIn, Snapshot, UiCmd};
use watchsync::{mpv, share};

struct Bridge {
    cmd: async_channel::Sender<UiCmd>,
}

/// Forward a UI action to the engine.
#[tauri::command]
fn send(bridge: State<'_, Bridge>, cmd: UiCmd) -> Result<(), String> {
    bridge.cmd.try_send(cmd).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct PlayerSetup {
    /// mpv is installed and usable.
    installed: bool,
    /// Homebrew is available to install it.
    has_brew: bool,
    /// croc (for sending movies) is installed.
    croc: bool,
    /// ...but too old to connect directly, so croc transfers are slow.
    croc_outdated: bool,
}

#[tauri::command]
fn player_setup() -> PlayerSetup {
    PlayerSetup {
        installed: mpv::is_installed(),
        has_brew: mpv::find_brew().is_some(),
        croc: share::is_installed(),
        croc_outdated: share::is_outdated(),
    }
}

/// `brew install croc`, or `brew upgrade croc` if it's too old.
#[tauri::command]
async fn install_croc() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(share::install_with_brew)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// `brew install mpv croc` (whatever's missing). Takes a minute or two.
#[tauri::command]
async fn install_player() -> Result<(), String> {
    // Batteries included: the player, plus croc for sending movies.
    tauri::async_runtime::spawn_blocking(|| {
        if !mpv::is_installed() {
            mpv::install_with_brew()?;
        }
        if !share::is_installed() || share::is_outdated() {
            share::install_with_brew()?;
        }
        anyhow::Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// The call window: always there while in a room (it carries the call's
/// audio even when collapsed), hidden otherwise.
pub(crate) const CALL: &str = "call";

/// "Show call" in the main window: bring the call window up, expanded.
#[tauri::command]
fn show_call(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(CALL) {
        let _ = w.show();
        let _ = w.emit_to(CALL, "call-expand", ());
    }
}

/// The Mac's full user name ("Akshat Dubey"), falling back to $USER.
fn display_name() -> String {
    std::process::Command::new("id")
        .arg("-F")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "Friend".into())
}

pub fn run() {
    let (snap_tx, snap_rx) = async_channel::bounded::<Snapshot>(1);
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<UiCmd>();
    let (signal_tx, signal_rx) = async_channel::unbounded::<SignalIn>();

    // The engine (networking + player control) gets its own runtime thread.
    let name = display_name();
    std::thread::spawn(move || {
        let options = Options {
            signals: Some(signal_tx),
            ..Options::default()
        };
        tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(engine::run_with(name, snap_tx, cmd_rx, options));
    });

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // watchsync://join/<code> links open the app and join the room.
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(Bridge { cmd: cmd_tx })
        .manage(hotkeys::Hotkeys::default())
        .invoke_handler(tauri::generate_handler![send, player_setup, install_player, install_croc, show_call, hotkeys::set_call_shortcuts, output::output_is_speakers, screen::choose_share, share_sound::share_sound_available, share_sound::start_share_sound, share_sound::stop_share_sound])
        .setup(move |app| {
            // Push every engine snapshot to the window.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut in_room = false;
                while let Ok(snap) = snap_rx.recv_blocking() {
                    let now_in_room = snap.phase == Phase::Room;
                    if now_in_room != in_room {
                        in_room = now_in_room;
                        if let Some(w) = handle.get_webview_window(CALL) {
                            let _ = if in_room { w.show() } else { w.hide() };
                        }
                        handle.state::<hotkeys::Hotkeys>().set_active(&handle, in_room);
                    }
                    let _ = handle.emit("snapshot", &snap);
                }
            });
            // Signalling from others goes to the call window only, in order.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                while let Ok(signal) = signal_rx.recv_blocking() {
                    let _ = handle.emit_to(CALL, "signal", &signal);
                }
            });
            // Sharing your screen asks which: the whole screen or a window.
            if let Some(call) = app.get_webview_window(CALL) {
                screen::install(&call);
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building WatchSync");

    app.run(|_, event| {
        if let RunEvent::Exit = event {
            // Don't leave a movie playing, or a transfer (and its
            // keep-awake) running, after the app quits.
            mpv::kill_player();
            share::kill_all();
            share_sound::stop();
        }
    });
}
