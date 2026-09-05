//! Rational time and frame-accurate math.
//!
//! The design rule: "Time is rational, not float. Store `gst::ClockTime`
//! (nanoseconds, u64) and frame positions as `i64` frame indices against a
//! `Rational` fps. Float seconds anywhere in the edit model is how
//! frame-accurate trims turn into off-by-one frames at 29.97."
//!
//! This module has zero GStreamer dependency by design (offcut-model is pure,
//! per the crate layout) — it stores time the same way GStreamer
//! does (nanoseconds) so the boundary crossing into offcut-engine is a plain
//! `u64` copy, not a unit conversion that can drift.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A duration in nanoseconds, matching `gst::ClockTime`'s representation
/// exactly so offcut-engine never has to convert units at the boundary.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Time(pub u64);

impl Time {
    pub const ZERO: Time = Time(0);
    const NANOS_PER_SEC: u64 = 1_000_000_000;

    pub const fn from_nanos(nanos: u64) -> Self {
        Time(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / Self::NANOS_PER_SEC as f64
    }

    /// Checked subtraction: returns `None` instead of panicking/wrapping on
    /// underflow. Every caller in this crate that computes a clip span uses
    /// this, never `-`, so an inverted in/out point is caught at the call
    /// site instead of becoming a silently wrapped u64.
    pub fn checked_sub(self, rhs: Time) -> Option<Time> {
        self.0.checked_sub(rhs.0).map(Time)
    }

    pub fn saturating_sub(self, rhs: Time) -> Time {
        Time(self.0.saturating_sub(rhs.0))
    }

    pub fn checked_add(self, rhs: Time) -> Option<Time> {
        self.0.checked_add(rhs.0).map(Time)
    }

    /// Divide a duration by a speed factor to get timeline duration.
    /// `factor` is always positive and finite for every `Speed` variant
    /// (see `Speed::factor`), so this never produces NaN/inf in practice;
    /// it's still a checked path because `Time` must never silently become
    /// a poisoned value that later corrupts a golden-file export hash.
    pub fn div_f64(self, factor: f64) -> Time {
        debug_assert!(factor.is_finite() && factor > 0.0, "speed factor must be positive and finite");
        Time((self.0 as f64 / factor).round() as u64)
    }
}

impl fmt::Debug for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Time({:.3}s)", self.as_secs_f64())
    }
}

/// An exact frame rate, e.g. 30000/1001 for 29.97 fps ("NTSC" rates).
/// Storing this as a fraction rather than a float is what keeps frame-index
/// math exact — `30000.0 / 1001.0` as an `f64` is not exactly representable
/// and accumulates error over a long timeline; the fraction never does.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// Common broadcast/phone-camera rates, for tests and UI display.
    pub const NTSC_FILM: Rational = Rational::new(24000, 1001); // 23.976
    pub const NTSC: Rational = Rational::new(30000, 1001); // 29.97
    pub const NTSC_60: Rational = Rational::new(60000, 1001); // 59.94
    pub const FILM: Rational = Rational::new(24, 1);
    pub const PAL: Rational = Rational::new(25, 1);
    pub const WEB_30: Rational = Rational::new(30, 1);
    pub const WEB_60: Rational = Rational::new(60, 1);

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Duration of exactly one frame at this rate.
    pub fn frame_duration(self) -> Time {
        Time((Time::from_nanos(1_000_000_000).as_nanos() as u128 * self.den as u128
            / self.num as u128) as u64)
    }

    /// Convert a frame index to a timestamp. This is the canonical
    /// direction (frame -> time); the reverse (`time_to_frame`) is
    /// intentionally *not* provided as a free function, because rounding a
    /// timestamp back to a frame index is exactly where "which frame is the
    /// playhead really on" bugs live — every caller that needs it should
    /// round explicitly and visibly (see `time_to_frame_floor`).
    ///
    /// Rounds the fractional-nanosecond remainder **up** (ceiling), not
    /// down. This is not cosmetic: at nanosecond granularity,
    /// `frame * 1e9 * den / num` is essentially never an exact integer for
    /// NTSC-family rates, so *some* rounding is unavoidable. Truncating
    /// down (the naive choice) produces a timestamp that is a fraction of
    /// a nanosecond *before* the frame's true start; `time_to_frame_floor`
    /// then correctly floors that back to the *previous* frame index,
    /// silently losing a frame on every non-exact rate. Rounding up
    /// guarantees the returned timestamp is never earlier than the true
    /// frame start, so flooring it always recovers the same frame — the
    /// overshoot is bounded by under 1ns, i.e. roughly 1e-8 of a frame's
    /// duration at 30fps, far below any meaningful precision. Caught by
    /// `frame_to_time_roundtrips_at_*` below; this comment exists so the
    /// next person to "simplify" this back to floor division reads it
    /// first.
    pub fn frame_to_time(self, frame: i64) -> Time {
        let numer = frame as i128 * 1_000_000_000i128 * self.den as i128;
        let denom = self.num as i128;
        if numer <= 0 {
            return Time::ZERO;
        }
        // Ceiling division for positive numer/denom: (numer + denom - 1) / denom.
        let nanos = (numer + denom - 1) / denom;
        Time(nanos as u64)
    }

    /// Floor-rounds a timestamp to the frame index that is playing at that
    /// instant. Named `_floor` deliberately: a timestamp that lands exactly
    /// between two frame boundaries (should not happen with exact rational
    /// math, but *can* happen at an engine boundary that only gives us a
    /// rounded gst position) always resolves to the earlier frame, matching
    /// what a player actually displays.
    pub fn time_to_frame_floor(self, time: Time) -> i64 {
        ((time.as_nanos() as i128 * self.num as i128) / (self.den as i128 * 1_000_000_000i128)) as i64
    }
}

