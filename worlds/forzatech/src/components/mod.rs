//! Forza Horizon 6's own data, as stored, on the entities the reader makes: where a placement comes from
//! (`forzatech.placement`), which `.modelbin` files a prototype or one of its forms is made of (`forzatech.model_files`)
//! and its levels before they are loaded (`forzatech.levels`),
//! the template a rule-placed prototype is (`forzatech.template`), and a material's chain from its model's instance
//! to its shader family (`forzatech.material`, with the textures it names in `forzatech.material_textures`). Nothing
//! here is lowered; passes read it.
use primase_core::{Component, Entity, Geometry, Linear, LodForm, world::Linearity};
use serde::Serialize;

/// One placement's source record: the parts of it its `core.transform` and `core.instance` do not hold.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Placement {
    pub origin: PlacementOrigin,
    /// The seasons it is not shown in: a byte of two equal nibbles, bit `s` set for season `s` (0 spring .. 3 winter).
    pub season_exclusion: u8,
    /// The material variant byte: a row of the model's variant table.
    pub material_variant: u8,
}

/// Which file placed it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum PlacementOrigin {
    /// A record of a `.pgeo` file (placed by rule).
    Rule {
        /// The file's zipcache path.
        file: String,
        /// The template library category (`PROPS`).
        category: String,
        /// The block's name: the model template it places.
        block: String,
        /// The record's index in its block.
        record: u32,
        /// The file's route, empty when none.
        route: String,
        /// The file's gameplay tag, empty when none.
        gameplay_tag: String,
        /// The record's instance variation bytes.
        variation: [u8; 4],
        /// The record's frame.
        frame: u16,
    },
    /// An instance of the track's `.pvs` (placed by hand).
    Hand {
        /// The instance's index.
        instance: u32,
        /// The instance's flags.
        flags: u16,
        /// The instance's HLOD parent.
        parent: Option<u32>,
        /// Per distance band, the LOD row drawn (an index of the prototype's forms, own mesh first); `None` where the
        /// band draws nothing.
        band_rows: [Option<u8>; 6],
        /// The gameplay record's tag, when the instance has a record.
        gameplay_tag: Option<String>,
        /// The gameplay record's routes.
        routes: Vec<u16>,
        /// The transform's tint.
        tint: [f32; 4],
        /// The transform's LOD profile group.
        lod_group: Option<u8>,
    },
}

/// The `.modelbin` files a prototype's own mesh or one of its forms is made of, in the order their meshes are joined,
/// and the level of detail taken from them: the meshes whose LOD flags have bit `level + 1` (`None`: every mesh).
/// No files: the form has no model in the install (the build stripped it).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ModelFiles {
    pub level: Option<u8>,
    pub files: Vec<ModelFile>,
}

/// A prototype's levels of detail as read, before their meshes are loaded (`forzatech.levels`): its own
/// [`ModelFiles`] are level `detail`, each form an unplaced entity holding the [`ModelFiles`] of its level, and
/// `default` indexes `[own, forms...]` as `core.lods` does. Core's LOD forms are of an entity with a mesh, so loading
/// makes `core.lods` of these once the meshes exist and removes this ([`crate::read::convert`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForzaLevels {
    pub detail: u8,
    pub forms: Vec<LodForm>,
    pub default: Option<usize>,
}

impl Component for ForzaLevels {
    const NAME: &'static str = "forzatech.levels";
    const GEOMETRY: Geometry = Geometry::None;
    /// The forms are entities of their own.
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.forms.iter().map(|f| f.form));
    }
}

/// One `.modelbin` GeoChunk entry and what its `Modl` header says.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelFile {
    pub chunk: usize,
    pub entry: usize,
    /// Its zipcache path.
    pub path: String,
    /// The header's LOD flags.
    pub lod_flags: u16,
    /// Whether the header says the file is one level of several.
    pub has_lod: bool,
    /// The header's first and last LOD.
    pub min_lod: i8,
    pub max_lod: i8,
    /// The header's box, in the game's frame.
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// A rule-placed prototype's model template, as its library stores it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Template {
    /// The template's name.
    pub name: String,
    /// The library category it is from.
    pub category: String,
    /// Its draw behaviour.
    pub draw_behaviour: String,
    /// Its model paths.
    pub models: Vec<String>,
    /// The distance ranges of the draw behaviour's slots that draw a LOD set of the template (its set for the slot is
    /// not the empty set 0), in slot order.
    pub ranges: Vec<DrawRange>,
}

