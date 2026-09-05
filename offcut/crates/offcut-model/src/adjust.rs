//! The design rule (Adjust, added per the product's scope expansion, hard-capped):
//! "Five sliders, fixed order, 0-100 each: Smooth, Tint, Skin tone, Blue
//! tone, Vignette. This is the *entire* surface — the product rules explicitly
//! forbids a sixth control or a curves/wheels affordance; do not let this
//! section grow."
//!
//! This struct is deliberately closed (no `Vec<Adjustment>`, no builder
//! that could accept a new named field without a compile error at every
//! call site) so that "add a sixth slider" costs a visible diff here, not
//! a quiet extension.

use serde::{Deserialize, Serialize};

/// A single 0..=100 adjustment value. Exists as its own type (rather than
/// five bare `u8` fields) so the shader-uniform boundary (offcut-render)
/// gets a single, testable `as_uniform` conversion instead of five
/// hand-written `/100.0`s that can drift out of sync with each other.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AdjustValue(u8);

impl AdjustValue {
    pub const MIN: u8 = 0;
    pub const MAX: u8 = 100;
    pub const ZERO: AdjustValue = AdjustValue(0);

    pub fn new(value: u8) -> Self {
        AdjustValue(value.min(Self::MAX))
    }

    pub fn get(self) -> u8 {
        self.0
    }

    /// Normalized `0.0..=1.0` float for the shader uniform. The design rule:
    /// "five `f32` uniforms in the same display/export fragment shader."
    /// Guaranteed finite and in-range by construction (`new` clamps),
    /// which is exactly the requirement: "adjust-uniform clamping
    /// (0..=100 never escapes as a shader NaN/negative)."
    pub fn as_uniform(self) -> f32 {
        self.0 as f32 / Self::MAX as f32
    }
}

impl Default for AdjustValue {
    fn default() -> Self {
        AdjustValue::ZERO
    }
}

/// the `AdjustSettings` — the exact five fields the design system names
/// in fixed order, and nothing else.
#[derive(Copy, Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct AdjustSettings {
    pub smooth: AdjustValue,
    pub tint: AdjustValue,
    pub skin_tone: AdjustValue,
    pub blue_tone: AdjustValue,
    pub vignette: AdjustValue,
}

impl AdjustSettings {
    /// The design system's reference render ships Skin tone 32, Vignette 18, the
    /// rest 0 — this constructor documents that as the mockup's example
    /// state, used by offcut-ui's fixture/preview data, not a claim about
    /// what a new clip should default to (new clips default to
    /// `AdjustSettings::default()`, all zero).
    pub fn mockup_reference() -> Self {
        Self {
            smooth: AdjustValue::default(),
            tint: AdjustValue::default(),
            skin_tone: AdjustValue::new(32),
            blue_tone: AdjustValue::default(),
            vignette: AdjustValue::new(18),
        }
    }

    /// True iff every slider is at zero — the "Reset all" target state
    /// (the design system's Adjust tab) and the state the perf gate
    /// compares against: "a frame-time delta between Crop/Adjust at rest
    /// vs. active (must be ~0)."
    pub fn is_at_rest(self) -> bool {
        self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_zero_and_at_rest() {
        let s = AdjustSettings::default();
        assert!(s.is_at_rest());
        assert_eq!(s.smooth.get(), 0);
        assert_eq!(s.vignette.get(), 0);
    }

    #[test]
    fn value_clamps_above_max_never_panics() {
        let v = AdjustValue::new(255);
        assert_eq!(v.get(), 100);
        assert_eq!(v.as_uniform(), 1.0);
    }

    #[test]
    fn uniform_is_always_finite_and_in_unit_range() {
        for raw in 0u16..=255 {
            let v = AdjustValue::new(raw as u8);
            let u = v.as_uniform();
            assert!(u.is_finite(), "raw={raw} produced non-finite uniform");
            assert!((0.0..=1.0).contains(&u), "raw={raw} -> uniform {u} out of range");
        }
    }

    #[test]
    fn mockup_reference_is_not_at_rest() {
        assert!(!AdjustSettings::mockup_reference().is_at_rest());
    }
}
