//! The GLB writer: a [`World`] to one GLB that Blender (and any glTF 2.0 reader) imports.
//!
//! What goes where:
//! - **Prototypes** (entities with a [`Mesh`]) become glTF meshes shared by every placement that uses them with the
//!   same resolved materials; a prototype placed with several [`MaterialVariants`] gets one mesh per distinct
//!   material list, all of them sharing the same accessors.
//! - **Placements** ([`Instance`] + [`Transform`]) become nodes, the transform written as the node's matrix exactly.
//!   A mirrored transform is written as it is (glTF allows a negative determinant and reverses the winding itself).
//!   A sheared one has no translation-rotation-scale decomposition, which glTF requires of node matrices, so it is
//!   refused: `bake-nonrigid` (primase-meshops; `mirrors=keep` for this writer) moves a shear into geometry before
//!   writing. Degenerate transforms are skipped and counted. An entity with a `Mesh` and a `Transform` but no `Instance` is placed as itself.
//! - **Collections**: Blender 4.3's importer makes no collections (every object goes to the active collection;
//!   nodes without a mesh become empties, children are parented). So groups are empty nodes, one per prototype or
//!   per tile ([`GroupBy`]), with `"primase.collection": <name>` in their extras so a Blender-side script can turn
//!   them into real collections. Blender's outliner already folds each group's placements under its empty.
//! - **Materials** come from `pbr.material` ([`PbrMaterial`]). A material entity without one (and not layered) is
//!   not written: its primitives have no material in glTF (the reader's default material), counted; a program that
//!   wants a look for them states it with `prepare-gltf-materials`. A primitive with no material has none in glTF,
//!   counted.
//! - **Texture coordinates**: glTF requires every set a material samples. A primitive that lacks one is refused, as
//!   invalid input: `fill-missing-texcoords` gives it zeros, when a program chooses so.
//! - **Vertex colour**: `COLOR_0` is written for a primitive whose material takes it (glTF multiplies it in). A
//!   written primitive whose material is `core.vertex_color` `ignored` (the source's shader does not take it) is
//!   written without it, counted; a prototype's `COLOR_0` accessor is not written at all when no material drawing
//!   that primitive, variants included, reads it ([`primase_core::color0_drawn`]).
//! - **Layered materials** (`core.layered`) become one glTF material with the `PRIMASE_layered_material` extension:
//!   the weight attribute (`COLOR_n`), the mapping and up to four layers, each an index of an ordinary material in
//!   the same file (the layer materials are written like any other; their images are shared) with the layer's UV
//!   set and transform. The host material is a verbatim copy of the first non-null layer's material (name and extras
//!   aside), so a reader that ignores the extension shows one plausible layer. The extension is only in
//!   `extensionsUsed`. A primitive drawn with a layered material but lacking its weight attribute is counted. Its
//!   `version` is 2 with per-pixel channels or the over composition, 3 with the height composition
//!   (`"composition": {"height": {"contrast": c}}` and `weights.heights`, a source per slot), 4 with a power channel
//!   (`{"power": {"v": source, "exponent": n}}`), so an older consumer doesn't draw it as the form it knows.
//! - **Textures**: a `core.texture` embeds its largest level as deterministic RGBA8 PNG (identical images embedded
//!   once). Normal maps must already be green-up with stored Z; use the explicit normalization pass first. The writer
//!   reads no file: a texture still only a `core.texture_file` is skipped and counted (a reader's load step,
//!   `primase_image::load_textures`, makes its pixels). A texture whose format an 8-bit PNG cannot hold exactly
//!   (`primase_image::rgba8_exact`: wider, floating-point or signed values) is not written, counted by format
//!   (`textures_unrepresentable`), its slots left empty: `lower-textures-rgba8` converts it, each conversion named.
//! - **Extras**: every component with a JSON form (`core.source`, dialect and opaque JSON components) that the glTF
//!   itself does not express goes into the extras of the object made from its entity, as
//!   `extras.primase["<component name>"] = {"version": n, "value": ...}`: placement -> node, prototype -> mesh,
//!   material -> material, texture -> texture; world resources -> the scene. Blender imports node, mesh, material
//!   and scene extras as custom properties. Components with no JSON form are counted per name in the report, never
//!   dropped silently; a component whose view would lose a value (a non-finite float) is an error. The
//!   environment's ambient light has no glTF form: it is data in the scene's extras, lighting nothing, counted
//!   (`ambient_unlit`).
//! - **Levels of detail**: glTF draws one mesh per node and the writer chooses no form, so a placement of a prototype
//!   with other forms (`core.lods` listing any) is not written, counted (`placements_with_lods`); a pipeline that wants
//!   it drawn chooses one form per prototype first (`select-lod`). The forms are not prototypes of their own. A
//!   `core.lods` without other forms leaves nothing to choose: the mesh is written, `core.lods` in its extras.
//! - **Everything else**: without a [`Selection`], an entity the writer neither writes nor counts under one of the
//!   causes above (a `core.marker`, a standalone collision soup, a material only an unplaced variant draws) is not in
//!   the glTF; its components are counted by name (`components_unwritten`). An entity left out whole under a counted
//!   cause (an unplaced prototype, a degenerate placement, one with levels of detail) takes what it reaches with it.
//!   A selection is the caller's choice of what to write, so what it leaves out is not counted.
//! - **Shadows**: what is stated of them goes into `extras["primase.shadows"] = {"version": 1, "cast": ...,
//!   "receive": ...}`, each key only when stated: a placement's `core.shadows` on its node, each field its own else
//!   its prototype's (`cast` `"off"`, `"on"` or `"only"`, a shadow proxy that is not drawn; absent casts; `receive` a
//!   boolean), and a light's `cast_shadows` on the light's node (`"off"` or `"on"`). Materials state none.
//! - **Unlit materials** (`Shading::Unlit`) carry `KHR_materials_unlit` (in `extensionsUsed`, not required), their
//!   other slots written as glTF's fallback for a viewer without it.
//! - **Samplers**: a texture's wrap and its stated filters (`magFilter`; `minFilter` from the minification and mip
//!   filters together). A data texture with no filter stated is sampled nearest without mips (it is read exactly).
//!   glTF's `minFilter` names both filters at once: a texture stating only one of the two has no `minFilter`
//!   written, counted (`filters_unexpressed`).
//!
//! - **Rigs** (`core.rig_instance`) become a node tree: a root node at the rig's placement, one node per joint of its
//!   skeleton under its parent, each with the joint's local transform (its rig's `core.rig_pose`, else its rest) as
//!   translation, rotation and scale (a matrix where it has no such form, which no animation may then drive).
//! - **Skins**: a placement with `core.skin_instance` draws its prototype's mesh with that binding's skin: one glTF skin
//!   per (rig, prototype, binding), its joints the rig's joint nodes ([`crate::rig`]: the binding's palettes unified
//!   into one list; slot indices rewritten; a whole-bound primitive spelled out as `JOINTS_0`/`WEIGHTS_0`), the inverse
//!   binds as stored. The skinned node has no transform (glTF places a skinned mesh by its joints) and sits at the
//!   scene's root. Weights must sum to 1 as stored (glTF requires it): others are refused, naming the pass that
//!   normalises them. A skinned prototype placed without a rig is drawn in its bind pose, without its skin, counted.
//! - **Morph targets** (`core.morphs`, each target one frame at weight 1) become the primitives' `targets` (dense
//!   deltas; a primitive without deltas for a target gets a zero accessor), the mesh's `weights` (defaults) and
//!   `extras.targetNames`; a placement's `core.morph_weights` its node's `weights`. Targets with in-between frames
//!   have no glTF form: not written, counted (`flatten` them first by choice).
//! - **Clips** bound by `core.clip_binding` become animations: a joint channel drives its rig's joint node, the morph
//!   channels of one mesh occurrence one `weights` channel (they must share key times and interpolation; else they
//!   are left out, counted). Interpolation as stated (`STEP`, `LINEAR`, `CUBICSPLINE`: core's rules are glTF's).
//!   What glTF has no form for is left out and counted: additive clips, root motion, source controls; a looping
//!   clip is marked in the animation's extras (`primase.loop`). Nothing is resampled.
//! - Rigs, skins and animations are written for a whole world, not with a [`Selection`] (tiles).
//!
//! The output is a function of the world: every list is built in entity order or first-use order over entities in
//! order, so the same world gives the same bytes (`tests::output_is_deterministic`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use primase_core::detail::{
    DetailMix, DetailedMaterial, Frozen, GlossScale, ReflectanceTerm, SurfaceLowering, WeightStep,
};
use primase_core::layered::{Channel, Composition, Source};
use primase_core::pbr::{
    AlphaMode, Filter, Filters, MipFilter, PbrMaterial, Shading, TextureRef, UvTransform, Wrap,
};
use primase_core::state::{Condition, PrimitiveCondition, StateOverride};
use primase_core::{
    BindingId, Clip, ClipBinding, ClipTarget, Extrapolation, Interpolation, JointId, MorphWeights, Morphs,
    Property, Rest, RigInstance, RigPose, Skeleton, Skin, SkinInstance,
};
use primase_core::{ColorSpace, LayeredMaterial, WeightMapping};
use primase_core::{
    Component, ComponentJson, Entity, Instance, Light, LightKind, Lods, MaterialVariants, Mesh, MeshFile,
    Name, Semantic, ShadowCast, Shadows, Texture, TextureFile, Tile, Topology, Transform, TransformKind,
    World,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use primase_core::json::f32s_value;
use primase_glb::{Buffers, Document, ElementType, PrimitiveAccessors};

use crate::{Error, Result};

/// How placements are grouped under empty nodes (Blender: parent empties; see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GroupBy {
    /// Every placement at the scene root.
    None,
    /// One group per prototype, named like it: all placements of a wall under one empty.
    #[default]
    Prototype,
    /// One group per placement `core.tile` (`tile_<col>_<row>`), `untiled` for placements without one.
    Tile,
}

/// Writer options.
#[derive(Clone, Debug, Default)]
pub struct WriteOptions {
    pub group_by: GroupBy,
    /// Also write prototypes that nothing places, at the origin (not with a [`Selection`]).
    pub include_unplaced: bool,
    /// The glTF scene's name.
    pub scene_name: Option<String>,
    /// Write only part of the world (a tile of it); `None`: everything.
    pub selection: Option<Selection>,
    /// Also write, for each placed prototype with [`MaterialVariants`], one glTF mesh per variant it can be drawn with
    /// (the variants its material maps name, its placements' own and those their `core.state_override`s switch to)
    /// that no placement draws, so a viewer that switches a placement's variant finds its mesh
    /// ([`Drawn::variants`]). Not with a [`Selection`].
    pub all_variants: bool,
    /// How many threads encode the images ([`primase_core::parallel`]): with more than one and no [`Selection`], every
    /// image a material binds is encoded up front, at most this many at once, the same bytes in the same order as one
    /// at a time. Default one.
    pub threads: primase_core::parallel::Threads,
    /// Image encodings shared with the caller's other writes (a tiled writer writing many documents of one world):
    /// each texture's PNG taken from it when it holds one of the same content, else encoded and kept. `None`: each
    /// write encodes its own. The same bytes either way.
    pub images: Option<ImageMemo>,
    /// The caller has verified the world's components ([`World::verify_components`]) and left it unchanged since: the
    /// writer does not verify them again. By default it does, at its entry: it indexes by what the verifiers state
    /// (morph deltas one per vertex, clip values per key, joints' parents in their skeleton), and a world that breaks
    /// them is an [`Error::World`], not an index out of range. A tiled writer that writes many documents of one world
    /// verifies it once.
    pub verified: bool,
}

/// PNG encodings of textures' level 0, kept between writes that share it ([`WriteOptions::images`]): by the texture's
/// content (format, size and level 0's bytes), so a texture changed between writes is encoded again; at most `cap`
/// bytes of PNGs held, the oldest dropped first (encoded again when they come back). Clones share one memo; usable
/// from several threads.
#[derive(Clone)]
pub struct ImageMemo(std::sync::Arc<std::sync::Mutex<Encodings>>);

#[derive(Default)]
struct Encodings {
    entries: BTreeMap<[u8; 32], Vec<u8>>,
    order: std::collections::VecDeque<[u8; 32]>,
    bytes: usize,
    cap: usize,
    dropped: u64,
}

impl ImageMemo {
    /// A memo holding at most `cap` bytes of PNGs.
    pub fn new(cap: usize) -> ImageMemo {
        ImageMemo(std::sync::Arc::new(std::sync::Mutex::new(Encodings {
            cap,
            ..Encodings::default()
        })))
    }

    /// Encodings dropped to stay within the bound, so far.
    pub fn dropped(&self) -> u64 {
        self.0.lock().map_or(0, |m| m.dropped)
    }

    /// `texture`'s level 0 as PNG ([`primase_image::texture_level_png`]): held, else encoded and kept.
    fn png(&self, texture: &Texture) -> primase_image::Result<Vec<u8>> {
        let mut h = Sha256::new();
        h.update(format!("{:?} {} {} ", texture.format, texture.width, texture.height).as_bytes());
        if let Some(level) = texture.levels.first() {
            h.update((level.len() as u64).to_le_bytes());
            h.update(level);
        }
        let key: [u8; 32] = h.finalize().into();
        // a poisoned memo only means another write panicked: encode without it
        if let Some(png) = self.0.lock().ok().and_then(|m| m.entries.get(&key).cloned()) {
            return Ok(png);
        }
        let png = primase_image::texture_level_png(texture, 0)?;
        if let Ok(mut m) = self.0.lock()
            && !m.entries.contains_key(&key)
        {
            m.bytes += png.len();
            m.entries.insert(key, png.clone());
            m.order.push_back(key);
            while m.bytes > m.cap {
                let Some(old) = m.order.pop_front() else { break };
                if let Some(b) = m.entries.remove(&old) {
                    m.bytes -= b.len();
                    m.dropped += 1;
                }
            }
        }
        Ok(png)
    }
}

