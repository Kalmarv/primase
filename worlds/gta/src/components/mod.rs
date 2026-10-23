//! The `gta.*` dialect: what GTA V's data says about an entity that the core components do not. Values are as
//! stored (flags, enums and hashes raw); the passes give them meaning.
//!
//! | entity | components |
//! |---|---|
//! | placement | `core.transform`, `core.instance`, [`GtaEntity`], `core.condition` when it exists in one map state only |
//! | prototype | `core.name` (archetype name), [`GtaArchetype`], `core.mesh` and [`GtaSources`] (the drawable's high LOD), [`GtaBlendStreams`] when its geometries store blend streams, [`GtaDrawable`], `core.lods` |
//! | LOD form | `core.mesh` and [`GtaSources`] (one of the drawable's coarser LODs), [`GtaBlendStreams`] likewise, unplaced |
//! | material | [`GtaShader`] (one per drawable and shader) |
//! | texture | `core.texture`, `core.name`, `core.source` |
//! | ped part | a prototype of a component ped's drawable dictionary entry: [`GtaPedPart`], [`GtaSkeletonRef`], `core.condition` on its slot's `gta.ped.<slot>` axis |
//! | effect | [`GtaEffect`] (the programs a drawable's shader name selects), [`GtaMaterialEffect`] (a material's effect) |
//! | shader program | `core.shader`, `core.name` |

pub mod collision;
pub mod state;
pub mod tint;
pub mod water;

use primase_core::frame::SourceFrame;
use primase_core::{Component, Entity, JsonError, Lods, Mesh, World};
use serde::Serialize;

use crate::rage::map::{ArchetypeDef, EntityDef, EntityKind};

/// GTA V's frame: x east, y north, z up, metres.
pub const GTA: SourceFrame = SourceFrame::ENU;

/// A copy of prototype `proto` for some of its placements to use instead: every component copied, its `core.lods`
/// forms copied with it (so a pass may change the copy's forms without touching the original's).
pub(crate) fn copy_prototype(w: &mut World, proto: Entity) -> Entity {
    let copy = w.spawn();
    w.copy_components(proto, copy);
    if let Some(mut lods) = w.get::<Lods>(copy).cloned() {
        for f in &mut lods.forms {
            let fc = w.spawn();
            w.copy_components(f.form, fc);
            f.form = fc;
        }
        w.insert(copy, lods);
    }
    copy
}

fn to_json<T: Serialize>(v: &T) -> Result<Option<serde_json::Value>, JsonError> {
    primase_core::json::to_value(v).map(Some)
}

/// A placed entity as its map stores it, and which map: its path, its index among the map's entities.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaEntity {
    pub ymap: String,
    pub index: u32,
    /// An interior instance (`CMloInstanceDef`), or an entity placed inside one: the interior instance's map index.
    pub interior: Option<u32>,
    /// The LOD parent entity (its map and index), from `parent_index` and, with flags bit 3, the map's LOD parent map.
    pub lod_parent: Option<(String, u32)>,
    pub archetype: u32,
    pub flags: u32,
    pub guid: u32,
    pub parent_index: i32,
    pub lod_dist: f32,
    pub child_lod_dist: f32,
    pub lod_level: i32,
    pub num_children: u32,
    pub priority_level: i32,
    pub ambient_occlusion_multiplier: i32,
    pub artificial_ambient_occlusion: i32,
    pub tint_value: u32,
}

impl GtaEntity {
    pub fn new(
        ymap: &str,
        index: u32,
        interior: Option<u32>,
        lod_parent: Option<(String, u32)>,
        e: &EntityDef,
    ) -> GtaEntity {
        GtaEntity {
            ymap: ymap.to_string(),
            index,
            lod_parent,
            interior: interior.or(match e.kind {
                EntityKind::Interior => Some(index),
                EntityKind::Entity => None,
            }),
            archetype: e.archetype,
            flags: e.flags,
            guid: e.guid,
            parent_index: e.parent_index,
            lod_dist: e.lod_dist,
            child_lod_dist: e.child_lod_dist,
            lod_level: e.lod_level,
            num_children: e.num_children,
            priority_level: e.priority_level,
            ambient_occlusion_multiplier: e.ambient_occlusion_multiplier,
            artificial_ambient_occlusion: e.artificial_ambient_occlusion,
            tint_value: e.tint_value,
        }
    }
}

