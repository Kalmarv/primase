//! Typed source identities and records the area reader keeps for the passes that interpret them. They describe what
//! the source stores and how it names it, never a target format's fields.
use fromsoft_formats::game::Resolution;
use primase_core::json::{JsonError, to_value};
use primase_core::{Component, Entity, Resource, Semantic, Stream};
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// What a placement of the sky is ([`SkyPart`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkyRole {
    /// A sky dome (`m9000xx`), one per sky state.
    Dome,
    /// A dome's ring of far mountains (`m9001xx`, `m900200`), drawn with its dome.
    Ring,
    /// The moon (the object `o005900`), placed towards the light by the game every frame.
    Moon,
    /// An Elden Ring asset of an area's sky map that the weather shows (`WeatherAssetReplaceParam`) and that draws with
    /// some program other than the sky's (the Erdtree's light shafts): placed where the sky map's layout places it, as
    /// a recorded frame draws one (`AEG096_611`, at the sky map's origin while the camera moved).
    Weather,
    /// An Elden Ring weather asset drawn only with sky programs (every material of the material table's sky class: the
    /// sky spheres, the cloud layers): the game draws it at the camera's x and z and its own height, as a recorded
    /// frame draws the cloud layer `AEG096_603` at three camera positions; `core.sky_anchor` horizontal.
    WeatherSky,
}

