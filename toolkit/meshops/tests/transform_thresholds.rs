//! `transform` classifies a basis in f32 with the thresholds `Transform::classify` names once in core.

use primase_core::{DEGENERATE_BASIS, SHEAR_TOLERANCE};

/// The f64 constants rounded to f32 are the f32 literals the operation used before they were shared, so the recorded operation
/// bits are unchanged.
#[test]
fn the_shared_thresholds_are_the_f32_literals_the_fixtures_were_recorded_with() {
    assert_eq!((DEGENERATE_BASIS as f32).to_bits(), 1e-6_f32.to_bits());
    assert_eq!((SHEAR_TOLERANCE as f32).to_bits(), 1e-4_f32.to_bits());
}