impl Component for GtaEntity {
    const NAME: &'static str = "gta.entity";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// An archetype as its map types file stores it, and which file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaArchetype {
    pub ytyp: String,
    pub name: u32,
    pub kind: &'static str,
    pub flags: u32,
    pub special_attribute: u32,
    pub lod_dist: f32,
    pub hd_texture_dist: f32,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub bs_centre: [f32; 3],
    pub bs_radius: f32,
    pub texture_dictionary: u32,
    pub drawable_dictionary: u32,
    pub physics_dictionary: u32,
    pub clip_dictionary: u32,
    pub asset_type: i32,
    pub asset_name: u32,
    pub time_flags: Option<u32>,
}

impl GtaArchetype {
    pub fn new(ytyp: &str, a: &ArchetypeDef) -> GtaArchetype {
        GtaArchetype {
            ytyp: ytyp.to_string(),
            name: a.name,
            kind: match a.kind {
                crate::rage::map::ArchetypeKind::Base => "base",
                crate::rage::map::ArchetypeKind::Time => "time",
                crate::rage::map::ArchetypeKind::Interior => "interior",
            },
            flags: a.flags,
            special_attribute: a.special_attribute,
            lod_dist: a.lod_dist,
            hd_texture_dist: a.hd_texture_dist,
            bb_min: a.bb_min,
            bb_max: a.bb_max,
            bs_centre: a.bs_centre,
            bs_radius: a.bs_radius,
            texture_dictionary: a.texture_dictionary,
            drawable_dictionary: a.drawable_dictionary,
            physics_dictionary: a.physics_dictionary,
            clip_dictionary: a.clip_dictionary,
            asset_type: a.asset_type,
            asset_name: a.asset_name,
            time_flags: a.time_flags,
        }
    }
}

impl Component for GtaArchetype {
    const NAME: &'static str = "gta.archetype";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// The map states whose archive sets a prototype's payload is loaded from: each resolves the archetype's drawable
/// (fragment, dictionary) and every texture dictionary of its lookup to the files the prototype is loaded from. A
/// prototype is one payload: both states share it only when both resolve every one of those to the same file, and
/// each state's other payload is a prototype of its own. Read from Story's archive set alone: Story.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct GtaResolvedIn {
    pub story: bool,
    pub online: bool,
}

impl Component for GtaResolvedIn {
    const NAME: &'static str = "gta.resolved_in";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// A shader parameter's value; textures as the entity the lookup found (`None`: not found or none bound).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum GtaParam {
    Texture {
        name: Option<String>,
        texture: Option<Entity>,
    },
    Floats(Vec<f32>),
    Sampler(u8),
    Other(u8),
}

/// A drawable's shader: effect name hash, preset word (the same for every shader in Enhanced), render bucket, bucket
/// mask, parameters by stored name hash.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaShader {
    pub drawable: String,
    pub index: u32,
    pub name: u32,
    pub preset: u32,
    pub render_bucket: u8,
    pub bucket_mask: u32,
    pub params: Vec<(u32, GtaParam)>,
}

impl Component for GtaShader {
    const NAME: &'static str = "gta.shader";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.params.iter().filter_map(|(_, p)| match p {
            GtaParam::Texture { texture, .. } => *texture,
            _ => None,
        }));
    }
}

/// The stage of a shader program of the Enhanced shader database.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GtaStage {
    Vertex,
    Pixel,
    Geometry,
    Domain,
    Hull,
    Compute,
}

/// One program an effect names: its stage, its name in the database and, where the program is a vertex or pixel
/// program whose interface was read, its `core.shader` entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GtaEffectProgram {
    pub stage: GtaStage,
    pub name: String,
    pub shader: Option<Entity>,
}

