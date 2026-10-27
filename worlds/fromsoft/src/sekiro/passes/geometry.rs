//! `lower-fromsoft-geometry`: the prototypes' models as core meshes, with the choices it takes as typed options.
//!
//! A prototype bound to a skeleton (`fromsoft.skeleton`, the character reader's) lowered with `pose=stored` keeps its
//! skin: each primitive of a dynamic mesh gets `Joints(0)` (the bones its vertices name, as stored: the FLVER's bone
//! indices, which index the skeleton directly when the mesh has no palette) and `Weights(0)` (as stored), and each form
//! a `core.skin` binding them to that skeleton, every joint a slot with the inverse of its rest in the skeleton's space
//! as its inverse bind (dynamic vertices are stored in that bind pose), the weights
//! `EngineNormalizes { floor: `[`SKIN_WEIGHT_FLOOR`]` }` (the game's rule, below) and the deformation linear. A
//! static mesh (`dynamic` 0) of a bound model has each vertex in the space of the one bone its NORMAL.w names, which
//! moves it rigidly (SoulsFormats' FLVER2 mesh and Smithbox's FLVER renderer state the same rule): it is posed into
//! the bind pose by that bone's rest, as `fromsoft-apply-rigid-skinning` poses it
//! ([`crate::passes::rigid::pose_static_mesh`]), and each vertex gets `Joints(0)` that bone and `Weights(0)` 1, so the
//! skin moves it with the bone exactly. A static mesh whose layout stores no normal W follows its node (`NodeIndex`)
//! the same way. Refused, by name: a dynamic mesh with a palette (palette-relative indices), a static mesh with a
//! palette (its NORMAL.w would name a palette entry), one with no normal W whose node names no bone, and a static
//! vertex whose NORMAL.w names no bone or that states other weights than one of 1: none of them is established for
//! these files.
use super::blockers::BlockerCards;
use crate::components::models::{RigidModel, SkeletonOf, SourceModel};
use crate::flver::{FaceSetKind, Model, Part};
use crate::materials::{self, Material};
use crate::pose::{StaticBones, static_bones};
use crate::sekiro::components::lowered::{SourceMeshes, UvTiling};
use crate::sekiro::source::{
    FaceCull, LayerInputs, MaterialShader, MaterialSlots, MeshMasks, PrototypeSource, ShaderBundle,
    ShaderStage,
};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::world::Resource;
use primase_core::{
    Asset, Binding, BindingId, Component, Deformation, Entity, Influences, Instance, LodForm, Lods, Mesh,
    Primitive, Semantic, Skeleton, Skin, Slot, Source, Stream, Topology, Transform, WeightRule, World,
};
use std::collections::{BTreeMap, BTreeSet};

/// The lower bound of the divisor the games' skinning vertex programs divide the four weights of a vertex by. Read
/// from the programs themselves (disassembled): Elden Ring's `C[DetailBlend]_Fwd_Skin.vpo` (Shader Model 6, `dxc
/// -dumpbin`) computes `dot4(1, 1, 1, 1, weights)`, `FMax(0x3F50624DE0000000, sum)` (0.001) and divides each weight by
/// it, and so does its `_Gbuf_Skin` program; Sekiro's `Character_AMSN_[DetailBlend]_Gbuf_Skin.vpo` (Shader Model 5,
/// `fxc -dumpbin`) does the same in `dp4 r0.x, l(1, 1, 1, 1), v4.xyzw` / `max r0.x, r0.x, l(0.001)` / `div r2.xyzw,
/// v4.xyzw, r0.xxxx`. A vertex whose four weights are all zero is left with the zero matrix (Elden Ring's c3000 has 743).
pub const SKIN_WEIGHT_FLOOR: f32 = 0.001;