impl fmt::Debug for ImageMemo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.lock() {
            Ok(m) => f
                .debug_struct("ImageMemo")
                .field("held", &m.entries.len())
                .field("bytes", &m.bytes)
                .field("cap", &m.cap)
                .finish(),
            Err(_) => f.write_str("ImageMemo(poisoned)"),
        }
    }
}

/// What to write of the world a tiled writer is given, without changing it: only these placements (instances or
/// self-placed meshes), each drawn with its prototype's mesh or a piece of its own and with extra node extras; only
/// these lights; the scene's extras without the omitted resources. The materials and textures written are those the
/// written meshes use, as always.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub placements: BTreeMap<Entity, Selected>,
    pub lights: BTreeSet<Entity>,
    /// Resources the scene's extras leave out (a writer that carries them elsewhere).
    pub omit_resources: BTreeSet<String>,
}

/// One selected placement.
#[derive(Clone, Debug, Default)]
pub struct Selected {
    /// A mesh of its own instead of its prototype's (the triangles of it a tile holds).
    pub piece: Option<Piece>,
    /// Extra entries of the node's `extras.primase`: (name, schema version, value), placed among the placement's own
    /// components in name order.
    pub extras: Vec<(String, u32, Value)>,
}

/// A placement's own piece of its prototype's mesh: the triangles of it a tile holds.
#[derive(Clone, Debug, Default)]
pub struct Piece {
    /// Its geometry; its primitives' materials are final (no variants apply).
    pub mesh: Mesh,
    /// The name its glTF mesh is given.
    pub name: String,
    /// For each of its primitives, the prototype primitive it is cut from. The prototype's components are written
    /// with the piece as with the prototype's own mesh; those that follow primitives
    /// ([`primase_core::Component::remap_primitives`]: provenance parts, dialect per-primitive data) remapped by it.
    pub from: Vec<usize>,
}

/// What the writer did, with counts of everything it skipped or approximated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriteReport {
    pub nodes: u64,
    pub group_nodes: u64,
    /// Lights written (`KHR_lights_punctual`).
    pub lights: u64,
    pub meshes: u64,
    pub primitives: u64,
    pub triangles: u64,
    pub materials: u64,
    pub textures: u64,
    pub images: u64,
    pub image_bytes: u64,
    /// Placements written as nodes (including self-placed and unplaced-at-origin ones).
    pub placements: u64,
    /// Mirrored placements, written with their negative-determinant matrix (glTF then reverses the winding).
    pub placements_mirrored: u64,
    pub placements_degenerate_skipped: u64,
    /// Lights not written because their transform is sheared: a glTF node matrix must decompose into translation,
    /// rotation and scale, and `bake-nonrigid` has no geometry of a light's to bake it into.
    pub lights_sheared_skipped: u64,
    /// Placements whose prototype has no `core.mesh` (nothing to draw).
    pub placements_without_mesh: u64,
    /// Placements (as `placements` counts them) not written because their prototype has levels of detail
    /// (`core.lods` with other forms): the writer chooses no form (`select-lod` does).
    pub placements_with_lods: u64,
    /// Prototypes nothing places (written at the origin only with `include_unplaced`).
    pub prototypes_unplaced: u64,
    /// Material entities without `pbr.material` (nor layered): not written, their primitives drawn with none.
    pub materials_without_pbr: u64,
    /// Layer slots of layered materials whose material has no `pbr.material`: written as empty slots.
    pub layers_without_pbr: u64,
    pub primitives_without_material: u64,
    /// `core.layered` materials written with `PRIMASE_layered_material`.
    pub layered_materials: u64,
    /// `pbr.detail` materials written with `PRIMASE_materials_detail_blend` beside their `pbr.material`.
    pub detail_materials: u64,
    /// `pbr.detail` materials written without the extension (their `pbr.material` alone), by reason.
    pub detail_materials_dropped: BTreeMap<String, u64>,
    /// Primitives drawn with a layered material but lacking its weight attribute (they render as the host).
    pub primitives_layered_without_weights: u64,
    /// Written primitives with a `COLOR_0` stream drawn without it: their material is `core.vertex_color` `ignored`.
    pub primitives_color0_ignored: u64,
    /// Texture slots left empty, by reason (`dds`, `other file format`, ...), counted per texture entity.
    pub textures_skipped: BTreeMap<String, u64>,
    /// Textures whose format an 8-bit PNG cannot hold exactly ([`primase_image::rgba8_exact`]: wider, floating-point
    /// or signed values), by format: not written, their slots left empty (`lower-textures-rgba8` converts them).
    pub textures_unrepresentable: BTreeMap<String, u64>,
    /// Material texture slots left empty because their texture was skipped or could not be held exactly.
    pub texture_slots_dropped: u64,
    /// Texture slots stating a minification filter without a mip filter, or the reverse: glTF's `minFilter` names
    /// both, so neither is written.
    pub filters_unexpressed: u64,
    /// Vertex streams not written, by glTF attribute name (skin attributes without a skeleton).
    pub streams_skipped: BTreeMap<String, u64>,
    /// Components with no JSON form, by name: not in the extras.
    pub extras_unencoded: BTreeMap<String, u64>,
    /// Rigs written as node trees, and their joint nodes.
    pub rigs: u64,
    pub joint_nodes: u64,
    /// glTF skins written.
    pub skins: u64,
    /// Placements of a skinned prototype without a `core.skin_instance` (or with a selection): drawn in the bind pose.
    pub skinned_without_rig: u64,
    /// Morph targets written (per mesh), and meshes whose targets have in-between frames (no glTF form, not written).
    pub morph_targets: u64,
    pub morphs_with_frames: u64,
    /// Animations and channels written.
    pub animations: u64,
    pub animation_channels: u64,
    /// Left out, with no glTF form: additive clips, clips' root motion, source control, draw-group and material channels, and morph channels
    /// of one occurrence that do not share key times and interpolation.
    pub clips_additive: u64,
    pub root_motions: u64,
    pub control_channels: u64,
    pub draw_group_channels: u64,
    pub material_parameter_channels: u64,
    pub morph_channels_unaligned: u64,
    /// `core.environment`'s ambient, 1 when the world has one: glTF has no ambient light, so nothing is lit by it
    /// (the scene's extras still carry it as data, unless the selection omits the environment).
    pub ambient_unlit: u64,
    /// Entities with components that are not written and not counted by any field above (markers, standalone
    /// collision, ...; see the module docs). Never with a [`Selection`].
    pub entities_unwritten: u64,
    /// Their components, by name.
    pub components_unwritten: BTreeMap<String, u64>,
    pub glb_bytes: u64,
    /// Where each thing written is in the glTF, by entity (see [`Layout`]).
    pub layout: Layout,
}

/// Where the writer put what it wrote, by the entity it came from, so a consumer that carries more than glTF says
/// (the preview manifest) can point into the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Each placement written as a node.
    pub placements: BTreeMap<Entity, Drawn>,
    /// Each rig written as a node tree.
    pub rigs: BTreeMap<Entity, RigNodes>,
}

/// A placement as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drawn {
    /// Its glTF node.
    pub node: usize,
    /// The glTF mesh its node draws.
    pub mesh: usize,
    /// With [`WriteOptions::all_variants`]: the glTF mesh of each variant the placement can be drawn with (its own
    /// among them), by [`Instance::variant`] value; empty otherwise.
    pub variants: BTreeMap<u32, usize>,
}

/// A rig as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RigNodes {
    /// The rig's root node.
    pub root: usize,
    /// Each joint's node.
    pub joints: BTreeMap<JointId, usize>,
}

impl fmt::Display for WriteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "glb {} bytes: {} nodes ({} groups), {} meshes, {} primitives, {} triangles, {} materials, {} textures, \
             {} images ({} bytes)",
            self.glb_bytes,
            self.nodes,
            self.group_nodes,
            self.meshes,
            self.primitives,
            self.triangles,
            self.materials,
            self.textures,
            self.images,
            self.image_bytes
        )?;
        writeln!(
            f,
            "placements {}: mirrored {}, degenerate skipped {}, without mesh {}, with levels of detail (not \
             written) {}; prototypes unplaced {}; lights sheared (not written) {}",
            self.placements,
            self.placements_mirrored,
            self.placements_degenerate_skipped,
            self.placements_without_mesh,
            self.placements_with_lods,
            self.prototypes_unplaced,
            self.lights_sheared_skipped
        )?;
        writeln!(
            f,
            "materials without pbr {} (layer slots {}), primitives without material {}, layered materials {} \
             (primitives without weights {}), primitives drawn without their COLOR_0 (material ignores it) {}, \
             texture slots dropped {}",
            self.materials_without_pbr,
            self.layers_without_pbr,
            self.primitives_without_material,
            self.layered_materials,
            self.primitives_layered_without_weights,
            self.primitives_color0_ignored,
            self.texture_slots_dropped
        )?;
        writeln!(f, "textures skipped {:?}", self.textures_skipped)?;
        writeln!(
            f,
            "textures an 8-bit PNG cannot hold exactly (not written) {:?}",
            self.textures_unrepresentable
        )?;
        writeln!(
            f,
            "texture filters glTF cannot express (min without mip, or mip without min) {}",
            self.filters_unexpressed
        )?;
        writeln!(f, "streams skipped {:?}", self.streams_skipped)?;
        writeln!(f, "extras unencoded {:?}", self.extras_unencoded)?;
        writeln!(
            f,
            "rigs {} ({} joint nodes), skins {}, skinned placements without a rig (bind pose) {}, morph targets {} \
             (meshes with in-between frames, not written {}), animations {} ({} channels); left out: additive clips \
             {}, root motions {}, control channels {}, draw-group channels {}, material-parameter channels {}, unaligned morph channels {}",
            self.rigs,
            self.joint_nodes,
            self.skins,
            self.skinned_without_rig,
            self.morph_targets,
            self.morphs_with_frames,
            self.animations,
            self.animation_channels,
            self.clips_additive,
            self.root_motions,
            self.control_channels,
            self.draw_group_channels,
            self.material_parameter_channels,
            self.morph_channels_unaligned
        )?;
        writeln!(f, "ambient not lit (no glTF form) {}", self.ambient_unlit)?;
        write!(
            f,
            "entities not written {}: components {:?}",
            self.entities_unwritten, self.components_unwritten
        )
    }
}

/// Components the glTF expresses itself, per kind of object; everything else with a JSON form goes to extras.
/// The extension a texture slot's [`UvTransform`] is written as.
const TEXTURE_TRANSFORM: &str = "KHR_texture_transform";
const EMISSIVE_STRENGTH: &str = "KHR_materials_emissive_strength";
const SPECULAR: &str = "KHR_materials_specular";
/// The detail blend's vendor extension ([`crate::detail`]).
const DETAIL: &str = crate::detail::NAME;
const UNLIT: &str = "KHR_materials_unlit";
/// The extension lights are written as.
const LIGHTS: &str = "KHR_lights_punctual";
const CONSUMED_LIGHT: &[&str] = &[Name::NAME, Transform::NAME, Light::NAME];
const CONSUMED_PLACEMENT: &[&str] = &[
    Name::NAME,
    Transform::NAME,
    Instance::NAME,
    Shadows::NAME,
    SkinInstance::NAME,
    MorphWeights::NAME,
    // what the preview manifest carries, by id (the plain writer refuses these: resolve-state first)
    Condition::NAME,
    PrimitiveCondition::NAME,
    StateOverride::NAME,
];
/// A placement's shadows are its node's, also when an entity with a mesh is placed as itself.
const CONSUMED_PROTOTYPE: &[&str] = &[
    Name::NAME,
    Mesh::NAME,
    MaterialVariants::NAME,
    MeshFile::NAME,
    Shadows::NAME,
    Skin::NAME,
    Morphs::NAME,
    Condition::NAME,
    PrimitiveCondition::NAME,
    StateOverride::NAME,
];
/// The extras key shadows are written under (see the module docs).
const SHADOWS: &str = "primase.shadows";
const LAYERED: &str = "PRIMASE_layered_material";
const CONSUMED_MATERIAL: &[&str] = &[Name::NAME, PbrMaterial::NAME, LayeredMaterial::NAME];
const CONSUMED_TEXTURE: &[&str] = &[Name::NAME, TextureFile::NAME, Texture::NAME];

/// Write the world as GLB bytes.
pub fn write(world: &World, options: &WriteOptions) -> Result<(Vec<u8>, WriteReport)> {
    let (doc, mut report) = write_document(world, options)?;
    let bytes = doc.to_glb()?;
    report.glb_bytes = bytes.len() as u64;
    Ok((bytes, report))
}