/// An effect of the shader database (what a drawable's shader name selects): the programs it can run, per stage in the
/// database's order, with the permutations (a name carries its hash). The programs are shared between effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GtaEffect {
    pub name: String,
    pub programs: Vec<GtaEffectProgram>,
}

impl Component for GtaEffect {
    const NAME: &'static str = "gta.effect";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.programs.iter().filter_map(|p| p.shader));
    }
}

/// One pass of a technique: the programs it runs (the vertex and pixel programs, the hull and domain programs of a
/// tessellated pass, a compute program), each as the effect lists it, the records of the effect's state tables it
/// draws with, and the pass record as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GtaPass {
    pub vertex: Option<GtaEffectProgram>,
    pub pixel: Option<GtaEffectProgram>,
    pub geometry: Option<GtaEffectProgram>,
    pub hull: Option<GtaEffectProgram>,
    pub domain: Option<GtaEffectProgram>,
    pub compute: Option<GtaEffectProgram>,
    /// The 0-based records of [`GtaEffectTechniques::rasterizer`], [`GtaEffectTechniques::depth_stencil`] and
    /// [`GtaEffectTechniques::blend`] the pass draws with (0 where the effect has no table: the engine's default).
    pub rasterizer: u8,
    pub depth_stencil: u8,
    pub blend: u8,
    pub record: Vec<u8>,
}

/// A technique of an effect: the passes the game draws with under a name (`deferred_draw`, `lightweight0_draw`,
/// `wdcascade_drawskinned`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GtaTechnique {
    pub name: String,
    pub passes: Vec<GtaPass>,
}

/// An effect's techniques ([`crate::rage::sgd2_effect`]) on its `gta.effect` entity, and the fixed-function state
/// tables stored with them (each pass names its records). An effect of the post-processing and
/// lighting kinds has none.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaEffectTechniques {
    pub techniques: Vec<GtaTechnique>,
    pub rasterizer: Vec<crate::rage::sgd2_effect::Rasterizer>,
    pub depth_stencil: Vec<crate::rage::sgd2_effect::DepthStencil>,
    pub blend: Vec<crate::rage::sgd2_effect::Blend>,
    pub samplers: Vec<crate::rage::sgd2_effect::SamplerDesc>,
}

impl Component for GtaEffectTechniques {
    const NAME: &'static str = "gta.effect_techniques";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for pass in self.techniques.iter().flat_map(|t| &t.passes) {
            out.extend(
                [
                    &pass.vertex,
                    &pass.pixel,
                    &pass.geometry,
                    &pass.hull,
                    &pass.domain,
                    &pass.compute,
                ]
                .into_iter()
                .flatten()
                .filter_map(|p| p.shader),
            );
        }
    }
}

/// A shader program's binding table, as the shader database stores it ([`crate::rage::sgd2_table`]), on the program's
/// `core.shader` entity: its constant buffers, textures and samplers with their registers and spaces, and each constant
/// with its byte offset in its buffer, named by hashes of the names in `names`. Not serialised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GtaShaderTable(pub crate::rage::sgd2_table::ProgramTable);

impl Component for GtaShaderTable {
    const NAME: &'static str = "gta.shader_table";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn payload_bytes(&self) -> u64 {
        let t = &self.0;
        let names: usize = t.names.iter().map(String::len).sum();
        u64::try_from(
            t.resources
                .len()
                .saturating_mul(28)
                .saturating_add(t.constants.len().saturating_mul(24))
                .saturating_add(names),
        )
        .unwrap_or(u64::MAX)
    }
}

/// The material parameters an effect renames, on the `gta.effect` entity: for each constant of its programs that a
/// drawable's material stores under another name, the program's name hash and the stored key
/// ([`crate::rage::sgd2_table::effect_parameters`]). Not serialised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GtaEffectParameters(pub Vec<crate::rage::sgd2_table::EffectParameter>);

impl Component for GtaEffectParameters {
    const NAME: &'static str = "gta.effect_parameters";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// The effect of the shader database a drawable's shader selects, on the material entity that holds the
/// [`GtaShader`]: the effect whose name hashes (joaat, lowercase) to the shader's `name`. The effect's programs are the
/// ones its [`GtaEffect`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct GtaMaterialEffect {
    pub effect: Entity,
}

