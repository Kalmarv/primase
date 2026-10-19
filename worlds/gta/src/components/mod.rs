//! The `gta.*` dialect: what GTA V's data says about an entity that the core components do not. Values are as
//! stored (flags, enums and hashes raw); the passes give them meaning.
//!
//! | entity | components |
//! |---|---|
//! | placement | `core.transform`, `core.instance`, [`GtaEntity`], `core.condition` when it exists in one map state only |
//! | prototype | `core.name` (archetype name), [`GtaArchetype`], `core.mesh` and [`GtaSources`] (the drawable's high LOD), [`GtaDrawable`], `core.lods` |
//! | LOD form | `core.mesh` and [`GtaSources`] (one of the drawable's coarser LODs), unplaced |
//! | material | [`GtaShader`] (one per drawable and shader) |
//! | texture | `core.texture`, `core.name`, `core.source` |

use primase_core::{Component, Entity, JsonError, Lods, Mesh, World};
use serde::Serialize;

use crate::rage::map::{ArchetypeDef, EntityDef, EntityKind};

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

/// A prototype's bones as its drawable and fragment store them ([`crate::rage::drawable::Bones`]: the skeleton, each bone
/// in its parent's space with its parent and tag, and a fragment's own bone pose). The passes compose a bone's pose
/// from it: `gta-pose-fragments` for the models on bones (`gta.bones`), `lower-gta-lights` for the light records.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GtaSkeleton(pub crate::rage::drawable::Bones);

impl Component for GtaSkeleton {
    const NAME: &'static str = "gta.skeleton";
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
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_json(self)
    }
}