/// [`write`] as a [`Document`], for a caller that changes it further before assembling the GLB once
/// (`WriteReport::glb_bytes` is then 0).
pub fn write_document(world: &World, options: &WriteOptions) -> Result<(Document, WriteReport)> {
    if !options.verified
        && let Some((name, entity, detail)) = world.verify_components().into_iter().next()
    {
        return Err(Error::World {
            entity,
            detail: format!("{name}: {detail}"),
        });
    }
    let mut w = Writer {
        world,
        options,
        report: WriteReport::default(),
        buffers: Buffers::default(),
        meshes: Vec::new(),
        materials: Vec::new(),
        textures: Vec::new(),
        samplers: Vec::new(),
        images: Vec::new(),
        proto_accessors: BTreeMap::new(),
        mesh_index: BTreeMap::new(),
        material_index: BTreeMap::new(),
        texture_index: BTreeMap::new(),
        image_of_entity: BTreeMap::new(),
        image_by_hash: BTreeMap::new(),
        sampler_index: BTreeMap::new(),
        texture_transform_used: false,
        emissive_strength_used: false,
        specular_used: false,
        detail_used: false,
        unlit_used: false,
        layered_weights: BTreeMap::new(),
        handled: BTreeSet::new(),
        left_out: Vec::new(),
        joint_nodes: BTreeMap::new(),
        matrix_joints: BTreeSet::new(),
        rig_roots: BTreeMap::new(),
        skins: Vec::new(),
        skin_index: BTreeMap::new(),
        layouts: BTreeMap::new(),
        morph_targets: BTreeMap::new(),
        animations: Vec::new(),
        drawn: BTreeMap::new(),
        encoded: BTreeMap::new(),
    };
    let json = w.document()?;
    Ok((Document::new(json, std::mem::take(&mut w.buffers.bin)), w.report))
}

/// With threads and no selection: every texture that a material reached from `places` (the placements written,
/// [`World::closure`]) binds, a `pbr.material`'s slots and a `core.layered`'s masks, and that the writer could embed,
/// encoded (or the encoder's error) on the stated threads, for [`Writer::image`] to take instead of encoding it then.
fn preencode(
    world: &World,
    options: &WriteOptions,
    places: &[Place],
) -> BTreeMap<Entity, std::result::Result<Vec<u8>, String>> {
    if options.threads.get() < 2 || options.selection.is_some() {
        return BTreeMap::new();
    }
    let reached = world.closure(places.iter().map(|p| p.entity));
    let mut bound = BTreeSet::new();
    for &e in &reached {
        if let Some(m) = world.get::<PbrMaterial>(e) {
            bound.extend(m.textures().map(|r| r.texture));
        }
        if let Some(m) = world.get::<LayeredMaterial>(e) {
            bound.extend(m.mask_textures().map(|r| r.texture));
        }
    }
    let mut todo: Vec<(Entity, &Texture)> = bound
        .into_iter()
        .filter_map(|e| world.get::<Texture>(e).map(|t| (e, t)))
        .filter(|(_, t)| {
            primase_image::rgba8_exact(t.format)
                && (t.role != primase_core::TexRole::Normal
                    || t.normal == Some(primase_core::NormalConvention::GLTF))
        })
        .collect();
    // the largest first, so the longest encodings do not start last; each image's bytes are its own either way
    todo.sort_by_key(|(e, t)| (std::cmp::Reverse(u64::from(t.width) * u64::from(t.height)), *e));
    match primase_core::parallel::map(options.threads, &todo, |_, (_, t)| {
        primase_image::texture_level_png(t, 0).map_err(|e| e.to_string())
    }) {
        Ok(done) => todo.iter().map(|(e, _)| *e).zip(done).collect(),
        // a panicking encoder: each image is encoded when it is reached instead, where the error is its own
        Err(_) => BTreeMap::new(),
    }
}

/// Write the world to a GLB file.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: writes the GLB file"
)]
pub fn write_file(world: &World, options: &WriteOptions, path: &Path) -> Result<WriteReport> {
    let (bytes, report) = write(world, options)?;
    std::fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    Ok(report)
}

/// A placement to write as one node.
struct Place {
    entity: Entity,
    prototype: Entity,
    variant: u32,
    transform: Transform,
}

/// A written mesh's source: its geometry, its glTF name, and the entity whose components go to its extras.
struct MeshFrom {
    src: Src,
    name: String,
    extras_of: ExtrasOf,
}

/// Whose components a written mesh's extras show.
#[derive(Clone, Debug)]
enum ExtrasOf {
    /// The prototype's, as they are.
    Prototype(Entity),
    /// The prototype's, per-primitive ones remapped to a piece cut from it ([`Piece::from`]).
    Piece { prototype: Entity, from: Vec<usize> },
}

/// Where a written mesh's geometry comes from: a prototype's mesh, or a selected placement's own piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Src {
    Prototype(Entity),
    Piece(Entity),
    /// A prototype's mesh drawn with one binding of its skin (its accessors carry the skin attributes).
    Skinned(Entity, BindingId),
}

/// Group a placement belongs to; the derived order is the order of the group nodes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Prototype(Entity),
    Tile(i32, i32),
    Untiled,
}

struct Writer<'w> {
    world: &'w World,
    options: &'w WriteOptions,
    /// The images encoded up front ([`preencode`]), taken as they are reached.
    encoded: BTreeMap<Entity, std::result::Result<Vec<u8>, String>>,
    report: WriteReport,
    /// The binary buffer, its views and accessors.
    buffers: Buffers,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    textures: Vec<Value>,
    samplers: Vec<Value>,
    images: Vec<Value>,
    proto_accessors: BTreeMap<Src, Vec<PrimitiveAccessors>>,
    mesh_index: BTreeMap<(Src, Vec<Option<Entity>>), usize>,
    /// Material entity -> glTF index and the texture coordinate sets its written texture slots sample.
    /// Each material entity's glTF material and the sets it samples; `None` for one without `pbr.material`.
    material_index: BTreeMap<Entity, Option<(usize, BTreeSet<u8>)>>,
    texture_index: BTreeMap<(Entity, [Wrap; 2], Filters), usize>,
    image_of_entity: BTreeMap<Entity, Option<usize>>,
    /// By the PNG's digest and whether the texture is data: an image tagged `primase.role` data is not shared with a
    /// colour texture of the same pixels, which a consumer may encode lossily.
    image_by_hash: BTreeMap<([u8; 32], bool), usize>,
    /// By wrap and whether the texture is data (sampled nearest, without mips).
    sampler_index: BTreeMap<([Wrap; 2], Filters, bool), usize>,
    /// A written texture slot has a transform (`KHR_texture_transform` is used).
    texture_transform_used: bool,
    /// A written material has an emissive strength other than 1 (`KHR_materials_emissive_strength` is used).
    emissive_strength_used: bool,
    specular_used: bool,
    /// A material carries `PRIMASE_materials_detail_blend`.
    detail_used: bool,
    unlit_used: bool,
    /// Layered material entity -> the glTF attribute carrying its weights.
    layered_weights: BTreeMap<Entity, String>,
    /// Entities whose components are written, or counted in the report as not written.
    handled: BTreeSet<Entity>,
    /// Entities left out whole under a counted cause, with what they reach.
    left_out: Vec<Entity>,
    /// Each rig's joint nodes, and which of them carry a matrix (no animation may drive those).
    joint_nodes: BTreeMap<(Entity, JointId), usize>,
    matrix_joints: BTreeSet<usize>,
    /// Each rig's root node.
    rig_roots: BTreeMap<Entity, usize>,
    skins: Vec<Value>,
    skin_index: BTreeMap<(Entity, Entity, BindingId), usize>,
    layouts: BTreeMap<(Entity, BindingId), crate::rig::SkinLayout>,
    /// Per prototype with written morph targets: per primitive its targets, the defaults and the names.
    morph_targets: BTreeMap<Entity, Option<MorphAccessors>>,
    animations: Vec<Value>,
    /// Each written placement's mesh and variant meshes ([`Drawn`], without its node).
    drawn: BTreeMap<Entity, (usize, BTreeMap<u32, usize>)>,
}

/// A prototype's morph targets as written.
#[derive(Clone, Debug)]
struct MorphAccessors {
    targets: Vec<Vec<Value>>,
    defaults: Vec<f32>,
    names: Vec<String>,
    ids: Vec<primase_core::MorphId>,
}

