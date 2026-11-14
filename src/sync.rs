//! Pure sync logic: shared playback state, drift correction, clock offset.
//! No I/O here, so everything is unit-testable.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// How far in the future "play" is scheduled, so every peer starts together.
pub const PLAY_LEAD_MS: f64 = 500.0;

/// Wall-clock milliseconds since the Unix epoch.
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// The authoritative playback state. Owned by the host, mirrored by guests.
/// Every peer can compute where the movie *should* be from this alone.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlaybackState {
    /// Incremented by the host on every change; older versions are ignored.
    pub version: u64,
    pub playing: bool,
    /// Media position (seconds) at `anchor_ms`.
    pub position: f64,
    /// Host-clock time at which `position` is true.
    pub anchor_ms: f64,
    /// Who caused this state, and what they did ("Aman paused").
    pub by: String,
    pub action: String,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            version: 0,
            playing: false,
            position: 0.0,
            anchor_ms: 0.0,
            by: String::new(),
            action: String::new(),
        }
    }
}

impl PlaybackState {
    /// Where the movie should be at `host_now_ms` (host clock).
    pub fn expected_position(&self, host_now_ms: f64) -> f64 {
        if self.playing && host_now_ms > self.anchor_ms {
            self.position + (host_now_ms - self.anchor_ms) / 1000.0
        } else {
            self.position
        }
    }

    /// Paused, or playing but the scheduled start hasn't arrived yet.
    pub fn should_be_paused(&self, host_now_ms: f64) -> bool {
        !self.playing || host_now_ms < self.anchor_ms
    }

    pub fn label(&self) -> Option<String> {
        if self.by.is_empty() {
            None
        } else {
            Some(format!("{} {}", self.by, self.action))
        }
    }
}

/// What a peer should do about its drift from the expected position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Correction {
    None,
    Speed(f64),
    Seek(f64),
}

/// Drift beyond this is fixed by a hard seek.
const SEEK_THRESHOLD: f64 = 1.5;
/// Drift beyond this starts a speed nudge. ~1.5 frames at 24fps, since
/// mpv's time-pos advances in whole frames.
const NUDGE_START: f64 = 0.06;
/// Once inside this, return to normal speed.
const NUDGE_STOP: f64 = 0.02;
const NUDGE: f64 = 0.05;

/// Decide how to correct drift. `actual`/`expected` in seconds.
pub fn correction(actual: f64, expected: f64, current_speed: f64) -> Correction {
    let drift = actual - expected;
    if drift.abs() > SEEK_THRESHOLD {
        return Correction::Seek(expected);
    }
    if drift.abs() > NUDGE_START {
        // Ahead → slow down, behind → speed up.
        let speed = if drift > 0.0 { 1.0 - NUDGE } else { 1.0 + NUDGE };
        return if (speed - current_speed).abs() > 1e-9 {
            Correction::Speed(speed)
        } else {
            Correction::None
        };
    }
    if drift.abs() < NUDGE_STOP && (current_speed - 1.0).abs() > 1e-9 {
        return Correction::Speed(1.0);
    }
    Correction::None
}

/// NTP-style estimate of `host_clock - local_clock`, using the
/// lowest-round-trip sample among recent pings.
#[derive(Default)]
pub struct ClockSync {
    samples: VecDeque<(f64, f64)>, // (rtt, offset)
}

impl ClockSync {
    const KEEP: usize = 16;

    /// `t0`: local send time, `host_ms`: host time when it replied, `t1`: local receive time.
    pub fn add_sample(&mut self, t0: f64, host_ms: f64, t1: f64) {
        let rtt = t1 - t0;
        if rtt < 0.0 {
            return;
        }
        let offset = host_ms - (t0 + t1) / 2.0;
        self.samples.push_back((rtt, offset));
        if self.samples.len() > Self::KEEP {
            self.samples.pop_front();
        }
    }

    pub fn offset(&self) -> f64 {
        self.samples
            .iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|s| s.1)
            .unwrap_or(0.0)
    }

    pub fn rtt(&self) -> Option<f64> {
        self.samples.iter().map(|s| s.0).min_by(|a, b| a.total_cmp(b))
    }
}

