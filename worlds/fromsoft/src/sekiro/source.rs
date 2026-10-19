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

/// Which group `lower-fromsoft-collision` or `lower-fromsoft-object-collision` lowered a `core.collision` soup into:
/// `game`, `water`, `objects`, or a state-specific group (`game_state0`, ...), and whether the group is water rather
/// than solid ground.
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

/// One MSB collision part as stored: its name, its hit filter and, with states read, its clause (`None`: no rule
/// decides it, or states were not read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CollisionPart {
    pub name: String,
    pub hit_filter: u8,
    pub condition: Option<primase_core::state::Clause>,
}

/// A file of the area's high-resolution map collision binder that MSB collision parts name (`fromsoft.map_collision`,
/// on an entity of its own), whole: every triangle of its compressed-mesh bodies posed by its body, through the map
/// offset into the world frame (metres), each vertex where the source puts it and in its stored order (the frame is a
/// reflection of the source's, so a stored triangle faces away from its stored winding), and per triangle its
/// HitMtrlParam ID as the shape key binds it (-1: the key binds none). The parts name it in MSB order, of every hit
/// filter. Selecting parts, mapping HitMtrl IDs to surfaces and cropping are `lower-fromsoft-collision`'s.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MapCollision {
    /// The member's file name (`h11_01_00_00_009500.hkx`).
    pub file: String,
    pub parts: Vec<CollisionPart>,
    pub triangles: Vec<[[f32; 3]; 3]>,
    pub materials: Vec<i32>,
    /// Shape-key rows the file marks invalid (all ones): they identify no primitive.
    pub invalid_material_keys: usize,
    /// Compressed-mesh primitives the file marks dead.
    pub dead_primitives: usize,
    /// The shape classes of the file's bodies the reader has no decoder for, one per body: not in `triangles`.
    pub unread_bodies: Vec<String>,
}
impl Component for MapCollision {
    const NAME: &'static str = "fromsoft.map_collision";
    /// World-frame triangles on an entity that is not placed.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for part in &self.parts {
            if let Some(c) = &part.condition {
                out.extend(c.0.iter().map(|l| l.axis));
            }
        }
    }
    /// 36 bytes per triangle and 4 per material.
    fn payload_bytes(&self) -> u64 {
        self.triangles.len() as u64 * 36 + self.materials.len() as u64 * 4
    }
    fn verify(&self, _e: Entity, _w: &primase_core::World) -> Result<(), String> {
        if self.materials.len() != self.triangles.len() {
            return Err(format!(
                "{}: {} materials for {} triangles",
                self.file,
                self.materials.len(),
                self.triangles.len()
            ));
        }
        Ok(())
    }
}

/// One rigid body of an object's collision file: its name, its stored motion type (what each value means is a pass's
/// interpretation), its shape class and its geometry as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectBody {
    pub name: String,
    pub motion_type: u64,
    pub shape: String,
    pub geometry: ObjectGeometry,
}

/// A body's stored geometry in its prototype's space (the world frame's axes, as the prototype's model).
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectGeometry {
    /// A compressed mesh: its triangles posed by the body, in stored vertex order (as [`MapCollision`]), and per
    /// triangle its HitMtrlParam ID (-1: none bound).
    Mesh {
        triangles: Vec<[[f32; 3]; 3]>,
        materials: Vec<i32>,
    },
    /// A convex shape (box, polytope, capsule) as stored: its core and convex radius in the body's own space and
    /// the file's axes, a capsule's segment, and `to_prototype`, the columns of the affine map from the body's space
    /// into the prototype's (the body's pose, then the file frame's reflection into the world frame's axes). It
    /// stores no material. Turning it into triangles is a pass's approximation.
    Convex {
        core: primase_havok::ConvexCore,
        capsule: Option<[[f32; 3]; 2]>,
        to_prototype: [[f32; 3]; 4],
    },
}

impl ObjectGeometry {
    /// Whether it is a convex shape, which stores no material.
    pub fn convex(&self) -> bool {
        matches!(self, ObjectGeometry::Convex { .. })
    }
}

