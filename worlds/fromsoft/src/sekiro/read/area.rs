//! Reading one FromSoftware map ([`read_area`] for Sekiro, [`read_game_area`] for any game): its placed models,
//! materials, texture files, layered-shader programs, collision files and bodies, lights, hang edges and spawn points,
//! selected and in the world frame, as source records. What differs between the games and is not stated by their files
//! (where a map keeps its files, how objects are found, tile parents) is [`crate::Game`]'s; what Elden Ring's read does
//! not cover yet (world states, materials without a definition) is in its notices.
use super::{animation, collision, draw_param, extras, objects, shaders, shadows, states, textures};
use crate::sekiro::source::{self, Region};
use crate::{
    Archive, Binder, Game, Install, Result,
    dcx::Decoder,
    flver::Model,
    invalid, materials,
    msb::{MapLayout, PartKind},
    mtd::MaterialDefinition,
};
use primase_core::{Instance, Name, Source, Transform, World};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The region the read keeps ([`Region`]: a rectangle and its margin, both the caller's): placements whose
    /// model's extent reaches the rectangle within the margin, lights and spawn points inside the rectangle. Collision
    /// files are read whole whatever the region (their crop is `reach` on the collision passes). None reads the whole
    /// area.
    pub region: Option<Region>,
    /// Which world states to bring in ([`states`]): none (default, every part present) or the event-flag rules of the
    /// block's script, as axes and conditions for `resolve-state`.
    pub states: StateClasses,
    /// Whether the larger maps above the map (Elden Ring's open-world tiles: [`crate::Game::parent_maps`]) also place
    /// their parts that belong to it: none (default: the map's own layout only) or included, moved into the map's frame.
    pub parents: Parents,
    /// The area's draw-param file ([`draw_param`]): none (default: the graphics environment is not read) or the file
    /// of this stem, one of [`draw_param::available`].
    pub draw_param: draw_param::DrawParams,
    /// Whether the animation binder of each kept object model (Sekiro's objects, Elden Ring's assets: doors, lifts) is
    /// read into the world beside the model: its Havok skeleton and clips, named after the model. Off by default: the
    /// binders are more to hold in memory, and the clips are not lowered onto the placements by the read.
    pub object_animations: bool,
}
/// What a read of the object models' animation binders found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectAnimations {
    /// Whether the binders were asked for ([`Options::object_animations`]).
    pub asked: bool,
    /// Kept object models that have an animation binder (a member of the model's binder, or one beside it).
    pub animated: usize,
    /// Kept object models read without one.
    pub still: usize,
    /// Havok skeleton bones and clips read, over all the binders.
    pub bones: usize,
    pub clips: usize,
    /// Binders that could not be read, as `<model>: <why>`.
    pub unreadable: Vec<String>,
    /// Files of the binders not read, by reason (the animation reader's own), with how many.
    pub refused: BTreeMap<String, usize>,
}
/// The grass the parts' configs name: the `GrassTypeParam` rows (Elden Ring's) and what they hold. The game scatters the
/// grass itself over the part by a row's density; no file holds a blade's place, so nothing is placed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrassTypes {
    /// Parts storing a grass config.
    pub parts: usize,
    /// Each row id the configs name (a negative id names none), and how many parts name it.
    pub named: BTreeMap<i32, usize>,
    /// The named rows the param holds.
    pub types: BTreeMap<i32, crate::param::GrassType>,
    /// Named rows the param does not hold.
    pub missing: Vec<i32>,
    /// Slots (a part names six) that name id 0 where the param holds no row 0: a slot that names none, not a row
    /// missing (Elden Ring's `m10_00` has 48217 of them).
    pub unset: usize,
    /// Why the param was not read, if it was not (no key, another layout).
    pub unresolved: Option<String>,
    /// The param's data version and row size as stored, when it was read.
    pub layout: Option<(u16, usize)>,
}
/// Whether a read brings in the parts the larger maps above it place for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Parents {
    /// The map's own layout only.
    #[default]
    None,
    /// Also the `<map>-<part>` parts of every larger map above it.
    Include,
}
/// The state axis classes a read produces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StateClasses {
    #[default]
    None,
    /// Event flags (`fromsoft.flag.<id>`) from the block's part-state common functions.
    Event,
}
/// What a read found and kept. Counts are source records (placements, prototypes, materials, texture files).
#[derive(Default, Debug)]
pub struct Report {
    /// The full area name read (`m11_01_00_00`).
    pub area: String,
    /// Selected placements' source shadow controls mapped to `core.shadows`
    /// (units: placements). Counts contradictory cast/only controls left unknown
    /// and names the renderer-specific controls kept only as `fromsoft.shadow_flags`.
    pub shadows: primase_core::pass::PassLog,
    pub extras: extras::Report,
    /// The placed map collision files read.
    pub collision: collision::Report,
    /// The kept object prototypes' collision files read.
    pub object_collision: objects::Report,
    pub prototypes: usize,
    /// Prototypes of the distant-backdrop series (`fromsoft.prototype` `far`), and their placements.
    pub far_prototypes: usize,
    pub far_placements: usize,
    pub placements: usize,
    pub materials: usize,
    /// Texture files kept as source payloads.
    pub textures: usize,
    /// Layered-shader programs kept (`fromsoft.shader_programs`).
    pub shader_programs: usize,
    /// Shader bundles kept (`fromsoft.shader_bundle`), and the programs in them by stage.
    pub shader_bundles: usize,
    pub bundle_programs: BTreeMap<source::ShaderStage, usize>,
    /// Shaders a material names whose bundle the install does not hold.
    pub missing_shader_bundles: Vec<String>,
    /// Bundles kept without their parameter definition: its metaparam is of a version the reader does not read.
    pub unread_metaparams: Vec<String>,
    /// Metaparam default textures the engine binds to slots a material leaves unbound that the install's catalog does
    /// not hold (their stems): such a slot falls to the bake's unbound policy.
    pub missing_default_textures: Vec<String>,
    /// Distinct placed models (`obj/<model>`, `asset/<model>`, `<map>/<model>`) the archive does not hold.
    pub missing_models: Vec<String>,
    /// Each prototype's box as the region rule took it ([`world_bounds`]; `None` for a model that uses no vertex), by
    /// its entity: what [`convert::AreaReader`] gives as the prototype's box.
    pub prototype_boxes: BTreeMap<primase_core::Entity, Option<[[f32; 3]; 2]>>,
    /// The larger maps whose parts for this map were read (`parents: include`), and those parts.
    pub parent_maps: Vec<String>,
    pub parent_placements: usize,
    /// Parts of the layout of a kind the read does not place (enemies, players, dummy objects and assets, connect
    /// collisions), by the word for the kind: they stand in the MSB and in no world. The collision parts are read as
    /// the collision files they name ([`collision`]).
    pub unplaced_parts: BTreeMap<&'static str, usize>,
    /// The draw-param file and what the read chose and left of it (`fromsoft.draw_param`; empty unless asked for).
    pub draw_param: draw_param::Report,
    /// Materials whose definitions the read does not resolve yet (Elden Ring's material binaries): they keep their
    /// FLVER bindings only, and no texture is read for them.
    pub undefined_materials: usize,
    /// Texture packages read whole because a material names one of their textures (Elden Ring's asset packages).
    pub texture_packages: usize,
    /// With `states: Event`: the rules read and the parts decided or left at their default.
    pub states: Option<states::Report>,
    /// Placements of the models in `missing_models`.
    pub skipped_missing: usize,
    /// Placements of read models whose face sets use no vertex.
    pub skipped_empty: usize,
    /// Placements outside the region and its margin.
    pub skipped_outside: usize,
    /// Material slots of the kept prototypes, by source shader.
    pub shader_parts: BTreeMap<String, usize>,
    /// Material slots of the kept prototypes drawn as surfaces, by the class their shader family states (`opaque`,
    /// `cutout`, `alpha`, `additive`, `leaves`, `decal`, `water`): the classes `lower-fromsoft-materials` turns into a
    /// material usage or a draw order. A slot of a class the game draws as something else (sky, a light volume, a
    /// shadow proxy) is in no count here.
    pub material_classes: BTreeMap<String, usize>,
    /// Placements of objects (`obj/<model>`, `asset/<model>`): drawn at their model's rest pose.
    pub object_placements: usize,
    /// The object models' animation binders ([`Options::object_animations`]).
    pub object_animation: ObjectAnimations,
    /// Parts of the layout that store a grass config (Elden Ring's): the grass the game grows on them is not read.
    pub grass_parts: usize,
    /// The grass types those parts name.
    pub grass: GrassTypes,
}
impl Report {
    /// What the read left out or could not find, as notices for a conversion's report.
    pub fn notices(&self) -> Vec<primase_core::convert::Notice> {
        use primase_core::convert::{Notice, NoticeKind};
        let stage = "fromsoft reader";
        let mut out = Vec::new();
        let listed = |reason: &str, items: &[String]| {
            items.iter().fold(
                Notice::new(stage, NoticeKind::Unsupported, reason, items.len() as u64),
                |n, i| n.example(i.clone()),
            )
        };
        if !self.missing_models.is_empty() {
            out.push(listed(
                "placed models not found in the archive",
                &self.missing_models,
            ));
        }
        if !self.unplaced_parts.is_empty() {
            out.push(self.unplaced_parts.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "map parts of kinds the read does not place (enemies, players, dummies, connect collisions)",
                    self.unplaced_parts.values().sum::<usize>() as u64,
                ),
                |n, (kind, count)| n.example(format!("{count} {kind}")),
            ));
        }
        if !self.missing_shader_bundles.is_empty() {
            out.push(listed(
                "shaders materials name whose bundle the install does not hold",
                &self.missing_shader_bundles,
            ));
        }
        if !self.unread_metaparams.is_empty() {
            out.push(listed(
                "shader bundles whose metaparam is of a version not read (programs kept, parameters not)",
                &self.unread_metaparams,
            ));
        }
        if !self.missing_default_textures.is_empty() {
            out.push(listed(
                "metaparam default named but not in the catalog",
                &self.missing_default_textures,
            ));
        }
        if !self.collision.missing_files.is_empty() {
            out.push(listed(
                "collision files named by the MSB but not in the area's binder",
                &self.collision.missing_files,
            ));
        }
        if !self.collision.unread_bodies.is_empty() {
            out.push(self.collision.unread_bodies.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "map collision bodies of shapes without a decoder, not read",
                    self.collision.unread_bodies.values().sum::<usize>() as u64,
                ),
                |n, (shape, count)| n.example(format!("{count} {shape}")),
            ));
        }
        if !self.extras.other_edges.is_empty() {
            out.push(self.extras.other_edges.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "EDGE records of other kinds than hang edges not read",
                    self.extras.other_edges.values().sum::<usize>() as u64,
                ),
                |n, (kind, count)| n.example(format!("{count} {kind}")),
            ));
        }
        let undecided = self
            .states
            .as_ref()
            .map_or(0, |s| s.undecided_parts.values().sum::<usize>());
        for (count, kind, reason) in [
            (
                self.skipped_missing,
                NoticeKind::Unsupported,
                "placements of models not found in the archive",
            ),
            (
                self.skipped_empty,
                NoticeKind::Unsupported,
                "placements of models whose face sets use no vertex",
            ),
            (
                self.undefined_materials,
                NoticeKind::Unsupported,
                "materials whose definition (material binary) is not read yet: FLVER bindings only, no textures",
            ),
            (
                self.material_classes.get("water").copied().unwrap_or(0),
                NoticeKind::Approximation,
                "water material slots (the shader family's name), drawn with the program's water colour and the source's \
                 normal map: the game's water program (depth colour, refraction, foam, flow) is not evaluated",
            ),
            (
                self.material_classes.get("decal").copied().unwrap_or(0),
                NoticeKind::Approximation,
                "decal material slots (`MeshDecal`), drawn over the surface beneath them on the decal layer, its queue a \
                 convention: the engine's decal pass order is not read",
            ),
            (
                self.object_placements,
                NoticeKind::Unsupported,
                if self.object_animation.asked {
                    "placed objects are drawn at their model's rest pose: the clips read from their animation binders \
                     are not lowered onto the placements yet"
                } else {
                    "placed objects are drawn at their model's rest pose: their animations (doors, lifts, any object's \
                     own animation binder) are not read (object_animations reads them)"
                },
            ),
            (
                self.grass_parts,
                NoticeKind::Unsupported,
                "parts storing a grass config: the game scatters grass over them from the grass type rows they name, by \
                 a density; no file holds a blade's place, so no grass is placed",
            ),
            (
                self.collision.unread_custom_primitives + self.object_collision.unread_custom_primitives,
                NoticeKind::Unsupported,
                "collision mesh custom primitives of an unknown kind (records whose first word ends in 0x52), not read",
            ),
            (
                usize::from(self.extras.spawn_points_not_read),
                NoticeKind::Unsupported,
                "spawn points not read: the map layout's spawn region code is not known",
            ),
            (
                usize::from(self.extras.missing_light_bank),
                NoticeKind::Unsupported,
                "no light bank 0",
            ),
            (
                usize::from(self.extras.missing_edge_binder),
                NoticeKind::Unsupported,
                "no hang-edge binder",
            ),
            (
                self.skipped_outside,
                NoticeKind::Selection,
                "placements outside the region and its margin",
            ),
            (
                self.extras.lights_outside_region,
                NoticeKind::Selection,
                "lights outside the region",
            ),
            (
                self.extras.spawn_points_outside_region,
                NoticeKind::Selection,
                "spawn points outside the region",
            ),
            (
                self.collision.unnamed_files,
                NoticeKind::Selection,
                "collision files of the area's binder no MSB part names, not read",
            ),
            (
                usize::from(self.states.is_none()),
                NoticeKind::Selection,
                "event-flag world states not read (states=event reads them): every part present",
            ),
            (
                undecided,
                NoticeKind::Approximation,
                "parts an uninterpreted script instruction names, kept at their default presence",
            ),
        ] {
            if count > 0 {
                out.push(Notice::new(stage, kind, reason, count as u64));
            }
        }
        out.extend(self.draw_param.notices());
        let grass = &self.grass;
        if grass.parts > 0 {
            if let Some(why) = &grass.unresolved {
                out.push(
                    Notice::new(
                        stage,
                        NoticeKind::Unsupported,
                        "grass types the parts name could not be resolved (GrassTypeParam not read)",
                        grass.named.len() as u64,
                    )
                    .example(why.clone()),
                );
            } else {
                out.push(grass.types.values().fold(
                    Notice::new(
                        stage,
                        NoticeKind::Unsupported,
                        "grass types the parts name (GrassTypeParam rows): their models and billboard textures are \
                         the game's, grown by density; not placed",
                        grass.types.len() as u64,
                    ),
                    |n, t| {
                        n.example(format!(
                            "row {} model {:?} billboard {:?} density {}",
                            t.id, t.model0, t.billboard_texture, t.base_density
                        ))
                    },
                ));
            }
            if !grass.missing.is_empty() {
                out.push(listed(
                    "grass type rows the parts name that the param does not hold",
                    &grass.missing.iter().map(i32::to_string).collect::<Vec<_>>(),
                ));
            }
        }
        let animations = &self.object_animation;
        if animations.asked {
            if animations.animated > 0 {
                out.push(Notice::new(
                    stage,
                    NoticeKind::Selection,
                    "object models with an animation binder: its Havok skeleton and clips are read into the world",
                    animations.animated as u64,
                ));
            }
            if !animations.unreadable.is_empty() {
                out.push(listed(
                    "object animation binders that could not be read",
                    &animations.unreadable,
                ));
            }
            for (why, files) in &animations.refused {
                out.push(Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    format!("object animation files not read: {why}"),
                    *files as u64,
                ));
            }
        }
        // the shadow controls' interpretation, as the read counted it
        for (reason, &count) in &self.shadows.approximations {
            out.push(Notice::new(
                stage,
                NoticeKind::Approximation,
                reason.clone(),
                count,
            ));
        }
        if let Some(&count) = self.shadows.counts.get("contradictory source cast/only controls") {
            out.push(Notice::new(
                stage,
                NoticeKind::Approximation,
                "placements with contradictory source cast/only shadow controls, casting left unknown",
                count,
            ));
        }
        out
    }
}