/// A two-way choice of `lower-fromsoft-geometry`, a [`Choice`] of its option words.
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
        impl Choice for $name {
            const VALUES: &'static [(&'static str, Self)] = &[($wa, Self::$a), ($wb, Self::$b)];
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
    /// Which members give a SpeedTree mesh its normal and tangent (`speedtree-frame=`).
    SpeedTreeFrame {
        /// Those its material's G-buffer vertex program reads ([`program_frame`]): what the game draws with.
        Program = "program",
        /// Its layout's two `UByte4Norm` members, as the model was authored ([`Model::speedtree_frame`]).
        Authored = "authored",
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
/// prototype left drawing nothing at any level is despawned with its placements, counted. `speedtree_frame`: a SpeedTree
/// mesh's normal and tangent from the members its material's program reads, or from its layout's pair (they differ for
/// three meshes of Elden Ring's, counted). Options, all required: `pose=rigid|stored`, `uv-tiling=keep|apply`,
/// `winding=stored|normals`, `degenerates=keep|drop`, `leave-out=` classes joined by `+` (`sky`, `light-volume`,
/// `shadow`, `billboard`; empty: none), `blocker-cards=draw|record`, `speedtree-frame=program|authored`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerFromSoftGeometry {
    pub pose: Pose,
    pub uv_tiling: Tiling,
    pub winding: Winding,
    pub degenerates: Degenerates,
    pub leave_out: BTreeSet<materials::SkipClass>,
    pub blocker_cards: Cards,
    pub speedtree_frame: SpeedTreeFrame,
}

impl LowerFromSoftGeometry {
    /// From its declared options.
    fn from_given(g: &Given) -> Result<Self, String> {
        let mut leave_out = BTreeSet::new();
        for class in g.required("leave-out").split('+').filter(|c| !c.is_empty()) {
            leave_out.insert(materials::SkipClass::from_word(class).ok_or_else(|| {
                format!("leave-out class {class} is not sky, light-volume, shadow or billboard")
            })?);
        }
        Ok(LowerFromSoftGeometry {
            pose: g.choice("pose"),
            uv_tiling: g.choice("uv-tiling"),
            winding: g.choice("winding"),
            degenerates: g.choice("degenerates"),
            leave_out,
            blocker_cards: g.choice("blocker-cards"),
            speedtree_frame: g.choice("speedtree-frame"),
        })
    }

    fn input(&self) -> &'static str {
        match self.pose {
            Pose::Rigid => RigidModel::NAME,
            Pose::Stored => SourceModel::NAME,
        }
    }
}

/// `lower-fromsoft-geometry`'s declaration ([`primase_core::pass::PassRegistry::add`]): every choice is the
/// program's, stated.
pub const LOWER_FROMSOFT_GEOMETRY: PassSpec = PassSpec {
    name: "lower-fromsoft-geometry",
    options: &[
        OptionSpec::choice::<Pose>(
            "pose",
            "the posed model fromsoft-apply-rigid-skinning made, or the stored one",
        ),
        OptionSpec::choice::<Tiling>(
            "uv-tiling",
            "UV0 kept, or multiplied by the material's tiling (recorded as fromsoft.uv_tiling)",
        ),
        OptionSpec::choice::<Winding>(
            "winding",
            "a part's winding as stored, or reversed where its stored normals oppose it on balance",
        ),
        OptionSpec::choice::<Degenerates>(
            "degenerates",
            "triangles of doubled area squared at most 1e-14 kept, or dropped (counted)",
        ),
        OptionSpec::required(
            "leave-out",
            "sky+light-volume+shadow+billboard",
            "material source classes whose meshes are not lowered, joined by + (sky, light-volume, shadow, \
             billboard; empty for none)",
        ),
        OptionSpec::choice::<Cards>(
            "blocker-cards",
            "interior blocker cards drawn, or their level-0 parts recorded in fromsoft.blocker_cards",
        ),
        OptionSpec::choice::<SpeedTreeFrame>(
            "speedtree-frame",
            "a SpeedTree mesh's normal and tangent from the members its program reads, or its layout's pair",
        ),
    ],
    make: |g| Ok(Box::new(LowerFromSoftGeometry::from_given(g)?)),
};