impl Writer<'_> {
    fn document(&mut self) -> Result<Value> {
        let places = self.places()?;
        self.encoded = preencode(self.world, self.options, &places);
        let mut nodes: Vec<Value> = Vec::new();
        let mut groups: BTreeMap<GroupKey, Vec<usize>> = BTreeMap::new();
        let mut roots: Vec<usize> = Vec::new();
        if self.options.selection.is_none() {
            self.rigs(&mut nodes, &mut roots)?;
        }
        let mut placement_nodes: BTreeMap<Entity, usize> = BTreeMap::new();
        for place in &places {
            let Some(node) = self.placement_node(place)? else {
                continue;
            };
            let skinned = node.get("skin").is_some();
            nodes.push(node);
            let index = nodes.len() - 1;
            placement_nodes.insert(place.entity, index);
            if let Some((mesh, variants)) = self.drawn.remove(&place.entity) {
                self.report.layout.placements.insert(
                    place.entity,
                    Drawn {
                        node: index,
                        mesh,
                        variants,
                    },
                );
            }
            match self.group_key(place) {
                // glTF places a skinned mesh by its joints: its node is a root
                Some(key) if !skinned => groups.entry(key).or_default().push(index),
                _ => roots.push(index),
            }
        }
        for (&(rig, joint), &node) in &self.joint_nodes {
            if let Some(&root) = self.rig_roots.get(&rig) {
                self.report
                    .layout
                    .rigs
                    .entry(rig)
                    .or_insert_with(|| RigNodes {
                        root,
                        joints: BTreeMap::new(),
                    })
                    .joints
                    .insert(joint, node);
            }
        }
        if self.options.selection.is_none() {
            self.animations(&placement_nodes)?;
        }
        // lights: root nodes placed by their transform, the light in KHR_lights_punctual
        let mut lights = Vec::new();
        let world = self.world;
        for (e, l) in world.iter::<Light>() {
            if self
                .options
                .selection
                .as_ref()
                .is_some_and(|s| !s.lights.contains(&e))
            {
                continue;
            }
            // a node matrix must decompose into translation, rotation and scale: a sheared light is not written
            if world
                .get::<Transform>(e)
                .is_some_and(|t| t.classify() == TransformKind::Sheared)
            {
                self.report.lights_sheared_skipped += 1;
                self.handled.insert(e);
                continue;
            }
            let mut node = Map::new();
            node.insert("name".into(), Value::String(self.name_of(e, "light")));
            if let Some(t) = world.get::<Transform>(e)
                && *t != Transform::IDENTITY
            {
                let c = t.cols;
                let m = [
                    c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1],
                    c[2][2], 0.0, c[3][0], c[3][1], c[3][2], 1.0,
                ];
                if m.iter().any(|x| !x.is_finite()) {
                    return Err(Error::World {
                        entity: e,
                        detail: "non-finite transform".into(),
                    });
                }
                node.insert("matrix".into(), f32s_value(&m)?);
            }
            node.insert("extensions".into(), json!({ LIGHTS: {"light": lights.len()} }));
            let extras = self.extras(e, CONSUMED_LIGHT)?;
            if let Some(extras) = with_shadows(extras, l.cast_shadows.map(cast_on), None) {
                node.insert("extras".into(), extras);
            }
            lights.push(self.light(e, l)?);
            self.handled.insert(e);
            nodes.push(Value::Object(node));
            roots.push(nodes.len() - 1);
        }
        self.report.lights = lights.len() as u64;
        self.count_unwritten();
        self.report.nodes = nodes.len() as u64;
        for (key, children) in groups {
            let name = match key {
                GroupKey::Prototype(p) => self.name_of(p, "prototype"),
                GroupKey::Tile(c, r) => format!("tile_{c}_{r}"),
                GroupKey::Untiled => "untiled".to_owned(),
            };
            let kind = match key {
                GroupKey::Prototype(_) => "prototype",
                _ => "tile",
            };
            nodes.push(json!({
                "name": name,
                "children": children,
                "extras": {"primase.collection": name, "primase.group": kind},
            }));
            roots.push(nodes.len() - 1);
            self.report.group_nodes += 1;
        }
        self.report.nodes = nodes.len() as u64;

        let mut scene = Map::new();
        if let Some(name) = &self.options.scene_name {
            scene.insert("name".into(), Value::String(name.clone()));
        }
        scene.insert("nodes".into(), json!(roots));
        if self
            .world
            .resource::<primase_core::Environment>()
            .is_some_and(|e| e.ambient.is_some())
        {
            self.report.ambient_unlit = 1;
        }
        let mut resources = self.world.resources_json().map_err(|e| Error::Invalid {
            item: format!("resource {}", e.component),
            detail: e.message,
        })?;
        if let Some(s) = &self.options.selection {
            resources.retain(|r| !s.omit_resources.contains(r.name));
        }
        if let Some(extras) = self.primase_extras(resources, &[]) {
            scene.insert("extras".into(), extras);
        }

        let mut root = Map::new();
        root.insert(
            "asset".into(),
            json!({"version": "2.0", "generator": concat!("primase-gltf ", env!("CARGO_PKG_VERSION"))}),
        );
        let mut used = Vec::new();
        if self.texture_transform_used {
            used.push(TEXTURE_TRANSFORM);
        }
        if self.emissive_strength_used {
            used.push(EMISSIVE_STRENGTH);
        }
        if self.specular_used {
            used.push(SPECULAR);
        }
        if self.unlit_used {
            used.push(UNLIT);
        }
        if self.detail_used {
            used.push(DETAIL);
        }
        if !self.layered_weights.is_empty() {
            used.push(LAYERED);
        }
        if !lights.is_empty() {
            used.push(LIGHTS);
            root.insert("extensions".into(), json!({ LIGHTS: {"lights": lights} }));
        }
        if !used.is_empty() {
            root.insert("extensionsUsed".into(), json!(used));
        }
        root.insert("scene".into(), json!(0));
        root.insert("scenes".into(), json!([Value::Object(scene)]));
        root.insert("nodes".into(), Value::Array(nodes));
        let lists = [
            ("skins", std::mem::take(&mut self.skins)),
            ("animations", std::mem::take(&mut self.animations)),
            ("meshes", std::mem::take(&mut self.meshes)),
            ("materials", std::mem::take(&mut self.materials)),
            ("textures", std::mem::take(&mut self.textures)),
            ("samplers", std::mem::take(&mut self.samplers)),
            ("images", std::mem::take(&mut self.images)),
            ("accessors", std::mem::take(&mut self.buffers.accessors)),
            ("bufferViews", std::mem::take(&mut self.buffers.views)),
        ];
        for (key, list) in lists {
            if !list.is_empty() {
                root.insert(key.into(), Value::Array(list));
            }
        }
        if !self.buffers.bin.is_empty() {
            root.insert("buffers".into(), json!([{"byteLength": self.buffers.bin.len()}]));
        }
        Ok(Value::Object(root))
    }

    /// Every placement to write, in entity order: instances, self-placed meshes, and (with `include_unplaced`)
    /// prototypes nothing places.
    fn places(&mut self) -> Result<Vec<Place>> {
        let world = self.world;
        let selection = self.options.selection.as_ref();
        let chosen = |e: Entity| selection.is_none_or(|s| s.placements.contains_key(&e));
        let mut places = Vec::new();
        let mut placed = BTreeSet::new();
        // a prototype's other forms are not prototypes of their own
        let forms: BTreeSet<Entity> = world
            .iter::<Lods>()
            .flat_map(|(_, l)| l.forms.iter().map(|f| f.form))
            .collect();
        for (e, inst) in world.iter::<Instance>() {
            if !chosen(e) {
                continue;
            }
            let transform = *world.get::<Transform>(e).ok_or_else(|| Error::World {
                entity: e,
                detail: "placement without core.transform".into(),
            })?;
            placed.insert(inst.prototype);
            places.push(Place {
                entity: e,
                prototype: inst.prototype,
                variant: inst.variant,
                transform,
            });
        }
        for (e, _) in world.iter::<Mesh>() {
            if world.has::<Instance>(e) {
                continue;
            }
            if let Some(&transform) = world.get::<Transform>(e) {
                if chosen(e) {
                    places.push(Place {
                        entity: e,
                        prototype: e,
                        variant: 0,
                        transform,
                    });
                }
            } else if !placed.contains(&e) && !forms.contains(&e) {
                self.report.prototypes_unplaced += 1;
                if self.options.include_unplaced && selection.is_none() {
                    places.push(Place {
                        entity: e,
                        prototype: e,
                        variant: 0,
                        transform: Transform::IDENTITY,
                    });
                } else {
                    self.left_out.push(e);
                }
            }
        }
        places.sort_by_key(|p| p.entity);
        if let Some(s) = selection
            && s.placements.len() != places.len()
            && let Some(&e) = s
                .placements
                .keys()
                .find(|e| places.binary_search_by_key(*e, |p| p.entity).is_err())
        {
            return Err(Error::World {
                entity: e,
                detail: "selected, but not a placement (an instance or a placed mesh)".into(),
            });
        }
        Ok(places)
    }

    fn group_key(&self, place: &Place) -> Option<GroupKey> {
        match self.options.group_by {
            GroupBy::None => None,
            GroupBy::Prototype => Some(GroupKey::Prototype(place.prototype)),
            GroupBy::Tile => Some(match self.world.get::<Tile>(place.entity) {
                Some(t) => GroupKey::Tile(t.col, t.row),
                None => GroupKey::Untiled,
            }),
        }
    }

    fn name_of(&self, e: Entity, what: &str) -> String {
        match self.world.get::<Name>(e) {
            Some(Name(n)) => n.clone(),
            None => format!("{what}_e{}", e.index()),
        }
    }

    fn placement_node(&mut self, place: &Place) -> Result<Option<Value>> {
        let world = self.world;
        let options = self.options;
        if world
            .get::<Lods>(place.prototype)
            .is_some_and(|l| !l.forms.is_empty())
        {
            // drawing any one form, or a piece cut from one, would be choosing it: select-lod's choice
            self.report.placements_with_lods += 1;
            self.left_out.push(place.entity);
            return Ok(None);
        }
        let selected = options
            .selection
            .as_ref()
            .and_then(|s| s.placements.get(&place.entity));
        let (mesh, from) = match selected.and_then(|s| s.piece.as_ref()) {
            // a piece: its own geometry and name, the prototype's components remapped to its primitives
            Some(piece) => (
                &piece.mesh,
                MeshFrom {
                    src: Src::Piece(place.entity),
                    name: piece.name.clone(),
                    extras_of: ExtrasOf::Piece {
                        prototype: place.prototype,
                        from: piece.from.clone(),
                    },
                },
            ),
            None => {
                let Some(mesh) = world.get::<Mesh>(place.prototype) else {
                    self.report.placements_without_mesh += 1;
                    self.handled.insert(place.entity);
                    return Ok(None);
                };
                (
                    mesh,
                    MeshFrom {
                        src: self.skinned_src(place),
                        name: self.name_of(place.prototype, "prototype"),
                        extras_of: ExtrasOf::Prototype(place.prototype),
                    },
                )
            }
        };
        let kind = place.transform.classify();
        let materials = match from.src {
            Src::Piece(_) => mesh.primitives().iter().map(|p| p.material).collect(),
            Src::Prototype(_) | Src::Skinned(..) => self.resolve_materials(place, mesh)?,
        };
        let mut node = Map::new();
        let name = match world.get::<Name>(place.entity) {
            Some(Name(n)) => n.clone(),
            None => from.name.clone(),
        };
        node.insert("name".into(), Value::String(name));
        let c = place.transform.cols;
        match kind {
            TransformKind::Degenerate => {
                self.report.placements_degenerate_skipped += 1;
                self.left_out.push(place.entity);
                return Ok(None);
            }
            TransformKind::Sheared => {
                return Err(Error::World {
                    entity: place.entity,
                    detail: concat!(
                        "a sheared transform: glTF node matrices must decompose into translation, rotation and ",
                        "scale; run bake-nonrigid first (mirrors=keep keeps mirrors, which glTF carries)"
                    )
                    .into(),
                });
            }
            TransformKind::Regular | TransformKind::Mirrored => {
                let mesh_index = self.shared_mesh(&from, mesh, materials, place.variant)?;
                node.insert("mesh".into(), json!(mesh_index));
                let variants = self.variant_meshes(place, &from, mesh, mesh_index)?;
                self.drawn.insert(place.entity, (mesh_index, variants));
                if kind == TransformKind::Mirrored {
                    self.report.placements_mirrored += 1;
                }
                if let Src::Skinned(prototype, binding) = from.src {
                    let skin = self.skin(place, prototype, binding)?;
                    node.insert("skin".into(), json!(skin));
                } else if c[..3] == Transform::IDENTITY.cols[..3] {
                    if c[3] != [0.0; 3] {
                        node.insert("translation".into(), f32s_value(&c[3])?);
                    }
                } else {
                    let m = [
                        c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1],
                        c[2][2], 0.0, c[3][0], c[3][1], c[3][2], 1.0,
                    ];
                    if m.iter().any(|x| !x.is_finite()) {
                        return Err(Error::World {
                            entity: place.entity,
                            detail: "non-finite transform".into(),
                        });
                    }
                    node.insert("matrix".into(), f32s_value(&m)?);
                }
            }
        }
        if let Some(weights) = self.node_weights(place, from.src)? {
            node.insert("weights".into(), weights);
        }
        let extras = match selected.filter(|s| !s.extras.is_empty()) {
            None => self.extras(place.entity, CONSUMED_PLACEMENT)?,
            Some(s) => {
                let mut shown = world.components_json(place.entity).map_err(|err| Error::World {
                    entity: place.entity,
                    detail: err.to_string(),
                })?;
                shown.extend(s.extras.iter().map(|(name, version, value)| ComponentJson {
                    name: name.as_str(),
                    version: *version,
                    value: Some(value.clone()),
                }));
                shown.sort_by(|a, b| a.name.cmp(b.name));
                self.primase_extras(shown, CONSUMED_PLACEMENT)
            }
        };
        // the placement's own, else its prototype's
        let own = self
            .world
            .get::<Shadows>(place.entity)
            .copied()
            .unwrap_or_default();
        let inherited = self
            .world
            .get::<Shadows>(place.prototype)
            .copied()
            .unwrap_or_default();
        let shadows = Shadows {
            cast: own.cast.or(inherited.cast),
            receive: own.receive.or(inherited.receive),
        };
        let cast = shadows.cast.map(|c| match c {
            ShadowCast::Off => "off",
            ShadowCast::On => "on",
            ShadowCast::Only => "only",
        });
        if let Some(extras) = with_shadows(extras, cast, shadows.receive) {
            node.insert("extras".into(), extras);
        }
        self.report.placements += 1;
        self.handled.insert(place.entity);
        Ok(Some(Value::Object(node)))
    }

    /// The material of each primitive for this placement's variant.
    fn resolve_materials(&self, place: &Place, mesh: &Mesh) -> Result<Vec<Option<Entity>>> {
        let variants = self.world.get::<MaterialVariants>(place.prototype);
        if let Some(v) = variants
            && v.0.len() != mesh.primitives().len()
        {
            return Err(Error::World {
                entity: place.prototype,
                detail: format!(
                    "core.material_variants has {} entries for {} primitives",
                    v.0.len(),
                    mesh.primitives().len()
                ),
            });
        }
        Ok(mesh
            .primitives()
            .iter()
            .enumerate()
            .map(|(i, p)| {
                variants
                    .and_then(|v| v.0[i].get(&place.variant).copied())
                    .or(p.material)
            })
            .collect())
    }

    /// With [`WriteOptions::all_variants`]: the glTF mesh of each variant `place` can be drawn with (see there), its
    /// own `mesh_index` for its own; none otherwise, and for a piece (its materials are final).
    fn variant_meshes(
        &mut self,
        place: &Place,
        from: &MeshFrom,
        mesh: &Mesh,
        mesh_index: usize,
    ) -> Result<BTreeMap<u32, usize>> {
        let mut out = BTreeMap::new();
        if !self.options.all_variants || matches!(from.src, Src::Piece(_)) {
            return Ok(out);
        }
        let world = self.world;
        let mut ids: BTreeSet<u32> = BTreeSet::from([place.variant]);
        if let Some(v) = world.get::<MaterialVariants>(place.prototype) {
            ids.extend(v.0.iter().flat_map(|m| m.keys().copied()));
        }
        if let Some(o) = world.get::<primase_core::state::StateOverride>(place.entity) {
            ids.extend(o.0.iter().filter_map(|(_, o)| match o {
                primase_core::state::Override::Variant(v) => Some(*v),
                _ => None,
            }));
        }
        for v in ids {
            let index = if v == place.variant {
                mesh_index
            } else {
                let as_variant = Place {
                    entity: place.entity,
                    prototype: place.prototype,
                    variant: v,
                    transform: place.transform,
                };
                let materials = self.resolve_materials(&as_variant, mesh)?;
                self.shared_mesh(from, mesh, materials, v)?
            };
            out.insert(v, index);
        }
        Ok(out)
    }

    /// The glTF mesh of a prototype (or a piece) with these materials, made on first use.
    fn shared_mesh(
        &mut self,
        from: &MeshFrom,
        mesh: &Mesh,
        materials: Vec<Option<Entity>>,
        variant: u32,
    ) -> Result<usize> {
        let key = (from.src, materials);
        if let Some(&i) = self.mesh_index.get(&key) {
            return Ok(i);
        }
        let accessors = self.prototype_accessors(from.src, mesh)?;
        let base: Vec<Option<Entity>> = mesh.primitives().iter().map(|p| p.material).collect();
        let mut name = from.name.clone();
        if key.1 != base {
            name = format!("{name}.v{variant}");
        }
        let index = self.push_mesh(from.src, &from.extras_of, name, &accessors, &key.1, mesh)?;
        self.mesh_index.insert(key, index);
        Ok(index)
    }

    fn push_mesh(
        &mut self,
        src: Src,
        extras_of: &ExtrasOf,
        name: String,
        accessors: &[PrimitiveAccessors],
        materials: &[Option<Entity>],
        mesh: &Mesh,
    ) -> Result<usize> {
        let morphs = self.morph_accessors(src, mesh)?;
        let mut prims = Vec::with_capacity(accessors.len());
        for (i, ((acc, material), prim)) in accessors.iter().zip(materials).zip(mesh.primitives()).enumerate()
        {
            let mut p = Map::new();
            let mut attrs: Map<String, Value> = acc
                .attributes
                .iter()
                .map(|(n, i)| (n.clone(), json!(i)))
                .collect();
            if let Some(m) = material
                && prim.streams.contains_key(&Semantic::Color(0))
                && !primase_core::uses_color0(self.world, *m)
            {
                attrs.remove("COLOR_0");
                self.report.primitives_color0_ignored += 1;
            }
            let written = match material {
                Some(m) => self.material(*m)?.map(|written| (*m, written)),
                None => None,
            };
            let material_index = match written {
                Some((m, (index, sets))) => {
                    // glTF requires every texture coordinate set a material samples and states no default
                    if let Some(set) = sets
                        .iter()
                        .find(|s| !attrs.contains_key(&format!("TEXCOORD_{s}")))
                    {
                        return Err(Error::World {
                            entity: m,
                            detail: format!(
                                "a primitive drawn with it lacks TEXCOORD_{set}, which it samples; give the primitive \
                                 the set, or run fill-missing-texcoords to give it zeros"
                            ),
                        });
                    }
                    if self
                        .layered_weights
                        .get(&m)
                        .is_some_and(|attribute| !attrs.contains_key(attribute))
                    {
                        self.report.primitives_layered_without_weights += 1;
                    }
                    Some(index)
                }
                None => {
                    self.report.primitives_without_material += 1;
                    None
                }
            };
            p.insert("attributes".into(), Value::Object(attrs));
            if let Some(i) = acc.indices {
                p.insert("indices".into(), json!(i));
            }
            if let Some(index) = material_index {
                p.insert("material".into(), json!(index));
            }
            if let Some(mode) = acc.mode {
                p.insert("mode".into(), json!(mode));
            }
            if let Some(m) = &morphs {
                p.insert("targets".into(), Value::Array(m.targets[i].clone()));
            }
            prims.push(Value::Object(p));
            self.report.primitives += 1;
            if prim.topology == Topology::Triangles {
                self.report.triangles +=
                    (prim.indices.as_ref().map_or(prim.vertex_count(), Vec::len) / 3) as u64;
            }
        }
        let mut m = Map::new();
        m.insert("name".into(), Value::String(name));
        m.insert("primitives".into(), Value::Array(prims));
        let extras = match extras_of {
            ExtrasOf::Prototype(e) => self.extras(*e, CONSUMED_PROTOTYPE)?,
            ExtrasOf::Piece { prototype, from } => self.piece_extras(*prototype, from, mesh)?,
        };
        let (ExtrasOf::Prototype(prototype) | ExtrasOf::Piece { prototype, .. }) = extras_of;
        self.handled.insert(*prototype);
        let extras = match &morphs {
            Some(mo) => {
                m.insert("weights".into(), f32s_value(&mo.defaults)?);
                let mut e = match extras {
                    Some(Value::Object(e)) => e,
                    _ => Map::new(),
                };
                e.insert("targetNames".into(), json!(mo.names));
                Some(Value::Object(e))
            }
            None => extras,
        };
        if let Some(extras) = extras {
            m.insert("extras".into(), extras);
        }
        self.meshes.push(Value::Object(m));
        self.report.meshes += 1;
        Ok(self.meshes.len() - 1)
    }

    /// Accessors of every primitive of a prototype, written once and shared by all its meshes.
    fn prototype_accessors(&mut self, src: Src, mesh: &Mesh) -> Result<Vec<PrimitiveAccessors>> {
        if let Some(a) = self.proto_accessors.get(&src) {
            return Ok(a.clone());
        }
        let (Src::Prototype(prototype) | Src::Piece(prototype) | Src::Skinned(prototype, _)) = src;
        if mesh.primitives().is_empty() {
            return Err(Error::World {
                entity: prototype,
                detail: "core.mesh without primitives".into(),
            });
        }
        let mut out = Vec::with_capacity(mesh.primitives().len());
        for (i, prim) in mesh.primitives().iter().enumerate() {
            // a piece's materials are final; a prototype's primitive is also drawn with its variants' materials
            let color0 = match src {
                Src::Prototype(_) | Src::Skinned(..) => {
                    primase_core::color0_drawn(self.world, prototype, mesh, i)
                }
                Src::Piece(_) => prim
                    .material
                    .is_none_or(|m| primase_core::uses_color0(self.world, m)),
            };
            let a = self
                .buffers
                .primitive(prim, color0)
                .map_err(|detail| Error::World {
                    entity: prototype,
                    detail: format!("primitive {i}: {detail}"),
                })?;
            let mut a = a;
            if let Src::Skinned(prototype, binding) = src {
                self.add_skin_attributes(prototype, binding, i, prim, &mut a)?;
            } else {
                for name in &a.skipped {
                    *self.report.streams_skipped.entry(name.clone()).or_default() += 1;
                }
            }
            out.push(a);
        }
        self.proto_accessors.insert(src, out.clone());
        Ok(out)
    }

    /// The glTF material of an entity, and the texture coordinate sets it samples; `None` for an entity with neither
    /// `pbr.material` nor `core.layered` (counted once).
    fn material(&mut self, e: Entity) -> Result<Option<(usize, BTreeSet<u8>)>> {
        if let Some(m) = self.material_index.get(&e) {
            return Ok(m.clone());
        }
        self.handled.insert(e);
        if let Some(layered) = self.world.get::<LayeredMaterial>(e) {
            return self.layered_material(e, &layered.clone()).map(Some);
        }
        let mut sets = BTreeSet::new();
        let Some(pbr) = self.world.get::<PbrMaterial>(e).cloned() else {
            self.report.materials_without_pbr += 1;
            self.material_index.insert(e, None);
            return Ok(None);
        };
        let mut m = Map::new();
        m.insert("name".into(), Value::String(self.name_of(e, "material")));
        let mut pmr = Map::new();
        if pbr.base_color != [1.0; 4] {
            pmr.insert("baseColorFactor".into(), self.factors(e, &pbr.base_color)?);
        }
        if let Some(t) = self.texture_info(pbr.base_color_texture, &mut sets)? {
            pmr.insert("baseColorTexture".into(), t);
        }
        if pbr.metallic != 1.0 {
            pmr.insert(
                "metallicFactor".into(),
                self.factors(e, &[pbr.metallic])?[0].clone(),
            );
        }
        if pbr.roughness != 1.0 {
            pmr.insert(
                "roughnessFactor".into(),
                self.factors(e, &[pbr.roughness])?[0].clone(),
            );
        }
        if let Some(t) = self.texture_info(pbr.metallic_roughness_texture, &mut sets)? {
            pmr.insert("metallicRoughnessTexture".into(), t);
        }
        if !pmr.is_empty() {
            m.insert("pbrMetallicRoughness".into(), Value::Object(pmr));
        }
        if let Some(Value::Object(mut t)) = self.texture_info(pbr.normal_texture, &mut sets)? {
            if pbr.normal_scale != 1.0 {
                t.insert("scale".into(), self.factors(e, &[pbr.normal_scale])?[0].clone());
            }
            m.insert("normalTexture".into(), Value::Object(t));
        }
        if let Some(Value::Object(mut t)) = self.texture_info(pbr.occlusion_texture, &mut sets)? {
            if pbr.occlusion_strength != 1.0 {
                t.insert(
                    "strength".into(),
                    self.factors(e, &[pbr.occlusion_strength])?[0].clone(),
                );
            }
            m.insert("occlusionTexture".into(), Value::Object(t));
        }
        if let Some(t) = self.texture_info(pbr.emissive_texture, &mut sets)? {
            m.insert("emissiveTexture".into(), t);
        }
        if pbr.emissive != [0.0; 3] {
            m.insert("emissiveFactor".into(), self.factors(e, &pbr.emissive)?);
        }
        let mut extensions = Map::new();
        if pbr.emissive_strength != 1.0 {
            let strength = self.factors(e, &[pbr.emissive_strength])?[0].clone();
            extensions.insert(EMISSIVE_STRENGTH.into(), json!({ "emissiveStrength": strength }));
            self.emissive_strength_used = true;
        }
        // a stated specular is written even at the extension's defaults: it is a fact, absence is not
        if let Some(sp) = pbr.specular {
            let mut k = Map::new();
            if sp.factor != 1.0 {
                k.insert("specularFactor".into(), self.factors(e, &[sp.factor])?[0].clone());
            }
            if let Some(t) = self.texture_info(sp.texture, &mut sets)? {
                k.insert("specularTexture".into(), t);
            }
            if sp.color != [1.0; 3] {
                k.insert("specularColorFactor".into(), self.factors(e, &sp.color)?);
            }
            if let Some(t) = self.texture_info(sp.color_texture, &mut sets)? {
                k.insert("specularColorTexture".into(), t);
            }
            extensions.insert(SPECULAR.into(), Value::Object(k));
            self.specular_used = true;
        }
        if let Some(detail) = self.world.get::<DetailedMaterial>(e).cloned() {
            match self.detail_blend(e, &detail, &mut sets)? {
                Ok(x) => {
                    extensions.insert(DETAIL.into(), x);
                    self.detail_used = true;
                    self.report.detail_materials += 1;
                }
                Err(why) => *self.report.detail_materials_dropped.entry(why).or_default() += 1,
            }
        }
        if pbr.shading == Shading::Unlit {
            extensions.insert(UNLIT.into(), json!({}));
            self.unlit_used = true;
        }
        if !extensions.is_empty() {
            m.insert("extensions".into(), Value::Object(extensions));
        }
        match pbr.alpha_mode {
            AlphaMode::Opaque => {}
            AlphaMode::Mask { cutoff } => {
                m.insert("alphaMode".into(), json!("MASK"));
                if cutoff != 0.5 {
                    m.insert("alphaCutoff".into(), self.factors(e, &[cutoff])?[0].clone());
                }
            }
            AlphaMode::Blend => {
                m.insert("alphaMode".into(), json!("BLEND"));
            }
        }
        if pbr.double_sided {
            m.insert("doubleSided".into(), json!(true));
        }
        if let Some(extras) = self.extras(e, CONSUMED_MATERIAL)? {
            m.insert("extras".into(), extras);
        }
        self.materials.push(Value::Object(m));
        self.report.materials += 1;
        let i = self.materials.len() - 1;
        self.material_index.insert(e, Some((i, sets.clone())));
        Ok(Some((i, sets)))
    }

    /// A light as `KHR_lights_punctual` writes it, named.
    fn light(&self, e: Entity, l: &Light) -> Result<Value> {
        let mut o = Map::new();
        o.insert("name".into(), Value::String(self.name_of(e, "light")));
        let Value::Object(fields) = punctual_light(l).map_err(|detail| Error::World { entity: e, detail })?
        else {
            unreachable!("punctual_light returns an object")
        };
        o.extend(fields);
        Ok(Value::Object(o))
    }

    fn factors(&self, e: Entity, xs: &[f32]) -> Result<Value> {
        if xs.iter().any(|x| !x.is_finite()) {
            return Err(Error::World {
                entity: e,
                detail: format!("non-finite material factor {xs:?}"),
            });
        }
        Ok(f32s_value(xs)?)
    }

    /// `{"index", "texCoord"}` (and its transform as `KHR_texture_transform`) for a texture slot, or `None` when its
    /// texture was skipped; adds the slot's texture coordinate set to `sets`.
    fn texture_info(&mut self, r: Option<TextureRef>, sets: &mut BTreeSet<u8>) -> Result<Option<Value>> {
        let Some(r) = r else { return Ok(None) };
        let Some(index) = self.texture(r)? else {
            self.report.texture_slots_dropped += 1;
            return Ok(None);
        };
        sets.insert(r.texcoord);
        let mut t = Map::new();
        t.insert("index".into(), json!(index));
        if r.texcoord != 0 {
            t.insert("texCoord".into(), json!(r.texcoord));
        }
        if let Some(x) = r.transform.filter(|x| *x != UvTransform::IDENTITY) {
            let k = self.uv_transform(r.texture, x)?;
            t.insert(
                "extensions".into(),
                json!({ TEXTURE_TRANSFORM: Value::Object(k) }),
            );
            self.texture_transform_used = true;
        }
        Ok(Some(Value::Object(t)))
    }

    /// A UV transform as `KHR_texture_transform` fields (defaults left out).
    fn uv_transform(&self, e: Entity, x: UvTransform) -> Result<Map<String, Value>> {
        let mut k = Map::new();
        if x.offset != [0.0; 2] {
            k.insert("offset".into(), self.factors(e, &x.offset)?);
        }
        if x.rotation != 0.0 {
            k.insert("rotation".into(), self.factors(e, &[x.rotation])?[0].clone());
        }
        if x.scale != [1.0; 2] {
            k.insert("scale".into(), self.factors(e, &x.scale)?);
        }
        Ok(k)
    }

    /// A `core.layered` material: its layers' materials first, then the host (a copy of the first non-null layer's
    /// material) carrying `PRIMASE_layered_material`. The sets are every set a layer's textures or the layer's own
    /// `texcoord` selects.
    fn layered_material(&mut self, e: Entity, layered: &LayeredMaterial) -> Result<(usize, BTreeSet<u8>)> {
        let attribute = match layered.weights.attribute {
            Semantic::Color(n) => format!("COLOR_{n}"),
            other => {
                return Err(Error::World {
                    entity: e,
                    detail: format!("layer weights must be a COLOR_n stream, not {other:?}"),
                });
            }
        };
        let mut sets = BTreeSet::new();
        let mut layers = Vec::with_capacity(layered.layers.len());
        let mut first = None;
        for layer in &layered.layers {
            let Some(layer) = layer else {
                layers.push(Value::Null);
                continue;
            };
            let Some((index, layer_sets)) = self.material(layer.material)? else {
                // a layer with no material the writer can express: an empty slot, as the format has
                self.report.layers_without_pbr += 1;
                layers.push(Value::Null);
                continue;
            };
            first.get_or_insert(index);
            sets.extend(layer_sets);
            sets.insert(layer.texcoord.unwrap_or(0));
            let mut o = Map::new();
            o.insert("material".into(), json!(index));
            if let Some(n) = layer.texcoord {
                o.insert("texCoord".into(), json!(n));
            }
            if let Some(x) = layer.transform.filter(|x| *x != UvTransform::IDENTITY) {
                o.insert(
                    "transform".into(),
                    Value::Object(self.uv_transform(layer.material, x)?),
                );
            }
            layers.push(Value::Object(o));
        }
        let mapping = match layered.weights.mapping {
            WeightMapping::Identity => "identity",
            WeightMapping::RgbProduct4 => "rgb_product_4",
        };
        let mut weights = Map::new();
        weights.insert("attribute".into(), json!(attribute));
        weights.insert("mapping".into(), json!(mapping));
        if let Some(channels) = &layered.weights.channels {
            let mut out = Vec::with_capacity(channels.len());
            for c in channels {
                out.push(self.layer_channel(e, c, &mut sets)?);
            }
            weights.insert("channels".into(), Value::Array(out));
        }
        if !layered.weights.heights.is_empty() {
            let mut out = Vec::with_capacity(layered.weights.heights.len());
            for h in &layered.weights.heights {
                out.push(self.layer_source(e, h, &mut sets)?);
            }
            weights.insert("heights".into(), Value::Array(out));
        }
        let mut host = match first {
            Some(i) => match &self.materials[i] {
                Value::Object(m) => m.clone(),
                _ => unreachable!("materials are objects"),
            },
            // every slot null: the blend is black everywhere
            None => {
                let mut m = Map::new();
                m.insert(
                    "pbrMetallicRoughness".into(),
                    json!({"baseColorFactor": [0.0, 0.0, 0.0, 1.0], "metallicFactor": 0.0}),
                );
                m
            }
        };
        host.insert("name".into(), Value::String(self.name_of(e, "material")));
        host.remove("extras");
        if let Some(extras) = self.extras(e, CONSUMED_MATERIAL)? {
            host.insert("extras".into(), extras);
        }
        let mut ext = Map::new();
        // version 2: per-pixel channels or the over composition; 3: the height composition; 4: a power channel. An
        // older consumer must not draw it as the version it knows
        let power = layered
            .weights
            .channels
            .iter()
            .flatten()
            .any(|c| matches!(c, Channel::Power { .. }));
        match layered.weights.composition {
            _ if power => {
                ext.insert("version".into(), json!(4));
            }
            Composition::Height { .. } => {
                ext.insert("version".into(), json!(3));
            }
            _ if layered.weights.per_pixel() => {
                ext.insert("version".into(), json!(2));
            }
            _ => {}
        }
        ext.insert("weights".into(), Value::Object(weights));
        match layered.weights.composition {
            Composition::Over => {
                ext.insert("composition".into(), json!("over"));
            }
            Composition::Height { contrast } => {
                let c = self.factors(e, &[contrast])?[0].clone();
                ext.insert("composition".into(), json!({"height": {"contrast": c}}));
            }
            Composition::Sum => {}
        }
        ext.insert("layers".into(), Value::Array(layers));
        host.insert("extensions".into(), json!({ LAYERED: Value::Object(ext) }));
        self.materials.push(Value::Object(host));
        self.report.materials += 1;
        self.report.layered_materials += 1;
        let i = self.materials.len() - 1;
        self.material_index.insert(e, Some((i, sets.clone())));
        self.layered_weights.insert(e, attribute);
        Ok((i, sets))
    }

    /// A texture slot of `PRIMASE_materials_detail_blend`: its texture info and `colorSpace` (the texture's own, else
    /// sRGB for a colour slot), or `None` where its texture could not be written; adds its set to `sets`.
    fn detail_slot(&mut self, r: TextureRef, colour: bool, sets: &mut BTreeSet<u8>) -> Result<Option<Value>> {
        let Some(Value::Object(mut info)) = self.texture_info(Some(r), sets)? else {
            return Ok(None);
        };
        let srgb = self
            .world
            .get::<Texture>(r.texture)
            .map_or(colour, |t| t.color_space == ColorSpace::Srgb);
        info.insert("colorSpace".into(), json!(if srgb { "srgb" } else { "linear" }));
        Ok(Some(Value::Object(info)))
    }

    /// A `pbr.detail` as `PRIMASE_materials_detail_blend` ([`crate::detail`]); `Err` with why where a texture of it
    /// could not be written (the material is then written without it, its `pbr.material` alone).
    fn detail_blend(
        &mut self,
        e: Entity,
        d: &DetailedMaterial,
        sets: &mut BTreeSet<u8>,
    ) -> Result<std::result::Result<Value, String>> {
        let DetailMix::DetailLerp(m) = &d.mix;
        let mut written = BTreeSet::new();
        // the slots in the order they are written, each a texture and whether it is a colour
        let mut textures = vec![
            (m.base_colour, true),
            (m.base_normal, false),
            (m.base_mask, false),
        ];
        for detail in &m.details {
            textures.extend([(detail.colour, true), (detail.normal, false)]);
        }
        for w in &m.weights {
            if let Source::Mask { texture, .. } = w.source {
                textures.push((texture, false));
            }
        }
        let mut infos = Vec::with_capacity(textures.len());
        for (r, colour) in textures {
            match self.detail_slot(r, colour, &mut written)? {
                Some(info) => infos.push(info),
                None => return Ok(Err(format!("texture {:?} could not be written", r.texture))),
            }
        }
        let mut infos = infos.into_iter();
        let mut next = || infos.next().expect("one info a slot");
        let mut x = Map::new();
        x.insert("equation".into(), json!("detail_lerp"));
        x.insert(
            "lowering".into(),
            json!(match d.lowering {
                SurfaceLowering::GlossReflectance => "gloss_reflectance",
            }),
        );
        x.insert("baseColorTexture".into(), next());
        x.insert("baseNormalTexture".into(), next());
        x.insert("baseMaskTexture".into(), next());
        let mut details = Vec::new();
        for detail in &m.details {
            let (colour, normal) = (next(), next());
            let gloss_scale = match detail.gloss_scale {
                GlossScale::BaseNormalZ => json!({"baseNormalZ": {}}),
                GlossScale::Constant(c) => json!({"constant": self.factors(e, &[c])?[0].clone()}),
            };
            let reflectance = match detail.reflectance {
                ReflectanceTerm::MaskPlusBias(b) => {
                    json!({"maskPlusBias": self.factors(e, &[b])?[0].clone()})
                }
                ReflectanceTerm::Bias(b) => json!({"bias": self.factors(e, &[b])?[0].clone()}),
            };
            let mut o = Map::new();
            o.insert("colorTexture".into(), colour);
            o.insert("normalTexture".into(), normal);
            o.insert("tint".into(), self.factors(e, &detail.tint)?);
            o.insert(
                "glossBias".into(),
                self.factors(e, &[detail.gloss_bias])?[0].clone(),
            );
            o.insert("glossScale".into(), gloss_scale);
            o.insert("reflectance".into(), reflectance);
            details.push(Value::Object(o));
        }
        x.insert("details".into(), Value::Array(details));
        let mut weights = Vec::new();
        for w in &m.weights {
            let source = match &w.source {
                Source::Mask { channels, bias, .. } => json!({"mask": {
                    "texture": next(),
                    "channels": self.factors(e, channels)?,
                    "bias": self.factors(e, &[*bias])?[0].clone(),
                }}),
                vertex @ Source::Vertex { .. } => self.layer_source(e, vertex, &mut written)?,
            };
            let mut steps = Vec::new();
            for step in &w.steps {
                steps.push(match *step {
                    WeightStep::Power(n) => json!({"power": n}),
                    WeightStep::Min(b) => json!({"min": self.factors(e, &[b])?[0].clone()}),
                });
            }
            weights.push(json!({"source": source, "steps": steps}));
        }
        x.insert("weights".into(), Value::Array(weights));
        x.insert("gain".into(), self.factors(e, &[m.gain])?[0].clone());
        x.insert(
            "normalStrength".into(),
            self.factors(e, &[m.normal_strength])?[0].clone(),
        );
        x.insert(
            "dielectricF0".into(),
            self.factors(e, &[m.dielectric_f0])?[0].clone(),
        );
        x.insert("color".into(), self.factors(e, &m.colour)?);
        let encoding = m.gloss_encoding;
        x.insert(
            "glossEncoding".into(),
            json!({
                "power": encoding.power,
                "root": encoding.root,
                "scale": self.factors(e, &[f32::from_bits(encoding.scale_bits)])?[0].clone(),
            }),
        );
        if let Some(j) = m.alpha {
            x.insert("alphaDetail".into(), json!(j));
        }
        let mut frozen = Vec::new();
        for f in &d.frozen {
            frozen.push(match f {
                Frozen::FrontFacing => json!({"frontFacing": {}}),
                Frozen::VertexInput {
                    semantic,
                    index,
                    value,
                } => json!({"vertexInput": {
                    "semantic": semantic, "index": index, "value": self.factors(e, value)?,
                }}),
                Frozen::EngineConstant { name, value } => json!({"engineConstant": {
                    "name": name, "value": self.factors(e, value)?,
                }}),
                Frozen::EngineTexture { name, value } => json!({"engineTexture": {
                    "name": name, "value": self.factors(e, value)?,
                }}),
            });
        }
        x.insert("frozen".into(), Value::Array(frozen));
        sets.extend(written);
        Ok(Ok(Value::Object(x)))
    }

    /// A weight or height source as `PRIMASE_layered_material` writes it.
    fn layer_source(&mut self, e: Entity, s: &Source, sets: &mut BTreeSet<u8>) -> Result<Value> {
        Ok(match s {
            Source::Vertex { semantic, channel } => {
                let attribute = match semantic {
                    Semantic::Color(n) => format!("COLOR_{n}"),
                    Semantic::TexCoord(n) => format!("TEXCOORD_{n}"),
                    other => {
                        return Err(Error::World {
                            entity: e,
                            detail: format!("a weight channel reads {other:?}: COLOR_n or TEXCOORD_n only"),
                        });
                    }
                };
                json!({"vertex": {"attribute": attribute, "channel": channel}})
            }
            Source::Mask {
                texture,
                channels,
                bias,
            } => {
                let info = self
                    .texture_info(Some(*texture), sets)?
                    .ok_or_else(|| Error::World {
                        entity: e,
                        detail: format!(
                            "mask texture {:?} could not be written (not loaded, or a format an 8-bit PNG cannot hold \
                             exactly: lower-textures-rgba8)",
                            texture.texture
                        ),
                    })?;
                json!({"mask": {
                    "texture": info,
                    "channels": self.factors(e, channels)?,
                    "bias": self.factors(e, &[*bias])?[0].clone(),
                }})
            }
        })
    }

    /// A per-pixel weight channel as `PRIMASE_layered_material` writes it.
    fn layer_channel(&mut self, e: Entity, c: &Channel, sets: &mut BTreeSet<u8>) -> Result<Value> {
        let mut source = |s: &Source| self.layer_source(e, s, sets);
        Ok(match c {
            Channel::Source(s) => json!({"source": source(s)?}),
            Channel::Lerp { a, b, t } => json!({"lerp": {"a": source(a)?, "b": source(b)?, "t": source(t)?}}),
            Channel::Threshold { v, m, k } => {
                let (v, m) = (source(v)?, source(m)?);
                json!({"threshold": {"v": v, "m": m, "k": self.factors(e, k)?}})
            }
            Channel::Power { v, exponent } => json!({"power": {"v": source(v)?, "exponent": exponent}}),
        })
    }

    fn texture(&mut self, r: TextureRef) -> Result<Option<usize>> {
        let key = (r.texture, r.wrap, r.filters);
        if let Some(&i) = self.texture_index.get(&key) {
            return Ok(Some(i));
        }
        let Some(image) = self.image(r.texture)? else {
            return Ok(None);
        };
        let data = self
            .world
            .get::<Texture>(r.texture)
            .is_some_and(|t| t.role == primase_core::TexRole::Data);
        let mut t = Map::new();
        // data is read exactly: with no filter stated, nearest without mips
        let filters = if data && r.filters == Filters::default() {
            Filters {
                mag: Some(Filter::Nearest),
                min: Some(Filter::Nearest),
                mip: Some(MipFilter::None),
            }
        } else {
            r.filters
        };
        if r.wrap != [Wrap::Repeat; 2] || filters != Filters::default() {
            let next = self.samplers.len();
            let sampler = *self.sampler_index.entry((r.wrap, filters, data)).or_insert(next);
            if sampler == next {
                let mut s = Map::new();
                if let Some(mag) = filters.mag {
                    s.insert("magFilter".into(), json!(filter_code(mag)));
                }
                match (filters.min, filters.mip) {
                    (Some(min), Some(mip)) => {
                        s.insert("minFilter".into(), json!(min_filter_code(min, mip)));
                    }
                    (None, None) => {}
                    _ => self.report.filters_unexpressed += 1,
                }
                s.insert("wrapS".into(), json!(wrap_code(r.wrap[0])));
                s.insert("wrapT".into(), json!(wrap_code(r.wrap[1])));
                self.samplers.push(Value::Object(s));
            }
            t.insert("sampler".into(), json!(sampler));
        }
        if data {
            // and must reach a consumer whole: no resizing or lossy coding of its image
            self.images[image]["extras"]["primase.role"] = json!("data");
        }
        t.insert("source".into(), json!(image));
        t.insert("name".into(), Value::String(self.name_of(r.texture, "texture")));
        if let Some(extras) = self.extras(r.texture, CONSUMED_TEXTURE)? {
            t.insert("extras".into(), extras);
        }
        self.textures.push(Value::Object(t));
        self.report.textures += 1;
        let i = self.textures.len() - 1;
        self.texture_index.insert(key, i);
        Ok(Some(i))
    }

    /// The image of a texture entity: its loaded pixels as canonical PNG.
    fn image(&mut self, e: Entity) -> Result<Option<usize>> {
        if let Some(&i) = self.image_of_entity.get(&e) {
            return Ok(i);
        }
        self.handled.insert(e);
        let skip = |w: &mut Self, reason: &str| {
            *w.report.textures_skipped.entry(reason.to_owned()).or_default() += 1;
            w.image_of_entity.insert(e, None);
            Ok(None)
        };
        let (bytes, image_name) = if let Some(texture) = self.world.get::<Texture>(e) {
            if texture.role == primase_core::TexRole::Normal
                && texture.normal != Some(primase_core::NormalConvention::GLTF)
            {
                return Err(Error::World {
                    entity: e,
                    detail:
                        "normal texture requires tangent/bitangent basis, X in red, green up and stored Z; run normalize-normals first"
                            .into(),
                });
            }
            if !primase_image::rgba8_exact(texture.format) {
                // its values have no exact 8-bit form: lower-textures-rgba8 converts it, as a program chooses
                *self
                    .report
                    .textures_unrepresentable
                    .entry(format!("{:?}", texture.format))
                    .or_default() += 1;
                self.image_of_entity.insert(e, None);
                return Ok(None);
            }
            let bytes = match self.encoded.remove(&e) {
                Some(encoded) => encoded.map_err(|detail| Error::World { entity: e, detail })?,
                None => match &self.options.images {
                    Some(memo) => memo.png(texture),
                    None => primase_image::texture_level_png(texture, 0),
                }
                .map_err(|error| Error::World {
                    entity: e,
                    detail: error.to_string(),
                })?,
            };
            (bytes, self.name_of(e, "image"))
        } else if self.world.has::<TextureFile>(e) {
            // a reader's load step makes the pixels (primase_image::load_textures); the writer reads no file
            return skip(self, "texture files not loaded");
        } else {
            return Err(Error::World {
                entity: e,
                detail: "texture entity with neither core.texture_file nor core.texture".into(),
            });
        };
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let data = self
            .world
            .get::<Texture>(e)
            .is_some_and(|t| t.role == primase_core::TexRole::Data);
        let index = match self.image_by_hash.get(&(hash, data)) {
            Some(&i) => i,
            None => {
                let view = self.buffers.view(&bytes, None);
                self.images
                    .push(json!({"name": image_name, "mimeType": "image/png", "bufferView": view}));
                self.report.images += 1;
                self.report.image_bytes += bytes.len() as u64;
                let i = self.images.len() - 1;
                self.image_by_hash.insert((hash, data), i);
                i
            }
        };
        self.image_of_entity.insert(e, Some(index));
        Ok(Some(index))
    }

    /// The extras of an entity: its components with a JSON form, minus what the glTF object already expresses, as
    /// `{"primase": {"<component name>": {"version": n, "value": ...}}}`. Components without one are counted in
    /// the report; a view that would lose a value (a non-finite float) is an error.
    fn extras(&mut self, e: Entity, consumed: &[&str]) -> Result<Option<Value>> {
        let shown = self.world.components_json(e).map_err(|err| Error::World {
            entity: e,
            detail: err.to_string(),
        })?;
        Ok(self.primase_extras(shown, consumed))
    }

    /// The extras of a piece cut from `prototype`: the prototype's components on a copy of it rebuilt to the piece
    /// ([`World::rebuild_primitives`]: per-primitive components remapped by `from`; the world itself is unchanged).
    fn piece_extras(&mut self, prototype: Entity, from: &[usize], piece: &Mesh) -> Result<Option<Value>> {
        let failed = |detail: String| Error::World {
            entity: prototype,
            detail,
        };
        // the one way to change a primitive list: the copy's mesh becomes the piece and every per-primitive
        // component follows `from`, checked (count and index range) by the world
        let mut copy = self.world.subset(&BTreeSet::from([prototype]));
        copy.rebuild_primitives(prototype, |_| Ok((piece.primitives().to_vec(), from.to_vec())))
            .map_err(failed)?;
        let shown = copy
            .components_json(prototype)
            .map_err(|err| failed(err.to_string()))?;
        Ok(self.primase_extras(shown, CONSUMED_PROTOTYPE))
    }

    /// Without a selection, count the components of every entity neither handled nor reached from one left out under a
    /// counted cause: what the glTF does not hold and no other field of the report names.
    /// The source a placement draws: its prototype skinned by its binding when it follows a written rig and its
    /// prototype has that binding; else the prototype as it is (a skinned one counted, drawn in its bind pose).
    fn skinned_src(&mut self, place: &Place) -> Src {
        let world = self.world;
        let skin = world.get::<Skin>(place.prototype);
        if let (Some(si), Some(skin)) = (world.get::<SkinInstance>(place.entity), skin)
            && self.rig_roots.contains_key(&si.rig)
            && skin.binding(si.binding).is_some()
        {
            return Src::Skinned(place.prototype, si.binding);
        }
        if skin.is_some() {
            self.report.skinned_without_rig += 1;
        }
        Src::Prototype(place.prototype)
    }

    /// The skin layout of a prototype's binding, made once.
    fn layout(&mut self, prototype: Entity, binding: BindingId) -> Result<crate::rig::SkinLayout> {
        if let Some(l) = self.layouts.get(&(prototype, binding)) {
            return Ok(l.clone());
        }
        let world = self.world;
        let failed = |detail: String| Error::World {
            entity: prototype,
            detail,
        };
        let skin = world
            .get::<Skin>(prototype)
            .ok_or_else(|| failed("no core.skin".into()))?;
        let b = skin
            .binding(binding)
            .ok_or_else(|| failed(format!("no binding {binding:?}")))?;
        let layout = crate::rig::skin_layout(skin, b).map_err(failed)?;
        self.layouts.insert((prototype, binding), layout.clone());
        Ok(layout)
    }

    /// A skinned primitive's `JOINTS_n`/`WEIGHTS_n` accessors, added to its attributes.
    fn add_skin_attributes(
        &mut self,
        prototype: Entity,
        binding: BindingId,
        i: usize,
        prim: &primase_core::Primitive,
        a: &mut PrimitiveAccessors,
    ) -> Result<()> {
        use crate::rig::SkinValues;
        let layout = self.layout(prototype, binding)?;
        let skin = self.world.get::<Skin>(prototype).expect("its layout was made");
        let failed = |detail: String| Error::World {
            entity: prototype,
            detail: format!("primitive {i}: {detail}"),
        };
        let (Some(how), Some(Some(map))) = (skin.influences.get(i).copied().flatten(), layout.slots.get(i))
        else {
            return Err(failed(
                "not skinned, in a skinned mesh: glTF draws every primitive of a skinned node by its skin"
                    .into(),
            ));
        };
        let narrow = layout.joints.len() <= 256;
        let n = prim.vertex_count();
        for (name, values) in crate::rig::skin_attributes(prim, how, map, narrow).map_err(failed)? {
            let index = match &values {
                SkinValues::JointsU8(v) => self.buffers.int_accessor(bytemuck::cast_slice(v), n, 5121, false),
                SkinValues::JointsU16(v) => {
                    self.buffers.int_accessor(bytemuck::cast_slice(v), n, 5123, false)
                }
                SkinValues::WeightsU8(v) => self.buffers.int_accessor(bytemuck::cast_slice(v), n, 5121, true),
                SkinValues::WeightsU16(v) => {
                    self.buffers.int_accessor(bytemuck::cast_slice(v), n, 5123, true)
                }
                SkinValues::WeightsF32(v) => {
                    self.buffers
                        .float_accessor(v.as_flattened(), ElementType::Vec4, None, true)?
                }
            };
            a.attributes.push((name, index));
        }
        a.skipped.clear();
        Ok(())
    }

    /// The glTF skin a skinned placement draws with, one per (rig, prototype, binding): the rig's joint nodes in the
    /// layout's order, their inverse binds, the rig's root node as its skeleton.
    fn skin(&mut self, place: &Place, prototype: Entity, binding: BindingId) -> Result<usize> {
        let rig = self
            .world
            .get::<SkinInstance>(place.entity)
            .expect("a skinned source has a skin instance")
            .rig;
        if let Some(&i) = self.skin_index.get(&(rig, prototype, binding)) {
            return Ok(i);
        }
        let layout = self.layout(prototype, binding)?;
        let mut joints = Vec::with_capacity(layout.joints.len());
        let mut ibm = Vec::with_capacity(layout.joints.len() * 16);
        for (j, b) in &layout.joints {
            let node = self.joint_nodes.get(&(rig, *j)).ok_or_else(|| Error::World {
                entity: place.entity,
                detail: format!("its skin binds joint {j:?}, which its rig's skeleton lacks"),
            })?;
            joints.push(*node);
            let c = b.cols;
            ibm.extend([
                c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1], c[2][2],
                0.0, c[3][0], c[3][1], c[3][2], 1.0,
            ]);
        }
        let accessor = self
            .buffers
            .float_accessor(&ibm, ElementType::Mat4, None, false)?;
        self.skins.push(json!({
            "name": format!("{}.skin{}", self.name_of(prototype, "prototype"), binding.0),
            "joints": joints,
            "inverseBindMatrices": accessor,
            "skeleton": self.rig_roots[&rig],
        }));
        self.report.skins += 1;
        let index = self.skins.len() - 1;
        self.skin_index.insert((rig, prototype, binding), index);
        Ok(index)
    }

    /// A prototype's morph targets as accessors, made once: `None` when it has none, when its targets have in-between
    /// frames (counted), or for a piece cut from it.
    fn morph_accessors(&mut self, src: Src, mesh: &Mesh) -> Result<Option<MorphAccessors>> {
        let (Src::Prototype(prototype) | Src::Skinned(prototype, _)) = src else {
            return Ok(None);
        };
        if let Some(m) = self.morph_targets.get(&prototype) {
            return Ok(m.clone());
        }
        let world = self.world;
        let made = match world.get::<Morphs>(prototype) {
            None => None,
            Some(m) if m.targets.is_empty() => None,
            Some(m) if !crate::rig::morphs_writable(m) => {
                self.report.morphs_with_frames += 1;
                None
            }
            Some(m) => {
                let mut targets = vec![Vec::new(); mesh.primitives().len()];
                for t in &m.targets {
                    let frame = &t.frames[0];
                    for (i, prim) in mesh.primitives().iter().enumerate() {
                        let n = prim.vertex_count();
                        let d = frame.primitives.get(i).cloned().flatten().unwrap_or_default();
                        // dense deltas, zero where a sparse list or a missing attribute has none
                        let dense = |values: &Option<Vec<[f32; 3]>>| -> Option<Vec<[f32; 3]>> {
                            let values = values.as_ref()?;
                            let mut out = vec![[0.0f32; 3]; n];
                            match &d.indices {
                                None => out.copy_from_slice(&values[..n.min(values.len())]),
                                Some(idx) => idx.iter().zip(values).for_each(|(&k, v)| out[k as usize] = *v),
                            }
                            Some(out)
                        };
                        let mut target = Map::new();
                        let positions = dense(&d.positions).unwrap_or_else(|| vec![[0.0; 3]; n]);
                        let (lo, hi) = primase_glb::bounds(&positions).unwrap_or(([0.0; 3], [0.0; 3]));
                        let index = self.buffers.float_accessor(
                            positions.as_flattened(),
                            ElementType::Vec3,
                            Some((lo.to_vec(), hi.to_vec())),
                            true,
                        )?;
                        target.insert("POSITION".into(), json!(index));
                        if prim.streams.contains_key(&Semantic::Normal)
                            && let Some(nd) = dense(&d.normals)
                        {
                            let index = self.buffers.float_accessor(
                                nd.as_flattened(),
                                ElementType::Vec3,
                                None,
                                true,
                            )?;
                            target.insert("NORMAL".into(), json!(index));
                        }
                        if prim.streams.contains_key(&Semantic::Tangent(0))
                            && let Some(td) = dense(&d.tangents)
                        {
                            let index = self.buffers.float_accessor(
                                td.as_flattened(),
                                ElementType::Vec3,
                                None,
                                true,
                            )?;
                            target.insert("TANGENT".into(), json!(index));
                        }
                        targets[i].push(Value::Object(target));
                    }
                }
                self.report.morph_targets += m.targets.len() as u64;
                Some(MorphAccessors {
                    targets,
                    defaults: m.targets.iter().map(|t| t.default_weight).collect(),
                    names: m.targets.iter().map(|t| t.name.clone()).collect(),
                    ids: m.targets.iter().map(|t| t.id).collect(),
                })
            }
        };
        self.morph_targets.insert(prototype, made.clone());
        Ok(made)
    }

    /// A placement's morph weights as its node's `weights` (every target, a default where it sets none), when its
    /// prototype's targets are written.
    fn node_weights(&mut self, place: &Place, src: Src) -> Result<Option<Value>> {
        let Some(weights) = self.world.get::<MorphWeights>(place.entity) else {
            return Ok(None);
        };
        let (Src::Prototype(prototype) | Src::Skinned(prototype, _)) = src else {
            return Ok(None);
        };
        let Some(Some(m)) = self.morph_targets.get(&prototype) else {
            return Ok(None);
        };
        let values: Vec<f32> = m
            .ids
            .iter()
            .zip(&m.defaults)
            .map(|(id, d)| weights.0.iter().find(|(t, _)| t == id).map_or(*d, |(_, v)| *v))
            .collect();
        Ok(Some(f32s_value(&values)?))
    }

    /// Every rig as a node tree: a root at its placement, a node per joint under its parent's, with the joint's local
    /// transform (its rig's pose, else its rest).
    fn rigs(&mut self, nodes: &mut Vec<Value>, roots: &mut Vec<usize>) -> Result<()> {
        let world = self.world;
        for (rig, r) in world.iter::<RigInstance>() {
            let failed = |detail: String| Error::World { entity: rig, detail };
            let skeleton = world
                .get::<Skeleton>(r.skeleton)
                .ok_or_else(|| failed("its skeleton holds no core.skeleton".into()))?;
            let pose: BTreeMap<JointId, Rest> = {
                let mut p: BTreeMap<JointId, Rest> = skeleton.joints.iter().map(|j| (j.id, j.rest)).collect();
                if let Some(rp) = world.get::<RigPose>(rig) {
                    p.extend(rp.joints.iter().copied());
                }
                p
            };
            let root = nodes.len();
            let placed = world
                .get::<Transform>(rig)
                .copied()
                .unwrap_or(Transform::IDENTITY);
            let c = placed.cols;
            nodes.push(json!({
                "name": self.name_of(rig, "rig"),
                "matrix": f32s_value(&[
                    c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1], c[2][2], 0.0,
                    c[3][0], c[3][1], c[3][2], 1.0,
                ])?,
            }));
            roots.push(root);
            self.rig_roots.insert(rig, root);
            for j in &skeleton.joints {
                let mut node = Map::new();
                node.insert("name".into(), json!(j.name));
                match primase_animation::pose::Trs::of(&pose[&j.id]) {
                    Ok(t) => {
                        let f = |v: &[f64]| v.iter().map(|&x| x as f32).collect::<Vec<f32>>();
                        if t.translation != [0.0; 3] {
                            node.insert("translation".into(), f32s_value(&f(&t.translation))?);
                        }
                        if t.rotation != [0.0, 0.0, 0.0, 1.0] {
                            node.insert("rotation".into(), f32s_value(&f(&t.rotation))?);
                        }
                        if t.scale != [1.0; 3] {
                            node.insert("scale".into(), f32s_value(&f(&t.scale))?);
                        }
                    }
                    Err(_) => {
                        let c = pose[&j.id].matrix().cols;
                        node.insert(
                            "matrix".into(),
                            f32s_value(&[
                                c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0],
                                c[2][1], c[2][2], 0.0, c[3][0], c[3][1], c[3][2], 1.0,
                            ])?,
                        );
                        self.matrix_joints.insert(nodes.len());
                    }
                }
                self.joint_nodes.insert((rig, j.id), nodes.len());
                nodes.push(Value::Object(node));
                self.report.joint_nodes += 1;
            }
            // children: each joint under its parent, roots under the rig's node
            for j in &skeleton.joints {
                let child = self.joint_nodes[&(rig, j.id)];
                let parent = match j.parent {
                    Some(p) => self.joint_nodes[&(rig, p)],
                    None => root,
                };
                let children = nodes[parent]
                    .as_object_mut()
                    .expect("nodes are objects")
                    .entry("children")
                    .or_insert_with(|| json!([]));
                children
                    .as_array_mut()
                    .expect("children is an array")
                    .push(json!(child));
            }
            self.handled.extend([rig, r.skeleton]);
            self.report.rigs += 1;
        }
        Ok(())
    }

    /// An input accessor (key times, with their range) and an output accessor (the values, `kind` a key).
    fn sampler(
        &mut self,
        times: &[f32],
        values: &[f32],
        kind: ElementType,
        interpolation: Interpolation,
    ) -> Result<Value> {
        let lo = times.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = times.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let input =
            self.buffers
                .float_accessor(times, ElementType::Scalar, Some((vec![lo], vec![hi])), false)?;
        let output = self.buffers.float_accessor(values, kind, None, false)?;
        let interpolation = match interpolation {
            Interpolation::Step => "STEP",
            Interpolation::Linear => "LINEAR",
            Interpolation::Hermite => "CUBICSPLINE",
        };
        Ok(json!({"input": input, "output": output, "interpolation": interpolation}))
    }

    /// Every clip binding as an animation (see the module docs).
    fn animations(&mut self, placement_nodes: &BTreeMap<Entity, usize>) -> Result<()> {
        let world = self.world;
        for (be, b) in world.iter::<ClipBinding>() {
            let failed = |detail: String| Error::World { entity: be, detail };
            let clip: &Clip = world
                .get::<Clip>(b.clip)
                .ok_or_else(|| failed("its clip is gone".into()))?;
            self.handled.extend([be, b.clip]);
            if clip.additive.is_some() {
                self.report.clips_additive += 1;
                continue;
            }
            if clip.root_motion.is_some() {
                self.report.root_motions += 1;
            }
            let mut channels = Vec::new();
            let mut samplers = Vec::new();
            for c in &clip.channels {
                match &c.target {
                    ClipTarget::Joint { joint, .. } => {
                        let rig = b
                            .rig
                            .ok_or_else(|| failed("joint channels without a rig".into()))?;
                        let Some(&node) = self.joint_nodes.get(&(rig, *joint)) else {
                            return Err(failed(format!("joint {joint:?} has no node in its rig")));
                        };
                        if self.matrix_joints.contains(&node) {
                            return Err(failed(format!(
                                "joint {joint:?} is animated but its pose has no translation-rotation-scale form"
                            )));
                        }
                        let (path, kind) = match c.property {
                            Property::Translation => ("translation", ElementType::Vec3),
                            Property::Rotation => ("rotation", ElementType::Vec4),
                            Property::Scale => ("scale", ElementType::Vec3),
                            Property::Scalar | Property::Visibility | Property::Values(_) =>
                                return Err(failed(format!("a {:?} channel on a joint", c.property))),
                        };
                        samplers.push(self.sampler(&c.times, &c.values, kind, c.interpolation)?);
                        channels.push(
                            json!({"sampler": samplers.len() - 1, "target": {"node": node, "path": path}}),
                        );
                    }
                    ClipTarget::Control { .. } => self.report.control_channels += 1,
                    ClipTarget::DrawGroup { .. } => self.report.draw_group_channels += 1,
                    ClipTarget::MaterialParameter { .. } => self.report.material_parameter_channels += 1,
                    ClipTarget::Morph { .. } => {}
                }
            }
            // morph channels: one weights channel per mesh occurrence, its targets' channels aligned
            for &m in &b.meshes {
                let drawn = world.get::<Instance>(m).map_or(m, |i| i.prototype);
                let Some(&node) = placement_nodes.get(&m) else {
                    continue;
                };
                let Some(Some(acc)) = self.morph_targets.get(&drawn).cloned() else {
                    continue;
                };
                let mine: Vec<_> = clip
                    .channels
                    .iter()
                    .filter(|c| matches!(c.target, ClipTarget::Morph { geometry, .. } if geometry == drawn))
                    .collect();
                let Some(first) = mine.first() else { continue };
                if mine
                    .iter()
                    .any(|c| c.times != first.times || c.interpolation != first.interpolation)
                {
                    self.report.morph_channels_unaligned += mine.len() as u64;
                    continue;
                }
                let base = world.get::<MorphWeights>(m);
                let per_key = if first.interpolation == Interpolation::Hermite {
                    3
                } else {
                    1
                };
                let mut values = Vec::with_capacity(first.times.len() * per_key * acc.ids.len());
                for k in 0..first.times.len() {
                    for part in 0..per_key {
                        for (id, default) in acc.ids.iter().zip(&acc.defaults) {
                            let driven = mine.iter().find(
                                |c| matches!(c.target, ClipTarget::Morph { morph, .. } if morph == *id),
                            );
                            values.push(match driven {
                                Some(c) => c.values[k * per_key + part],
                                // an undriven target holds its occurrence's weight (zero tangents)
                                None if per_key == 3 && part != 1 => 0.0,
                                None => base
                                    .and_then(|w| w.0.iter().find(|(t, _)| t == id))
                                    .map_or(*default, |(_, v)| *v),
                            });
                        }
                    }
                }
                samplers.push(self.sampler(
                    &first.times,
                    &values,
                    ElementType::Scalar,
                    first.interpolation,
                )?);
                channels.push(
                    json!({"sampler": samplers.len() - 1, "target": {"node": node, "path": "weights"}}),
                );
            }
            if channels.is_empty() {
                continue;
            }
            self.report.animation_channels += channels.len() as u64;
            let mut a = Map::new();
            a.insert("name".into(), json!(clip.name));
            a.insert("channels".into(), Value::Array(channels));
            a.insert("samplers".into(), Value::Array(samplers));
            if clip.extrapolation == Extrapolation::Loop {
                a.insert(
                    "extras".into(),
                    json!({"primase.loop": true, "primase.duration": clip.duration}),
                );
            }
            self.animations.push(Value::Object(a));
            self.report.animations += 1;
        }
        Ok(())
    }

    fn count_unwritten(&mut self) {
        if self.options.selection.is_some() {
            return;
        }
        let world = self.world;
        let mut accounted = world.closure(self.left_out.iter().copied());
        accounted.extend(&self.handled);
        let mut entities = BTreeSet::new();
        for name in world.component_names() {
            for e in world.entities_named(name) {
                if !accounted.contains(&e) {
                    *self
                        .report
                        .components_unwritten
                        .entry(name.to_owned())
                        .or_default() += 1;
                    entities.insert(e);
                }
            }
        }
        self.report.entities_unwritten = entities.len() as u64;
    }

    fn primase_extras(&mut self, shown: Vec<ComponentJson<'_>>, consumed: &[&str]) -> Option<Value> {
        let mut map = Map::new();
        for c in shown {
            if consumed.contains(&c.name) {
                continue;
            }
            match c.value {
                Some(v) => {
                    map.insert(c.name.to_owned(), json!({"version": c.version, "value": v}));
                }
                None => *self.report.extras_unencoded.entry(c.name.to_owned()).or_default() += 1,
            }
        }
        (!map.is_empty()).then(|| json!({ "primase": map }))
    }
}