/// The word a notice uses for a part kind the read does not place.
fn unplaced_word(kind: PartKind) -> &'static str {
    match kind {
        PartKind::Enemy => "enemies",
        PartKind::Player => "player starts",
        PartKind::DummyObject => "dummy objects",
        PartKind::DummyAsset => "dummy assets",
        PartKind::DummyEnemy => "dummy enemies",
        PartKind::ConnectCollision => "connect collisions",
        PartKind::MapPiece | PartKind::Object | PartKind::Asset | PartKind::Collision => "placed or read parts",
    }
}

/// The grass types the parts' configs name, resolved against the game's grass type param. A param that cannot be read
/// (Elden Ring's lives in `regulation.bin`, whose key the install may not supply) or is of another layout is said, not
/// an error of the area read: the parts are still counted.
fn grass_types(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    parts: &[crate::msb::PartGrass],
) -> GrassTypes {
    let mut out = GrassTypes {
        parts: parts.len(),
        ..GrassTypes::default()
    };
    for id in parts.iter().flat_map(|p| p.ids).filter(|id| *id >= 0) {
        *out.named.entry(id).or_default() += 1;
    }
    let param = super::params::read_param(install, archive, decoder, "GrassTypeParam.param")
        .and_then(|param| crate::param::GrassParam::from_param(&param));
    match param {
        Err(why) => out.unresolved = Some(why.to_string()),
        Ok(param) => {
            out.layout = Some((param.data_version, param.row_size));
            // id 0 is the slot that names none, unless the param does hold a row 0
            if param.row(0).is_none()
                && let Some(slots) = out.named.remove(&0)
            {
                out.unset = slots;
            }
            for &id in out.named.keys() {
                match param.row(id) {
                    Some(row) => {
                        out.types.insert(id, row.clone());
                    }
                    None => out.missing.push(id),
                }
            }
        }
    }
    out
}