/// A placement of the sky the area read brought in (`fromsoft.sky_part`, [`crate::sekiro::read::sky`]): what it is, and
/// the values of the `fromsoft.sky` state axis it is drawn in, as a mask (bit `i` for value `i`; its `core.condition`
/// says the same): one for a Sekiro dome or ring, the weathers that show it for an Elden Ring weather asset, none (0)
/// for the moon, drawn in every state. `lower-fromsoft-sky` lowers it: the moon's anchor, the draw order of the sky's
/// materials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SkyPart {
    pub role: SkyRole,
    pub states: u64,
}
impl Component for SkyPart {
    const NAME: &'static str = "fromsoft.sky_part";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
///
/// With [`crate::sekiro::Options::f_collision`] the file of the area's `f` collision binder for the same model
/// (`f11_01_00_00_009500.hkx` beside `h11_01_00_00_009500.hkx`: one extern mesh body per stored body) is read too, as
/// `f_triangles`, in the same frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MapCollision {
    /// The member's file name (`h11_01_00_00_009500.hkx`).
    pub file: String,
    pub parts: Vec<CollisionPart>,
    pub triangles: Vec<[[f32; 3]; 3]>,
    pub materials: Vec<i32>,
    /// The triangles of the `f` binder's file for the same model, in the world frame like `triangles`, each in its
    /// stored order: they store no shape key and no HitMtrl ID. Empty where the read did not ask for them or the
    /// binder holds no file for the model.
    pub f_triangles: Vec<[[f32; 3]; 3]>,
    /// Shape-key rows the file marks invalid (all ones): they identify no primitive.
    pub invalid_material_keys: usize,
    /// Compressed-mesh primitives the file marks dead.
    pub dead_primitives: usize,
    /// The shape classes of the file's bodies the reader has no decoder for, one per body: not in `triangles`.
    pub unread_bodies: Vec<String>,
}
impl Component for MapCollision {
    const NAME: &'static str = "fromsoft.map_collision";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
    /// The parts' clauses reduced as `core.condition` is (`resolve-state` runs before the lowering in a selection, and
    /// removes the axes a lowering would copy a clause on): a part whose clause fails in the resolved state leaves the
    /// record, a clause left with no literal becomes none (the part is there whatever the other axes are), and a record
    /// left with no part is not present: its entity is not.
    fn restate(&self, resolved: &primase_core::state::Resolved<'_>) -> primase_core::world::Restated<Self> {
        use primase_core::world::Restated;
        if !self.parts.iter().any(|p| {
            p.condition
                .as_ref()
                .is_some_and(|c| c.0.iter().any(|l| resolved.has(l.axis)))
        }) {
            return Restated::Unchanged;
        }
        let mut parts = Vec::with_capacity(self.parts.len());
        for part in &self.parts {
            let condition = match &part.condition {
                None => None,
                Some(c) => match resolved.reduce(c) {
                    None => continue,
                    Some(r) if r.0.is_empty() => None,
                    Some(r) => Some(r),
                },
            };
            parts.push(CollisionPart {
                condition,
                ..part.clone()
            });
        }
        if parts.is_empty() {
            return Restated::Absent;
        }
        Restated::Replaced(MapCollision {
            parts,
            ..self.clone()
        })
    }
    /// 36 bytes per triangle (the `f` ones included) and 4 per material.
    fn payload_bytes(&self) -> u64 {
        (self.triangles.len() + self.f_triangles.len()) as u64 * 36 + self.materials.len() as u64 * 4
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

/// One stored collision file of an object prototype: its file name, its resolution and every body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectCollisionFile {
    /// The binder member's file name (`o000100.hkx`, `aeg007_310_l.hkx`).
    pub file: String,
    pub resolution: Resolution,
    pub bodies: Vec<ObjectBody>,
}

/// An object prototype's collision as stored (`fromsoft.object_collision`): every collision file the game keeps for it,
/// each with its resolution, for `lower-fromsoft-object-collision` to choose from and place with each placement. A
/// prototype with no such file has none.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectBodies {
    pub files: Vec<ObjectCollisionFile>,
}
impl ObjectBodies {
    /// Every body of every file.
    pub fn bodies(&self) -> impl Iterator<Item = &ObjectBody> {
        self.files.iter().flat_map(|f| &f.bodies)
    }
}
impl Component for ObjectBodies {
    const NAME: &'static str = "fromsoft.object_collision";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: every stored collision file with its resolution (Sekiro's one main HKX is a high one).
    const VERSION: u32 = 2;
    /// Prototype-space geometry, mapped as the prototype's mesh: mesh triangles moved (a reflection reversing their
    /// order), a convex body's map composed with the mapping.
    fn apply_linear(&self, l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        let mut out = self.clone();
        for body in out.files.iter_mut().flat_map(|f| &mut f.bodies) {
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
        self.bodies()
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
        for f in &self.files {
            for b in &f.bodies {
                if let ObjectGeometry::Mesh { triangles, materials } = &b.geometry
                    && materials.len() != triangles.len()
                {
                    return Err(format!(
                        "{} body {}: {} materials for {} triangles",
                        f.file,
                        b.name,
                        materials.len(),
                        triangles.len()
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A region a read is bounded to: an east/north rectangle in the world frame, `[x0, y0, x1, y1]`, and the metres around
/// it a placement's extent may reach in from and still be read. Both are the caller's: a stated region states its
/// margin.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Region {
    pub rect: [f32; 4],
    pub margin: f32,
}

/// What a read covered (`fromsoft.read_scope`): the game, the area, the region it was bounded to with its placement
/// margin, and whether event-flag world states were read. Passes that crop to the read region, or that read a game's
/// own tables (its HitMtrl classes), read it here, so a program need not restate it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReadScope {
    pub game: fromsoft_formats::Game,
    pub area: String,
    pub region: Option<Region>,
    pub states: bool,
}
impl Resource for ReadScope {
    const NAME: &'static str = "fromsoft.read_scope";
    /// 2: the region's margin is the region's (`region.margin`), absent with it. 3: the game.
    const VERSION: u32 = 3;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// What a BTL light record stores (`fromsoft.light`), on an entity placed where the light is: its transform is the
/// stored position and rotation through the map offset, in the world frame, so a spot shines along its local -Z.
/// Colours are the stored bytes (the game squares them), powers the stored multipliers (the unit of one record's
/// `colour^2 * power`), `cone_angle` the stored full angle in degrees of a spot's vertical axis and `cone_width` the
/// full angle of its horizontal axis (record offset 184; 0 where the file stores none, and the game then uses
/// `cone_angle` for both), `exposure_weight` the float at record offset 224 (the weight of the game's per-frame
/// exposure term on the light's colour; 0 on every Elden Ring light), `radius` the stored reach in metres.
/// `tangent` is the light's local +X in the world frame, kept so it survives a pass that re-bases the transform.
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
    pub cone_width: f32,
    pub exposure_weight: f32,
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// Placed by its entity's transform; the tangent is world space.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// A light property of an effect (`fromsoft.effect_light`) as the effect file stores it: its components at the first
/// keyframe (`initial`: the constant itself for a constant) and where the property settles (`settled`: the last
/// keyframe of a function that does not loop, the constant for a constant; `None` for a function that loops).
/// `keyframes` is 0 for a constant. Modifiers (random variation, dependence on an outside value) are not applied:
/// `modifiers` counts them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectValue {
    pub initial: Vec<f32>,
    pub settled: Option<Vec<f32>>,
    pub keyframes: usize,
    pub modifiers: usize,
}

/// Where an effect light was placed from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum EffectPlace {
    /// A map SFX region (type 5), by name: the effect stands at the region's pose.
    Region { region: String },
    /// A placed object (Sekiro) or asset (Elden Ring): the effect stands at the model's dummy polygon `dummy` (its
    /// `reference_id`) as the param row `row`'s slot `slot` says.
    Model {
        part: String,
        model: String,
        row: i32,
        slot: usize,
        dummy: i32,
    },
}

/// The shape of an effect light.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum EffectShape {
    /// A point light reaching `radius` metres.
    Point { radius: EffectValue },
    /// A spot light: an elliptic cone from the point to a base at `far` metres, of half-extents `radius_x` and
    /// `radius_y` there, with the tip cut off at `near`. The spot's axis is the effect's local +Z (the file frame).
    Spot {
        near: EffectValue,
        far: EffectValue,
        radius_x: EffectValue,
        radius_y: EffectValue,
    },
}

/// What a point or spot light action of an effect file (FXR) stores (`fromsoft.effect_light`), on an entity placed where
/// the light stands: the placement ([`EffectPlace`]) composed with the static node transforms above the light, in the
/// world frame. Values are as stored: colours are floats (8-bit values over 255 in every light of both installs),
/// `diffuse_multiplier` the stored multiplier (`None` in Dark Souls III's layout, which has none) of which a spot's is
/// ten times the unit a point's is in (the fxr project's reading of the files), `diffuse` carries the colour's
/// alpha. `starts_disabled` is the SFX region's flag; `chosen_in_state_0` whether the node's state-config map
/// chooses the light's config in the effect's first state. The light's flicker and fade-out are kept, not applied.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectLight {
    pub effect: i32,
    pub place: EffectPlace,
    pub shape: EffectShape,
    pub diffuse: EffectValue,
    pub diffuse_multiplier: Option<EffectValue>,
    pub specular: EffectValue,
    pub specular_multiplier: Option<EffectValue>,
    pub separate_specular: bool,
    pub shadows: bool,
    pub flickers: bool,
    pub fade_out_frames: i32,
    pub starts_disabled: bool,
    pub chosen_in_state_0: bool,
    /// The static node transforms the placement was composed with.
    pub static_transforms: usize,
    /// The effects that spawned this light's effect, the named one first (empty: the effect the route names holds it).
    pub via: Vec<i32>,
}
impl Component for EffectLight {
    const NAME: &'static str = "fromsoft.effect_light";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// Placed by its entity's transform.
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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

/// The map layout's regions and events as stored (`fromsoft.map_records`, a resource; read only with
/// [`crate::sekiro::Options::map_records`]): every region of every type with its shape (sounds, effects, wind,
/// environment-map points and boxes, messages, weather overrides and the rest) and every event of every type, with the
/// bytes of each type's own data. `lower-fromsoft-map-records` lowers them. Regions whose position lies outside the
/// read region are not kept (counted); events have no place of their own and are all kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MapRecords {
    pub regions: Vec<fromsoft_formats::msb::Region>,
    pub events: Vec<fromsoft_formats::msb::Event>,
    /// The parts of the kinds the read does not place as models (enemies, player starts, dummies, connect
    /// collisions) whose position lies inside the read region.
    pub parts: Vec<fromsoft_formats::msb::UnplacedPart>,
    /// The region type code of spawn points in the layout's generation, which `lower-fromsoft-extras` makes markers
    /// of: the records pass leaves those regions to it.
    pub spawn_region: Option<u32>,
}
impl Resource for MapRecords {
    const NAME: &'static str = "fromsoft.map_records";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// How one draw-param group's value set was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SetChosen {
    /// The value ID most of the area's parts name for the group (their graphics config; the lowest on a tie).
    ByParts,
    /// The group stores one value set only.
    OnlySet,
    /// No part names an ID for the group and it stores several: the set of value ID 0.
    ZeroId,
    /// Source values retained for inspection; the owner/activation selector is unresolved.
    /// These sets must not be used as the active scene environment.
    Unresolved,
}

/// One sample of a field of a draw-param group at the chosen value set: its key, name, value and stored time of day
/// (version 5). A field stored at several hours is several samples of one key, in stored order; after
/// `sample-fromsoft-draw-params` it is one, with the hour sampled as its `time`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DrawField {
    pub key: String,
    pub name: String,
    pub value: fromsoft_formats::gparam::Value,
    pub time: Option<f32>,
}

/// One draw-param group (`LightSet ParamEditor`, `FogParamEditor`, `Tone Map[Yebis]`) at the value set the area's parts
/// name.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DrawGroup {
    pub key: String,
    pub name: String,
    /// The value ID of the set read, and why it was the one.
    pub id: i32,
    pub chosen: SetChosen,
    pub fields: Vec<DrawField>,
}

impl DrawGroup {
    /// The value of the field with key `key`: its first stored sample.
    pub fn value(&self, key: &str) -> Option<&fromsoft_formats::gparam::Value> {
        self.fields.iter().find(|f| f.key == key).map(|f| &f.value)
    }
}

/// The area's graphics environment as its draw-param file stores it (`fromsoft.draw_param`, a resource): the groups at
/// the value sets the area's parts name. The file is the caller's choice ([`crate::sekiro::DrawParams`]); which sets
/// are read, and what is not, is in the read's notices. The meaning of the fields (units, the sun's angle convention)
/// is `lower-fromsoft-environment`'s.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DrawParam {
    /// The file's stem (`m11_01_0720`).
    pub file: String,
    /// The file's GPARAM version.
    pub version: u32,
    pub groups: Vec<DrawGroup>,
}
impl Resource for DrawParam {
    const NAME: &'static str = "fromsoft.draw_param";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}
impl DrawParam {
    /// The selected group with key `key`; unresolved source sets are inspection data only.
    pub fn group(&self, key: &str) -> Option<&DrawGroup> {
        self.groups
            .iter()
            .find(|g| g.key == key && g.chosen != SetChosen::Unresolved)
    }
}

/// Per primitive of a mesh lowered from a FLVER, whether its source face set culls back faces: the face set's
/// `CullBackfaces` byte (header byte 5), a source fact that varies by face set, not by material or class. The material
/// lowering makes it the sidedness of the material the primitive draws with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FaceCull {
    pub primitives: Vec<bool>,
}
impl Component for FaceCull {
    const NAME: &'static str = "fromsoft.face_cull";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    /// Which side is culled follows the triangles' winding, which a mirror's transform keeps.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.primitives, from).map(|primitives| FaceCull { primitives }))
    }
}

