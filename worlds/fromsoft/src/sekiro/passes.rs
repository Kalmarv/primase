//! The passes a program runs on an area the reader assembled. The reader keeps the source; each of these interprets
//! part of it, states what it approximates and logs its counts. Choices that change what is produced (which face
//! sets, which material classes, repairs) are options with the faithful reading (the source as stored) as the
//! default; a choice with no faithful reading (which model to lower, a calibration, a quality cap) is required.
//! Each pass is made by a constructor taking its required choices and setters for the rest, from its options
//! through the same calls.
pub(super) mod bake_assembly;
pub mod blockers;
pub mod layers;
pub mod lights;
pub mod normals;
pub mod pbr;
pub(super) mod reflectance;
pub mod settings;
use super::source::{FaceCull, LayerInputs, MaterialSlots, PrototypeSource};
use crate::flver::{FaceSetKind, Model, Part};
use crate::materials::{self, LayeredMaterial, Material};
use crate::rigid::{ApplyRigidSkinning, RigidModel, SourceModel};
use blockers::BlockerCards;
use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::pbr::{PbrMaterial, UvTransform};
use primase_core::world::Resource;
use primase_core::{
    Component, DrawOrder, Entity, Instance, LodForm, Lods, MaterialUsage, Mesh, Primitive, Semantic, Source,
    Stream, Topology, Transform, World,
};
use settings::{PixelCap, Seconds};
use std::collections::{BTreeMap, BTreeSet};

/// A two-way choice of `lower-fromsoft-geometry` with its option words.
macro_rules! choice {
    ($(#[$doc:meta])* $name:ident { $(#[$da:meta])* $a:ident = $wa:literal, $(#[$db:meta])* $b:ident = $wb:literal $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $(#[$da])*
            $a,
            $(#[$db])*
            $b,
        }
        impl $name {
            /// Its option word.
            pub fn word(self) -> &'static str {
                match self {
                    Self::$a => $wa,
                    Self::$b => $wb,
                }
            }
            fn parse(key: &str, value: &str) -> Result<Self, String> {
                match value {
                    $wa => Ok(Self::$a),
                    $wb => Ok(Self::$b),
                    _ => Err(format!("{key}={value} is not {} or {}", $wa, $wb)),
                }
            }
        }
    };
}

choice! {
    /// Which model `lower-fromsoft-geometry` lowers (`pose=`).
    Pose {
        /// `fromsoft.rigid_model`: static meshes posed by their bones (`fromsoft-apply-rigid-skinning` first).
        Rigid = "rigid",
        /// `fromsoft.flver` as stored: static vertices left in their bones' spaces.
        Stored = "stored",
    }
}
choice! {
    /// What `lower-fromsoft-geometry` does with a material's UV tiling (`uv-tiling=`).
    Tiling {
        /// UV0 as stored.
        Keep = "keep",
        /// UV0 multiplied by the material's tiling, recorded as `fromsoft.uv_tiling`.
        Apply = "apply",
    }
}
choice! {
    /// The winding `lower-fromsoft-geometry` gives a part (`winding=`).
    Winding {
        /// As stored.
        Stored = "stored",
        /// Reversed where the part's stored normals oppose its winding on balance.
        Normals = "normals",
    }
}
choice! {
    /// What `lower-fromsoft-geometry` does with degenerate triangles, of doubled area squared at most 1e-14
    /// (`degenerates=`).
    Degenerates {
        /// Kept.
        Keep = "keep",
        /// Dropped, counted.
        Drop = "drop",
    }
}
choice! {
    /// What `lower-fromsoft-geometry` does with interior blocker cards (`blocker-cards=`).
    Cards {
        /// Drawn as any mesh.
        Draw = "draw",
        /// Their level-0 parts recorded in `fromsoft.blocker_cards` with every placement of their prototype.
        Record = "record",
    }
}

/// A material's UV tiling applied to a mesh's UV0 by `lower-fromsoft-geometry{uv-tiling=apply}`
/// (`fromsoft.uv_tiling`, on the material entity): the scale every primitive drawing it was multiplied by, so a
/// material pass binds its textures relative to it.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct UvTiling(pub [f32; 2]);
impl Component for UvTiling {
    const NAME: &'static str = "fromsoft.uv_tiling";
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// `lower-fromsoft-geometry`: every prototype's model as `core.mesh`, one primitive per mesh and face set, its
/// material the slot's `fromsoft.material` entity; level-0 face sets on the prototype, levels 1 and 2 as `core.lods`
/// forms (`select-lod` chooses), a form holding the meshes that have a face set of its level (no source default, as
/// the game switches by distance). Positions, unit normals, stored tangents (handedness reflected) and UV0 in the
/// world frame; a layered material's Byte4C members as raw `Color(n)` streams listed by `fromsoft.layer_inputs`.
///
/// Options: `pose=rigid|stored` (required: which model is lowered, the posed one `fromsoft-apply-rigid-skinning` made
/// or the stored one, has no faithful default), and choices whose default is the source as stored: `uv-tiling=keep|apply` (multiply UV0 by
/// the material's tiling, recorded as `fromsoft.uv_tiling`); `winding=stored|normals` (reverse a part whose stored
/// normals oppose its winding on balance); `degenerates=keep|drop` (triangles of doubled area squared at most
/// 1e-14); `leave-out=` classes joined by `+` (`sky`, `light-volume`, `shadow`: meshes of materials of that source
/// class are not lowered); `blocker-cards=draw|record` (record the level-0 parts of interior blocker cards in
/// `fromsoft.blocker_cards` with every placement of their prototype, instead of drawing them). A prototype left
/// drawing nothing at any level is despawned with its placements, counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerFromSoftGeometry {
    pose: Pose,
    uv_tiling: Tiling,
    winding: Winding,
    degenerates: Degenerates,
    /// Material skip classes whose meshes are not lowered.
    leave_out: BTreeSet<materials::SkipClass>,
    blocker_cards: Cards,
}

