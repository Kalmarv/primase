//! The passes a program runs on an area the reader assembled. The reader keeps the source; each of these interprets
//! part of it, states what it approximates and logs its counts. Choices that change what is produced (which face
//! sets, which material classes, repairs) are options with the faithful reading as the default.
use super::blockers::{self, BlockerCards};
use super::source::{LayerInputs, MaterialSlots, PrototypeSource};
use crate::flver::{FaceSetKind, Model, Part};
use crate::materials::{self, Material};
use crate::rigid::{ApplyRigidSkinning, RigidModel, SourceModel};
use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::world::Resource;
use primase_core::{
    Component, Entity, Instance, LodForm, Lods, Mesh, Primitive, Semantic, Source, Stream, Topology,
    Transform, World,
};
use std::collections::{BTreeMap, BTreeSet};

/// Which model `lower-fromsoft-geometry` lowers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pose {
    /// `fromsoft.rigid_model`: static meshes posed by their bones (`fromsoft-apply-rigid-skinning` first).
    #[default]
    Rigid,
    /// `fromsoft.flver` as stored: static vertices left in their bones' spaces.
    Stored,
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
/// Options, each a choice the default leaves faithful: `pose=rigid|stored`; `uv-tiling=keep|apply` (multiply UV0 by
/// the material's tiling, recorded as `fromsoft.uv_tiling`); `winding=stored|normals` (reverse a part whose stored
/// normals oppose its winding on balance); `degenerates=keep|drop` (triangles of doubled area squared at most
/// 1e-14); `leave-out=` classes joined by `+` (`sky`, `light-volume`, `shadow`: meshes of materials of that source
/// class are not lowered); `blocker-cards=draw|record` (record the level-0 parts of interior blocker cards in
/// `fromsoft.blocker_cards` with every placement of their prototype, instead of drawing them). A prototype left
/// drawing nothing at any level is despawned with its placements, counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LowerFromSoftGeometry {
    pub pose: Pose,
    pub apply_uv_tiling: bool,
    pub repair_winding: bool,
    pub drop_degenerates: bool,
    /// Material skip classes left out, by their [`materials::skip_class`] word.
    pub leave_out: BTreeSet<String>,
    pub record_blocker_cards: bool,
}

fn choice(key: &str, value: &str, options: [&str; 2]) -> Result<bool, String> {
    match value {
        v if v == options[0] => Ok(false),
        v if v == options[1] => Ok(true),
        _ => Err(format!("{key}={value} is not {} or {}", options[0], options[1])),
    }
}

