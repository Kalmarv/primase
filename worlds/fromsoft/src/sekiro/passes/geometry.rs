//! `lower-fromsoft-geometry`: the prototypes' models as core meshes, with the choices it takes as typed options.
use super::blockers::BlockerCards;
use crate::components::models::{RigidModel, SourceModel};
use crate::flver::{FaceSetKind, Model, Part};
use crate::materials::{self, Material};
use crate::sekiro::components::lowered::UvTiling;
use crate::sekiro::source::{FaceCull, LayerInputs, MaterialSlots, PrototypeSource};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::world::Resource;
use primase_core::{
    Component, Entity, Instance, LodForm, Lods, Mesh, Primitive, Semantic, Source, Stream, Topology,
    Transform, World,
};
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
    cards: Vec<super::blockers::Card>,
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
            if self.blocker_cards == Cards::Record && super::blockers::is_card(material) {
                if let Some(face_set) = mesh.faces.iter().position(|f| f.is_lod0())
                    && let Some(part) = self.part(model, index, face_set, material, log)?
                {
                    out.cards
                        .push(super::blockers::Card::from_part(part, material.clone()));
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
                    crate::sekiro::source::byte4_uv_members(model, index, &part.source_vertices)
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
        let card_sets: BTreeMap<Entity, (String, &Vec<super::blockers::Card>)> = prototypes
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