impl fmt::Debug for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{} ({:.3}fps)", self.num, self.den, self.as_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_duration_2997_is_not_exactly_33ms() {
        // The classic float trap: 1/29.97 ~= 33.366...ms, not 33ms flat.
        // If this were computed as `1.0 / 29.97 * 1e9` in f64 and summed
        // over a long timeline, drift accumulates. Exact rational math does
        // not drift regardless of how many frames you sum.
        let d = Rational::NTSC.frame_duration();
        // 1001/30000 s = 33_366_666.66ns, rounds to 33_366_666ns (integer
        // division truncates here by construction of frame_duration).
        assert_eq!(d.as_nanos(), 33_366_666);
    }

    #[test]
    fn frame_to_time_roundtrips_at_2997_over_long_timeline() {
        // Sum 10 minutes of frames at 29.97 the "exact" way (frame index *
        // rational) and confirm frame_to_time -> time_to_frame_floor
        // recovers the same frame index for every frame in a 10-minute
        // timeline. This is the test that would catch float-seconds drift.
        let fps = Rational::NTSC;
        let total_frames = (10 * 60) as i64 * 30; // ~10 min at ~30fps nominal
        for frame in 0..total_frames {
            let t = fps.frame_to_time(frame);
            let recovered = fps.time_to_frame_floor(t);
            assert_eq!(
                recovered, frame,
                "frame {frame} round-tripped to {recovered} at 29.97fps after summing {frame} frames"
            );
        }
    }

    #[test]
    fn frame_to_time_roundtrips_at_23976() {
        let fps = Rational::NTSC_FILM;
        for frame in 0..(5 * 60 * 24) {
            let t = fps.frame_to_time(frame);
            assert_eq!(fps.time_to_frame_floor(t), frame);
        }
    }

    #[test]
    fn frame_to_time_roundtrips_at_5994() {
        let fps = Rational::NTSC_60;
        for frame in 0..(2 * 60 * 60) {
            let t = fps.frame_to_time(frame);
            assert_eq!(fps.time_to_frame_floor(t), frame);
        }
    }

    #[test]
    fn checked_sub_none_on_underflow() {
        assert_eq!(Time::from_nanos(5).checked_sub(Time::from_nanos(10)), None);
        assert_eq!(
            Time::from_nanos(10).checked_sub(Time::from_nanos(5)),
            Some(Time::from_nanos(5))
        );
    }

    #[test]
    fn div_f64_half_speed_doubles_duration() {
        let d = Time::from_nanos(1_000_000_000); // 1s
        assert_eq!(d.div_f64(0.5).as_nanos(), 2_000_000_000);
        assert_eq!(d.div_f64(2.0).as_nanos(), 500_000_000);
        assert_eq!(d.div_f64(4.0).as_nanos(), 250_000_000);
    }
}
