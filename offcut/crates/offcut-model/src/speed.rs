//! The design rule: "Four discrete chips: 0.5x 1x 2x 4x. No slider — a closed
//! set is Samsung's actual behavior and it removes an entire class of
//! 'why is my export 47 minutes' bug." §4.3 also decides: "4x implies
//! muted audio" — that rule lives here (`Speed::implies_mute`) so the UI,
//! engine, and export path all consult one source of truth instead of each
//! re-deciding it.

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Speed {
    Half,
    #[default]
    One,
    Two,
    Four,
}

impl Speed {
    pub const ALL: [Speed; 4] = [Speed::Half, Speed::One, Speed::Two, Speed::Four];

    pub fn factor(self) -> f64 {
        match self {
            Speed::Half => 0.5,
            Speed::One => 1.0,
            Speed::Two => 2.0,
            Speed::Four => 4.0,
        }
    }

    /// the decision, encoded once: "At 4x, pitch-preserved audio
    /// is unintelligible anyway — decision: 4x implies muted audio."
    pub fn implies_mute(self) -> bool {
        matches!(self, Speed::Four)
    }

    /// The GStreamer `pitch` element's `tempo` property value for this
    /// speed (the design rule, §4.3: `soundtouch`'s `pitch tempo=` replaces the
    /// unavailable `scaletempo`). Named distinctly from `factor` even
    /// though the value is identical today, because the day this needs to
    /// diverge from the raw playback-rate factor (e.g. a future clamp),
    /// every call site is already using the right accessor.
    pub fn audio_tempo(self) -> f64 {
        self.factor()
    }

    pub fn label(self) -> &'static str {
        match self {
            Speed::Half => "0.5×",
            Speed::One => "1×",
            Speed::Two => "2×",
            Speed::Four => "4×",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_four_implies_mute() {
        assert!(!Speed::Half.implies_mute());
        assert!(!Speed::One.implies_mute());
        assert!(!Speed::Two.implies_mute());
        assert!(Speed::Four.implies_mute());
    }

    #[test]
    fn factors_are_the_documented_closed_set() {
        assert_eq!(Speed::Half.factor(), 0.5);
        assert_eq!(Speed::One.factor(), 1.0);
        assert_eq!(Speed::Two.factor(), 2.0);
        assert_eq!(Speed::Four.factor(), 4.0);
    }

    #[test]
    fn default_is_one_x() {
        assert_eq!(Speed::default(), Speed::One);
    }
}