impl Component for GtaMaterialEffect {
    const NAME: &'static str = "gta.material_effect";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.effect);
    }
    fn verify(&self, _e: Entity, w: &primase_core::World) -> Result<(), String> {
        if w.has::<GtaEffect>(self.effect) {
            Ok(())
        } else {
            Err(format!("{:?} is no gta.effect", self.effect))
        }
    }
}

/// What a drawable says beyond its meshes: its file, its box and bounding sphere (GTA's frame, as stored), and per LOD
/// (high, medium, low, very low) the distance it is drawn to and its render mask. On the prototype; the meshes are the
/// prototype's own (the high LOD) and its `core.lods` forms (the coarser LODs that have models), each with its
/// [`GtaSources`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaDrawable {
    pub path: String,
    pub bbox: [[f32; 3]; 2],
    pub bounding_sphere: ([f32; 3], f32),
    pub lod_dist: [f32; 4],
    pub render_masks: [u32; 4],
    /// Per LOD, each model's render mask as stored (model 0x2C), in model order: the passes a model is drawn in (bit
    /// 0 the visible pass; see `lower-gta-pbr`).
    pub model_masks: [Vec<u16>; 4],
}

impl Component for GtaDrawable {
    const NAME: &'static str = "gta.drawable";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: the meshes' sources and the coarser LODs moved to [`GtaSources`] and `core.lods`; 3: `model_masks`.
    const VERSION: u32 = 3;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// Which drawable LOD a mesh is (0 high, 1 medium, 2 low, 3 very low) and, for each of its primitives in order, the GTA
/// model and geometry (the index within its model) it came from. On every entity holding a drawable's mesh: the
/// prototype (the high LOD) and each of its LOD forms. The reader makes a primitive per geometry, in order; passes that
/// drop or split primitives keep this in step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GtaSources {
    pub lod: u8,
    pub sources: Vec<[u32; 2]>,
}

impl Component for GtaSources {
    const NAME: &'static str = "gta.sources";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(
            primase_core::remap_list(&self.sources, from).map(|sources| GtaSources {
                lod: self.lod,
                sources,
            }),
        )
    }
    /// One source per primitive of the entity's mesh.
    fn verify(&self, e: Entity, w: &primase_core::World) -> Result<(), String> {
        let n = w.get::<Mesh>(e).map_or(0, |m| m.primitives().len());
        if n != self.sources.len() {
            return Err(format!("{n} primitives, {} sources", self.sources.len()));
        }
        Ok(())
    }
}

/// Per primitive of the entity's mesh, the bone its model is bound to (the model's skeleton binding), `None` for a
/// skinned model or a drawable without a skeleton: drawn as stored. The bones are the prototype's `gta.skeleton`;
/// `gta-pose-fragments` places the models by their bones' poses and removes this.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaBones(pub Vec<Option<u16>>);

impl Component for GtaBones {
    const NAME: &'static str = "gta.bones";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.0, from).map(GtaBones))
    }
    /// One bone (or none) per primitive of the entity's mesh.
    fn verify(&self, e: Entity, w: &primase_core::World) -> Result<(), String> {
        let n = w.get::<Mesh>(e).map_or(0, |m| m.primitives().len());
        if n != self.0.len() {
            return Err(format!("{n} primitives, {} bones", self.0.len()));
        }
        Ok(())
    }
}

/// Per primitive of the entity's mesh, its geometry's vertex blend streams as stored (`BLENDWEIGHT0` and
/// `BLENDINDICES0`, four bytes a vertex each) with the geometry's bone palette, `None` for a geometry storing neither.
/// The indices name entries of the palette, which holds bone ids (skeleton tags): they are no core skin (`core.mesh`'s
/// joints and weights, which need a skeleton and its binding) until a pass lowers them.
#[derive(Clone, Debug, PartialEq)]
pub struct GtaBlendStreams(pub Vec<Option<BlendStreams>>);

