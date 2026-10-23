//! `lower-fromsoft-geometry`: the prototypes' models as core meshes, with the choices it takes as typed options.
//!
//! A prototype bound to a skeleton (`fromsoft.skeleton`, the character reader's) lowered with `pose=stored` keeps its
//! skin: each primitive of a dynamic mesh gets `Joints(0)` (the bones its vertices name, as stored: the FLVER's bone
//! indices, which index the skeleton directly when the mesh has no palette) and `Weights(0)` (as stored), and each form
//! a `core.skin` binding them to that skeleton, every joint a slot with the inverse of its rest in the skeleton's space
//! as its inverse bind (dynamic vertices are stored in that bind pose), the weights `AsStored` (the game's rule for
//! their sum is not established) and the deformation linear. A mesh with a palette (palette-relative indices) or a
//! static mesh (vertices in their bone's space) in a bound model is refused: neither is established for these files.
use super::blockers::BlockerCards;
use crate::components::models::{RigidModel, SkeletonOf, SourceModel};
use crate::flver::{FaceSetKind, Model, Part};
use crate::materials::{self, Material};
use crate::sekiro::components::lowered::UvTiling;
use crate::sekiro::source::{FaceCull, LayerInputs, MaterialSlots, PrototypeSource};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::world::Resource;
use primase_core::{
    Binding, BindingId, Component, Deformation, Entity, Influences, Instance, LodForm, Lods, Mesh, Primitive,
    Semantic, Skeleton, Skin, Slot, Source, Stream, Topology, Transform, WeightRule, World,
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
/// world frame; a layered material's Byte4C members as raw `Color(n)` streams, its second UV set as `TexCoord(1)` and
/// its second vertex colour as `Color(9)`, all listed by `fromsoft.layer_inputs` (a mesh that stores no second UV set or
/// colour gets no such stream).
///
/// Every field is the program's choice, stated (no default): `pose`, which model is lowered (the posed one
/// `fromsoft-apply-rigid-skinning` made, or the stored one); `uv_tiling` (multiply UV0 by the material's tiling,
/// recorded as `fromsoft.uv_tiling`, or keep it); `winding` (reverse a part whose stored normals oppose its winding on
/// balance, or keep it); `degenerates` (drop triangles of doubled area squared at most 1e-14, or keep them);
/// `leave_out`, material source classes whose meshes are not lowered; `blocker_cards` (record the level-0 parts of
/// interior blocker cards in `fromsoft.blocker_cards` with every placement of their prototype, or draw them). A
/// prototype left drawing nothing at any level is despawned with its placements, counted. Options, all required:
/// `pose=rigid|stored`, `uv-tiling=keep|apply`, `winding=stored|normals`, `degenerates=keep|drop`, `leave-out=`
/// classes joined by `+` (`sky`, `light-volume`, `shadow`; empty: none), `blocker-cards=draw|record`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerFromSoftGeometry {
    pub pose: Pose,
    pub uv_tiling: Tiling,
    pub winding: Winding,
    pub degenerates: Degenerates,
    pub leave_out: BTreeSet<materials::SkipClass>,
    pub blocker_cards: Cards,
}