pub use crate::layer_bake::resolve::SECOND_COLOUR;

/// Per primitive of a prototype, the streams it carries only because the layered M[Multiple] shaders read them as
/// vertex inputs, in the order lowered: the FLVER Byte4C UV members (member `n` is `Color(n)`, UNORM8 exactly as
/// stored: layer weights, snow, tint and detail), the second UV set (`TexCoord(1)`, the `zw` of the packed first
/// input) and the second vertex colour ([`SECOND_COLOUR`]). Their meaning belongs to the passes that evaluate those
/// shaders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LayerInputs {
    pub primitives: Vec<Vec<Semantic>>,
}
impl Component for LayerInputs {
    const NAME: &'static str = "fromsoft.layer_inputs";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
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
    /// streams their entry lists and the entry itself; the component goes once no entry lists a stream.
    /// They are shader inputs, not vertex attributes: a pass that consumed them, or lowered their material without
    /// them, takes them out, and only those. Returns the streams removed.
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
            for (index, mut primitive) in mesh.primitives_mut().enumerate() {
                let Some(members) = inputs.primitives.get_mut(index) else {
                    continue;
                };
                if members.is_empty() || !consumed(e, index, &primitive) {
                    continue;
                }
                for semantic in std::mem::take(members) {
                    removed += u64::from(primitive.streams_mut().remove(&semantic).is_some());
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

/// Where one FLVER mesh keeps what the layered programs read as vertex inputs, by its element layout: UV storage
/// slots count each UV set, so a Float4 or packed-short pair element (`TEXCOORD0`, uv0 in `xy` and uv1 in `zw`) takes
/// two, and the colour elements count in the order the reader decodes them (`Vertex::colors`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerLayout {
    /// The Byte4C UV members: `(member index, UV storage slot)`, in member order.
    pub byte4: Vec<(u8, usize)>,
    /// The UV storage slot of the second UV set: slot 1, when neither slot 0 nor slot 1 is a Byte4C member.
    pub second_uv: Option<usize>,
    /// The position among the mesh's colour elements of the one the FLVER indexes 1.
    pub second_colour: Option<usize>,
    /// UV storage slots the mesh holds, and colour elements.
    pub uv_slots: usize,
    pub colours: usize,
}

impl LayerLayout {
    pub fn of(model: &fromsoft_formats::flver::Model, mesh: usize) -> fromsoft_formats::Result<Self> {
        use crate::invalid;
        let m = model
            .meshes
            .get(mesh)
            .ok_or_else(|| invalid("FLVER layer inputs", mesh, "mesh index"))?;
        let mut layout = Self::default();
        for &index in &m.layouts {
            for member in model
                .layouts
                .get(index)
                .ok_or_else(|| invalid("FLVER layer inputs", index, "layout index"))?
            {
                match member.semantic {
                    5 => {
                        if member.kind == 0x13 {
                            let n = u8::try_from(member.index)
                                .map_err(|_| invalid("FLVER layer inputs", index, "member index"))?;
                            layout.byte4.push((n, layout.uv_slots));
                        }
                        layout.uv_slots += if matches!(member.kind, 3 | 0x16) { 2 } else { 1 };
                    }
                    10 => {
                        if member.index == 1 {
                            layout.second_colour = Some(layout.colours);
                        }
                        layout.colours += 1;
                    }
                    _ => {}
                }
            }
        }
        let byte4_at = |slot| layout.byte4.iter().any(|&(_, s)| s == slot);
        if layout.uv_slots >= 2 && !byte4_at(0) && !byte4_at(1) {
            layout.second_uv = Some(1);
        }
        Ok(layout)
    }
}

/// The FLVER Byte4C UV members of one mesh as `(member index, values per selected vertex)`, in member order.
/// `vertices` are the source vertex indices of the selected (compacted) vertices.
pub fn byte4_uv_members(
    model: &fromsoft_formats::flver::Model,
    mesh: usize,
    vertices: &[u32],
) -> fromsoft_formats::Result<Vec<(u8, Vec<[u8; 4]>)>> {
    use crate::invalid;
    let layout = LayerLayout::of(model, mesh)?;
    let m = &model.meshes[mesh];
    layout
        .byte4
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
                .collect::<fromsoft_formats::Result<Vec<_>>>()?;
            Ok((index, values))
        })
        .collect()
}