/// The input indices (`TEXCOORD<n>`) of the normal and the tangent a SpeedTree material's G-buffer vertex program
/// (`<shader>_Gbuf.vpo` of its bundle) reads: its last two `TEXCOORD` inputs, which each of Elden Ring's SpeedTree
/// programs unpacks (`x * 2 - 1`) and outputs as the normal and the tangent (`tests/speedtree.rs` runs every one on its
/// meshes' vertices).
pub fn program_frame(bundle: &ShaderBundle) -> Result<[u32; 2], String> {
    let name = format!("{}_Gbuf.vpo", bundle.name);
    let program = bundle
        .program(&name)
        .filter(|p| p.stage == ShaderStage::Vertex)
        .ok_or_else(|| format!("{}: no vertex program {name}", bundle.name))?;
    if program.format != iridia_interface::ShaderFormat::Dxil {
        return Err(format!(
            "{name}: a SpeedTree program read as DXIL, this one is not"
        ));
    }
    let interface = iridia_dxil::interface(&program.code).map_err(|e| format!("{name}: {e}"))?;
    let mut texcoords: Vec<u32> = interface
        .inputs
        .iter()
        .filter(|i| i.label.name == "TEXCOORD")
        .map(|i| i.label.index)
        .collect();
    texcoords.sort_unstable();
    match texcoords[..] {
        [.., normal, tangent] => Ok([normal, tangent]),
        _ => Err(format!("{name}: fewer than two TEXCOORD inputs")),
    }
}

/// One level of a prototype's lowered geometry: its primitives, each one's layer-input members and face-set cull.
#[derive(Default)]
struct Level {
    primitives: Vec<Primitive>,
    inputs: Vec<Vec<Semantic>>,
    cull: Vec<bool>,
    /// Per primitive, the FLVER mesh it was lowered from.
    source_mesh: Vec<u32>,
    /// Per primitive, its influences when its prototype keeps a skin.
    skin: Vec<Option<Influences>>,
}