/// One geometry's vertex blend streams as stored, one entry a vertex in each.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendStreams {
    /// `BLENDWEIGHT0`: four unsigned bytes a vertex.
    pub weights: Option<Vec<[u8; 4]>>,
    /// `BLENDINDICES0`: four bytes a vertex, entries of the geometry's bone palette.
    pub indices: Option<Vec<[u8; 4]>>,
    /// The geometry's bone palette: the bone ids (skeleton tags) `indices` index, as stored; empty when it has none.
    pub palette: Vec<u16>,
}

impl BlendStreams {
    fn streams(&self) -> impl Iterator<Item = &Vec<[u8; 4]>> {
        self.weights.iter().chain(&self.indices)
    }
}

impl Component for GtaBlendStreams {
    const NAME: &'static str = "gta.blend_streams";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
    /// Bytes a vertex, not local-space geometry.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
    fn payload_bytes(&self) -> u64 {
        let values: usize = self
            .0
            .iter()
            .flatten()
            .flat_map(BlendStreams::streams)
            .map(Vec::len)
            .sum();
        u64::try_from(values).map_or(u64::MAX, |n| n.saturating_mul(4))
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.0, from).map(GtaBlendStreams))
    }
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        let gather = |v: &Vec<[u8; 4]>| -> Result<Vec<[u8; 4]>, String> {
            from.iter()
                .map(|&i| {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| v.get(i).copied())
                        .ok_or_else(|| format!("vertex {i} of {}", v.len()))
                })
                .collect()
        };
        let Some(entry) = self.0.get(primitive) else {
            return Some(Err(format!("primitive {primitive} of {}", self.0.len())));
        };
        // a geometry without blend streams: nothing to follow
        let Some(b) = entry else {
            return Some(Ok(self.clone()));
        };
        let remapped = (|| {
            Ok(BlendStreams {
                weights: b.weights.as_ref().map(gather).transpose()?,
                indices: b.indices.as_ref().map(gather).transpose()?,
                palette: b.palette.clone(),
            })
        })();
        Some(remapped.map(|b| {
            let mut out = self.clone();
            if let Some(slot) = out.0.get_mut(primitive) {
                *slot = Some(b);
            }
            out
        }))
    }
    /// One entry per primitive of the entity's mesh, each stream one value per vertex of its primitive.
    fn verify(&self, e: Entity, w: &primase_core::World) -> Result<(), String> {
        let Some(mesh) = w.get::<Mesh>(e) else {
            return Ok(());
        };
        let primitives = mesh.primitives();
        if primitives.len() != self.0.len() {
            return Err(format!(
                "{} primitives, {} blend entries",
                primitives.len(),
                self.0.len()
            ));
        }
        for (i, (p, b)) in primitives.iter().zip(&self.0).enumerate() {
            if let Some(n) = b
                .iter()
                .flat_map(BlendStreams::streams)
                .map(Vec::len)
                .find(|&n| n != p.vertex_count())
            {
                return Err(format!(
                    "primitive {i}: {n} blend values for {} vertices",
                    p.vertex_count()
                ));
            }
        }
        Ok(())
    }
}

/// A prototype's bones as its drawable and fragment store them ([`crate::rage::drawable::Bones`]: the skeleton, each bone
/// in its parent's space with its parent and tag, and a fragment's own bone pose). The passes compose a bone's pose
/// from it: `gta-pose-fragments` for the models on bones (`gta.bones`), `lower-gta-lights` for the light records.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaSkeleton(pub crate::rage::drawable::Bones);

impl Component for GtaSkeleton {
    const NAME: &'static str = "gta.skeleton";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// The prototype whose [`GtaSkeleton`] a skinned prototype is bound to: a component ped's drawables carry no skeleton
/// (their blend indices are bone indices of the ped's, which its fragment holds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct GtaSkeletonRef(pub Entity);

impl Component for GtaSkeletonRef {
    const NAME: &'static str = "gta.skeleton_ref";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.0);
    }
    fn verify(&self, _e: Entity, w: &primase_core::World) -> Result<(), String> {
        if w.has::<GtaSkeleton>(self.0) {
            Ok(())
        } else {
            Err(format!("{:?} holds no gta.skeleton", self.0))
        }
    }
}