impl LowerFromSoftGeometry {
    /// The model `pose` names lowered as stored: UV0 as stored, windings as stored, degenerate triangles kept, every
    /// material class lowered, blocker cards drawn.
    pub fn new(pose: Pose) -> Self {
        LowerFromSoftGeometry {
            pose,
            uv_tiling: Tiling::Keep,
            winding: Winding::Stored,
            degenerates: Degenerates::Keep,
            leave_out: BTreeSet::new(),
            blocker_cards: Cards::Draw,
        }
    }
    pub fn with_uv_tiling(mut self, uv_tiling: Tiling) -> Self {
        self.uv_tiling = uv_tiling;
        self
    }
    pub fn with_winding(mut self, winding: Winding) -> Self {
        self.winding = winding;
        self
    }
    pub fn with_degenerates(mut self, degenerates: Degenerates) -> Self {
        self.degenerates = degenerates;
        self
    }
    /// The material skip classes whose meshes are not lowered.
    pub fn with_leave_out(mut self, leave_out: BTreeSet<materials::SkipClass>) -> Self {
        self.leave_out = leave_out;
        self
    }
    pub fn with_blocker_cards(mut self, blocker_cards: Cards) -> Self {
        self.blocker_cards = blocker_cards;
        self
    }
    pub fn pose(&self) -> Pose {
        self.pose
    }

    /// From its options, through [`LowerFromSoftGeometry::new`] and its setters.
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let pose = o
            .get("pose")
            .ok_or("pose=rigid|stored is required: which model is lowered is the program's choice")?;
        let mut pass = Self::new(Pose::parse("pose", pose)?);
        for (k, v) in o {
            pass = match k.as_str() {
                "pose" => pass,
                "uv-tiling" => pass.with_uv_tiling(Tiling::parse(k, v)?),
                "winding" => pass.with_winding(Winding::parse(k, v)?),
                "degenerates" => pass.with_degenerates(Degenerates::parse(k, v)?),
                "blocker-cards" => pass.with_blocker_cards(Cards::parse(k, v)?),
                "leave-out" => {
                    let mut classes = BTreeSet::new();
                    for class in v.split('+').filter(|c| !c.is_empty()) {
                        classes.insert(materials::SkipClass::from_word(class).ok_or_else(|| {
                            format!("leave-out class {class} is not sky, light-volume or shadow")
                        })?);
                    }
                    pass.with_leave_out(classes)
                }
                _ => return Err(format!("unknown option {k}")),
            };
        }
        Ok(pass)
    }
    fn input(&self) -> &'static str {
        match self.pose {
            Pose::Rigid => RigidModel::NAME,
            Pose::Stored => SourceModel::NAME,
        }
    }
}

/// One level of a prototype's lowered geometry: its primitives, each one's layer-input members and face-set cull.
#[derive(Default)]
struct Level {
    primitives: Vec<Primitive>,
    inputs: Vec<Vec<u8>>,
    cull: Vec<bool>,
}

impl Level {
    /// Its primitives on `e` as a mesh, with their face-set cull and any layer inputs.
    fn insert(self, world: &mut World, e: Entity) {
        world.insert(e, Mesh::new(self.primitives));
        world.insert(
            e,
            FaceCull {
                primitives: self.cull,
            },
        );
        if self.inputs.iter().any(|i| !i.is_empty()) {
            world.insert(
                e,
                LayerInputs {
                    primitives: self.inputs,
                },
            );
        }
    }
}

/// One prototype's lowered geometry, before it goes into the world.
#[derive(Default)]
struct Lowered {
    levels: BTreeMap<u8, Level>,
    cards: Vec<blockers::Card>,
}

impl LowerFromSoftGeometry {
    fn part(
        &self,
        model: &Model,
        mesh: usize,
        face_set: usize,
        material: &Material,
        log: &mut PassLog,
    ) -> Result<Option<Part>, String> {
        let Some(mut part) = model.decode(mesh, face_set).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        if self.uv_tiling == Tiling::Apply {
            part.apply_uv_scale(material.uv_scale)
                .map_err(|e| e.to_string())?;
        }
        if self.winding == Winding::Normals && part.repair_winding().map_err(|e| e.to_string())? {
            log.approximate("parts reversed to agree with their stored normals", 1);
        }
        if self.degenerates == Degenerates::Drop {
            log.count(
                "degenerate triangles dropped",
                part.drop_degenerates().map_err(|e| e.to_string())? as u64,
            );
        }
        if part.mesh.indices.is_empty() {
            log.count("face sets left without triangles", 1);
            return Ok(None);
        }
        part.mesh.validate().map_err(|e| e.to_string())?;
        Ok(Some(part))
    }

    fn lower(&self, world: &World, prototype: Entity, log: &mut PassLog) -> Result<Lowered, String> {
        let model = match self.pose {
            Pose::Rigid => world.get::<RigidModel>(prototype).map(|m| &m.0),
            Pose::Stored => world.get::<SourceModel>(prototype).map(|m| &m.0),
        }
        .ok_or_else(|| format!("{prototype:?}: no {}", self.input()))?;
        let slots = &world
            .get::<MaterialSlots>(prototype)
            .ok_or_else(|| format!("{prototype:?}: no fromsoft.material_slots"))?
            .0;
        let mut out = Lowered::default();
        let mut mesh_levels = Vec::new();
        for (index, mesh) in model.meshes.iter().enumerate() {
            let entity = *slots
                .get(mesh.material)
                .ok_or_else(|| format!("{prototype:?}: mesh {index} material slot {}", mesh.material))?;
            let material = world
                .get::<Material>(entity)
                .ok_or_else(|| format!("{prototype:?}: slot {} has no fromsoft.material", mesh.material))?;
            if let Some(class) = material.skip
                && self.leave_out.contains(&class)
            {
                log.count(&format!("meshes left out: {}", class.word()), 1);
                continue;
            }
            if self.blocker_cards == Cards::Record && blockers::is_card(material) {
                if let Some(face_set) = mesh.faces.iter().position(|f| f.is_lod0())
                    && let Some(part) = self.part(model, index, face_set, material, log)?
                {
                    out.cards.push(blockers::Card::from_part(part, material.clone()));
                }
                continue;
            }
            if mesh.vertices.iter().any(|v| !v.colors.is_empty()) {
                log.approximate(
                    "meshes with vertex colours not lowered (their shader meaning is unknown)",
                    1,
                );
            }
            if mesh.vertices.iter().any(|v| v.uvs.len() > 1) {
                log.approximate("meshes with UV sets beyond UV0 not lowered", 1);
            }
            let mut levels = BTreeSet::new();
            for (face_set, f) in mesh.faces.iter().enumerate() {
                let level = match f.kind() {
                    FaceSetKind::MotionBlur(_) => {
                        log.approximate("motion-blur face sets not lowered", 1);
                        continue;
                    }
                    FaceSetKind::Lod(level) => level,
                };
                if !levels.insert(level) {
                    log.approximate("second face sets of one level not lowered", 1);
                    continue;
                }
                let Some(part) = self.part(model, index, face_set, material, log)? else {
                    continue;
                };
                let layer = if material.layered.is_some() {
                    super::source::byte4_uv_members(model, index, &part.source_vertices)
                        .map_err(|e| e.to_string())?
                } else {
                    Vec::new()
                };
                let mut streams = BTreeMap::from([
                    (Semantic::Position, Stream::F32x3(part.mesh.positions)),
                    (Semantic::Normal, Stream::F32x3(part.mesh.normals)),
                    (Semantic::TexCoord(0), Stream::F32x2(part.mesh.uv)),
                ]);
                if !part.tangents.is_empty() {
                    streams.insert(Semantic::Tangent, Stream::F32x4(part.tangents));
                }
                let members = layer.iter().map(|(n, _)| *n).collect();
                for (n, values) in layer {
                    streams.insert(Semantic::Color(n), Stream::U8x4(values));
                }
                let out = out.levels.entry(level).or_default();
                out.primitives.push(Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(part.mesh.indices),
                    material: Some(entity),
                });
                out.inputs.push(members);
                out.cull.push(f.cull);
                log.count(&format!("level {level} primitives"), 1);
            }
            mesh_levels.push(levels);
        }
        // A coarser form holds the meshes that have a face set of its level; the others are not in it.
        for level in out.levels.keys().filter(|&&l| l > 0) {
            let absent = mesh_levels
                .iter()
                .filter(|l| !l.is_empty() && !l.contains(level))
                .count();
            if absent > 0 {
                log.count(
                    &format!("meshes without a level {level} face set, absent from that form"),
                    absent as u64,
                );
            }
        }
        Ok(out)
    }
}