/// A picture crop everyone in the room shares. Defined by aspect ratio of the
/// *source* video, so every screen shows exactly the same part of the frame.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Crop {
    #[default]
    Original,
    /// 4:3
    Classic,
    /// IMAX 1.43:1 (70mm film, full height)
    #[serde(rename = "imax_143")]
    Imax143,
    /// 1.66:1 European widescreen
    #[serde(rename = "euro_166")]
    Euro166,
    /// 16:9
    Wide,
    /// 1.85:1 flat
    Flat,
    /// IMAX 1.90:1 (digital IMAX, "IMAX Enhanced")
    #[serde(rename = "imax_190")]
    Imax190,
    /// 2.00:1 (Univisium, streaming series)
    #[serde(rename = "univisium_200")]
    Univisium200,
    /// 2.20:1 (70mm)
    #[serde(rename = "todd_ao_220")]
    ToddAo220,
    /// 2.39:1 scope
    Scope,
    /// 2.76:1 Ultra Panavision 70
    #[serde(rename = "ultra_276")]
    Ultra276,
}

impl Crop {
    /// Tallest to widest.
    pub const ALL: [Crop; 11] = [
        Crop::Original,
        Crop::Classic,
        Crop::Imax143,
        Crop::Euro166,
        Crop::Wide,
        Crop::Flat,
        Crop::Imax190,
        Crop::Univisium200,
        Crop::ToddAo220,
        Crop::Scope,
        Crop::Ultra276,
    ];

    pub fn aspect(self) -> Option<f64> {
        Some(match self {
            Crop::Original => return None,
            Crop::Classic => 4.0 / 3.0,
            Crop::Imax143 => 1.43,
            Crop::Euro166 => 1.66,
            Crop::Wide => 16.0 / 9.0,
            Crop::Flat => 1.85,
            Crop::Imax190 => 1.90,
            Crop::Univisium200 => 2.0,
            Crop::ToddAo220 => 2.20,
            Crop::Scope => 2.39,
            Crop::Ultra276 => 2.76,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Crop::Original => "Original",
            Crop::Classic => "4:3",
            Crop::Imax143 => "IMAX 1.43:1",
            Crop::Euro166 => "1.66:1",
            Crop::Wide => "16:9",
            Crop::Flat => "1.85:1",
            Crop::Imax190 => "IMAX 1.90:1",
            Crop::Univisium200 => "2.00:1",
            Crop::ToddAo220 => "2.20:1",
            Crop::Scope => "2.39:1",
            Crop::Ultra276 => "2.76:1",
        }
    }
}

fn even(x: f64) -> u32 {
    ((x / 2.0).round() * 2.0).max(2.0) as u32
}

/// mpv `video-crop` value ("WxH+X+Y", source pixels) for a centred crop to
/// `crop`'s aspect ratio. `w`/`h` are the coded size, `dw`/`dh` the display
/// size (they differ for anamorphic video). Empty string = no crop.
pub fn crop_geometry(w: u32, h: u32, dw: u32, dh: u32, crop: Crop) -> String {
    let Some(target) = crop.aspect() else {
        return String::new();
    };
    if w == 0 || h == 0 || dw == 0 || dh == 0 {
        return String::new();
    }
    let display = dw as f64 / dh as f64;
    if ((display - target) / target).abs() < 0.01 {
        return String::new(); // already that shape
    }
    if display > target {
        // Too wide: trim the sides.
        let cw = even(w as f64 * target / display).min(w);
        format!("{cw}x{h}+{}+0", (w - cw) / 2)
    } else {
        // Too tall: trim top and bottom.
        let ch = even(h as f64 * display / target).min(h);
        format!("{w}x{ch}+0+{}", (h - ch) / 2)
    }
}

pub fn format_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing_at(position: f64, anchor_ms: f64) -> PlaybackState {
        PlaybackState {
            playing: true,
            position,
            anchor_ms,
            ..Default::default()
        }
    }

    #[test]
    fn expected_position_advances_only_after_anchor() {
        let s = playing_at(100.0, 10_000.0);
        assert_eq!(s.expected_position(9_000.0), 100.0);
        assert!(s.should_be_paused(9_000.0));
        assert!((s.expected_position(12_500.0) - 102.5).abs() < 1e-9);
        assert!(!s.should_be_paused(12_500.0));
    }

