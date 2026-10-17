//! The `skate.*` dialect: what scene-interchange v1 carries beyond the core components.
//!
//! How a scene maps onto the world (entities in the manifest's order: textures, materials, prototypes, placements,
//! collision soups, lights, grind curves, water, markers, then the prototypes' other levels of detail):
//!
//! | manifest | entity with |
//! |---|---|
//! | `textures.<key>` | [`SkateTexture`], `core.texture_file`, `core.name` (`name`), `core.source` |
//! | `materials.<key>` | [`SkateMaterial`] (texture references are entities), `core.name` (`label`), `core.source` |
//! | `prototypes[i]` | [`SkatePrototype`], [`SkateParts`], `core.mesh_file` (with each part's material), `core.material_variants` (when a part has variants), `core.name`, `core.source`, `core.collision` and [`crate::SkateRock`] (its collision), `core.lods` (`detail`, `lods`, `lod_default`) |
//! | `prototypes[i].lods[k]` | a form of `core.lods`: [`SkateParts`], `core.mesh_file` (the prototype's GLB; none for `"mesh": null`), `core.material_variants`, [`SkateForm`] (its other keys, when it has any) |
//! | `PLAC` record | `core.transform`, `core.instance`, `core.tile` (flag 1), [`SkatePlacement`], `core.source` (`placement_sources`) |
//! | `collision[i]` | [`SkateCollision`], `core.collision` and [`crate::SkateRock`] (soups' surfaces named in `core.surfaces`) |
//! | `LGHT` record | [`SkateLight`], `core.source` (`light_sources`) |
//! | `CURV` curve | [`SkateGrindCurve`] |
//! | `water[i]` | [`SkateWater`] |
//! | `markers[i]` | [`SkateMarker`], `core.name` |
//! | everything else | the [`SkateScene`] resource |
//!
//! Scene space is the world frame (metres, right-handed, +Y up, -Z north), so nothing is converted. Optional
//! manifest fields stay `Option`s: the reader never fills in a default, so writing gives back what was there.
//! Keys this crate does not model (`extras`, `extensions`, anything unknown) are kept per object in `other`, in
//! their order. A string `source` becomes `core.source`; any other `source` value stays in `other`.

use primase_core::{Component, Entity, JsonError, NormalGreen, NormalZ, Resource, World};
use serde::Serialize;
use serde_json::{Map, Value};

/// Keys of a manifest object this crate does not model, carried untouched (`extras`, `extensions`, unknown keys).
pub type Other = Map<String, Value>;

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name { $($(#[$vm])* $v),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$v => $s),+ }
            }
            pub fn parse(s: &str) -> Option<$name> {
                match s { $($s => Some($name::$v),)+ _ => None }
            }
        }
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

str_enum!(
    /// A material's class.
    MaterialClass {
        Opaque = "opaque",
        /// Alpha tested solid (fences, grilles).
        Cutout = "cutout",
        /// Thin overlay on another surface.
        Decal = "decal",
        Glass = "glass",
        Water = "water",
        Terrain = "terrain",
        /// Vertex-channel blend of ordinary materials, carried by core.layered.
        Layered = "layered",
        /// Foliage cards.
        Leaves = "leaves",
        /// Trunks, branches.
        Wood = "wood",
        /// Other blended.
        Alpha = "alpha",
    }
);

str_enum!(AlphaMode { Opaque = "opaque", Mask = "mask", Blend = "blend" });
str_enum!(Domain { Surface = "surface", Decal = "decal", Foliage = "foliage" });
str_enum!(
    /// How UVs outside 0-1 sample the base texture.
    Wrap { Repeat = "repeat", Extend = "extend", Clip = "clip", Mirror = "mirror" }
);
str_enum!(CollisionMode { TriangleMesh = "triangle_mesh", Water = "water", Hull = "hull" });
str_enum!(MarkerKind { Spawn = "spawn", Travel = "travel", Trainer = "trainer", Vfx = "vfx", TriggerEffect = "trigger_effect" });
str_enum!(SnapMode { None = "none", Ground = "ground", Street = "street", Open = "open" });

/// The standard collision surfaces, by index (a soup's surface byte without a `surfaces` list).
pub const STANDARD_SURFACES: [&str; 17] = [
    "default",
    "concrete",
    "asphalt",
    "metal",
    "wood",
    "marble",
    "grass",
    "brick",
    "glass",
    "earth",
    "sand",
    "gravel",
    "plastic",
    "rubber",
    "metal_grate",
    "metal_thin",
    "roofing",
];

/// A file inside the scene folder, as the manifest names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileRef {
    /// Relative to the scene folder, forward slashes.
    pub path: String,
    /// Lower-case hex; the reader checked it, the writer writes the copied file's.
    pub sha256: Option<String>,
}

