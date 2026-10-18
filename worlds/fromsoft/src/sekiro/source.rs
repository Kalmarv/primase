//! Typed source identities and records the area reader keeps for the passes that interpret them. They describe what
//! the source stores and how it names it, never a target format's fields.
use primase_core::json::{JsonError, to_value};
use primase_core::{Component, Entity, Resource};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// A prototype's source model: `<map>/<model>` for map pieces, `obj/<model>` for objects; `far` for a map piece of
/// the area's distant-backdrop series (`m9xxxxx`), which the game draws only from afar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PrototypeSource {
    pub id: String,
    pub far: bool,
}
impl Component for PrototypeSource {
    const NAME: &'static str = "fromsoft.prototype";
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// The material entity (holding `fromsoft.material`) of each material index of a prototype's `fromsoft.flver`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialSlots(pub Vec<Entity>);
impl Component for MaterialSlots {
    const NAME: &'static str = "fromsoft.material_slots";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(&self.0);
    }
}

/// Which source collision group a `core.collision` soup holds: `game`, `water`, `objects`, or a state-specific
/// group (`game_state0`, ...), and whether the group is water rather than solid ground.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CollisionSource {
    pub group: String,
    pub water: bool,
}
impl Component for CollisionSource {
    const NAME: &'static str = "fromsoft.collision";
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// What a BTL light record stores (`fromsoft.light`), on an entity placed where the light is: its transform is the
/// stored position and rotation through the map offset, in the world frame, so a spot shines along its local -Z.
/// Colours are the stored bytes (their transfer function is an interpretation), powers the stored multipliers
/// (their unit is not established), `cone_angle` the stored half-angle in degrees, `radius` the stored reach in
/// metres. `tangent` is the light's local +X in the world frame, kept so it survives a pass that re-bases the
/// transform.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightSource {
    pub name: String,
    pub kind: LightKind,
    pub diffuse: [u8; 3],
    pub diffuse_power: f32,
    pub specular: [u8; 3],
    pub specular_power: f32,
    pub cast_shadows: bool,
    pub cone_angle: f32,
    pub radius: f32,
    pub tangent: [f32; 3],
}
/// A BTL light's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LightKind {
    Point,
    Spot,
    Directional,
}
impl Component for LightSource {
    const NAME: &'static str = "fromsoft.light";
    /// Placed by its entity's transform; the tangent is world space.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// One MSB spawn point inside the read region: its name as stored, position in the world frame (metres) and stored
/// rotation in degrees (its facing convention is not established).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SpawnPoint {
    pub name: String,
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
}

/// A spawn point's stored rotation in degrees (`fromsoft.spawn_point`), on the marker `lower-fromsoft-extras` makes
/// of it: the marker's transform places it but does not turn it, as the facing convention is not established.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SpawnRotation {
    pub rotation_degrees: [f32; 3],
}
impl Component for SpawnRotation {
    const NAME: &'static str = "fromsoft.spawn_point";
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// One stored hang edge: its endpoints in the world frame, in edge order, and the edge file it came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HangSegment {
    pub file: String,
    pub points: [[f32; 3]; 2],
}