impl Pass for LowerFromSoftGeometry {
    fn name(&self) -> &'static str {
        "lower-fromsoft-geometry"
    }
    fn options(&self) -> Options {
        // every setting; an empty leave-out set is no option
        let mut o = Options::from([
            ("pose".to_owned(), self.pose.word().to_owned()),
            ("uv-tiling".to_owned(), self.uv_tiling.word().to_owned()),
            ("winding".to_owned(), self.winding.word().to_owned()),
            ("degenerates".to_owned(), self.degenerates.word().to_owned()),
            ("blocker-cards".to_owned(), self.blocker_cards.word().to_owned()),
        ]);
        if !self.leave_out.is_empty() {
            o.insert(
                "leave-out".into(),
                self.leave_out
                    .iter()
                    .map(|c| c.word())
                    .collect::<Vec<_>>()
                    .join("+"),
            );
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        match self.pose {
            Pose::Rigid => &[RigidModel::NAME, MaterialSlots::NAME],
            Pose::Stored => &[SourceModel::NAME, MaterialSlots::NAME],
        }
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Mesh::NAME,
            Lods::NAME,
            LayerInputs::NAME,
            FaceCull::NAME,
            UvTiling::NAME,
            BlockerCards::NAME,
        ]
    }
    /// The posed model it lowered; the source model stays.
    fn removes(&self) -> &[&'static str] {
        match self.pose {
            Pose::Rigid => &[RigidModel::NAME],
            Pose::Stored => &[],
        }
    }
    fn modifies(&self) -> &[&'static str] {
        &[BlockerCards::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "motion-blur face sets, vertex colours and UV sets beyond UV0 not lowered; with winding=normals, a part's winding inferred from its stored normals"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let prototypes: Vec<Entity> = world.iter::<PrototypeSource>().map(|(e, _)| e).collect();
        let mut placements = BTreeMap::<Entity, Vec<Entity>>::new();
        for (e, i) in world.iter::<Instance>() {
            placements.entry(i.prototype).or_default().push(e);
        }
        // Everything is checked and lowered before the world changes.
        let mut lowered = Vec::with_capacity(prototypes.len());
        for &prototype in &prototypes {
            if world.has::<Mesh>(prototype) {
                return Err(format!("{prototype:?} already has core.mesh"));
            }
            lowered.push(self.lower(world, prototype, log)?);
        }
        if self.uv_tiling == Tiling::Apply {
            let used: BTreeSet<Entity> = lowered
                .iter()
                .flat_map(|l| {
                    l.levels
                        .values()
                        .flat_map(|l| l.primitives.iter().filter_map(|p| p.material))
                })
                .collect();
            for e in used {
                let scale = world.get::<Material>(e).expect("lowered material").uv_scale;
                world.insert(e, UvTiling(scale));
            }
        }
        // Cards are recorded for every placement of their prototype, in placement order.
        let mut cards = world.remove_resource::<BlockerCards>().unwrap_or_default();
        let card_sets: BTreeMap<Entity, (String, &Vec<blockers::Card>)> = prototypes
            .iter()
            .zip(&lowered)
            .filter(|(_, l)| !l.cards.is_empty())
            .map(|(&p, l)| {
                (
                    p,
                    (
                        world.get::<PrototypeSource>(p).expect("listed").id.clone(),
                        &l.cards,
                    ),
                )
            })
            .collect();
        for (p, i) in world.iter::<Instance>() {
            if let Some((id, set)) = card_sets.get(&i.prototype) {
                let source = world.get::<Source>(p).map(|s| s.0.clone()).unwrap_or_default();
                let transform = world
                    .get::<Transform>(p)
                    .ok_or_else(|| format!("placement {p:?} without a transform"))?;
                cards.record(id, source, transform, set);
            }
        }
        log.count(
            "blocker-card meshes recorded",
            card_sets.values().map(|(_, c)| c.len() as u64).sum(),
        );
        drop(card_sets);
        for (prototype, mut out) in prototypes.into_iter().zip(lowered) {
            world.remove::<RigidModel>(prototype);
            let placed = placements.remove(&prototype).unwrap_or_default();
            if out.levels.values().all(|l| l.primitives.is_empty()) {
                // the placements are core content, dropped by name; the prototype holds none yet (its source
                // model was lowered to nothing), so it declares nothing and is counted
                for p in placed {
                    primase_core::ledger::despawn(
                        world,
                        p,
                        log,
                        "placements of prototypes left drawing nothing",
                    );
                }
                primase_core::ledger::despawn(world, prototype, log, "prototypes left drawing nothing");
                log.count("prototypes left drawing nothing", 1);
                continue;
            }
            let finest = out.levels.remove(&0).unwrap_or_default();
            let mut forms = Vec::new();
            for (level, lowered) in out.levels {
                let form = world.spawn();
                lowered.insert(world, form);
                forms.push(LodForm { detail: level, form });
            }
            finest.insert(world, prototype);
            if !forms.is_empty() {
                log.count("prototypes with other levels of detail", 1);
                world.insert(
                    prototype,
                    Lods {
                        detail: 0,
                        forms,
                        default: None,
                    },
                );
            }
            log.count("prototypes lowered", 1);
        }
        if !cards.prototypes.is_empty() || !cards.placements.is_empty() {
            world.insert_resource(cards);
        }
        Ok(())
    }
}

