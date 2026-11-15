//! The call's system-wide shortcuts (mic, camera). They work from any app,
//! including the movie in fullscreen, but only while you're in a room, so
//! they don't take ⌥⌘M / ⌥⌘V away from other apps the rest of the time.

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub const DEFAULT_MIC: &str = "Alt+Command+M";
pub const DEFAULT_CAM: &str = "Alt+Command+V";

pub struct Hotkeys {
    inner: Mutex<State>,
}

struct State {
    mic: Shortcut,
    cam: Shortcut,
    active: bool,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            inner: Mutex::new(State {
                mic: DEFAULT_MIC.parse().expect("default mic shortcut"),
                cam: DEFAULT_CAM.parse().expect("default camera shortcut"),
                active: false,
            }),
        }
    }
}

/// "Alt+Command+M" → a shortcut, or a message saying what's wrong.
pub fn parse(s: &str) -> Result<Shortcut, String> {
    let shortcut: Shortcut = s.parse().map_err(|_| format!("“{s}” isn't a shortcut WatchSync can use"))?;
    if shortcut.mods.is_empty() {
        return Err("Shortcuts need ⌘, ⌥, ⌃ or ⇧, so they don't fire while you type.".into());
    }
    Ok(shortcut)
}

impl Hotkeys {
    /// Change the shortcuts (registered at once if in a room).
    pub fn set<R: Runtime>(&self, app: &AppHandle<R>, mic: &str, cam: &str) -> Result<(), String> {
        let (mic, cam) = (parse(mic)?, parse(cam)?);
        if mic == cam {
            return Err("Mic and camera need different shortcuts.".into());
        }
        let mut s = self.inner.lock().unwrap();
        let (old_mic, old_cam) = (s.mic, s.cam);
        s.mic = mic;
        s.cam = cam;
        if let Err(e) = apply(app, &s) {
            // Keep the ones that worked.
            s.mic = old_mic;
            s.cam = old_cam;
            let _ = apply(app, &s);
            return Err(e);
        }
        Ok(())
    }

    /// In a room or not.
    pub fn set_active<R: Runtime>(&self, app: &AppHandle<R>, active: bool) {
        let mut s = self.inner.lock().unwrap();
        if s.active != active {
            s.active = active;
            if let Err(e) = apply(app, &s) {
                eprintln!("call shortcuts: {e}");
            }
        }
    }
}

fn apply<R: Runtime>(app: &AppHandle<R>, s: &State) -> Result<(), String> {
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;
    if !s.active {
        return Ok(());
    }
    for (shortcut, action) in [(s.mic, "mic"), (s.cam, "cam")] {
        gs.on_shortcut(shortcut, move |app, _, event| {
            if event.state == ShortcutState::Pressed {
                let _ = app.emit_to(crate::CALL, "call-shortcut", action);
            }
        })
        .map_err(|e| format!("{shortcut} is taken by another app ({e})"))?;
    }
    Ok(())
}

/// Main window: use these shortcuts (from Settings).
#[tauri::command]
pub fn set_call_shortcuts(app: AppHandle, mic: String, cam: String) -> Result<(), String> {
    app.state::<Hotkeys>().set(&app, &mic, &cam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_need_a_modifier_and_must_parse() {
        assert!(parse(DEFAULT_MIC).is_ok() && parse(DEFAULT_CAM).is_ok());
        assert!(parse("Shift+Alt+KeyK").is_ok());
        assert!(parse("M").is_err());
        assert!(parse("Command+Nope").is_err());
    }
}