impl Level {
    /// Its primitives on `e` as a mesh, with their face-set cull, any layer inputs, their FLVER meshes when `sources`
    /// (a prototype whose meshes carry display mask marks), and its skin (bound by `palette`: the skeleton and its
    /// slots) when its primitives have influences.
    fn insert(self, world: &mut World, e: Entity, palette: Option<&(Entity, Vec<Slot>)>, sources: bool) {
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
                    weights: WeightRule::EngineNormalizes {
                        floor: SKIN_WEIGHT_FLOOR,
                    },
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
        if sources {
            world.insert(e, SourceMeshes(self.source_mesh));
        }
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
    /// The joints (by index) whose rest has no inverse, which hold the identity as their inverse bind: no vertex may
    /// weight them.
    unbound: Vec<usize>,
}

/// Every joint of the skeleton held by `skeleton` as a palette slot, its inverse bind the inverse of the joint's rest
/// in the skeleton's space; the joints whose rest has no inverse (a bone scaled to nothing: the movement-sampling
/// helpers at the root of Elden Ring's `c8000`) hold the identity, and are listed by index in `unbound`: nothing may
/// weight them.
fn skeleton_palette(
    world: &World,
    skeleton: Entity,
    unbound: &mut Vec<usize>,
) -> Result<(Entity, Vec<Slot>), String> {
    let s = world
        .get::<Skeleton>(skeleton)
        .ok_or_else(|| format!("{skeleton:?} holds no core.skeleton"))?;
    let rests = primase_animation::pose::model_transforms(s, &primase_animation::pose::locals(s, None))
        .map_err(|e| e.0)?;
    let slots = s
        .joints
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let inverse = primase_animation::pose::inverse(&rests[&j.id]).unwrap_or_else(|| {
                unbound.push(i);
                primase_animation::pose::affine(&Transform::IDENTITY)
            });
            Slot {
                joint: j.id,
                inverse_bind: primase_animation::pose::transform(&inverse),
            }
        })
        .collect::<Vec<_>>();
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

    /// A SpeedTree model whose meshes' programs read another normal and tangent than their layouts' pairs, given
    /// those (`frames`: each shader bundle's [`program_frame`], read once); `None` where nothing changes.
    fn program_frames(
        &self,
        world: &World,
        model: &Model,
        slots: &[Entity],
        frames: &mut BTreeMap<Entity, [u32; 2]>,
        log: &mut PassLog,
    ) -> Result<Option<Model>, String> {
        if self.speedtree_frame == SpeedTreeFrame::Authored || !model.speedtree {
            return Ok(None);
        }
        let mut changed: Option<Model> = None;
        for (index, mesh) in model.meshes.iter().enumerate() {
            let Some(authored) = model.speedtree_frame(index).map_err(|e| e.to_string())? else {
                continue;
            };
            let entity = *slots
                .get(mesh.material)
                .ok_or_else(|| format!("mesh {index} material slot {}", mesh.material))?;
            let bundle = world
                .get::<MaterialShader>(entity)
                .ok_or_else(|| {
                    format!(
                        "SpeedTree mesh {index}: its material has no shader bundle, so the members its program reads \
                         as its normal and tangent are unknown (speedtree-frame=authored takes its layout's)"
                    )
                })?
                .0;
            let frame = match frames.get(&bundle) {
                Some(f) => *f,
                None => {
                    let b = world
                        .get::<ShaderBundle>(bundle)
                        .ok_or_else(|| format!("{bundle:?} holds no fromsoft.shader_bundle"))?;
                    let f = program_frame(b)?;
                    frames.insert(bundle, f);
                    f
                }
            };
            if frame != authored {
                changed
                    .get_or_insert_with(|| model.clone())
                    .set_speedtree_frame(index, frame)
                    .map_err(|e| e.to_string())?;
                log.count(
                    "SpeedTree meshes whose program reads another normal and tangent than its layout's pair",
                    1,
                );
            }
        }
        Ok(changed)
    }

    fn lower(
        &self,
        world: &World,
        prototype: Entity,
        frames: &mut BTreeMap<Entity, [u32; 2]>,
        log: &mut PassLog,
    ) -> Result<Lowered, String> {
        let model = match self.pose {
            Pose::Rigid => world.get::<RigidModel>(prototype).map(|m| &m.0),
            Pose::Stored => world.get::<SourceModel>(prototype).map(|m| &m.0),
        }
        .ok_or_else(|| format!("{prototype:?}: no {}", self.input()))?;
        let slots = &world
            .get::<MaterialSlots>(prototype)
            .ok_or_else(|| format!("{prototype:?}: no fromsoft.material_slots"))?
            .0;
        let reframed = self
            .program_frames(world, model, slots, frames, log)
            .map_err(|e| format!("{prototype:?}: {e}"))?;
        let model = reframed.as_ref().unwrap_or(model);
        let mut out = Lowered::default();
        if self.pose == Pose::Stored
            && let Some(s) = world.get::<SkeletonOf>(prototype)
        {
            out.palette = Some(skeleton_palette(world, s.0, &mut out.unbound)?);
            if !out.unbound.is_empty() {
                log.count(
                    "joints whose rest has no inverse (a bone scaled to nothing), holding the identity as inverse bind: no vertex weights them",
                    out.unbound.len() as u64,
                );
            }
        }
        let posed;
        let mut static_follow = vec![None; model.meshes.len()];
        let model = if out.palette.is_some() && model.meshes.iter().any(|m| !m.dynamic) {
            let (bound, follow) =
                bind_static_meshes(model, log).map_err(|e| format!("{prototype:?}: {e}"))?;
            posed = bound;
            static_follow = follow;
            &posed
        } else {
            model
        };
        let wide = model.bones.len() > 256;
        let mut mesh_levels = Vec::new();
        for (index, mesh) in model.meshes.iter().enumerate() {
            if out.palette.is_some() && mesh.dynamic && !mesh.palette.is_empty() {
                return Err(format!(
                    "{prototype:?}: mesh {index} is palette-relative in a model bound to a skeleton: not established for these files"
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
            // a material binary's mesh (Elden Ring's) keeps every UV set and its first vertex colour: the programs the
            // material is bound to read them (`source_streams`)
            let source_streams_kept =
                matches!(material.definition, Some(crate::materials::Definition::Matbin(_)));
            let kept_colours = usize::from(layout.second_colour.is_some() || source_streams_kept);
            let kept_uvs = layout.byte4.len() + usize::from(layout.second_uv.is_some());
            if mesh.vertices.iter().any(|v| v.colors.len() > kept_colours) {
                log.approximate(
                    "meshes with vertex colours not lowered (their shader meaning is unknown)",
                    1,
                );
            }
            if !source_streams_kept && mesh.vertices.iter().any(|v| v.uvs.len() > 1 + kept_uvs) {
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
                    streams.insert(Semantic::Tangent(0), Stream::F32x4(part.tangents));
                }
                streams.extend(
                    part.further_tangents
                        .into_iter()
                        .map(|(set, values)| (Semantic::Tangent(set), Stream::F32x4(values))),
                );
                let members = layer.iter().map(|(semantic, _)| *semantic).collect();
                streams.extend(layer);
                if source_streams_kept {
                    streams.extend(source_streams(&mesh.vertices, &part.source_vertices));
                }
                let skinned = out.palette.is_some();
                if skinned {
                    // a dynamic vertex's influences as stored; a static one's the bone it follows, at 1
                    // (`bind_static_meshes` made sure it names one)
                    let influences = |k: &u32| {
                        let v = &mesh.vertices[*k as usize];
                        match static_follow[index].and_then(|b| b.of(v)) {
                            Some(bone) => ([bone as u16, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
                            None => (v.bone_indices, v.bone_weights),
                        }
                    };
                    if let Some(bad) = part.source_vertices.iter().find_map(|k| {
                        let (joints, weights) = influences(k);
                        joints
                            .iter()
                            .zip(weights)
                            .find(|&(&j, w)| w > 0.0 && out.unbound.contains(&(j as usize)))
                            .map(|(&j, _)| j)
                    }) {
                        return Err(format!(
                            "{prototype:?}: a vertex of mesh {index} is weighted to joint {bad}, whose rest has no inverse"
                        ));
                    }
                    let joints = part.source_vertices.iter().map(|k| influences(k).0);
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
                        Stream::F32x4(part.source_vertices.iter().map(|k| influences(k).1).collect()),
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
                out.source_mesh.push(index as u32);
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

/// `model` with its static meshes posed into the bind pose by the bone each vertex follows (see the module
/// documentation), for a model bound to a skeleton, and those bones per mesh (`None` for a dynamic one): a static mesh
/// with a palette, one with no normal W whose node names no bone, a static vertex whose NORMAL.w names no bone and one
/// that states other weights than one of 1 are refused.
fn bind_static_meshes(model: &Model, log: &mut PassLog) -> Result<(Model, Vec<Option<StaticBones>>), String> {
    let worlds = crate::pose::bone_matrices(&model.bones)?;
    let mut posed = model.clone();
    let mut bones = vec![None; model.meshes.len()];
    for (index, mesh) in posed.meshes.iter_mut().enumerate() {
        if mesh.dynamic {
            continue;
        }
        if !mesh.palette.is_empty() {
            return Err(format!(
                "mesh {index} is static with a bone palette in a model bound to a skeleton (its NORMAL.w would name a palette entry): not established for these files"
            ));
        }
        let follows = static_bones(model, index);
        if let StaticBones::Unbound(node) = follows {
            return Err(format!(
                "mesh {index} is static, stores no normal W and its NodeIndex {node} names no bone of the {}: a skin needs one",
                model.bones.len()
            ));
        }
        for v in &mesh.vertices {
            if follows.of(v).is_none_or(|b| b >= worlds.len()) {
                return Err(format!(
                    "mesh {index} is static and a vertex's NORMAL.w {} names no bone of the {}",
                    v.normal_w,
                    worlds.len()
                ));
            }
            let weights: Vec<f32> = v.bone_weights.iter().copied().filter(|&w| w != 0.0).collect();
            if !weights.is_empty() && weights != [1.0] {
                return Err(format!(
                    "mesh {index} is static and a vertex states the weights {:?}: a static vertex follows one bone at 1",
                    v.bone_weights
                ));
            }
        }
        crate::passes::rigid::pose_static_mesh(mesh, follows, &worlds, log)?;
        log.count(
            match follows {
                StaticBones::PerVertex => {
                    "static meshes of a bound model bound rigidly to the bone their vertices' NORMAL.w names"
                }
                StaticBones::Node(_) => {
                    "static meshes of a bound model bound rigidly to their node (no normal W)"
                }
                StaticBones::Unbound(_) => unreachable!("refused above"),
            },
            1,
        );
        bones[index] = Some(follows);
    }
    Ok((posed, bones))
}

/// The UV sets beyond the first, as `TexCoord(n)` in the order the vertex layout stores them (a four component UV member
/// is two sets), and the first vertex colour as `Color(0)`, of the vertices `vertices` of a mesh: what a material
/// binary's programs read besides the first UV set. A set or colour the mesh's vertices do not all have is not kept.
fn source_streams(mesh: &[crate::flver::Vertex], vertices: &[u32]) -> Vec<(Semantic, Stream)> {
    let at = |v: &u32| &mesh[*v as usize];
    let sets = vertices.iter().map(|v| at(v).uvs.len()).min().unwrap_or(0);
    let mut out = Vec::new();
    for set in 1..sets.min(usize::from(u8::MAX)) {
        out.push((
            Semantic::TexCoord(set as u8),
            Stream::F32x2(
                vertices
                    .iter()
                    .map(|v| [at(v).uvs[set][0], at(v).uvs[set][1]])
                    .collect(),
            ),
        ));
    }
    if vertices.iter().all(|v| !at(v).colors.is_empty()) {
        out.push((
            Semantic::Color(0),
            Stream::F32x4(vertices.iter().map(|v| at(v).colors[0]).collect()),
        ));
    }
    out
}

impl Pass for LowerFromSoftGeometry {
    fn name(&self) -> &'static str {
        "lower-fromsoft-geometry"
    }
    fn options(&self) -> Options {
        let leave_out: Vec<&str> = self.leave_out.iter().map(|c| c.word()).collect();
        Options::from([
            ("pose".to_owned(), self.pose.text().to_owned()),
            ("uv-tiling".to_owned(), self.uv_tiling.text().to_owned()),
            ("winding".to_owned(), self.winding.text().to_owned()),
            ("degenerates".to_owned(), self.degenerates.text().to_owned()),
            ("leave-out".to_owned(), leave_out.join("+")),
            ("blocker-cards".to_owned(), self.blocker_cards.text().to_owned()),
            (
                "speedtree-frame".to_owned(),
                self.speedtree_frame.text().to_owned(),
            ),
        ])
    }
    /// A read may hold no model to lower (an area of collision alone, or whose only models were the distant backdrop
    /// `drop-fromsoft-far` left out): nothing to lower, the step recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
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
            SourceMeshes::NAME,
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
    /// The blocker cards it records, and the assets that list a prototype left drawing nothing (it is despawned, and
    /// they stop listing it).
    fn modifies(&self) -> &[&'static str] {
        &[BlockerCards::NAME, Asset::NAME]
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
        let mut frames = BTreeMap::new();
        for &prototype in &prototypes {
            if world.has::<Mesh>(prototype) {
                return Err(format!("{prototype:?} already has core.mesh"));
            }
            lowered.push(self.lower(world, prototype, &mut frames, log)?);
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
            let sources = world.has::<MeshMasks>(prototype);
            let mut forms = Vec::new();
            for (level, lowered) in out.levels {
                let form = world.spawn();
                lowered.insert(world, form, palette.as_ref(), sources);
                forms.push(LodForm { detail: level, form });
            }
            finest.insert(world, prototype, palette.as_ref(), sources);
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