impl LowerFromSoftGeometry {
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let mut pass = Self::default();
        for (k, v) in o {
            match k.as_str() {
                "pose" => {
                    pass.pose = if choice(k, v, ["rigid", "stored"])? {
                        Pose::Stored
                    } else {
                        Pose::Rigid
                    }
                }
                "uv-tiling" => pass.apply_uv_tiling = choice(k, v, ["keep", "apply"])?,
                "winding" => pass.repair_winding = choice(k, v, ["stored", "normals"])?,
                "degenerates" => pass.drop_degenerates = choice(k, v, ["keep", "drop"])?,
                "blocker-cards" => pass.record_blocker_cards = choice(k, v, ["draw", "record"])?,
                "leave-out" => {
                    for class in v.split('+').filter(|c| !c.is_empty()) {
                        if !materials::SKIP_CLASSES.iter().any(|(word, _)| *word == class) {
                            return Err(format!(
                                "leave-out class {class} is not sky, light-volume or shadow"
                            ));
                        }
                        pass.leave_out.insert(class.to_owned());
                    }
                }
                _ => return Err(format!("unknown option {k}")),
            }
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

/// One prototype's lowered geometry, before it goes into the world.
#[derive(Default)]
struct Lowered {
    /// Per level: primitives and their layer-input member lists.
    levels: BTreeMap<u8, (Vec<Primitive>, Vec<Vec<u8>>)>,
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
        if self.apply_uv_tiling {
            part.apply_uv_scale(material.uv_scale)
                .map_err(|e| e.to_string())?;
        }
        if self.repair_winding && part.repair_winding().map_err(|e| e.to_string())? {
            log.approximate("parts reversed to agree with their stored normals", 1);
        }
        if self.drop_degenerates {
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
            if let Some(class) = material.skip.as_deref().and_then(materials::skip_class)
                && self.leave_out.contains(class)
            {
                log.count(&format!("meshes left out: {class}"), 1);
                continue;
            }
            if self.record_blocker_cards && blockers::is_card(material) {
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
                let (primitives, inputs) = out.levels.entry(level).or_default();
                primitives.push(Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(part.mesh.indices),
                    material: Some(entity),
                });
                inputs.push(members);
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
        let mut o = Options::new();
        let mut set = |k: &str, on: bool, v: &str| {
            if on {
                o.insert(k.to_owned(), v.to_owned());
            }
        };
        set("pose", self.pose == Pose::Stored, "stored");
        set("uv-tiling", self.apply_uv_tiling, "apply");
        set("winding", self.repair_winding, "normals");
        set("degenerates", self.drop_degenerates, "drop");
        set("blocker-cards", self.record_blocker_cards, "record");
        if !self.leave_out.is_empty() {
            o.insert(
                "leave-out".into(),
                self.leave_out.iter().cloned().collect::<Vec<_>>().join("+"),
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
        if self.apply_uv_tiling {
            let used: BTreeSet<Entity> = lowered
                .iter()
                .flat_map(|l| {
                    l.levels
                        .values()
                        .flat_map(|(p, _)| p.iter().filter_map(|p| p.material))
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
            if out.levels.values().all(|(p, _)| p.is_empty()) {
                for p in placed {
                    world.despawn(p);
                    log.count("placements of prototypes left drawing nothing", 1);
                }
                world.despawn(prototype);
                log.count("prototypes left drawing nothing", 1);
                continue;
            }
            let (primitives, inputs) = out.levels.remove(&0).unwrap_or_default();
            let mut forms = Vec::new();
            for (level, (primitives, inputs)) in out.levels {
                let form = world.spawn();
                world.insert(form, Mesh::new(primitives));
                if inputs.iter().any(|i| !i.is_empty()) {
                    world.insert(form, LayerInputs { primitives: inputs });
                }
                forms.push(LodForm { detail: level, form });
            }
            world.insert(prototype, Mesh::new(primitives));
            if inputs.iter().any(|i| !i.is_empty()) {
                world.insert(prototype, LayerInputs { primitives: inputs });
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

/// `drop-fromsoft-far`: the placements of distant-backdrop prototypes (`fromsoft.prototype` `far`) and those
/// prototypes despawned, counted; nothing else changes.
#[derive(Clone, Copy, Debug, Default)]
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
            world.despawn(e);
            log.count("distant-backdrop placements dropped", 1);
        }
        for e in far {
            world.despawn(e);
            log.count("distant-backdrop prototypes dropped", 1);
        }
        Ok(())
    }
}

fn without_options<P: Pass + Default + 'static>(options: &Options) -> Result<Box<dyn Pass>, String> {
    if let Some(k) = options.keys().next() {
        return Err(format!("unknown option {k}"));
    }
    Ok(Box::new(P::default()))
}

/// Register the FromSoft area passes by name.
pub fn register(registry: &mut PassRegistry) {
    registry.register("fromsoft-apply-rigid-skinning", |o| {
        if let Some(k) = o.keys().next() {
            return Err(format!("unknown option {k}"));
        }
        Ok(Box::new(ApplyRigidSkinning))
    });
    registry.register("lower-fromsoft-geometry", |o| {
        Ok(Box::new(LowerFromSoftGeometry::from_options(o)?))
    });
    registry.register("drop-fromsoft-far", without_options::<DropFromSoftFar>);
    registry.register(
        "lower-fromsoft-extras",
        without_options::<super::extras::LowerFromSoftExtras>,
    );
    registry.register_requiring(
        "lower-fromsoft-lights",
        |o| Ok(Box::new(super::lights::LowerFromSoftLights::from_options(o)?)),
        &[("candela-per-power", "1")],
    );
}