    #[test]
    fn paused_state_stays_put() {
        let s = PlaybackState {
            position: 42.0,
            ..Default::default()
        };
        assert_eq!(s.expected_position(1e12), 42.0);
        assert!(s.should_be_paused(1e12));
    }

    #[test]
    fn correction_bands() {
        assert_eq!(correction(10.0, 10.01, 1.0), Correction::None);
        assert_eq!(correction(10.2, 10.0, 1.0), Correction::Speed(1.0 - NUDGE));
        assert_eq!(correction(9.8, 10.0, 1.0), Correction::Speed(1.0 + NUDGE));
        // already nudging the right way → no new command
        assert_eq!(correction(9.8, 10.0, 1.0 + NUDGE), Correction::None);
        // converged → back to normal
        assert_eq!(correction(10.005, 10.0, 1.0 + NUDGE), Correction::Speed(1.0));
        // hysteresis band keeps the current speed
        assert_eq!(correction(10.04, 10.0, 1.0 - NUDGE), Correction::None);
        assert_eq!(correction(15.0, 10.0, 1.0), Correction::Seek(10.0));
    }

    #[test]
    fn clock_sync_prefers_lowest_rtt() {
        let mut c = ClockSync::default();
        // host is 5000ms ahead; noisy sample with asymmetric delay
        c.add_sample(0.0, 5_080.0, 200.0); // rtt 200, offset 4980
        c.add_sample(1_000.0, 6_010.0, 1_020.0); // rtt 20, offset 5000
        assert_eq!(c.offset(), 5_000.0);
        assert_eq!(c.rtt(), Some(20.0));
    }

    #[test]
    fn crop_trims_bars_from_a_16x9_frame() {
        // 2.39:1 from 1920x1080: keep the middle 1920x804
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Scope), "1920x804+0+138");
        // 4:3 from 16:9 trims the sides
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Classic), "1440x1080+240+0");
        // the test clip
        assert_eq!(crop_geometry(320, 180, 320, 180, Crop::Scope), "320x134+0+23");
    }

    #[test]
    fn crop_is_a_no_op_when_the_shape_already_matches() {
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Wide), "");
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Original), "");
        assert_eq!(crop_geometry(0, 0, 0, 0, Crop::Scope), "");
    }

    #[test]
    fn crop_handles_anamorphic_video() {
        // DVD: 720x480 coded, shown as 16:9 (853x480). 2.39:1 trims rows only.
        assert_eq!(crop_geometry(720, 480, 853, 480, Crop::Scope), "720x356+0+62");
        // 4:3 trims coded columns in proportion: 720 * (4/3) / (853/480) ≈ 540
        assert_eq!(crop_geometry(720, 480, 853, 480, Crop::Classic), "540x480+90+0");
    }

    #[test]
    fn crop_serialises_as_snake_case() {
        assert_eq!(serde_json::to_string(&Crop::Scope).unwrap(), "\"scope\"");
        assert_eq!(serde_json::to_string(&Crop::Imax143).unwrap(), "\"imax_143\"");
        // The UI's list must match: every preset round-trips.
        for c in Crop::ALL {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<Crop>(&json).unwrap(), c);
        }
    }

    #[test]
    fn imax_and_cinema_crops_from_a_16x9_frame() {
        // IMAX 1.43:1 is taller than 16:9, so it trims the sides.
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Imax143), "1544x1080+188+0");
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Euro166), "1792x1080+64+0");
        // The rest are wider, so they trim top and bottom.
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Imax190), "1920x1010+0+35");
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Univisium200), "1920x960+0+60");
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::ToddAo220), "1920x872+0+104");
        assert_eq!(crop_geometry(1920, 1080, 1920, 1080, Crop::Ultra276), "1920x696+0+192");
        // An IMAX 1.90 release cropped to 1.90 is already that shape.
        assert_eq!(crop_geometry(1920, 1010, 1920, 1010, Crop::Imax190), "");
    }

    #[test]
    fn presets_run_tallest_to_widest() {
        let aspects: Vec<f64> = Crop::ALL.iter().filter_map(|c| c.aspect()).collect();
        assert!(aspects.windows(2).all(|w| w[0] < w[1]), "{aspects:?}");
    }

    #[test]
    fn formats_time() {
        assert_eq!(format_time(65.9), "1:05");
        assert_eq!(format_time(10_355.69), "2:52:35");
    }
}