/// A distance range a template is drawn in: `start <= distance < end`, metres from the camera.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DrawRange {
    /// The draw behaviour slot.
    pub slot: u8,
    pub start: f32,
    pub end: f32,
}

/// The `.pvs` LOD profile groups, as stored: a hand-placed placement's `lod_group` indexes them.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LodProfiles(pub Vec<Vec<LodProfile>>);

/// One LOD profile record: its quality tier, its pass, and its six bands' distances and fades in metres. LOD row `k`
/// of an instance is drawn in `[max(distance of bands 0..k), distance of band k)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LodProfile {
    pub tier: u16,
    pub pass: u16,
    pub distances: [f32; 6],
    pub fades: [f32; 6],
}

impl primase_core::Resource for LodProfiles {
    const NAME: &'static str = "forzatech.lod_profiles";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A material entity's `MatI` blob: the model file holding it and its id there. The instance itself is read from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialRef {
    pub chunk: usize,
    pub entry: usize,
    /// The file's zipcache path.
    pub file: String,
    pub id: u32,
}

/// Per primitive of a mesh, the `Mesh` record it was read from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MeshRecords(pub Vec<MeshRecord>);

/// What a `Mesh` record says that its primitive does not hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MeshRecord {
    /// The model file's zipcache path.
    pub file: String,
    /// The record's `Name`.
    pub name: String,
    pub lod_flags: u16,
    /// The bucket flags (render buckets: opaque, decal, transparent, shadow ...).
    pub bucket_flags: u16,
    pub bucket_order: u8,
}

impl Component for MeshRecords {
    const NAME: &'static str = "forzatech.mesh_records";
    const GEOMETRY: Geometry = Geometry::PerPrimitive;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::world::remap_list(&self.0, from).map(MeshRecords))
    }
}

macro_rules! plain {
    ($t:ty, $name:literal) => {
        impl Component for $t {
            const NAME: &'static str = $name;
            const GEOMETRY: Geometry = Geometry::None;
            fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
                Linearity::Invariant
            }
            fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
                primase_core::json::to_value(self).map(Some)
            }
        }
    };
}

plain!(Placement, "forzatech.placement");
plain!(Template, "forzatech.template");
plain!(MaterialRef, "forzatech.material_ref");

impl Component for ModelFiles {
    const NAME: &'static str = "forzatech.model_files";
    /// Names the files the mesh is read from; the mesh's geometry is not described by it.
    const GEOMETRY: Geometry = Geometry::None;
    /// The boxes are in the game's frame, not the prototype's local space: they do not follow it.
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A parameter of one level of a material's chain, as stored.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Parameter {
    /// The hash the family's `PARM` table names it by.
    pub hash: u32,
    /// The second hash, when its flag is set.
    pub flag_hash: Option<u32>,
    /// Its name in the family's `PARM` table, when the table declares the hash.
    pub name: Option<String>,
    pub value: Value,
}

/// A parameter's value, as stored ([`forzatech_formats::material::Value`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Value {
    Vector([f32; 4]),
    Colour([f32; 4]),
    Float(f32),
    /// The u32 as stored.
    Bool(u32),
    Int(i32),
    Swizzle([f32; 4]),
    /// A texture: its game path as stored (often empty) and the path hash textures are found by.
    Texture {
        path: String,
        path_hash: u32,
    },
    Sampler([u32; 4]),
    /// A gradient's entries, 4 bytes each.
    Gradient(Vec<u8>),
    Vector2([f32; 2]),
    Type12([u32; 4]),
}

/// Which level of a material's chain a parameter is defined at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Level {
    /// The model's material instance.
    Instance,
    /// The instance's parent material.
    Material,
    /// The parent's shader family's defaults (`DFPR`).
    Shader,
}

/// A model's material instance (`MATI`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InstanceLevel {
    /// The `MATI` blob's `Name`.
    pub name: String,
    pub atst: [u8; 2],
    pub xtra: Vec<u8>,
    /// The parent material's game path (`Game:\Media\...\<name>.materialbin`).
    pub parent: String,
    pub parameters: Vec<Parameter>,
    /// The 12 bytes after a version 2.0 list.
    pub trailer: Option<[u8; 12]>,
}