/// Every stream the layered programs read as a vertex input beyond position, normal, tangent and the first UV set, for
/// the selected vertices of one mesh, by the stream that carries it: the Byte4C members as `Color(n)`, the second UV
/// set as `TexCoord(1)` (x and y of the stored pair) and the second colour as [`SECOND_COLOUR`]. A mesh that stores
/// no second UV set or no second colour gets no such stream: what the engine feeds the program then is not known, and
/// is not made up here.
pub fn layer_streams(
    model: &fromsoft_formats::flver::Model,
    mesh: usize,
    vertices: &[u32],
) -> fromsoft_formats::Result<Vec<(Semantic, Stream)>> {
    use crate::invalid;
    let layout = LayerLayout::of(model, mesh)?;
    if layout
        .byte4
        .iter()
        .any(|&(n, _)| Semantic::Color(n) == SECOND_COLOUR)
    {
        return Err(invalid(
            "FLVER layer inputs",
            mesh,
            "a Byte4C member takes the stream kept for the second colour",
        ));
    }
    let mut streams: Vec<_> = byte4_uv_members(model, mesh, vertices)?
        .into_iter()
        .map(|(n, values)| (Semantic::Color(n), Stream::U8x4(values)))
        .collect();
    let m = &model.meshes[mesh];
    let vertex = |v: u32| {
        m.vertices
            .get(v as usize)
            .ok_or_else(|| invalid("FLVER layer inputs", v as usize, "vertex index"))
    };
    if let Some(slot) = layout.second_uv {
        let uv = vertices
            .iter()
            .map(|&v| {
                vertex(v)?
                    .uvs
                    .get(slot)
                    .map(|uv| [uv[0], uv[1]])
                    .ok_or_else(|| invalid("FLVER layer inputs", slot, "missing declared second UV set"))
            })
            .collect::<fromsoft_formats::Result<Vec<_>>>()?;
        streams.push((Semantic::TexCoord(1), Stream::F32x2(uv)));
    }
    if let Some(at) = layout.second_colour {
        let colour = vertices
            .iter()
            .map(|&v| {
                vertex(v)?
                    .colors
                    .get(at)
                    .copied()
                    .ok_or_else(|| invalid("FLVER layer inputs", at, "missing declared second colour"))
            })
            .collect::<fromsoft_formats::Result<Vec<_>>>()?;
        streams.push((SECOND_COLOUR, Stream::F32x4(colour)));
    }
    Ok(streams)
}