impl LowerFromSoftGeometry {
    /// From its options, every one required.
    pub fn from_options(o: &Options) -> Result<Self, String> {
        const KEYS: [&str; 6] = [
            "pose",
            "uv-tiling",
            "winding",
            "degenerates",
            "leave-out",
            "blocker-cards",
        ];
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| {
            o.get(k).map(String::as_str).ok_or_else(|| {
                format!(
                    "{k} is required: pose=rigid|stored, uv-tiling=keep|apply, winding=stored|normals, \
                     degenerates=keep|drop, leave-out= (sky+light-volume+shadow, empty for none), \
                     blocker-cards=draw|record"
                )
            })
        };
        let mut leave_out = BTreeSet::new();
        for class in get("leave-out")?.split('+').filter(|c| !c.is_empty()) {
            leave_out.insert(
                materials::SkipClass::from_word(class)
                    .ok_or_else(|| format!("leave-out class {class} is not sky, light-volume or shadow"))?,
            );
        }
        Ok(LowerFromSoftGeometry {
            pose: Pose::parse("pose", get("pose")?)?,
            uv_tiling: Tiling::parse("uv-tiling", get("uv-tiling")?)?,
            winding: Winding::parse("winding", get("winding")?)?,
            degenerates: Degenerates::parse("degenerates", get("degenerates")?)?,
            leave_out,
            blocker_cards: Cards::parse("blocker-cards", get("blocker-cards")?)?,
        })
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
    inputs: Vec<Vec<Semantic>>,
    cull: Vec<bool>,
    /// Per primitive, its influences when its prototype keeps a skin.
    skin: Vec<Option<Influences>>,
}

impl Level {
    /// Its primitives on `e` as a mesh, with their face-set cull, any layer inputs, and its skin (bound by `palette`:
    /// the skeleton and its slots) when its primitives have influences.
    fn insert(self, world: &mut World, e: Entity, palette: Option<&(Entity, Vec<Slot>)>) {
        if let Some((skeleton, slots)) = palette
            && self.skin.iter().any(Option::is_some)
        {
            world.insert(
                e,
                Skin {
                    bindings: vec![Binding {
                        id: BindingId(0),
                        skeleton: *skeleton,
                        palettes: self.skin.iter().map(|i| i.map(|_| slots.clone())).collect(),
                        deformation: Deformation::Linear,
                    }],
                    influences: self.skin,
                    weights: WeightRule::AsStored,
                },
            );
        }
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
    /// The skeleton its skin binds to and every joint's slot, when it keeps one.
    palette: Option<(Entity, Vec<Slot>)>,
}

/// Every joint of the skeleton held by `skeleton` as a palette slot, its inverse bind the inverse of the joint's rest
/// in the skeleton's space.
fn skeleton_palette(world: &World, skeleton: Entity) -> Result<(Entity, Vec<Slot>), String> {
    let s = world
        .get::<Skeleton>(skeleton)
        .ok_or_else(|| format!("{skeleton:?} holds no core.skeleton"))?;
    let rests = primase_animation::pose::model_transforms(s, &primase_animation::pose::locals(s, None))
        .map_err(|e| e.0)?;
    let slots = s
        .joints
        .iter()
        .map(|j| {
            let inverse = primase_animation::pose::inverse(&rests[&j.id])
                .ok_or_else(|| format!("joint {:?}'s rest has no inverse", j.id))?;
            Ok(Slot {
                joint: j.id,
                inverse_bind: primase_animation::pose::transform(&inverse),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((skeleton, slots))
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
            part.apply_uv_scale(crate::preview::preview(material).uv_scale)
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
        if self.pose == Pose::Stored
            && let Some(s) = world.get::<SkeletonOf>(prototype)
        {
            out.palette = Some(skeleton_palette(world, s.0)?);
        }
        let wide = model.bones.len() > 256;
        let mut mesh_levels = Vec::new();
        for (index, mesh) in model.meshes.iter().enumerate() {
            if out.palette.is_some() && (!mesh.palette.is_empty() || !mesh.dynamic) {
                return Err(format!(
                    "{prototype:?}: mesh {index} is {} in a model bound to a skeleton: not established for these files",
                    if mesh.dynamic {
                        "palette-relative"
                    } else {
                        "static (vertices in their bone's space)"
                    }
                ));
            }
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
            // a layered mesh keeps what its programs read (the Byte4C members, the second UV set and the second
            // colour: `layer_streams`); what is left of its colours and UV sets is not lowered
            let layout = if material.layered.is_some() {
                crate::sekiro::source::LayerLayout::of(model, index).map_err(|e| e.to_string())?
            } else {
                Default::default()
            };
            let kept_colours = usize::from(layout.second_colour.is_some());
            let kept_uvs = layout.byte4.len() + usize::from(layout.second_uv.is_some());
            if mesh.vertices.iter().any(|v| v.colors.len() > kept_colours) {
                log.approximate(
                    "meshes with vertex colours not lowered (their shader meaning is unknown)",
                    1,
                );
            }
            if mesh.vertices.iter().any(|v| v.uvs.len() > 1 + kept_uvs) {
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
                    crate::sekiro::source::layer_streams(model, index, &part.source_vertices)
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
                let members = layer.iter().map(|(semantic, _)| *semantic).collect();
                streams.extend(layer);
                let skinned = out.palette.is_some();
                if skinned {
                    let vertex = |k: &u32| &mesh.vertices[*k as usize];
                    let joints = part.source_vertices.iter().map(|k| vertex(k).bone_indices);
                    streams.insert(
                        Semantic::Joints(0),
                        if wide {
                            Stream::U16x4(joints.collect())
                        } else {
                            Stream::U8x4(joints.map(|j| j.map(|x| x as u8)).collect())
                        },
                    );
                    streams.insert(
                        Semantic::Weights(0),
                        Stream::F32x4(
                            part.source_vertices
                                .iter()
                                .map(|k| vertex(k).bone_weights)
                                .collect(),
                        ),
                    );
                }
                let out = out.levels.entry(level).or_default();
                out.skin.push(skinned.then_some(Influences::Streams { sets: 1 }));
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
        let leave_out: Vec<&str> = self.leave_out.iter().map(|c| c.word()).collect();
        Options::from([
            ("pose".to_owned(), self.pose.word().to_owned()),
            ("uv-tiling".to_owned(), self.uv_tiling.word().to_owned()),
            ("winding".to_owned(), self.winding.word().to_owned()),
            ("degenerates".to_owned(), self.degenerates.word().to_owned()),
            ("leave-out".to_owned(), leave_out.join("+")),
            ("blocker-cards".to_owned(), self.blocker_cards.word().to_owned()),
        ])
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
            Skin::NAME,
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
        "motion-blur face sets, vertex colours and UV sets not lowered but those a layered material's programs read; with winding=normals, a part's winding inferred from its stored normals"
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
                let scale =
                    crate::preview::preview(world.get::<Material>(e).expect("lowered material")).uv_scale;
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
            let palette = out.palette.take();
            let mut forms = Vec::new();
            for (level, lowered) in out.levels {
                let form = world.spawn();
                lowered.insert(world, form, palette.as_ref());
                forms.push(LodForm { detail: level, form });
            }
            finest.insert(world, prototype, palette.as_ref());
            if palette.is_some() {
                log.count("prototypes lowered with their skin", 1);
            }
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
