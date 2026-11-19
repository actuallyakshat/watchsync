//! Your Mac's sound for a screen share (see share_sound.m): captured
//! natively, because WebKit can't share system audio, and streamed to the
//! call window as raw stereo float samples, which it sends to friends.

use tauri::ipc::{Channel, InvokeResponseBody};

/// Samples are sent in chunks this long (20 ms at 48 kHz): small enough to
/// keep the delay down, big enough not to flood the window.
const CHUNK_FRAMES: usize = 960;

/// Stereo interleaved samples → little-endian bytes, ready to send.
fn to_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Mono or stereo frames → stereo.
fn stereo(samples: &[f32], channels: u32, into: &mut Vec<f32>) {
    if channels == 1 {
        for &s in samples {
            into.extend_from_slice(&[s, s]);
        }
    } else {
        into.extend_from_slice(samples);
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::c_void;
    use std::sync::Mutex;

    type Callback = unsafe extern "C" fn(*mut c_void, *const f32, u32, u32);

    unsafe extern "C" {
        fn ws_sound_available() -> i32;
        fn ws_sound_start(ctx: *mut c_void, cb: Callback, rate: *mut f64) -> i32;
        fn ws_sound_stop();
    }

    struct Sink {
        channel: Channel<InvokeResponseBody>,
        pending: Mutex<Vec<f32>>,
    }

    /// The running capture's sink (boxed: the native side holds a pointer).
    static RUNNING: Mutex<Option<Box<Sink>>> = Mutex::new(None);

    unsafe extern "C" fn on_audio(ctx: *mut c_void, samples: *const f32, frames: u32, channels: u32) {
        if ctx.is_null() || samples.is_null() || !(1..=2).contains(&channels) {
            return;
        }
        let sink = unsafe { &*(ctx as *const Sink) };
        let samples = unsafe { std::slice::from_raw_parts(samples, (frames * channels) as usize) };
        let Ok(mut pending) = sink.pending.lock() else { return };
        stereo(samples, channels, &mut pending);
        if pending.len() >= CHUNK_FRAMES * 2 {
            let bytes = to_bytes(&pending);
            pending.clear();
            // The window may be gone (reloading): nothing to do about it.
            let _ = sink.channel.send(InvokeResponseBody::Raw(bytes));
        }
    }

    pub fn available() -> bool {
        unsafe { ws_sound_available() == 1 }
    }

    pub fn start(channel: Channel<InvokeResponseBody>) -> Result<f64, String> {
        let mut running = RUNNING.lock().map_err(|e| e.to_string())?;
        stop_locked(&mut running);
        let sink = Box::new(Sink { channel, pending: Mutex::new(Vec::with_capacity(CHUNK_FRAMES * 4)) });
        let mut rate = 0.0;
        let ctx = &*sink as *const Sink as *mut c_void;
        let status = unsafe { ws_sound_start(ctx, on_audio, &mut rate) };
        if status != 0 {
            return Err(if status == -1 { "needs macOS 14.2".into() } else { format!("Core Audio error {status}") });
        }
        *running = Some(sink);
        Ok(rate)
    }

    fn stop_locked(running: &mut Option<Box<Sink>>) {
        if running.is_some() {
            // Waits for the last callback, so the sink can go.
            unsafe { ws_sound_stop() };
            *running = None;
        }
    }

    pub fn stop() {
        if let Ok(mut running) = RUNNING.lock() {
            stop_locked(&mut running);
        }
    }
}

/// Whether this Mac can share its sound (macOS 14.2 and later).
#[tauri::command]
pub fn share_sound_available() -> bool {
    #[cfg(target_os = "macos")]
    return mac::available();
    #[cfg(not(target_os = "macos"))]
    false
}

/// Start sending the Mac's sound (minus WatchSync's own) to `on_audio`, as
/// stereo f32 little-endian chunks. Returns the sample rate.
#[tauri::command]
pub async fn start_share_sound(on_audio: Channel<InvokeResponseBody>) -> Result<f64, String> {
    #[cfg(target_os = "macos")]
    return tauri::async_runtime::spawn_blocking(move || mac::start(on_audio)).await.map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = on_audio;
        Err("not supported".into())
    }
}

#[tauri::command]
pub async fn stop_share_sound() {
    #[cfg(target_os = "macos")]
    let _ = tauri::async_runtime::spawn_blocking(mac::stop).await;
}

/// The app is quitting.
pub fn stop() {
    #[cfg(target_os = "macos")]
    mac::stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_becomes_stereo_and_bytes_are_little_endian() {
        let mut out = Vec::new();
        stereo(&[0.5, -1.0], 1, &mut out);
        assert_eq!(out, [0.5, 0.5, -1.0, -1.0]);
        stereo(&[0.25, 0.75], 2, &mut out);
        assert_eq!(out.len(), 6);
        assert_eq!(to_bytes(&[1.0]), 1.0f32.to_le_bytes().to_vec());
    }
}