fn cast_on(cast: bool) -> &'static str {
    if cast { "on" } else { "off" }
}

/// `extras` with `"primase.shadows"` added when anything about them is known.
fn with_shadows(extras: Option<Value>, cast: Option<&str>, receive: Option<bool>) -> Option<Value> {
    if cast.is_none() && receive.is_none() {
        return extras;
    }
    let mut shadows = Map::new();
    shadows.insert("version".into(), json!(1));
    if let Some(cast) = cast {
        shadows.insert("cast".into(), json!(cast));
    }
    if let Some(receive) = receive {
        shadows.insert("receive".into(), json!(receive));
    }
    let mut extras = extras.unwrap_or_else(|| json!({}));
    extras
        .as_object_mut()
        .expect("extras are an object")
        .insert(SHADOWS.into(), Value::Object(shadows));
    Some(extras)
}

/// glTF's `magFilter` (and a `minFilter` without mips): 9728 NEAREST, 9729 LINEAR.
fn filter_code(f: Filter) -> u32 {
    match f {
        Filter::Nearest => 9728,
        Filter::Linear => 9729,
    }
}

/// glTF's `minFilter`, which names the filter within a level and across levels together.
fn min_filter_code(min: Filter, mip: MipFilter) -> u32 {
    match (min, mip) {
        (min, MipFilter::None) => filter_code(min),
        (Filter::Nearest, MipFilter::Nearest) => 9984,
        (Filter::Linear, MipFilter::Nearest) => 9985,
        (Filter::Nearest, MipFilter::Linear) => 9986,
        (Filter::Linear, MipFilter::Linear) => 9987,
    }
}

