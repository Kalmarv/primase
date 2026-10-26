//! The World data the Shaun White Skateboarding reader attaches (`shaunwhite.*`).

use std::collections::BTreeMap;

use primase_core::{Component, Entity, Geometry};

/// Where a placement comes from: its cell entry, its Entity object's id, and its Visual's ResourceRef kind byte (0xfe
/// or 0xff; its meaning is not read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedBy {
    pub cell: String,
    pub entity: u32,
    pub kind: u8,
}

impl Component for PlacedBy {
    const NAME: &'static str = "shaunwhite.placed_by";
    const GEOMETRY: Geometry = Geometry::None;
}

/// A Material as the source stores it: its id, its object's name (empty when no forge read holds it), and the
/// textures of the slots read (`diffuse`, `normal`, `specular`: `core.texture` entities). Lowered by
/// `lower-shaunwhite-material`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialRef {
    pub id: u32,
    pub name: String,
    pub textures: BTreeMap<String, Entity>,
}

impl Component for MaterialRef {
    const NAME: &'static str = "shaunwhite.material";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures.values().copied());
    }
}