/// `drop-fromsoft-far`: the placements of distant-backdrop prototypes (`fromsoft.prototype` `far`) dropped by name
/// (a named drop of each placement), and those prototypes despawned, counted (they hold no core content yet);
/// nothing else changes.
#[derive(Clone, Copy, Debug)]
pub struct DropFromSoftFar;

impl Pass for DropFromSoftFar {
    fn name(&self) -> &'static str {
        "drop-fromsoft-far"
    }
    fn requires(&self) -> &[&'static str] {
        &[PrototypeSource::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let far: BTreeSet<Entity> = world
            .iter::<PrototypeSource>()
            .filter(|(_, s)| s.far)
            .map(|(e, _)| e)
            .collect();
        let placed: Vec<Entity> = world
            .iter::<Instance>()
            .filter(|(_, i)| far.contains(&i.prototype))
            .map(|(e, _)| e)
            .collect();
        for e in placed {
            primase_core::ledger::despawn(world, e, log, "distant-backdrop placements");
        }
        for e in far {
            primase_core::ledger::despawn(world, e, log, "distant-backdrop prototypes");
            log.count("distant-backdrop prototypes dropped", 1);
        }
        Ok(())
    }
}

/// `bake-fromsoft-layers{max-size=N,sample-time=S}`: bake the static layered materials of the placed lowered
/// prototypes (a `fromsoft.material` with a layered definition, drawn by primitives carrying `fromsoft.layer_inputs`)
/// into core PBR from the source programs and texture files the reader kept. Each material group's triangles are
/// reduced to their mean inputs, clustered into at most three constant levels, and each level's shader evaluated
/// into albedo, normal, gloss (ORM) and HDR emission images at the group's reference size capped at `max-size`, sampled
/// from base mips with bilinear wrapping in a static frame (unit pre-exposure, fixed camera, the global time at
/// `sample-time` seconds, which scrolls some layers); parts are split by level through `World::rebuild_primitives`.
/// Both options are required: the cap is the program's quality choice (also the size of a group without a colour
/// image), the time its choice of the animated layers' frame.
///
/// The layer inputs of the primitives it baked are consumed (their `Color(n)` streams and entries); unplaced
/// prototypes' stay. A level-of-detail form or a placed mesh without a source prototype that carries layer inputs is
/// refused before anything changes: choose a level of detail first (`select-lod`). The source resources are kept,
/// also when it fails.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BakeFromSoftLayers {
    /// The baked images' largest edge.
    max_size: PixelCap,
    /// The seconds of global time the programs that read it are sampled at.
    sample_time: Seconds,
}

impl BakeFromSoftLayers {
    pub fn new(max_size: PixelCap, sample_time: Seconds) -> Self {
        BakeFromSoftLayers {
            max_size,
            sample_time,
        }
    }
    pub fn max_size(&self) -> PixelCap {
        self.max_size
    }
    pub fn sample_time(&self) -> Seconds {
        self.sample_time
    }