fn wrap_code(w: Wrap) -> u32 {
    match w {
        Wrap::Repeat => 10497,
        Wrap::Clamp => 33071,
        Wrap::Mirror => 33648,
    }
}

/// A light's `KHR_lights_punctual` object without its name (defaults left out: white, intensity 1, no range; a
/// directional light has no range). An error for a non-finite value, or a spot's cones outside glTF's ranges.
pub fn punctual_light(l: &Light) -> std::result::Result<Value, String> {
    let finite = |xs: &[f32]| f32s_value(xs).map_err(|_| format!("non-finite material factor {xs:?}"));
    let mut o = Map::new();
    let kind = match l.kind {
        LightKind::Point => "point",
        LightKind::Directional => "directional",
        LightKind::Spot {
            inner_cone,
            outer_cone,
        } => {
            // the glTF ranges: 0 <= inner < pi / 2, 0 < outer <= pi / 2, and core's own rule that the inner is not past
            // the outer (equal is a hard edge)
            let half_pi = std::f32::consts::FRAC_PI_2;
            let in_range = 0.0 <= inner_cone
                && inner_cone <= outer_cone
                && outer_cone <= half_pi
                && outer_cone > 0.0
                && inner_cone < half_pi;
            if !in_range {
                return Err(format!(
                    "spot cones {inner_cone} and {outer_cone}: glTF needs 0 <= inner <= outer <= pi / 2, outer above 0"
                ));
            }
            let cones = finite(&[inner_cone, outer_cone])?;
            o.insert(
                "spot".into(),
                json!({"innerConeAngle": cones[0], "outerConeAngle": cones[1]}),
            );
            "spot"
        }
    };
    o.insert("type".into(), json!(kind));
    if l.color != [1.0; 3] {
        o.insert("color".into(), finite(&l.color)?);
    }
    if l.intensity != 1.0 {
        o.insert("intensity".into(), finite(&[l.intensity])?[0].clone());
    }
    if let Some(r) = l.range
        && l.kind != LightKind::Directional
    {
        o.insert("range".into(), finite(&[r])?[0].clone());
    }
    Ok(Value::Object(o))
}
