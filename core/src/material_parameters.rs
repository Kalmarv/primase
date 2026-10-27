//! Material defaults beyond PBR. Names and meanings belong to the source lowering, not to core.
use crate::{Component, Entity, Geometry, JsonError, World};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// `core.material_parameters`, on a material: named rest values, one to four finite floats each.
/// A `ClipTarget::MaterialParameter` channel replaces a named value at its declared width. With
/// no driving channel the material retains this default. Core assigns no visibility, colour-space,
/// buffer-address or event-layout meaning to a name; the game's lowering states those separately.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialParameters(pub BTreeMap<String, Vec<f32>>);

impl Component for MaterialParameters {
    const NAME: &'static str = "core.material_parameters";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        for (name, values) in &self.0 {
            if name.trim().is_empty() {
                return Err("empty material parameter name".into());
            }
            if !(1..=4).contains(&values.len()) {
                return Err(format!("{name}: {} values, expected 1 to 4", values.len()));
            }
            if !values.iter().all(|value| value.is_finite()) {
                return Err(format!("{name}: non-finite value"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn widths_are_one_to_four_and_values_are_finite() {
        let mut w = World::new();
        let e = w.spawn();
        for width in 0..=5 {
            let p = MaterialParameters(BTreeMap::from([("opaque name".into(), vec![1.; width])]));
            assert_eq!(p.verify(e, &w).is_ok(), (1..=4).contains(&width));
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let p = MaterialParameters(BTreeMap::from([("value".into(), vec![value])]));
            assert!(p.verify(e, &w).is_err());
            assert!(p.to_json().is_err());
        }
        assert!(
            MaterialParameters(BTreeMap::from([(" ".into(), vec![0.])]))
                .verify(e, &w)
                .is_err()
        );
        assert!(MaterialParameters::default().verify(e, &w).is_ok());
    }
    #[test]
    fn json_round_trip_preserves_names_values_and_signed_zero() {
        let p = MaterialParameters(BTreeMap::from([
            ("a tint".into(), vec![0.1, 0.2, 0.3, 1.]),
            ("a scalar".into(), vec![-0.]),
        ]));
        let restored: MaterialParameters = serde_json::from_value(p.to_json().unwrap().unwrap()).unwrap();
        assert_eq!(restored, p);
        assert_eq!(restored.0["a scalar"][0].to_bits(), (-0f32).to_bits());
        // Material parameters describe no primitive or local-space geometry and therefore have no remap.
        assert!(p.remap_primitives(&[2, 0, 2]).is_none());
    }
}