/// A drawable of a component ped, as the ped's variation info offers it ([`crate::rage::ped`]): its slot, its index in
/// the slot, the name hash of its entry in the ped's drawable dictionary, and what the info says of it, as stored.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaPedPart {
    /// The slot's name (`head`, `uppr`, ...).
    pub slot: &'static str,
    pub drawable: u32,
    pub entry: u32,
    pub prop_mask: u8,
    pub alternatives: u8,
    /// Its texture variations: `[id, distribution]` each.
    pub textures: Vec<[u8; 2]>,
    pub owns_cloth: bool,
}

impl Component for GtaPedPart {
    const NAME: &'static str = "gta.ped_part";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// A prototype's light sources as its drawable or fragment stores them, every record raw, each in the space of the bone
/// its `bone_id` tags (the bones are the prototype's `gta.skeleton`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaLights {
    pub records: Vec<crate::rage::drawable::LightAttr>,
}

impl Component for GtaLights {
    const NAME: &'static str = "gta.lights";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 3: the records alone (their bones are `gta.skeleton`); 2 held each record's bone pose.
    const VERSION: u32 = 3;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

/// A placed light's record as its drawable or fragment stores it (on the entity `lower-gta-lights` made for it): its
/// hours, flashiness, volume and corona fields stay here for writers that use them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaLight(pub crate::rage::drawable::LightAttr);

impl Component for GtaLight {
    const NAME: &'static str = "gta.light";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{Primitive, Semantic, Stream, Topology};

    fn triangle(n: usize) -> Primitive {
        Primitive {
            topology: Topology::Triangles,
            streams: [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; n]))].into(),
            indices: None,
            material: None,
        }
    }

    /// Blend streams follow their mesh: per primitive when the list is rebuilt, per vertex when a primitive's
    /// vertices are compacted; a length that no longer matches its primitive fails verification.
    #[test]
    fn blend_streams_follow_primitives_and_vertices() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![triangle(3), triangle(3)]));
        let b = BlendStreams {
            weights: Some(vec![[255, 0, 0, 0], [128, 127, 0, 0], [0, 255, 0, 0]]),
            indices: Some(vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]]),
            palette: vec![17, 40],
        };
        w.insert(e, GtaBlendStreams(vec![None, Some(b)]));
        assert_eq!(w.verify_components(), vec![]);
        // the second primitive kept alone, then its vertices reordered and one dropped
        w.rebuild_primitives(e, |m| Ok((vec![m.primitives()[1].clone()], vec![1])))
            .unwrap();
        w.edit_mesh(e, |m| {
            let mut p = m.primitive_mut(0).unwrap();
            // two vertices kept: drawn as points, so the primitive stays whole
            *p.topology_mut() = Topology::Points;
            *p.streams_mut() = [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; 2]))].into();
            Ok(None)
        })
        .unwrap();
        w.remap_vertices(e, 0, &[2, 0]).unwrap();
        let kept = &w.get::<GtaBlendStreams>(e).unwrap().0;
        assert_eq!(
            kept,
            &[Some(BlendStreams {
                weights: Some(vec![[0, 255, 0, 0], [255, 0, 0, 0]]),
                indices: Some(vec![[1, 0, 0, 0], [0, 0, 0, 0]]),
                palette: vec![17, 40],
            })]
        );
        assert_eq!(w.verify_components(), vec![]);
        // a vertex it does not have is an error, changing nothing
        assert!(
            w.remap_vertices(e, 0, &[5])
                .unwrap_err()
                .contains("gta.blend_streams")
        );
        // stale against its mesh: caught
        w.insert(e, Mesh::new(vec![triangle(3)]));
        let failures = w.verify_components();
        assert!(
            failures
                .iter()
                .any(|(n, _, m)| *n == GtaBlendStreams::NAME && m.contains("2 blend values for 3")),
            "{failures:?}"
        );
    }
}