/// A table reference: one file, or chunk files read in order as one table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TableRef {
    pub files: Vec<FileRef>,
    /// Written as a list (also for one file).
    pub list: bool,
    /// Written as an object, `{"file": ...}` (also without a sha256).
    pub object: bool,
    /// Per file as read: `[records, vertices or points]` (0 for kinds without the second). The writer splits the
    /// table the same way while the counts still add up, else writes it whole into the first file.
    pub counts: Vec<[u32; 2]>,
}

impl TableRef {
    pub fn single(path: &str) -> TableRef {
        TableRef {
            files: vec![FileRef {
                path: path.to_owned(),
                sha256: None,
            }],
            list: false,
            object: false,
            counts: Vec::new(),
        }
    }
}

/// `streaming_distances` (metres).
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct StreamingDistances {
    pub near: Option<f64>,
    pub medium: Option<f64>,
    pub far: Option<f64>,
    pub other: Other,
}

/// `texture_budget`.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct TextureBudget {
    pub max: Option<f64>,
    pub aux_weight: Option<f64>,
    pub other: Other,
}

/// `min_height`.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct MinHeight {
    pub height: Option<f64>,
    pub stub_below: Option<f64>,
    /// Pins the floor instead of measuring it.
    pub floor: Option<f64>,
    pub other: Other,
}

/// `pause_map`: the front end's own pause-map picture.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PauseMap {
    pub image: FileRef,
    /// Where the image is now (copied to `image.path` on write).
    pub image_file: std::path::PathBuf,
    /// Ground frame `[east0, north0, east1, north1]`.
    pub bounds: [f32; 4],
    pub other: Other,
}

/// The scene as a whole: the manifest's top-level fields that are not entities.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct SkateScene {
    /// `[A-Za-z0-9_-]+`.
    pub name: String,
    pub display_name: Option<String>,
    pub generator: Option<String>,
    /// Ground frame `[east0, north0, east1, north1]`, half-open.
    pub region: Option<[f32; 4]>,
    /// The names the soups' surface bytes index (`"packed:<n>"` allowed); `None`: the standard table.
    pub surfaces: Option<Vec<String>>,
    pub streaming_distances: Option<StreamingDistances>,
    pub texture_budget: Option<TextureBudget>,
    pub min_height: Option<MinHeight>,
    pub max_render_edge: Option<f64>,
    pub pause_map: Option<PauseMap>,
    pub trainer_note: Option<String>,
    pub light_prefix: Option<String>,
    pub warnings: Option<Vec<String>>,
    /// Copied into stats.json as `front_end`.
    pub stats: Option<Value>,
    /// Where the tables are written.
    pub placements: Option<TableRef>,
    pub lights: Option<TableRef>,
    pub grind_curves: Option<TableRef>,
    /// The string lists as read: the writer keeps their order and appends new strings, so unchanged scenes keep
    /// their indices. `None`: the manifest had no such key.
    pub placement_sources: Option<Vec<String>>,
    pub light_groups: Option<Vec<String>>,
    pub light_sources: Option<Vec<String>>,
    /// The manifest's top-level keys in their order (the writer keeps it, and writes present-but-empty lists).
    pub key_order: Vec<String>,
    pub other: Other,
}

impl Resource for SkateScene {
    const NAME: &'static str = "skate.scene";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A texture entry (its file is `core.texture_file`, its `name` `core.name`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateTexture {
    /// The manifest key materials use.
    pub key: String,
    pub file: FileRef,
    pub stem: Option<String>,
    pub source_format: Option<String>,
    pub source_size: Option<[i32; 2]>,
    /// `color_space`: `srgb` or `linear`.
    pub linear: Option<bool>,
    pub green: Option<NormalGreen>,
    pub z: Option<NormalZ>,
    pub other: Other,
}