/// A parent material (`.materialbin`), as the track's `bin.zip` holds it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaterialLevel {
    /// The zip entry read (`materials\<season>\_0x....materialbin`, or `materials\...` for summer).
    pub entry: String,
    /// The asset manifest's source path for it.
    pub source: String,
    /// The `MATL` blob's `Name`.
    pub name: String,
    pub atst: [u8; 2],
    pub blst: [u8; 5],
    pub xtra: Option<Vec<u8>>,
    /// The shader's game path (`Game:\Media\...\<family>.shaderbin`).
    pub shader: String,
    pub technique: String,
    pub third: String,
    /// The `MTUD` pairs.
    pub user_data: Vec<(String, String)>,
    pub parameters: Vec<Parameter>,
    pub trailer: Option<[u8; 12]>,
}

/// One entry of a family's `CBMP`, `TXMP` or `SPMP` map: a parameter hash and its constant offset, texture slot or
/// sampler slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Slot {
    pub hash: u32,
    pub value: u16,
    pub guid: [u8; 16],
}

/// A family's declared parameter (`PARM`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Declared {
    /// The CRC-32 of its name.
    pub name_hash: u32,
    pub name: String,
    /// The hash material parameters carry.
    pub hash: u32,
    pub extra: Vec<u8>,
}

/// A shader family (`.shaderbin`), as the track's `bin.zip` holds it: the tables its materials resolve through.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShaderLevel {
    /// The zip entry read (`shaders\_0x....\<family>.shaderbin`).
    pub entry: String,
    /// The asset manifest's source path for it.
    pub source: String,
    /// The family: the entry's file name less `.shaderbin`.
    pub family: String,
    pub declared: Vec<Declared>,
    pub constants: Vec<Slot>,
    pub textures: Vec<Slot>,
    pub samplers: Vec<Slot>,
    pub constants_size: u16,
    /// The `DFPR` defaults.
    pub defaults: Vec<Parameter>,
    pub trailer: Option<[u8; 12]>,
}

/// A material's chain, as stored: its instance, the parent material and the parent's shader family, each when the
/// track's `bin.zip` holds it. Which value a parameter takes is [`ForzaMaterial::resolved`]'s, not stored.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForzaMaterial {
    pub instance: InstanceLevel,
    pub material: Option<MaterialLevel>,
    pub shader: Option<ShaderLevel>,
}

impl ForzaMaterial {
    /// The levels' parameters, nearest first: the instance's, the material's, the shader's defaults.
    pub fn levels(&self) -> impl Iterator<Item = (Level, &Parameter)> {
        let instance = self.instance.parameters.iter().map(|p| (Level::Instance, p));
        let material = self
            .material
            .iter()
            .flat_map(|m| &m.parameters)
            .map(|p| (Level::Material, p));
        let shader = self
            .shader
            .iter()
            .flat_map(|s| &s.defaults)
            .map(|p| (Level::Shader, p));
        instance.chain(material).chain(shader)
    }

    /// Each parameter hash's nearest definition (instance, then material, then the shader's defaults), in the order
    /// [`ForzaMaterial::levels`] first meets each hash.
    pub fn resolved(&self) -> Vec<(Level, &Parameter)> {
        let mut seen = std::collections::HashSet::new();
        self.levels().filter(|(_, p)| seen.insert(p.hash)).collect()
    }

    /// A parameter hash's nearest definition.
    pub fn get(&self, hash: u32) -> Option<(Level, &Parameter)> {
        self.levels().find(|(_, p)| p.hash == hash)
    }
}

/// The textures a material's resolved texture parameters name, by path hash: the texture entity when the track's
/// GeoChunks hold it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialTextures(pub Vec<MaterialTexture>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialTexture {
    pub path_hash: u32,
    /// `None`: no GeoChunk or library zip entry has a game path of that hash.
    pub texture: Option<primase_core::Entity>,
}

plain!(ForzaMaterial, "forzatech.material");

impl Component for MaterialTextures {
    const NAME: &'static str = "forzatech.material_textures";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<primase_core::Entity>) {
        out.extend(self.0.iter().filter_map(|t| t.texture));
    }
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
