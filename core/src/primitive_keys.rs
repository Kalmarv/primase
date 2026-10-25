//! Values a shader's variant key reads from each primitive, independent of the game's mesh component.
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{Component, Entity, Geometry, JsonError, Mesh, World};

/// `core.primitive_keys`, on a mesh: one u32 per primitive for every named field. A
/// `shader::VariantKey::field` names one of these exact keys; core does not interpret its namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimitiveKeys(pub BTreeMap<String, Vec<u32>>);

impl Component for PrimitiveKeys {
    const NAME: &'static str = "core.primitive_keys";
    const GEOMETRY: Geometry = Geometry::PerPrimitive;
    fn to_json(&self) -> Result<Option<Value>, JsonError> { crate::json::to_value(self).map(Some) }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self,String>> {
        Some(self.0.iter().map(|(field,values)| {
            crate::remap_list(values,from).map(|values|(field.clone(),values))
                .map_err(|error|format!("{field}: {error}"))
        }).collect::<Result<BTreeMap<_,_>,_>>().map(Self))
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(),String> {
        let mesh = w.get::<Mesh>(e).ok_or("primitive keys on an entity without core.mesh")?;
        for (field,values) in &self.0 {
            if field.trim().is_empty() { return Err("empty primitive-key field".into()); }
            if values.len() != mesh.primitives().len() {
                return Err(format!("{field}: {} values for {} primitives",values.len(),mesh.primitives().len()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Primitive, Topology};
    fn fixture() -> PrimitiveKeys { PrimitiveKeys(BTreeMap::from([
        ("first.variant".into(),vec![0,1,2]),("another.layout".into(),vec![3,4,u32::MAX])])) }
    #[test]
    fn each_field_matches_the_mesh_and_names_are_not_empty() {
        let mut w = World::new(); let e = w.spawn(); let mut keys = fixture();
        assert!(keys.verify(e,&w).is_err());
        w.insert(e,Mesh::new((0..3).map(|_|Primitive { topology:Topology::Triangles,
            streams:Default::default(),indices:None,material:None }).collect()));
        assert!(keys.verify(e,&w).is_ok());
        keys.0.insert(" ".into(),vec![0;3]); assert!(keys.verify(e,&w).is_err());
        keys.0.remove(" "); keys.0.insert("short".into(),vec![0]); assert!(keys.verify(e,&w).is_err());
    }
    #[test]
    fn every_field_remaps_together_without_partial_results() {
        let keys = fixture(); let mapped = keys.remap_primitives(&[2,0,2]).unwrap().unwrap();
        assert_eq!(mapped.0["first.variant"],vec![2,0,2]);
        assert_eq!(mapped.0["another.layout"],vec![u32::MAX,3,u32::MAX]);
        assert!(keys.remap_primitives(&[3]).unwrap().is_err());
        assert!(keys.remap_primitives(&[]).unwrap().unwrap().0.values().all(Vec::is_empty));
        assert_eq!(keys,fixture());
    }
    #[test]
    fn json_round_trip_keeps_fields_order_and_full_u32_values() {
        let keys = fixture(); let json = keys.to_json().unwrap().unwrap();
        assert_eq!(serde_json::from_value::<PrimitiveKeys>(json).unwrap(),keys);
    }
    #[test]
    fn rebuilding_a_mesh_carries_both_key_fields_and_draw_groups_atomically() {
        let mut w = World::new(); let e = w.spawn();
        w.insert(e,Mesh::new((0..3).map(|_|Primitive { topology:Topology::Triangles,
            streams:Default::default(),indices:None,material:None }).collect()));
        w.insert(e,fixture());
        w.insert(e,crate::DrawGroups { scope:"character".into(),groups:vec![Some(0),None,Some(63)] });
        w.rebuild_primitives(e,|mesh|Ok((vec![mesh.primitives()[2].clone(),mesh.primitives()[0].clone()],vec![2,0]))).unwrap();
        assert_eq!(w.get::<PrimitiveKeys>(e).unwrap().0["first.variant"],vec![2,0]);
        assert_eq!(w.get::<PrimitiveKeys>(e).unwrap().0["another.layout"],vec![u32::MAX,3]);
        assert_eq!(w.get::<crate::DrawGroups>(e).unwrap().groups,vec![Some(63),Some(0)]);
        assert!(w.get::<PrimitiveKeys>(e).unwrap().verify(e,&w).is_ok());
        assert!(w.get::<crate::DrawGroups>(e).unwrap().verify(e,&w).is_ok());
        let before_keys = w.get::<PrimitiveKeys>(e).unwrap().clone();
        let before_groups = w.get::<crate::DrawGroups>(e).unwrap().clone();
        assert!(w.rebuild_primitives(e,|mesh|Ok((vec![mesh.primitives()[0].clone()],vec![2]))).is_err());
        assert_eq!(w.get::<Mesh>(e).unwrap().primitives().len(),2);
        assert_eq!(w.get::<PrimitiveKeys>(e),Some(&before_keys));
        assert_eq!(w.get::<crate::DrawGroups>(e),Some(&before_groups));
    }
}