/// A material entry. Texture references are the texture entities.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateMaterial {
    /// The manifest key parts use (unique up to case).
    pub key: String,
    pub class: MaterialClass,
    pub base: Option<Entity>,
    pub normal: Option<Entity>,
    pub mask: Option<Entity>,
    /// sRGB bytes.
    pub base_color: Option<[u8; 4]>,
    pub tint: Option<[u8; 3]>,
    pub alpha: Option<AlphaMode>,
    pub cutoff: Option<f64>,
    pub two_sided: Option<bool>,
    pub domain: Option<Domain>,
    pub metallic: Option<f64>,
    pub roughness: Option<f64>,
    pub wrap: Option<Wrap>,
    pub ocean: Option<bool>,
    pub cast_shadows: Option<bool>,
    pub emissive: Option<bool>,
    /// Parts fade by their COLOR_0 alpha: drawn blended even when `alpha` says otherwise.
    pub vertex_alpha: Option<bool>,
    pub other: Other,
}

/// A collision soup reference (a world `collision[i]` entry or a prototype's `collision`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SoupRef {
    pub table: TableRef,
    pub mode: Option<CollisionMode>,
    /// World soups only.
    pub ground: Option<bool>,
    pub region_cut: Option<bool>,
    pub other: Other,
}

/// One part of a mesh's primitive, in draw order (an element of [`SkateParts`]). The part's material is
/// `core.mesh_file.materials[i]` and its variants `core.material_variants[i]`.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct Part {
    /// Provenance (a string, or GTA's `{shader, bucket, model, geometry}`).
    pub source: Option<Value>,
    /// The part has a `variants` object (possibly empty: it still makes the prototype one with variants).
    pub variants_key: bool,
    pub other: Other,
}

/// A prototype entry: a mesh in its own local space, placed any number of times. Its parts are [`SkateParts`], on
/// the prototype and on each of its other forms (`core.lods`), as they go with the form's mesh.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkatePrototype {
    /// Unique; placements refer to the prototype by its index in the list.
    pub id: String,
    /// The GLB as the manifest names it; `None`: `"glb": null`, no geometry.
    pub glb: Option<FileRef>,
    /// uint32 identity (default FNV-1a 32 of the id's UTF-16 code units).
    pub key: Option<u32>,
    pub foliage: Option<bool>,
    pub ground: Option<bool>,
    pub collision: Option<SoupRef>,
    pub other: Other,
}

/// A mesh's parts (`skate.parts`): one per primitive, in draw order, on a prototype and on each of its other forms.
/// A prototype without geometry and a form that draws nothing have none.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct SkateParts(pub Vec<Part>);

/// The keys of a `lods` entry this crate does not model (`skate.form`, on the form; only when it has any).
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct SkateForm {
    pub other: Other,
}

/// A placement's skate fields (its matrix is `core.transform`, its prototype and variant `core.instance`, a given
/// tile `core.tile`).
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct SkatePlacement {
    /// Bounding radius (flag 2).
    pub radius: Option<f32>,
    /// Clip given (flag 4) and its value (flag 8).
    pub clip: Option<bool>,
    pub no_collision: bool,
    pub split_by_tile: bool,
    /// Flag bits this version does not define (kept). PLAC bits 8-11 belong to
    /// `core.priority` and must not be set here.
    pub unknown_flags: u32,
}

/// A world collision soup entry (`collision[i]`); its triangles are the entity's [`SkateSoup`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateCollision(pub SoupRef);

/// A triangle soup in scene space (world soups) or the prototype's local space (prototype collision), with v1's
/// surface bytes. The reader puts soups in the world as `core.collision`; the writers still accept this on an entity
/// without one, for worlds that build it directly, until they move to `core.collision`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SkateSoup {
    pub positions: Vec<[f32; 3]>,
    /// Counter-clockwise fronts.
    pub triangles: Vec<[u32; 3]>,
    /// Per triangle: bits 0-6 index the scene's surfaces, bit 7 natural rock.
    pub surfaces: Vec<u8>,
}