/// Source files by name, as stored, each an immutable buffer: a world's copy (a part's) shares them.
pub type Payloads = BTreeMap<String, std::sync::Arc<[u8]>>;

/// Source texture files by stem, as stored (DDS bytes), for every texture a selected material binds. Kept for
/// the passes that lower or bake materials; not serialised.
#[derive(Clone, Debug, Default)]
pub struct TexturePayloads(pub Payloads);
impl Resource for TexturePayloads {
    const NAME: &'static str = "fromsoft.texture_payloads";
}

/// The G-buffer pixel shader programs (`<shader>_<switch>_Gbuf.ppo`, DXBC as stored) of the selected layered
/// materials, for passes that evaluate them. Not serialised.
#[derive(Clone, Debug, Default)]
pub struct ShaderPrograms(pub Payloads);
impl Resource for ShaderPrograms {
    const NAME: &'static str = "fromsoft.shader_programs";
}

/// The stage a compiled shader program runs in, by its file's extension in the shader bundle (`.vpo`, `.hpo`, `.dpo`,
/// `.ppo`); each is a DXBC container whose own shader type agrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShaderStage {
    Vertex,
    Hull,
    Domain,
    Pixel,
}
impl ShaderStage {
    /// The stage a program file's extension (`vpo`, `hpo`, `dpo`, `ppo`) names.
    pub fn from_extension(extension: &str) -> Option<ShaderStage> {
        match extension {
            "vpo" => Some(ShaderStage::Vertex),
            "hpo" => Some(ShaderStage::Hull),
            "dpo" => Some(ShaderStage::Domain),
            "ppo" => Some(ShaderStage::Pixel),
            _ => None,
        }
    }
}

