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
    fn formats_time() {
        assert_eq!(format_time(65.9), "1:05");
        assert_eq!(format_time(10_355.69), "2:52:35");
    }
}