fn area_name(area: &str) -> Result<String> {
    let value = if area.len() == 6 {
        format!("{area}_00_00")
    } else {
        area.to_owned()
    };
    if value.len() != 12
        || !value.bytes().enumerate().all(|(i, b)| match i {
            0 => b == b'm',
            3 | 6 | 9 => b == b'_',
            _ => b.is_ascii_digit(),
        })
    {
        return Err(invalid("FromSoftware map", 0, "expected mNN_NN or mNN_NN_NN_NN"));
    }
    Ok(value)
}
/// Whether a placement of a model with world-frame box `[min, max]` reaches the region within its margin.
fn inside(bounds: &[[f32; 3]; 2], t: &Transform, o: &Options) -> bool {
    let Some(Region { rect: r, margin }) = o.region else {
        return true;
    };
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for mask in 0..8 {
        let p = t.apply(std::array::from_fn(|i| {
            bounds[usize::from(mask & (1 << i) != 0)][i]
        }));
        let p = [p[0], p[1], -p[2]];
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    max[0] >= r[0] - margin && min[0] < r[2] + margin && max[2] >= r[1] - margin && min[2] < r[3] + margin
}

/// A read model: the FLVER as stored, its resolved materials, its posed extent in the world frame, and for an object
/// its collision files' names, resolutions and decoded bytes.
struct Prototype {
    model: Model,
    materials: Vec<materials::Material>,
    bounds: Option<[[f32; 3]; 2]>,
    collision: Vec<(String, crate::game::Resolution, Vec<u8>)>,
    /// An object model's animation binder, if it has one and the read was asked for it.
    animation: Option<Vec<u8>>,
}

/// Which placements (model key, transform) the read keeps, in order, by index; counted into `report` by why the
/// others are not: a model the archive does not hold, a model whose face sets use no vertex (`bounds` holds `None`),
/// an extent outside the region.
fn select_placements(
    placements: &[(String, Transform)],
    bounds: &BTreeMap<String, Option<[[f32; 3]; 2]>>,
    options: &Options,
    report: &mut Report,
) -> Vec<usize> {
    let mut kept = Vec::new();
    for (i, (key, transform)) in placements.iter().enumerate() {
        match bounds.get(key) {
            None => report.skipped_missing += 1,
            Some(None) => report.skipped_empty += 1,
            Some(Some(b)) if !inside(b, transform, options) => report.skipped_outside += 1,
            Some(Some(_)) => kept.push(i),
        }
    }
    kept
}

/// Read an area directly from installed archives into an empty world. The caller supplies its trusted
/// decompressor; no environment variables or install paths are guessed. On failure the destination world is
/// unchanged.
///
/// Each prototype holds its model as stored (`fromsoft.flver`), its source identity (`fromsoft.prototype`, with the
/// distant-backdrop class) and the material entity of each of its FLVER materials (`fromsoft.material_slots`, each
/// holding `fromsoft.material`); an object prototype also its collision bodies (`fromsoft.object_collision`).
/// Placements are `core.instance` with their MSB transform, source label, shadow controls and, with states read,
/// conditions. Every texture a kept material binds is kept as stored (`fromsoft.texture_payloads`), the bundle of every
/// shader a kept material names with all its programs (`fromsoft.shader_bundle`, linked by `fromsoft.material_shader`)
/// and the G-buffer programs of its layered materials (`fromsoft.shader_programs`). Each map collision file the MSB's collision parts
/// name is a `fromsoft.map_collision` entity; lights are `fromsoft.light` records where they stand; hang edges and
/// spawn points are `fromsoft.area_extras`; what the read covered is `fromsoft.read_scope`.
/// A model's posed box in the world frame (the file frame's z reversed), `[min, max]` metres: the extent the read's
/// region rule uses and [`convert::AreaReader`] gives as a prototype's box. `None` for a model whose face sets use no
/// vertex.
pub fn world_bounds(model: &Model) -> std::result::Result<Option<[[f32; 3]; 2]>, String> {
    Ok(crate::pose::posed_bounds(model)?
        .map(|[min, max]| [[min[0], min[1], -max[2]], [max[0], max[1], -min[2]]]))
}

/// Every material definition (MTD) of the install, by stem: what a model's materials resolve against.
pub(super) fn material_definitions(
    archive: &Archive,
    decoder: &Decoder<'_>,
) -> Result<BTreeMap<String, MaterialDefinition>> {
    let mut definitions = BTreeMap::new();
    for f in Binder::read(&archive.read("/mtd/allmaterialbnd.mtdbnd.dcx", decoder)?, decoder)?.files {
        let name = f.name.as_deref().unwrap_or("");
        if crate::names::fold(name).ends_with(".mtd") {
            definitions.insert(
                materials::stem(name),
                MaterialDefinition::read(&decoder.decode_if_dcx(&f.bytes)?)?,
            );
        }
    }
    Ok(definitions)
}

/// The definitions `game`'s materials resolve against, by stem: Sekiro's MTDs, or the game's material binaries.
pub(super) fn game_material_definitions(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
) -> Result<BTreeMap<String, materials::Definition>> {
    if game.material_binders().is_empty() {
        Ok(material_definitions(archive, decoder)?
            .into_iter()
            .map(|(stem, d)| (stem, materials::Definition::Mtd(d)))
            .collect())
    } else {
        material_binaries(archive, decoder, game)
    }
}

/// Every material binary of `game`'s material binders, by the stem of its name (the `.matxml` stem its FLVER
/// materials name): the first binder holding a name gives it.
fn material_binaries(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
) -> Result<BTreeMap<String, materials::Definition>> {
    let mut definitions = BTreeMap::new();
    for path in game.material_binders() {
        if !archive.contains(path) {
            continue;
        }
        for f in Binder::read(&archive.read(path, decoder)?, decoder)?.files {
            let name = f.name.as_deref().unwrap_or("");
            if !crate::names::fold(name).ends_with(".matbin") {
                continue;
            }
            let material = crate::matbin::Material::read(&decoder.decode_if_dcx(&f.bytes)?)
                .map_err(|e| invalid("material binary", 0, format!("{name}: {e}")))?;
            definitions
                .entry(materials::stem(name))
                .or_insert(materials::Definition::Matbin(material));
        }
    }
    Ok(definitions)
}

pub fn read_area(
    install: impl AsRef<Path>,
    area: &str,
    decoder: &Decoder<'_>,
    options: &Options,
    destination: &mut World,
) -> Result<Report> {
    read_game_area(
        &Install::sekiro(install.as_ref()),
        area,
        decoder,
        options,
        destination,
    )
}

/// A placement the read considers: its model's key, the part as stored (from the map's layout or a larger map's),
/// the map whose layout holds it, and the offset (source x, z) from that map's frame into the read map's.
struct Placed {
    key: String,
    part: crate::msb::Part,
    map: String,
    offset: [f64; 2],
}

/// [`read_area`] for `install`'s game: what differs is [`crate::Game`]'s. Elden Ring's read does not read world states
/// yet (`states` other than none is refused); materials without a definition are counted in the report's notices.
pub fn read_game_area(
    install: &Install,
    area: &str,
    decoder: &Decoder<'_>,
    options: &Options,
    destination: &mut World,
) -> Result<Report> {
    let game = install.game;
    if destination.entities().next().is_some() {
        return Err(invalid("FromSoftware map", 0, "destination must be empty"));
    }
    if options.region.is_some_and(|Region { rect: r, margin }| {
        !(margin.is_finite() && margin >= 0.)
            || !r.iter().all(|v| v.is_finite())
            || r[0] >= r[2]
            || r[1] >= r[3]
    }) {
        return Err(invalid("FromSoftware map", 0, "invalid region"));
    }
    if game != Game::Sekiro && options.states != StateClasses::None {
        return Err(invalid(
            "FromSoftware map",
            0,
            format!("{}: event-flag world states are not read yet", game.name()),
        ));
    }
    let area = area_name(area)?;
    let archive = install.open()?;
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), decoder)?)?;
    let mut report = Report::default();
    // the definitions materials resolve against (Sekiro's MTDs, or the game's material binaries), and the texture
    // catalog: Sekiro's area discovery, or the game's common packages, then the packages its materials name
    let definitions = game_material_definitions(&archive, decoder, game)?;
    let mut catalog = if game.material_binders().is_empty() {
        textures::Catalog::discover(&archive, decoder, &area[..3])?
    } else {
        let mut catalog = textures::Catalog::default();
        for path in game.common_textures() {
            if archive.contains(path) {
                catalog.add(&archive.read(path, decoder)?)?;
            }
        }
        catalog
    };
    let mut texture_packages = BTreeSet::new();
    let object = game.object_prefix();
    let mut placements = Vec::new();
    let mut requests = BTreeSet::new();
    let mut maps = vec![(area.clone(), layout.clone(), [0.; 2], None)];
    if options.parents == Parents::Include {
        for (parent, offset) in game.parent_maps(&area) {
            let path = format!("/map/mapstudio/{parent}.msb.dcx");
            if !archive.contains(&path) {
                continue;
            }
            let parent_layout = MapLayout::read(&archive.read(&path, decoder)?)?;
            report.parent_maps.push(parent.clone());
            maps.push((parent, parent_layout, offset, Some(format!("{area}-"))));
        }
    }
    // map pieces, then objects (assets), each in layout order, the map's own before its parents'
    for objects in [false, true] {
        for (map, map_layout, offset, prefix) in &maps {
            for p in &map_layout.parts {
                if prefix
                    .as_ref()
                    .is_some_and(|prefix| !p.name.starts_with(prefix.as_str()))
                {
                    continue;
                }
                let kind = match p.kind {
                    PartKind::MapPiece if !objects => map.as_str(),
                    PartKind::Object | PartKind::Asset if objects => object,
                    // counted once, on the first of the two passes; the collision parts are the collision files'
                    PartKind::Enemy
                    | PartKind::Player
                    | PartKind::DummyObject
                    | PartKind::DummyEnemy
                    | PartKind::DummyAsset
                    | PartKind::ConnectCollision
                        if !objects =>
                    {
                        *report.unplaced_parts.entry(unplaced_word(p.kind)).or_default() += 1;
                        continue;
                    }
                    _ => continue,
                };
                let Some(model) = p.model.map(|i| map_layout.models[i].name.as_str()) else {
                    continue;
                };
                let key = format!("{kind}/{model}");
                requests.insert(key.clone());
                report.parent_placements += usize::from(prefix.is_some());
                placements.push(Placed {
                    key,
                    part: p.clone(),
                    map: map.clone(),
                    offset: *offset,
                });
            }
        }
    }
    let mut prototypes = BTreeMap::new();
    for key in requests {
        let (kind, model) = key.split_once('/').expect("constructed key");
        let is_object = kind == object;
        let (path, leaf, collision_files) = if is_object {
            let o = game.object_model(model)?;
            (o.binder, o.flver, o.collision)
        } else {
            let name = format!(
                "{kind}_{}",
                model.strip_prefix('m').ok_or_else(|| invalid(
                    "FromSoftware model",
                    0,
                    "map model prefix"
                ))?
            );
            (
                format!("{}/{name}.mapbnd.dcx", game.map_folder(kind)?),
                format!("{name}.flver"),
                Vec::new(),
            )
        };
        if !archive.contains(&path) {
            report.missing_models.push(key);
            continue;
        }
        let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
        if is_object {
            for f in &binder.files {
                if f.bytes.starts_with(b"TPF\0") {
                    catalog.add(&f.bytes)?;
                }
            }
        }
        // the object's animation binder, held as bytes until the model is known to be kept
        let animation = if is_object && options.object_animations {
            animation::object_binder(decoder, &binder)?
        } else {
            None
        };
        // a member by file name; `dcx`: also its compressed form
        let member = |leaf: &str, dcx: bool| {
            binder.files.iter().find(|f| {
                f.name
                    .as_deref()
                    .unwrap_or("")
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|s| {
                        crate::names::fold(s) == crate::names::fold(leaf)
                            || dcx
                                && s.strip_suffix(".dcx")
                                    .is_some_and(|s| crate::names::fold(s) == crate::names::fold(leaf))
                    })
            })
        };
        let Some(f) = member(&leaf, false) else {
            report.missing_models.push(key);
            continue;
        };
        // each collision file where it is: a member of the model's binder, or of a binder of its own
        let mut collision = Vec::new();
        for source in &collision_files {
            let bytes = match &source.binder {
                None => member(&source.member, true)
                    .map(|f| decoder.decode_if_dcx(&f.bytes))
                    .transpose()?,
                Some(path) if archive.contains(path) => Binder::read(&archive.read(path, decoder)?, decoder)?
                    .files
                    .into_iter()
                    .find(|f| {
                        let name = f.name.as_deref().unwrap_or("");
                        let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
                        let leaf = leaf.strip_suffix(".dcx").unwrap_or(leaf);
                        crate::names::fold(leaf) == crate::names::fold(&source.member)
                    })
                    .map(|f| decoder.decode_if_dcx(&f.bytes))
                    .transpose()?,
                Some(_) => None,
            };
            if let Some(bytes) = bytes {
                collision.push((source.member.clone(), source.resolution, bytes));
            }
        }
        let model =
            Model::read(&f.bytes).map_err(|e| invalid("FromSoftware model", 0, format!("{key}: {e}")))?;
        // the packages named after the textures the model's materials name, read once each
        for m in &model.materials {
            let defined = definitions.get(&materials::stem(&m.mtd));
            let paths = defined
                .map(|d| {
                    d.textures()
                        .into_iter()
                        .map(|t| t.1.to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
                .into_iter()
                .chain(m.textures.iter().map(|t| t.path.clone()));
            for path in paths {
                let stem = materials::stem(&path);
                if path.trim().is_empty() || catalog.entries.contains_key(&stem) {
                    continue;
                }
                if let Some(package) = game.texture_package(&stem)
                    && texture_packages.insert(package.clone())
                    && archive.contains(&package)
                {
                    catalog.add(&archive.read(&package, decoder)?)?;
                    report.texture_packages += 1;
                }
            }
        }
        let materials = model
            .materials
            .iter()
            .map(|m| {
                materials::resolve_definition(m, definitions.get(&materials::stem(&m.mtd)), game, |n| {
                    catalog.entries.contains_key(n)
                })
            })
            .collect();
        let bounds =
            world_bounds(&model).map_err(|e| invalid("FromSoftware model", 0, format!("{key}: {e}")))?;
        prototypes.insert(
            key,
            Prototype {
                model,
                materials,
                bounds,
                collision,
                animation,
            },
        );
    }
    // a larger map's part moves into the read map's frame by its centre's offset there, in f64 rounded once
    let placed = placements
        .iter()
        .map(|p| {
            let mut part = p.part.clone();
            part.position[0] = (f64::from(part.position[0]) - p.offset[0]) as f32;
            part.position[2] = (f64::from(part.position[2]) - p.offset[1]) as f32;
            Ok((p.key.clone(), part.transform()?))
        })
        .collect::<Result<Vec<_>>>()?;
    let bounds = prototypes.iter().map(|(k, p)| (k.clone(), p.bounds)).collect();
    let selected: Vec<(String, usize, Transform)> = select_placements(&placed, &bounds, options, &mut report)
        .into_iter()
        .map(|i| (placed[i].0.clone(), i, placed[i].1))
        .collect();
    let used: BTreeSet<&String> = selected.iter().map(|(key, _, _)| key).collect();
    prototypes.retain(|key, _| used.contains(key));

    let mut wanted = BTreeSet::new();
    let mut wanted_shaders = BTreeSet::new();
    let mut layered = Vec::new();
    for proto in prototypes.values() {
        for m in &proto.materials {
            *report.shader_parts.entry(m.shader.clone()).or_default() += 1;
            if m.skip.is_none() {
                *report.material_classes.entry(m.class.clone()).or_default() += 1;
            }
            report.undefined_materials += usize::from(game != Game::Sekiro && m.definition.is_none());
            for b in m.bindings.values() {
                if catalog.entries.contains_key(&b.texture) {
                    wanted.insert(b.texture.clone());
                }
            }
            layered.extend(m.layered.as_ref());
            // the bundle of the shader the material's definition (MTD or material binary) names
            if m.definition.is_some() {
                wanted_shaders.insert(m.shader.clone());
            }
        }
    }
    let mut bundles = shaders::Bundles::default();
    bundles.read(&archive, decoder, game, &wanted_shaders)?;
    // the textures the engine binds to the slots a material binds nothing to (Elden Ring's metaparam defaults); one
    // the catalog lacks is named in the report, not dropped silently
    let (held, missing): (Vec<String>, Vec<String>) = bundles
        .default_textures(game, prototypes.values().flat_map(|p| &p.materials))
        .into_iter()
        .partition(|t| catalog.entries.contains_key(t));
    wanted.extend(held);
    report.missing_default_textures = missing;
    let payloads = catalog.load(&archive, decoder, &wanted)?;
    let programs = bundles.layered_programs(layered.into_iter())?;
    report.textures = payloads.len();
    report.shader_programs = programs.len();

    let mut world = World::default();
    let mut states = match options.states {
        StateClasses::None => None,
        StateClasses::Event => {
            let script =
                crate::emevd::Script::read(&archive.read(&format!("/event/{area}.emevd.dcx"), decoder)?)?;
            let mut s = states::States::read(&script);
            s.spawn_axes(&mut world);
            Some(s)
        }
    };
    let mut proto_entities = BTreeMap::new();
    let mut material_entities = BTreeMap::new();
    let mut material_shaders = Vec::new();
    for (key, proto) in prototypes {
        let mut slots = Vec::with_capacity(proto.materials.len());
        for m in proto.materials {
            let identity = m.identity()?;
            let e = if let Some(e) = material_entities.get(&identity) {
                *e
            } else {
                let e = world.spawn();
                world.insert(e, Name(m.mtd.clone()));
                if let Some(layered) = &m.layered {
                    world.insert(e, layered.clone());
                }
                // linked to the bundle of the shader its definition (an MTD or a material binary) names, as read above
                if m.definition.is_some() {
                    material_shaders.push((e, m.shader.clone()));
                }
                world.insert(e, m);
                material_entities.insert(identity, e);
                report.materials += 1;
                e
            };
            slots.push(e);
        }
        let far = key
            .strip_prefix(&format!("{area}/"))
            .is_some_and(|model| game.far_model(model));
        let e = world.spawn();
        world.insert(e, Name(key.split_once('/').expect("key").1.to_owned()));
        report.prototype_boxes.insert(e, proto.bounds);
        world.insert(e, crate::components::models::SourceModel(proto.model));
        world.insert(e, source::MaterialSlots(slots));
        world.insert(e, source::PrototypeSource { id: key.clone(), far });
        if key.starts_with(&format!("{object}/")) {
            report.object_collision.models += 1;
            report.object_animation.asked = options.object_animations;
            if options.object_animations {
                let model = key.split_once('/').expect("key").1;
                match proto.animation {
                    None => report.object_animation.still += 1,
                    Some(bytes) => {
                        report.object_animation.animated += 1;
                        match animation::read_member(&bytes, decoder, model, &mut world) {
                            Ok(read) => {
                                report.object_animation.bones += read.report.bones;
                                report.object_animation.clips += read.report.animations;
                                for (why, files) in read.report.refused {
                                    *report.object_animation.refused.entry(why).or_default() += files.len();
                                }
                            }
                            Err(why) => report.object_animation.unreadable.push(format!("{model}: {why}")),
                        }
                    }
                }
            }
        }
        if !proto.collision.is_empty() {
            let files = proto
                .collision
                .iter()
                .map(|(file, resolution, bytes)| {
                    objects::bodies(file, *resolution, bytes, &mut report.object_collision)
                        .map_err(|e| invalid("object collision", 0, format!("{key}: {file}: {e}")))
                })
                .collect::<Result<Vec<_>>>()?;
            report.object_collision.models_with_collision += 1;
            world.insert(e, source::ObjectBodies { files });
        }
        proto_entities.insert(key, (e, far));
        report.prototypes += 1;
        report.far_prototypes += usize::from(far);
    }
    world.insert_resource(source::TexturePayloads(payloads));
    world.insert_resource(source::ShaderPrograms(programs));
    bundles.spawn(&mut world, &material_shaders);
    report.shader_bundles = bundles.bundles.len();
    for b in bundles.bundles.values() {
        for p in &b.programs {
            *report.bundle_programs.entry(p.stage).or_default() += 1;
        }
    }
    report.missing_shader_bundles = bundles.missing.iter().cloned().collect();
    report.unread_metaparams = bundles.unread_metaparams.iter().cloned().collect();
    // the parts whose graphics config says which value sets of the draw-param file apply here: the kept placements of
    // the map's own layout, and its collision parts (read whole, whatever the region)
    let environment_parts: BTreeSet<usize> = selected
        .iter()
        .map(|(_, index, _)| &placements[*index])
        .filter(|p| p.map == area)
        .map(|p| p.part.index)
        .chain(
            layout
                .parts
                .iter()
                .filter(|p| p.kind == PartKind::Collision)
                .map(|p| p.index),
        )
        .collect();
    for (key, index, transform) in selected {
        let Placed { part, map, .. } = &placements[index];
        let (prototype, far) = proto_entities[&key];
        let e = world.spawn();
        if let Some(s) = states.as_mut()
            && let Some(clause) = s.clause(part)
        {
            world.insert(e, primase_core::state::Condition(vec![clause]));
        }
        world.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        world.insert(e, transform);
        world.insert(e, Source(format!("{map}:{}", part.name)));
        shadows::attach(&mut world, e, part.shadows, &mut report.shadows);
        report.placements += 1;
        report.far_placements += usize::from(far);
        report.object_placements += usize::from(key.starts_with(&format!("{object}/")));
    }
    let grass = layout.grass_configs()?;
    report.grass_parts = grass.len();
    if !grass.is_empty() {
        report.grass = grass_types(install, &archive, decoder, &grass);
    }
    let folder = game.map_folder(&area)?;
    report.collision = collision::read(
        &archive,
        decoder,
        &folder,
        &area,
        &layout,
        states.as_mut(),
        &mut world,
    )?;
    report.extras = extras::assemble(
        &archive,
        decoder,
        &folder,
        &area,
        &layout,
        options.region.map(|r| r.rect),
        &mut world,
    )?;
    let configs: Vec<_> = if options.draw_param == draw_param::DrawParams::None {
        Vec::new()
    } else {
        layout
            .gparam_configs()?
            .into_iter()
            .filter(|c| environment_parts.contains(&c.part))
            .collect()
    };
    let (environment, draw_param_report) =
        draw_param::read(&archive, decoder, game, &area, &options.draw_param, &configs)?;
    if let Some(environment) = environment {
        world.insert_resource(environment);
    }
    report.draw_param = draw_param_report;
    world.insert_resource(source::ReadScope {
        game,
        area: area.clone(),
        region: options.region,
        states: states.is_some(),
    });
    report.states = states.map(|s| s.report);
    report.area = area;
    *destination = world;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{Options, Region, Report, select_placements};
    use primase_core::Transform;
    use primase_core::convert::NoticeKind;
    use std::collections::BTreeMap;

    #[test]
    fn notices_name_what_the_read_left_out_by_kind() {
        let mut report = Report {
            missing_models: vec!["obj/o000001".into()],
            skipped_missing: 3,
            skipped_outside: 7,
            states: Some(Default::default()),
            ..Report::default()
        };
        report.collision.missing_files = vec!["h11_00_00_00_009500.hkx".into()];
        report.collision.unnamed_files = 2;
        report.extras.missing_light_bank = true;
        report.extras.lights_outside_region = 4;
        report.extras.spawn_points_outside_region = 5;
        report.extras.other_edges = BTreeMap::from([("grapple".into(), 6)]);
        let notices = report.notices();
        let find = |reason: &str| notices.iter().find(|n| n.reason.starts_with(reason)).unwrap();
        // distinct models, and their placements apart
        let models = find("placed models not found");
        assert_eq!((models.kind, models.count), (NoticeKind::Unsupported, 1));
        assert_eq!(models.examples, ["obj/o000001"]);
        assert_eq!(find("placements of models not found").count, 3);
        for (reason, count) in [
            ("placements outside the region", 7),
            ("lights outside the region", 4),
            ("spawn points outside the region", 5),
            ("collision files of the area's binder no MSB part names", 2),
        ] {
            let n = find(reason);
            assert_eq!((n.kind, n.count), (NoticeKind::Selection, count), "{reason}");
        }
        assert_eq!(
            find("collision files named").examples,
            ["h11_00_00_00_009500.hkx"]
        );
        assert_eq!(find("EDGE records of other kinds").examples, ["6 grapple"]);
        assert_eq!(find("no light bank").count, 1);
        assert_eq!(notices.len(), 9, "{notices:?}");
        // a read without states says so; nothing else is left out of an empty read
        let default = Report::default().notices();
        assert_eq!(default.len(), 1, "{default:?}");
        assert_eq!(default[0].kind, NoticeKind::Selection);
        assert!(default[0].reason.starts_with("event-flag world states not read"));
    }

    #[test]
    fn a_missing_model_is_not_an_empty_one() {
        let at = |x: f32| Transform {
            cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [x, 0., 0.]],
        };
        let placements = [
            ("obj/missing".to_owned(), at(0.)),
            ("obj/missing".to_owned(), at(1.)),
            ("obj/empty".to_owned(), at(0.)),
            ("obj/box".to_owned(), at(0.)),
            ("obj/box".to_owned(), at(1000.)),
        ];
        let bounds = BTreeMap::from([
            ("obj/empty".to_owned(), None),
            ("obj/box".to_owned(), Some([[0.; 3], [1.; 3]])),
        ]);
        let mut report = Report::default();
        let options = Options {
            region: Some(Region {
                rect: [-10., -10., 10., 10.],
                margin: 30.,
            }),
            ..Options::default()
        };
        assert_eq!(
            select_placements(&placements, &bounds, &options, &mut report),
            [3]
        );
        assert_eq!(
            (
                report.skipped_missing,
                report.skipped_empty,
                report.skipped_outside
            ),
            (2, 1, 1)
        );
    }

    /// A stated region states its margin, and a margin that isn't a finite distance is refused by name before anything
    /// is read (here a folder that isn't an install).
    #[test]
    fn a_region_states_a_usable_margin() {
        let decoder = crate::read::dcx::Decoder {
            oodle: None,
            max_output: 1,
        };
        for margin in [-1.0, f32::NAN, f32::INFINITY] {
            let options = Options {
                region: Some(Region {
                    rect: [-10., -10., 10., 10.],
                    margin,
                }),
                ..Options::default()
            };
            let e = super::read_area(
                "/nonexistent",
                "m11_01",
                &decoder,
                &options,
                &mut primase_core::World::default(),
            )
            .unwrap_err();
            assert!(e.to_string().contains("invalid region"), "{margin}: {e}");
        }
    }
}