/// Area extras without a core concept until a pass lowers them, in source order: every hang-edge segment of the
/// area (the region does not select them) and the spawn points inside the region.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AreaExtras {
    pub hang_segments: Vec<HangSegment>,
    pub spawn_points: Vec<SpawnPoint>,
}
impl Resource for AreaExtras {
    const NAME: &'static str = "fromsoft.area_extras";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// Per primitive of a prototype, the FLVER Byte4C UV members it carries as raw vertex data: member `n` is the
/// primitive's `Color(n)` stream, UNORM8 exactly as stored. The layered M[Multiple] shaders read these as layer
/// weights, snow, tint and detail; their meaning belongs to the passes that evaluate those shaders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LayerInputs {
    pub primitives: Vec<Vec<u8>>,
}
impl Component for LayerInputs {
    const NAME: &'static str = "fromsoft.layer_inputs";
    /// Per-vertex shader inputs, not positions or directions.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.primitives, from).map(|primitives| LayerInputs { primitives }))
    }
}
impl LayerInputs {
    /// Remove every prototype's layer inputs, the component and the `Color(n)` streams it lists, from every
    /// primitive of its mesh (a pass may have split them). They are shader inputs, not vertex colours: a pass that
    /// consumed them, or lowered their materials without them, takes them out. Returns the streams removed.
    pub fn remove_all(world: &mut primase_core::World) -> u64 {
        let owners: Vec<_> = world.iter::<LayerInputs>().map(|(e, _)| e).collect();
        let mut removed = 0;
        for e in owners {
            let Some(inputs) = world.remove::<LayerInputs>(e) else {
                continue;
            };
            let members: std::collections::BTreeSet<u8> = inputs.primitives.into_iter().flatten().collect();
            if let Some(mesh) = world.get_mut::<primase_core::Mesh>(e) {
                for primitive in mesh.primitives_mut() {
                    for &n in &members {
                        removed += u64::from(
                            primitive
                                .streams
                                .remove(&primase_core::Semantic::Color(n))
                                .is_some(),
                        );
                    }
                }
            }
        }
        removed
    }
}

/// The FLVER Byte4C UV members of one mesh as `(member index, values per selected vertex)`, in member order.
/// `vertices` are the source vertex indices of the selected (compacted) vertices.
pub fn byte4_uv_members(
    model: &crate::flver::Model,
    mesh: usize,
    vertices: &[u32],
) -> crate::Result<Vec<(u8, Vec<[u8; 4]>)>> {
    use crate::invalid;
    let m = model
        .meshes
        .get(mesh)
        .ok_or_else(|| invalid("FLVER layer inputs", mesh, "mesh index"))?;
    let mut members = Vec::new();
    let mut slot = 0;
    for &layout in &m.layouts {
        for member in model
            .layouts
            .get(layout)
            .ok_or_else(|| invalid("FLVER layer inputs", layout, "layout index"))?
        {
            if member.semantic == 5 {
                if member.kind == 0x13 {
                    let index = u8::try_from(member.index)
                        .map_err(|_| invalid("FLVER layer inputs", layout, "member index"))?;
                    members.push((index, slot));
                }
                slot += if matches!(member.kind, 3 | 0x16) { 2 } else { 1 };
            }
        }
    }
    members
        .into_iter()
        .map(|(index, slot)| {
            let values = vertices
                .iter()
                .map(|&v| {
                    let vertex = m
                        .vertices
                        .get(v as usize)
                        .ok_or_else(|| invalid("FLVER layer inputs", v as usize, "vertex index"))?;
                    vertex
                        .layer_bytes
                        .iter()
                        .find(|(s, _)| *s == slot)
                        .map(|(_, b)| *b)
                        .ok_or_else(|| invalid("FLVER layer inputs", slot, "missing declared Byte4C payload"))
                })
                .collect::<crate::Result<Vec<_>>>()?;
            Ok((index, values))
        })
        .collect()
}

/// Source texture files by stem, as stored (DDS bytes), for every texture a selected material binds. Kept for
/// the passes that lower or bake materials; not serialised.
#[derive(Clone, Debug, Default)]
pub struct TexturePayloads(pub BTreeMap<String, Vec<u8>>);
impl Resource for TexturePayloads {
    const NAME: &'static str = "fromsoft.texture_payloads";
}

/// The G-buffer pixel shader programs (`<shader>_<switch>_Gbuf.ppo`, DXBC as stored) of the selected layered
/// materials, for passes that evaluate them. Not serialised.
#[derive(Clone, Debug, Default)]
pub struct ShaderPrograms(pub BTreeMap<String, Vec<u8>>);
impl Resource for ShaderPrograms {
    const NAME: &'static str = "fromsoft.shader_programs";
}