/// An object prototype's collision file as stored (`fromsoft.object_collision`): every body of its main HKX, for
/// `lower-fromsoft-object-collision` to place with each placement. A prototype whose binder has no such file has none.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectBodies {
    /// The binder member's file name (`o000100.hkx`).
    pub file: String,
    pub bodies: Vec<ObjectBody>,
}
impl Component for ObjectBodies {
    const NAME: &'static str = "fromsoft.object_collision";
    /// Prototype-space geometry, mapped as the prototype's mesh: mesh triangles moved (a reflection reversing their
    /// order), a convex body's map composed with the mapping.
    fn apply_linear(&self, l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        let mut out = self.clone();
        for body in &mut out.bodies {
            match &mut body.geometry {
                ObjectGeometry::Mesh { triangles, .. } => {
                    for t in triangles {
                        *t = t.map(|p| l.point(p));
                        if l.mirrors() {
                            t.swap(1, 2);
                        }
                    }
                }
                ObjectGeometry::Convex { to_prototype, .. } => {
                    *to_prototype = to_prototype.map(|c| l.point(c));
                }
            }
        }
        primase_core::world::Linearity::Applied(out)
    }
    /// 36 bytes per mesh triangle and 4 per material; a convex core's vertices and planes.
    fn payload_bytes(&self) -> u64 {
        self.bodies
            .iter()
            .map(|b| match &b.geometry {
                ObjectGeometry::Mesh { triangles, materials } => {
                    triangles.len() as u64 * 36 + materials.len() as u64 * 4
                }
                ObjectGeometry::Convex { core, .. } => {
                    core.vertices.len() as u64 * 12 + core.planes.len() as u64 * 16
                }
            })
            .sum()
    }
    fn verify(&self, _e: Entity, _w: &primase_core::World) -> Result<(), String> {
        for b in &self.bodies {
            if let ObjectGeometry::Mesh { triangles, materials } = &b.geometry
                && materials.len() != triangles.len()
            {
                return Err(format!(
                    "{} body {}: {} materials for {} triangles",
                    self.file,
                    b.name,
                    materials.len(),
                    triangles.len()
                ));
            }
        }
        Ok(())
    }
}

/// What a read covered (`fromsoft.read_scope`): the area, the region it was bounded to (east/north rectangle in the
/// world frame) and its placement margin, and whether event-flag world states were read. Passes that crop to the
/// read region read it here, so a program need not restate it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReadScope {
    pub area: String,
    pub region: Option<[f32; 4]>,
    pub region_margin: f32,
    pub states: bool,
}
impl Resource for ReadScope {
    const NAME: &'static str = "fromsoft.read_scope";
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
    /// Take out the layer inputs of the primitives `consumed` selects (by owner, primitive index and primitive): the
    /// `Color(n)` streams their entry lists and the entry itself; the component goes once no entry lists a member.
    /// They are shader inputs, not vertex colours: a pass that consumed them, or lowered their material without them,
    /// takes them out, and only those. Returns the streams removed.
    pub fn consume(
        world: &mut primase_core::World,
        mut consumed: impl FnMut(Entity, usize, &primase_core::Primitive) -> bool,
    ) -> u64 {
        let owners: Vec<_> = world.iter::<LayerInputs>().map(|(e, _)| e).collect();
        let mut removed = 0;
        for e in owners {
            let mut inputs = world.get::<LayerInputs>(e).expect("listed owner").clone();
            let Some(mesh) = world.get_mut::<primase_core::Mesh>(e) else {
                continue;
            };
            for (index, primitive) in mesh.primitives_mut().iter_mut().enumerate() {
                let Some(members) = inputs.primitives.get_mut(index) else {
                    continue;
                };
                if members.is_empty() || !consumed(e, index, primitive) {
                    continue;
                }
                for n in std::mem::take(members) {
                    removed += u64::from(
                        primitive
                            .streams
                            .remove(&primase_core::Semantic::Color(n))
                            .is_some(),
                    );
                }
            }
            if inputs.primitives.iter().all(Vec::is_empty) {
                world.remove::<LayerInputs>(e);
            } else {
                world.insert(e, inputs);
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