/// A light.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateLight {
    pub spot: bool,
    pub position: [f32; 3],
    /// Unit; where a spot points (fixes a point light's basis too).
    pub direction: [f32; 3],
    /// Not parallel to the direction; all 0: the writer picks one.
    pub tangent: [f32; 3],
    /// Linear RGB.
    pub color: [f32; 3],
    /// Studio watts.
    pub energy: f32,
    pub range: f32,
    pub radius: f32,
    /// Full cone angle (radians).
    pub spot_size: f32,
    pub spot_blend: f32,
    /// Studio's 7-bit mask.
    pub time_of_day: u32,
    pub cast_shadow: bool,
    /// Flag 2.
    pub priority: Option<f32>,
    /// `light_groups` entry ("" the default group).
    pub group: String,
    pub unknown_flags: u32,
}

/// A grind curve: a polyline along the top of a rail or ledge.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateGrindCurve {
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
    /// Bits 4-7: 0 rail, 1 ledge, 2 coping.
    pub kind: u8,
    /// Metres; 0: the default.
    pub radius: f32,
    /// 0 metal_rail, 1 metal, 2 metal_thin, 3 concrete, 4 wood.
    pub surface: u32,
    pub unknown_flags: u32,
}

/// A flat water surface: a convex outline in the ground frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateWater {
    pub corners: Vec<[f32; 2]>,
    pub height: Option<f64>,
    pub other: Other,
}

/// A marker's snap: the mode alone (`"street"`) or an object with parameters.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Snap {
    Mode(SnapMode),
    Object {
        mode: SnapMode,
        above: Option<f64>,
        from: Option<String>,
        min_up: Option<f64>,
        rings: Option<Vec<f32>>,
        directions: Option<f64>,
        max_above_ground: Option<f64>,
        headroom: Option<f64>,
        other: Other,
    },
}

/// A marker (its `name` is `core.name`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateMarker {
    pub kind: MarkerKind,
    /// Scene space; the height may be `None` (`null`: the snap finds it).
    pub position: Option<([f32; 2], Option<f32>)>,
    pub yaw: Option<f64>,
    /// Effects: 3x4 column-major transform.
    pub m: Option<[f32; 12]>,
    pub blueprint: Option<String>,
    pub snap: Option<Snap>,
    pub lift: Option<f64>,
    pub shelter: Option<bool>,
    pub trainer: Option<bool>,
    pub other: Other,
}

impl SkateMarker {
    /// `[x, y, z]` with `y` possibly absent.
    pub fn xyz(&self) -> Option<(f32, Option<f32>, f32)> {
        self.position.map(|([x, z], y)| (x, y, z))
    }
}

macro_rules! shown {
    ($($t:ty = $n:literal),+ $(,)?) => {$(
        impl Component for $t {
            const NAME: &'static str = $n;
            fn to_json(&self) -> Result<Option<Value>, JsonError> {
                primase_core::json::to_value(self).map(Some)
            }
        }
    )+};
}

impl Component for SkateMaterial {
    const NAME: &'static str = "skate.material";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend([self.base, self.normal, self.mask].into_iter().flatten());
    }
}

shown!(
    SkateTexture = "skate.texture",
    SkatePrototype = "skate.prototype",
    SkateParts = "skate.parts",
    SkateForm = "skate.form",
    SkatePlacement = "skate.placement",
    SkateCollision = "skate.collision",
    SkateLight = "skate.light",
    SkateGrindCurve = "skate.grind_curve",
    SkateWater = "skate.water",
    SkateMarker = "skate.marker",
);

impl Component for SkateSoup {
    const NAME: &'static str = "skate.soup";
    /// Counts, not the triangles.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(serde_json::json!({
            "vertices": self.positions.len(),
            "triangles": self.triangles.len(),
        })))
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.triangles.len() != self.surfaces.len() {
            return Err(format!(
                "{} triangles, {} surface bytes",
                self.triangles.len(),
                self.surfaces.len()
            ));
        }
        let n = self.positions.len();
        if let Some((t, i)) = self
            .triangles
            .iter()
            .enumerate()
            .find_map(|(t, tri)| tri.iter().find(|&&i| i as usize >= n).map(|i| (t, *i)))
        {
            return Err(format!("triangle {t} indexes vertex {i} of {n}"));
        }
        if let Some(i) = self.positions.iter().flatten().position(|x| !x.is_finite()) {
            return Err(format!("vertex {} is not finite", i / 3));
        }
        Ok(())
    }
}