    /// From its options (both required), through [`BakeFromSoftLayers::new`].
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let mut max_size = None;
        let mut sample_time = None;
        for (k, v) in o {
            match k.as_str() {
                "max-size" => max_size = Some(PixelCap::parse(k, v)?),
                "sample-time" => sample_time = Some(Seconds::parse(k, v)?),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(BakeFromSoftLayers::new(
            max_size.ok_or("max-size is required: the baked images' largest edge is the program's choice")?,
            sample_time
                .ok_or("sample-time is required: the frame of the animated layers is the program's choice")?,
        ))
    }
}

impl Pass for BakeFromSoftLayers {
    fn name(&self) -> &'static str {
        "bake-fromsoft-layers"
    }
    fn options(&self) -> Options {
        Options::from([
            ("max-size".to_owned(), self.max_size.to_string()),
            ("sample-time".to_owned(), self.sample_time.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME, LayerInputs::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            "pbr.material",
            "core.texture",
            "core.name",
            Material::NAME,
            materials::LayeredMaterial::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[LayerInputs::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "layer inputs as constant per-triangle levels; base-mip bilinear-wrap sampling; static frame at the sample time"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        use super::source::{ShaderPrograms, TexturePayloads};
        // the meshes it bakes: placed source prototypes'; inputs elsewhere are refused (a form) or left (unplaced)
        let placed: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let forms: BTreeSet<Entity> = world
            .iter::<Lods>()
            .flat_map(|(_, l)| l.forms.iter().map(|f| f.form))
            .collect();
        let mut unplaced = 0;
        for (e, _) in world.iter::<LayerInputs>() {
            if placed.contains(&e) && world.has::<PrototypeSource>(e) {
                continue;
            }
            if forms.contains(&e) || placed.contains(&e) {
                return Err(format!(
                    "{e:?} carries layer inputs this bake does not bake (a level-of-detail form, or a placed mesh \
                     without fromsoft.prototype): choose a level of detail first (select-lod)"
                ));
            }
            unplaced += 1;
        }
        if unplaced > 0 {
            log.count("unplaced meshes whose layer inputs stay", unplaced);
        }
        // lent read-only while the world changes, then put back as they were: borrowed, not written
        let none = BTreeMap::new();
        let result = world.resource_scope::<ShaderPrograms, _>(|world, shaders| {
            world.resource_scope::<TexturePayloads, _>(|world, payloads| {
                let bake = layers::AreaBake::with_sources(
                    world,
                    shaders.map_or(&none, |s| &s.0),
                    payloads.map_or(&none, |p| &p.0),
                )?;
                let prototypes: BTreeMap<String, Entity> = world
                    .iter::<PrototypeSource>()
                    .map(|(e, s)| (s.id.clone(), e))
                    .collect();
                bake_assembly::assemble(
                    &bake,
                    &prototypes,
                    self.max_size.get(),
                    self.sample_time.get(),
                    world,
                )
            })
        });
        let (report, baked) = result.map_err(|e| e.to_string())?;
        // counted after the split: a part baked at k levels gives k primitives' streams
        log.count(
            "layer input streams consumed",
            LayerInputs::consume(world, |e, i, _| baked.contains(&(e, i))),
        );
        for (k, v) in [
            ("source materials baked", report.source_materials),
            ("bake groups", report.materials),
            ("bake levels", report.variants),
            ("bake levels with non-unit UV periods", report.nonunit_uv_variants),
            ("bake groups with gloss", report.gloss_materials),
            ("bake groups without gloss", report.missing_gloss_materials),
            ("bake groups with emission", report.emissive_materials),
            ("bake groups without emission", report.missing_emissive_materials),
        ] {
            log.count(k, v as u64);
        }
        for (k, v) in report.policies.counts {
            log.count(&k, v);
        }
        for (k, v) in report.policies.approximations {
            log.approximate(&k, v);
        }
        Ok(())
    }
}

/// `lower-fromsoft-materials{layers=preserve|preview}`: source materials as core PBR from their bindings and the
/// texture files the reader kept (decoded here, named `<stem>` and `<stem>@normal`): base and normal maps in the
/// source normal convention (`normalize-normals` converts), gloss to roughness, traced metallic constants and maps
/// (unevidenced metallic is dielectric 0, counted), the preview fallback colour without a base texture (counted),
/// texture transforms relative to the tiling a geometry pass applied. Baked materials keep their bindings. What a
/// lowered material's source class states about its surfaces is set with it ([`pbr::class_usage`]): water
/// and leaves as their `core.material_usage`, decals as their `core.draw_order`. A material is two-sided exactly where
/// the face sets drawing it do not cull back faces (`fromsoft.face_cull`), split when they differ
/// ([`pbr::apply_face_cull`]).
///
/// A layered material no pass baked needs the program's choice: `layers=preserve` leaves it as source data
/// (unlowered, its layer inputs kept, counted); `layers=preview` lowers it from its main-layer preview, a counted
/// approximation, and drops the layer inputs (shader inputs, not vertex colours). Without the option such a material
/// is an error. Materials of a skipped source class, and source materials no primitive draws, are not lowered, counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowerFromSoftMaterials {
    layers: Option<pbr::Layers>,
}

impl LowerFromSoftMaterials {
    /// `layers`: what becomes of a layered material no pass baked; `None`: such a material is an error.
    pub fn new(layers: Option<pbr::Layers>) -> Self {
        LowerFromSoftMaterials { layers }
    }
    pub fn layers(&self) -> Option<pbr::Layers> {
        self.layers
    }

    /// From its options, through [`LowerFromSoftMaterials::new`].
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let mut layers = None;
        for (k, v) in o {
            layers = match (k.as_str(), v.as_str()) {
                ("layers", "preserve") => Some(pbr::Layers::Preserve),
                ("layers", "preview") => Some(pbr::Layers::Preview),
                ("layers", v) => return Err(format!("layers={v} is not preserve or preview")),
                _ => return Err(format!("unknown option {k}")),
            };
        }
        Ok(Self::new(layers))
    }
}

impl Pass for LowerFromSoftMaterials {
    fn name(&self) -> &'static str {
        "lower-fromsoft-materials"
    }
    fn options(&self) -> Options {
        match self.layers {
            None => Options::new(),
            Some(l) => Options::from([(
                "layers".to_owned(),
                match l {
                    pbr::Layers::Preserve => "preserve",
                    pbr::Layers::Preview => "preview",
                }
                .to_owned(),
            )]),
        }
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        // a material drawn both culled and not is split: its copy has the source material's components
        &[
            "pbr.material",
            "core.texture",
            "core.name",
            MaterialUsage::NAME,
            DrawOrder::NAME,
            Material::NAME,
            LayeredMaterial::NAME,
            UvTiling::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[LayerInputs::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &["pbr.material", "core.texture", Mesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "unevidenced metallic as dielectric 0; per-light gloss correction omitted; with layers=preview, unbaked layered materials from their main-layer preview; preview fallback colour"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        use super::source::TexturePayloads;
        // lent read-only (a world without the reader's payloads has no texture file to decode): borrowed, not written
        let lowered = world.resource_scope::<TexturePayloads, _>(|world, payloads| {
            pbr::lower_drawn(world, payloads.map_or(&BTreeMap::new(), |p| &p.0), self.layers)
        });
        let (lowered, previewed) = lowered.map_err(|e| e.to_string())?;
        // the inputs of the layers a preview stands for: not vertex colours, so they go with the layers; the inputs of
        // materials left as source data stay
        let dropped = LayerInputs::consume(world, |_, _, p| {
            p.material.is_some_and(|m| previewed.contains(&m))
        });
        if dropped > 0 {
            log.approximate(
                "layer input streams dropped with layered materials lowered from previews",
                dropped,
            );
        }
        log.absorb(lowered);
        pbr::apply_face_cull(world, log).map_err(|e| e.to_string())
    }
}

/// `drop-fromsoft-texture-transforms`: every texture binding of a lowered FromSoft material (a `pbr.material` on an
/// entity with `fromsoft.material`) loses its transform, so each texture is sampled at the coordinates
/// `lower-fromsoft-geometry{uv-tiling=apply}` scaled by the albedo binding's FLVER tiling, as the earlier exporter
/// sampled them (only the main albedo's tiling scales the mesh UVs; a normal or detail map's own scale is not applied).
/// Sekiro's normal and roughness maps carry their binding's scale relative to the albedo's
/// (`lower-fromsoft-materials`), and a material that mixes them cannot have them baked into one coordinate set, so a
/// target without texture transforms (ReSkate's dialect) needs them gone. Each non-identity transform left out is a
/// counted approximation, by slot; other sources' materials are not this pass's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropFromSoftTextureTransforms;

impl DropFromSoftTextureTransforms {
    /// The pass; it has no settings.
    pub fn new() -> Self {
        DropFromSoftTextureTransforms
    }
}

impl Pass for DropFromSoftTextureTransforms {
    fn name(&self) -> &'static str {
        "drop-fromsoft-texture-transforms"
    }
    fn requires(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, Material::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "FromSoft textures sampled at the albedo's tiling, their own transform left out"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<Entity> = world
            .iter::<PbrMaterial>()
            .filter(|(e, _)| world.has::<Material>(*e))
            .map(|(e, _)| e)
            .collect();
        for e in materials {
            let Some(m) = world.get_mut::<PbrMaterial>(e) else {
                continue;
            };
            let specular = m.specular.as_mut();
            let (specular, specular_colour) = match specular {
                Some(s) => (&mut s.texture, &mut s.color_texture),
                None => (&mut None, &mut None),
            };
            for (slot, r) in [
                ("base colour", &mut m.base_color_texture),
                ("metallic-roughness", &mut m.metallic_roughness_texture),
                ("normal", &mut m.normal_texture),
                ("occlusion", &mut m.occlusion_texture),
                ("emissive", &mut m.emissive_texture),
                ("specular", specular),
                ("specular colour", specular_colour),
            ] {
                if let Some(t) = r.as_mut().and_then(|r| r.transform.take())
                    && t != UvTransform::IDENTITY
                {
                    log.approximate(
                        &format!(
                            "{slot} textures sampled at the albedo's tiling, their own transform left out"
                        ),
                        1,
                    );
                }
            }
        }
        Ok(())
    }
}

/// A condition's stable text, for grouping soups by condition.
pub(crate) fn condition_key(c: &primase_core::state::Condition) -> String {
    c.0.iter()
        .map(|cl| {
            cl.0.iter()
                .map(|l| format!("{:?}:{:x}", l.axis, l.values))
                .collect::<Vec<_>>()
                .join("&")
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// `pass`, which takes no options: an option is refused.
fn without_options(options: &Options, pass: impl Pass + 'static) -> Result<Box<dyn Pass>, String> {
    if let Some(k) = options.keys().next() {
        return Err(format!("unknown option {k}"));
    }
    Ok(Box::new(pass))
}

/// Register the FromSoft area passes by name.
pub fn register(registry: &mut PassRegistry) {
    registry.register("fromsoft-apply-rigid-skinning", |o| {
        without_options(o, ApplyRigidSkinning)
    });
    registry.register_requiring(
        "lower-fromsoft-geometry",
        |o| Ok(Box::new(LowerFromSoftGeometry::from_options(o)?)),
        &[("pose", "rigid")],
    );
    registry.register("drop-fromsoft-far", |o| without_options(o, DropFromSoftFar));
    registry.register_requiring(
        "bake-fromsoft-layers",
        |o| Ok(Box::new(BakeFromSoftLayers::from_options(o)?)),
        &[("max-size", "512"), ("sample-time", "0")],
    );
    registry.register("drop-fromsoft-texture-transforms", |o| {
        without_options(o, DropFromSoftTextureTransforms::new())
    });
    registry.register("lower-fromsoft-materials", |o| {
        Ok(Box::new(LowerFromSoftMaterials::from_options(o)?))
    });
    registry.register_requiring(
        "lower-fromsoft-collision",
        |o| {
            Ok(Box::new(super::collision::LowerFromSoftCollision::from_options(
                o,
            )?))
        },
        &[("hit-filters", "8+27")],
    );
    registry.register_requiring(
        "lower-fromsoft-object-collision",
        |o| {
            Ok(Box::new(
                super::objects::LowerFromSoftObjectCollision::from_options(o)?,
            ))
        },
        &[("convex", "tessellate")],
    );
    registry.register("lower-fromsoft-extras", |o| {
        without_options(o, super::extras::LowerFromSoftExtras)
    });
    registry.register_requiring(
        "lower-fromsoft-lights",
        |o| Ok(Box::new(lights::LowerFromSoftLights::from_options(o)?)),
        &[("candela-per-power", "1"), ("spot-blend", "0.15")],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::LayeredMaterial;

    /// The distant backdrop's placements are dropped by name, so the conservation check (enforced) sees every
    /// placement it removes declared; its prototypes, which hold no core content yet, declare nothing.
    #[test]
    fn far_placements_are_declared_drops() {
        use primase_core::ledger::{Conservation, Measure};
        use primase_core::pass::{PassManager, Pipeline, RunReport};
        let mut world = World::new();
        let far = world.spawn();
        world.insert(
            far,
            PrototypeSource {
                id: "m9000000".into(),
                far: true,
            },
        );
        let near = world.spawn();
        world.insert(
            near,
            PrototypeSource {
                id: "m0000000".into(),
                far: false,
            },
        );
        for prototype in [far, far, near] {
            let p = world.spawn();
            world.insert(
                p,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            world.insert(p, Transform::IDENTITY);
        }
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_conservation(Conservation::Enforce)
            .run(
                &mut world,
                &Pipeline {
                    passes: vec![Box::new(DropFromSoftFar)],
                },
                &mut report,
            )
            .unwrap();
        assert_eq!(report.steps[0].log.declared(Measure::Placements), 2);
        assert_eq!(world.iter::<Instance>().count(), 1);
        assert!(!world.is_alive(far));
    }

    /// A layered source material (no texture bound), `skip` its skip class.
    fn layered(skip: Option<materials::SkipClass>) -> Material {
        let mut m = materials::resolve(
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                gx: vec![],
                textures: vec![],
            },
            None,
            |_| false,
        );
        m.albedo = None;
        m.normal = None;
        m.skip = skip;
        m.layered = Some(LayeredMaterial {
            mtd: m.mtd.clone(),
            definition: crate::mtd::MaterialDefinition {
                shader: "M[Multiple].spx".into(),
                description: String::new(),
                parameters: vec![],
                textures: vec![],
                block_lengths: vec![],
            },
            bindings: BTreeMap::new(),
        });
        m
    }

    /// A triangle drawn with `material`, carrying layer input member 1.
    fn primitive(material: Entity) -> Primitive {
        Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
                ),
                (Semantic::Color(1), Stream::U8x4(vec![[255, 0, 0, 0]; 3])),
            ]),
            indices: Some(vec![0, 1, 2]),
            material: Some(material),
        }
    }

    fn mesh_with_inputs(world: &mut World, material: Entity) -> Entity {
        let e = world.spawn();
        world.insert(e, Mesh::new(vec![primitive(material)]));
        world.insert(
            e,
            LayerInputs {
                primitives: vec![vec![1]],
            },
        );
        e
    }

    #[test]
    fn the_bake_refuses_two_placed_prototypes_of_one_source_id() {
        // the source id orders the bake's inputs; two prototypes sharing one would leave that order to their entities
        let mut world = World::new();
        let m = world.spawn();
        world.insert(m, layered(None));
        for _ in 0..2 {
            let prototype = mesh_with_inputs(&mut world, m);
            world.insert(
                prototype,
                PrototypeSource {
                    id: "obj/o000001".into(),
                    far: false,
                },
            );
            let placement = world.spawn();
            world.insert(
                placement,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            world.insert(placement, Transform::IDENTITY);
        }
        let before = world.clone();
        let bake = BakeFromSoftLayers::new(PixelCap::new(64).unwrap(), Seconds::new(0.).unwrap());
        let err = bake.run(&mut world, &mut PassLog::default()).unwrap_err();
        assert!(
            err.contains("two placed prototypes with source id obj/o000001"),
            "{err}"
        );
        assert_eq!(
            world.iter::<LayerInputs>().count(),
            before.iter::<LayerInputs>().count()
        );
    }

    #[test]
    fn the_bake_refuses_level_of_detail_forms_before_changing_anything_and_leaves_unplaced_inputs() {
        let mut world = World::new();
        let m = world.spawn();
        world.insert(m, layered(None));
        let prototype = mesh_with_inputs(&mut world, m);
        world.insert(
            prototype,
            PrototypeSource {
                id: "obj/o000001".into(),
                far: false,
            },
        );
        let form = mesh_with_inputs(&mut world, m);
        world.insert(
            prototype,
            Lods {
                detail: 0,
                forms: vec![LodForm { detail: 1, form }],
                default: None,
            },
        );
        let placement = world.spawn();
        world.insert(
            placement,
            Instance {
                prototype,
                variant: 0,
            },
        );
        world.insert(placement, Transform::IDENTITY);
        let before = world.clone();
        let bake = BakeFromSoftLayers::new(PixelCap::new(64).unwrap(), Seconds::new(0.).unwrap());
        let err = bake.run(&mut world, &mut PassLog::default()).unwrap_err();
        assert!(err.contains("select-lod"), "{err}");
        assert_eq!(
            world.iter::<LayerInputs>().count(),
            before.iter::<LayerInputs>().count()
        );
        assert!(
            world.get::<Mesh>(form).unwrap().primitives()[0]
                .streams
                .contains_key(&Semantic::Color(1))
        );

        // an unplaced prototype's inputs are not the bake's: they stay, streams and all
        let mut world = World::new();
        let m = world.spawn();
        world.insert(m, layered(None));
        let unplaced = mesh_with_inputs(&mut world, m);
        world.insert(
            unplaced,
            PrototypeSource {
                id: "obj/o000002".into(),
                far: false,
            },
        );
        let mut log = PassLog::default();
        bake.run(&mut world, &mut log).unwrap();
        assert_eq!(log.counts["unplaced meshes whose layer inputs stay"], 1);
        assert_eq!(log.counts["layer input streams consumed"], 0);
        assert!(world.has::<LayerInputs>(unplaced));
        assert!(
            world.get::<Mesh>(unplaced).unwrap().primitives()[0]
                .streams
                .contains_key(&Semantic::Color(1))
        );
    }

    #[test]
    fn a_preview_drops_only_the_inputs_of_the_materials_it_previewed() {
        let mut world = World::new();
        let previewed = world.spawn();
        world.insert(previewed, layered(None));
        let skipped = world.spawn();
        world.insert(skipped, layered(Some(materials::SkipClass::Sky)));
        let e = world.spawn();
        world.insert(e, Mesh::new(vec![primitive(previewed), primitive(skipped)]));
        world.insert(
            e,
            FaceCull {
                primitives: vec![true; 2],
            },
        );
        world.insert(
            e,
            LayerInputs {
                primitives: vec![vec![1], vec![1]],
            },
        );
        let mut log = PassLog::default();
        LowerFromSoftMaterials::new(Some(super::pbr::Layers::Preview))
            .run(&mut world, &mut log)
            .unwrap();
        assert_eq!(
            log.approximations["layer input streams dropped with layered materials lowered from previews"],
            1
        );
        let mesh = world.get::<Mesh>(e).unwrap();
        assert!(!mesh.primitives()[0].streams.contains_key(&Semantic::Color(1)));
        // the skipped class was not lowered: its inputs are still the source's
        assert!(mesh.primitives()[1].streams.contains_key(&Semantic::Color(1)));
        assert_eq!(world.get::<LayerInputs>(e).unwrap().primitives, [vec![], vec![1]]);
    }

    #[test]
    fn a_lowered_material_carries_what_its_source_class_states_about_its_surfaces() {
        use primase_core::pass::{PassManager, Pipeline, RunReport};
        use primase_core::{DrawLayer, MaterialUsage};
        // the reader's class from each shader family, as resolve gives it
        let source = |shader: &str, parameters: Vec<crate::mtd::Parameter>| {
            materials::resolve(
                &crate::flver::Material {
                    name: "fixture".into(),
                    mtd: "fixture.mtd".into(),
                    gx: vec![],
                    textures: vec![],
                },
                Some(&crate::mtd::MaterialDefinition {
                    shader: shader.into(),
                    description: String::new(),
                    parameters,
                    textures: vec![],
                    block_lengths: vec![],
                }),
                |_| false,
            )
        };
        let int = |name: &str, v: i32| crate::mtd::Parameter {
            name: name.into(),
            value: crate::mtd::Value::Int(vec![v]),
        };
        let mut world = World::new();
        let mut drawn = Vec::new();
        for (shader, parameters, class) in [
            ("GXFlver_Tessellation_Water.spx", vec![], "water"),
            ("M[Translucency]_Leaves.spx", vec![], "leaves"),
            ("GXFlver_MeshDecal.spx", vec![], "decal"),
            ("M[ARSN].spx", vec![int("g_BlendMode", 2)], "alpha"),
            ("M[ARSN].spx", vec![int("g_AlphaRef", 128)], "cutout"),
            ("M[ARSN].spx", vec![], "opaque"),
        ] {
            let m = source(shader, parameters);
            assert_eq!(m.class, class, "{shader}");
            let e = world.spawn();
            world.insert(e, m);
            drawn.push(e);
        }
        let mesh = world.spawn();
        world.insert(mesh, Mesh::new(drawn.iter().map(|&m| primitive(m)).collect()));
        world.insert(
            mesh,
            FaceCull {
                primitives: vec![true; drawn.len()],
            },
        );
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                &mut world,
                &Pipeline {
                    passes: vec![Box::new(LowerFromSoftMaterials::new(None))],
                },
                &mut report,
            )
            .unwrap();
        let [water, leaves, decal, alpha, cutout, opaque] = drawn[..] else {
            unreachable!()
        };
        assert_eq!(world.get::<MaterialUsage>(water), Some(&MaterialUsage::Water));
        assert_eq!(world.get::<MaterialUsage>(leaves), Some(&MaterialUsage::Foliage));
        assert_eq!(
            world.get::<DrawOrder>(decal),
            Some(&DrawOrder::conventional(
                DrawLayer::Decal,
                super::pbr::DECAL_QUEUE
            ))
        );
        assert!(!world.get::<DrawOrder>(decal).unwrap().depth_write);
        // the class states nothing more: no usage on a decal, no draw order on water or leaves (their alpha mode's)
        for e in [decal, alpha, cutout, opaque] {
            assert!(world.get::<MaterialUsage>(e).is_none());
        }
        for e in [water, leaves, alpha, cutout, opaque] {
            assert!(world.get::<DrawOrder>(e).is_none());
        }
        assert_eq!(DrawOrder::of(&world, leaves).layer, DrawLayer::Cutout);
        assert_eq!(DrawOrder::of(&world, water).layer, DrawLayer::Transparent);
        let log = &report.steps[0].log;
        assert_eq!(log.counts["materials of class water given their usage"], 1);
        assert_eq!(log.counts["materials of class leaves given their usage"], 1);
        assert_eq!(log.counts["materials of class decal given their draw order"], 1);
    }

    #[test]
    fn a_lowered_material_is_as_sided_as_the_face_sets_drawing_it_and_split_when_they_differ() {
        use primase_core::pass::{PassManager, Pipeline, RunReport};
        use primase_core::pbr::PbrMaterial;
        use primase_core::{Name, Semantic};
        let mut world = World::new();
        let material = |world: &mut World, name: &str| {
            let m = world.spawn();
            let mut source = layered(None);
            source.layered = None;
            world.insert(m, source);
            world.insert(m, primase_core::Name(name.into()));
            m
        };
        let culled = material(&mut world, "culled");
        let open = material(&mut world, "open");
        let both = material(&mut world, "both");
        // two meshes: the first draws `both` culled and `open`, the second `culled` and `both` not culled
        let first = world.spawn();
        world.insert(first, Mesh::new(vec![primitive(both), primitive(open)]));
        world.insert(
            first,
            FaceCull {
                primitives: vec![true, false],
            },
        );
        let second = world.spawn();
        world.insert(second, Mesh::new(vec![primitive(culled), primitive(both)]));
        world.insert(
            second,
            FaceCull {
                primitives: vec![true, false],
            },
        );
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                &mut world,
                &Pipeline {
                    passes: vec![Box::new(LowerFromSoftMaterials::new(None))],
                },
                &mut report,
            )
            .unwrap();
        let sided = |e: Entity| world.get::<PbrMaterial>(e).unwrap().double_sided;
        assert!(!sided(culled));
        assert!(sided(open));
        assert!(!sided(both));
        // the primitive drawing `both` without culling draws its two-sided copy, which keeps its source
        let copy = world.get::<Mesh>(second).unwrap().primitives()[1]
            .material
            .unwrap();
        assert_ne!(copy, both);
        assert!(sided(copy));
        assert_eq!(world.get::<Name>(copy).unwrap().0, "both_two_sided");
        assert_eq!(
            serde_json::to_value(world.get::<Material>(copy).unwrap()).unwrap(),
            serde_json::to_value(world.get::<Material>(both).unwrap()).unwrap()
        );
        assert_eq!(
            world.get::<Mesh>(first).unwrap().primitives()[0].material,
            Some(both)
        );
        assert!(
            world.get::<Mesh>(second).unwrap().primitives()[1]
                .streams
                .contains_key(&Semantic::Position)
        );
        let log = &report.steps[0].log;
        assert_eq!(log.counts["materials split by face-set cull"], 1);
        assert_eq!(
            log.counts["materials single-sided: their face sets cull back faces"],
            1
        );
        assert_eq!(log.counts["materials two-sided: their face sets do not cull"], 1);

        // a mesh drawing a lowered material without its face sets' cull is refused: sidedness is never a default
        let mut world = World::new();
        let m = material(&mut world, "unknown");
        let mesh = world.spawn();
        world.insert(mesh, Mesh::new(vec![primitive(m)]));
        let err = LowerFromSoftMaterials::new(None)
            .run(&mut world, &mut PassLog::default())
            .unwrap_err();
        assert!(err.contains("without fromsoft.face_cull"), "{err}");
    }

    #[test]
    fn fromsoft_textures_lose_their_transforms_and_other_materials_keep_theirs() {
        use primase_core::pass::{PassManager, Pipeline, RunReport};
        use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
        let mut world = World::new();
        let t = world.spawn();
        let scaled = |s: f32| {
            let mut r = TextureRef::new(t);
            r.transform = Some(UvTransform {
                scale: [s, s],
                ..UvTransform::IDENTITY
            });
            r
        };
        let material = PbrMaterial {
            base_color_texture: Some(TextureRef::new(t)),
            normal_texture: Some(scaled(4.)),
            metallic_roughness_texture: Some(scaled(1.)),
            ..PbrMaterial::default()
        };
        let fromsoft = world.spawn();
        world.insert(fromsoft, material.clone());
        world.insert(fromsoft, layered(None));
        let other = world.spawn();
        world.insert(other, material.clone());
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run(
                &mut world,
                &Pipeline {
                    passes: vec![Box::new(DropFromSoftTextureTransforms::new())],
                },
                &mut report,
            )
            .unwrap();
        assert!(
            world
                .get::<PbrMaterial>(fromsoft)
                .unwrap()
                .textures()
                .all(|r| r.transform.is_none())
        );
        // another source's material is not this pass's
        assert_eq!(world.get::<PbrMaterial>(other), Some(&material));
        let log = &report.steps[0].log;
        // the identity transform is no approximation
        assert_eq!(log.approximations.len(), 1, "{:?}", log.approximations);
        assert_eq!(
            log.approximations["normal textures sampled at the albedo's tiling, their own transform left out"],
            1
        );
    }
}