/// One compiled program of a shader bundle: its file name as the bundle stores it, its stage, its container's format
/// (Shader Model 5 DXBC in Sekiro, Shader Model 6 DXIL in Elden Ring) and its container as stored. The name says which draw it serves, in the game's own words (`M[Multiple]_3_Gbuf.ppo`: the shader, its
/// static switch, the G-buffer pass; `M[Multiple]_Gbuf_Skin@[cl].vpo`: the pass, skinned, cloth; `_[A]` an
/// alpha-tested pixel program, `_FT` a tessellated vertex program). Nothing here chooses a program for a draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderProgram {
    pub name: String,
    pub stage: ShaderStage,
    pub format: iridia_interface::ShaderFormat,
    pub code: std::sync::Arc<[u8]>,
}

/// A shader's bundle (`<shader>.shaderbdle` of one of the game's bundle binders, [`fromsoft_formats::Game::shader_binders`]) on an
/// entity of its own, named as the shader (`M[Multiple]`): every program it stores, in its order, for every pass the game draws the shader with
/// (forward, depth, shadow, velocity, G-buffer...), and the shader's parameter definition (`parameters`, `None` where
/// the bundle's metaparam is of a version the reader does not read: `Report::unread_metaparams` names them). A
/// material's `fromsoft.material_shader` names its bundle. Not serialised.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderBundle {
    pub name: String,
    /// The game whose binders hold it: its programs' format and G-buffer layout are that game's.
    pub game: fromsoft_formats::Game,
    pub programs: Vec<ShaderProgram>,
    pub parameters: Option<fromsoft_formats::metaparam::Metaparam>,
}
impl ShaderBundle {
    /// The program stored under `name`, following the game's case-insensitive path lookup. The record keeps its spelling.
    pub fn program(&self, name: &str) -> Option<&ShaderProgram> {
        let name = fromsoft_formats::names::fold(name);
        self.programs
            .iter()
            .find(|p| fromsoft_formats::names::fold(&p.name) == name)
    }
    /// The programs of one stage, in stored order.
    pub fn stage(&self, stage: ShaderStage) -> impl Iterator<Item = &ShaderProgram> {
        self.programs.iter().filter(move |p| p.stage == stage)
    }
}
impl Component for ShaderBundle {
    const NAME: &'static str = "fromsoft.shader_bundle";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn payload_bytes(&self) -> u64 {
        self.programs.iter().map(|p| p.code.len() as u64).sum()
    }
}

