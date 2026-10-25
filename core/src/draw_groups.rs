//! Clip-driven draw groups, distinct from static state conditions. Hidden defaults do not remove geometry.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{Component, Entity, Geometry, JsonError, Mesh, World};

/// `core.draw_groups`, on a mesh: the group of each primitive, or none (unaffected by group channels).
/// A clip addresses only groups of the same exact `scope`. Group numbers are 0..64, indexing the
/// occurrence's [`DrawGroupDefaults::shown`] mask. A lowering resolves a source's group vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawGroups {
    pub scope: String,
    pub groups: Vec<Option<u32>>,
}

/// `core.draw_group_defaults`, on a drawn occurrence or rig: visibility with no clip, or when a
/// channel inherits. Bit n states whether group n is shown. Defaults belong to the occurrence,
/// so two occurrences of one mesh may have different defaults without duplicating its groups.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawGroupDefaults {
    pub scope: String,
    pub shown: u64,
}

impl Component for DrawGroups {
    const NAME: &'static str = "core.draw_groups";
    const GEOMETRY: Geometry = Geometry::PerPrimitive;
    fn to_json(&self) -> Result<Option<Value>, JsonError> { crate::json::to_value(self).map(Some) }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(crate::remap_list(&self.groups, from).map(|groups| Self { scope: self.scope.clone(), groups }))
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if self.scope.trim().is_empty() { return Err("empty draw-group scope".into()); }
        let mesh = w.get::<Mesh>(e).ok_or("draw groups on an entity without core.mesh")?;
        if self.groups.len() != mesh.primitives().len() {
            return Err(format!("{} groups for {} primitives", self.groups.len(), mesh.primitives().len()));
        }
        if let Some((primitive, group)) = self.groups.iter().enumerate()
            .find_map(|(i, group)| group.filter(|group| *group >= 64).map(|group| (i, group))) {
            return Err(format!("primitive {primitive}: group {group} is outside the 64-bit default mask"));
        }
        Ok(())
    }
}

impl Component for DrawGroupDefaults {
    const NAME: &'static str = "core.draw_group_defaults";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> { crate::json::to_value(self).map(Some) }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.scope.trim().is_empty() { return Err("empty draw-group default scope".into()); }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Primitive, Topology};
    fn mesh() -> (World, Entity) {
        let mut w = World::new(); let e = w.spawn();
        w.insert(e, Mesh::new((0..3).map(|_| Primitive { topology: Topology::Triangles,
            streams: Default::default(), indices: None, material: None }).collect()));
        (w, e)
    }
    fn groups() -> DrawGroups { DrawGroups { scope: "character".into(), groups: vec![Some(0), None, Some(63)] } }
    #[test]
    fn groups_validate_count_scope_and_mask_width() {
        let (w,e) = mesh(); let mut g = groups();
        assert!(g.verify(e,&w).is_ok());
        g.groups[2] = Some(64); assert!(g.verify(e,&w).is_err());
        g.groups[2] = Some(63); g.groups.pop(); assert!(g.verify(e,&w).is_err());
        let mut g = groups(); g.scope.clear(); assert!(g.verify(e,&w).is_err());
        let mut empty = World::new(); let e = empty.spawn(); assert!(groups().verify(e,&empty).is_err());
    }
    #[test]
    fn primitive_remap_reorders_repeats_drops_and_refuses_out_of_range() {
        let g = groups();
        let mapped = g.remap_primitives(&[2,0,2,1]).unwrap().unwrap();
        assert_eq!(mapped.groups, vec![Some(63),Some(0),Some(63),None]);
        assert_eq!(mapped.scope,g.scope);
        assert!(g.remap_primitives(&[]).unwrap().unwrap().groups.is_empty());
        assert!(g.remap_primitives(&[3]).unwrap().is_err());
        assert_eq!(g.groups,vec![Some(0),None,Some(63)]);
    }
    #[test]
    fn defaults_preserve_every_bit_and_belong_to_a_scope() {
        let (w,e) = mesh(); let mut d = DrawGroupDefaults { scope: "character".into(), shown: 1u64 << 63 };
        assert!(d.verify(e,&w).is_ok());
        assert!(d.remap_primitives(&[2,0]).is_none());
        let json = d.to_json().unwrap().unwrap();
        assert_eq!(serde_json::from_value::<DrawGroupDefaults>(json).unwrap(),d);
        d.scope = " ".into(); assert!(d.verify(e,&w).is_err());
    }
    #[test]
    fn group_json_round_trip_keeps_none_and_order() {
        let g = groups(); let json = g.to_json().unwrap().unwrap();
        assert_eq!(json,serde_json::json!({"scope":"character","groups":[0,null,63]}));
        assert_eq!(serde_json::from_value::<DrawGroups>(json).unwrap(),g);
    }
}