/// On a material entity (`fromsoft.material`): the bundle entity of the shader its definition names. Absent where the
/// material has no definition, or the install holds no bundle of that shader (`Report::missing_shader_bundles`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialShader(pub Entity);
impl Component for MaterialShader {
    const NAME: &'static str = "fromsoft.material_shader";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.0);
    }
}

/// The display mask marks of each FLVER mesh of a character's model (`fromsoft.mesh_masks`, on the prototype), by mesh
/// index: the numbers `#<k>#` in the name of the mesh's material, in the order they stand there (none: the mesh
/// carries no mark). The game shows a marked mesh when `modelDispMask<k>` of the NPC's row is set
/// ([`NpcRow`](fromsoft_formats::param::NpcRow)); whether a mesh with several marks needs all or one of them is not established,
/// so they are kept as stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MeshMasks(pub Vec<Vec<u32>>);
impl Component for MeshMasks {
    const NAME: &'static str = "fromsoft.mesh_masks";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
}

/// The NPC param's rows as the reader reads them (`fromsoft.npc_rows`): every row's id and its 32 display mask bits.
/// Which character a row dresses is the program's statement, given to the pass that selects an outfit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NpcRows(pub Vec<fromsoft_formats::param::NpcRow>);
impl Resource for NpcRows {
    const NAME: &'static str = "fromsoft.npc_rows";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}
