//! Reading one FromSoftware map ([`read_game_area`]): its placed models,
//! materials, texture files, layered-shader programs, collision files and bodies, lights, hang edges and spawn points,
//! selected and in the world frame, as source records. What differs between the games and is not stated by their files
//! (where a map keeps its files, how objects are found, tile parents) is [`fromsoft_formats::Game`]'s; what Elden Ring's read does
//! not cover yet (world states, materials without a definition) is in its notices.
use super::{
    animation, collision, draw_param, effect_lights, effects, extras, groups, objects, shaders, shadows, sky,
    states, texture_switch, textures,
};
use crate::sekiro::source::{self, Region};
use crate::{invalid, materials};
use fromsoft_formats::{
    Archive, Binder, Game, Install, Result,
    dcx::Decoder,
    flver::Model,
    msb::{MapLayout, PartKind},
    mtd::MaterialDefinition,
};
use primase_core::{Instance, Name, Source, Transform, World};
use primase_foundations::parallel::Threads;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Named current weather and fire-ash state, independently observed in Elden Ring.
    /// Selects the installed sky membership; weather-layer composition is not yet traced.
    pub weather: Option<super::weather::Snapshot>,
    /// Named, independently observed flags for the installed common event 100.
    /// Missing flags remain unknown; this supplies assignments, not axis defaults.
    pub world_state: Option<super::world_state::FlagSet>,
    /// The region the read keeps ([`Region`]: a rectangle and its margin, both the caller's): placements whose
    /// model's extent reaches the rectangle within the margin, lights and spawn points inside the rectangle. Collision
    /// files are read whole whatever the region (their crop is `reach` on the collision passes). None reads the whole
    /// area.
    pub region: Option<Region>,
    /// Which world states to bring in ([`states`]): none (default, every part present) or the event-flag rules of the
    /// block's script, as axes and conditions for `resolve-state`.
    pub states: StateClasses,
    /// Whether the parts' draw groups become presence axes ([`groups`]): none (default, every part present) or one axis
    /// per draw group of a kept placement and a condition on each placement, for `resolve-state`.
    pub display_groups: groups::DisplayGroups,
    /// Whether the larger maps above the map (Elden Ring's open-world tiles: [`fromsoft_formats::Game::parent_maps`]) also place
    /// their parts that belong to it: none (default: the map's own layout only) or included, moved into the map's frame.
    pub parents: Parents,
    /// The area's draw-param file ([`draw_param`]): none (default: the graphics environment is not read), the file
    /// of this stem (one of [`fromsoft_formats::area::draw_params`]) or the file one collision part's scene config names.
    pub draw_param: draw_param::DrawParams,
    /// Whether the animation binder of each kept object model (Sekiro's objects, Elden Ring's assets: doors, lifts) is
    /// read into the world beside the model: its Havok skeleton and clips, named after the model. Off by default: the
    /// binders are more to hold in memory, and the clips are not lowered onto the placements by the read.
    pub object_animations: bool,
    /// Whether the layout's regions and events of every type are read into the world as `fromsoft.map_records`
    /// ([`crate::sekiro::source::MapRecords`]): off by default (the read keeps the parts, the collision, the lights
    /// and the spawn points, and counts the rest in its notices); `lower-fromsoft-map-records` lowers them.
    pub map_records: bool,
    /// Whether each map collision file's partner in the area's `f` collision binder (the file of the same model, made
    /// of extern meshes) is read with it, as [`crate::sekiro::source::MapCollision::f_triangles`]: off by default
    /// (the read keeps the `h` binder's files only); `lower-fromsoft-collision{f-meshes=on}` lowers them.
    pub f_collision: bool,
    /// Whether the point and spot lights of effect files (FXR) are read where they stand ([`effect_lights`]): the
    /// map's SFX regions, and the effects the kept objects (Sekiro) and assets (Elden Ring) spawn through their model
    /// effect row at the model's dummy polygons; off by default (it reads the effect binders and the params).
    /// `lower-fromsoft-lights` lowers them, in the BTL lights' unit.
    pub effect_lights: bool,
    /// Whether the sky is read with the area ([`sky`]): none (default) or included, the states as the `fromsoft.sky`
    /// axis with no default: Sekiro's sky map's domes, rings and moon and the area's own unplaced domes; Elden Ring's
    /// assets of the area's sky map that its weathers show, each with the weathers that show it.
    pub sky: sky::Sky,
    /// Whether the frame the game's renderer draws is read as the world's `core.render_graph` ([`super::render_graph`]):
    /// off by default. Its programs are read from the install's engine binders; its steps are the game's, as recorded
    /// frames of the running game show them, whatever the area.
    pub render_graph: bool,
    /// Read GIIV coefficient packages and environment boxes. Sekiro retains unresolved
    /// states for `lower-fromsoft-irradiance`; Elden Ring requires `environment` selection.
    pub irradiance: bool,
    /// Explicit environment texture states, blend factor and quality tier for Elden Ring.
    /// Supplying it also enables the environment read. Required when `irradiance` is
    /// requested there; no clock-to-package default is guessed.
    pub environment: Option<super::environment::Selection>,
    /// Whether the effects themselves are read where they stand ([`effects`]): the same routes as `effect_lights`, but the whole
    /// effect (`fromsoft.effect` on an entity placed there, the effect files and the textures they name) rather than its lights;
    /// off by default. `lower-fromsoft-effects` lowers them to `core.particles`.
    pub effects: bool,
    /// Whether each read texture's low-resolution twin is read too ([`source::LowTexturePayloads`]): the game's
    /// `<stem>_l`, a texture of its own beside the texture (Sekiro's area binders) or in the `_l` package beside its
    /// package (Elden Ring's assets). Off by default: the read is as it was. A texture with no twin keeps only itself
    /// and is named in [`Report::missing_low_textures`]. The game holds both and picks one per placement: the lowering
    /// gives a texture its twin as a form (`core.texture_forms`), and Elden Ring's placements whose draw param switches
    /// carry the rule (`core.texture_switch`, with the world's `core.texture_switch_view`; [`texture_switch`]). A
    /// binding the game draws coarse whatever the level (an Elden Ring asset's binding to another asset group's
    /// texture, [`materials::drawn_coarse`]; observed, not traced) binds the twin itself.
    pub low_textures: bool,
    /// The texture quality Elden Ring's switch is read at, with `low_textures` ([`texture_switch::TextureQuality`]):
    /// `Max` by default; the read refuses any other.
    pub texture_quality: texture_switch::TextureQuality,
}
/// What a read of the object models' animation binders found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectAnimations {
    /// Whether the binders were asked for ([`Options::object_animations`]).
    pub asked: bool,
    /// Kept object models that have an animation binder (an `.anibnd` member of the model's binder).
    pub animated: usize,
    /// Kept object models read without one.
    pub still: usize,
    /// Havok skeleton bones and clips read, over all the binders.
    pub bones: usize,
    pub clips: usize,
    /// Events put on those clips from the binders' `.tae` files, and the records no clip matched.
    pub events: usize,
    pub tae_unmatched: usize,
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
    pub types: BTreeMap<i32, fromsoft_formats::param::GrassType>,
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
    /// The explicit weather input and the installed sky/global layer it names.
    pub weather: Option<super::weather::Report>,
    pub world_state: Option<super::world_state::Report>,
    pub irradiance: super::irradiance::Report,
    pub environment: Option<super::environment::Report>,
    /// The full area name read (`m11_01_00_00`).
    pub area: String,
    /// Selected placements' source shadow controls mapped to `core.shadows`
    /// (units: placements). Counts contradictory cast/only controls left unknown
    /// and names the renderer-specific controls kept only as `fromsoft.shadow_flags`.
    pub shadows: primase_core::pass::PassLog,
    pub extras: extras::Report,
    /// The effect lights placed and what was left ([`Options::effect_lights`]).
    pub effect_lights: effect_lights::Report,
    /// The effects placed and what was left ([`Options::effects`]).
    pub effects: effects::Report,
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
    /// With `low_textures`: the low-resolution twins read ([`source::LowTexturePayloads`]), the `_l` packages read
    /// for them, and the read textures that have no twin, by stem (they keep only themselves).
    pub low_textures: usize,
    pub low_texture_packages: usize,
    pub missing_low_textures: Vec<String>,
    /// With `low_textures`: the material bindings of the read models that the game draws in their texture's coarse
    /// form whatever the placement's level, bound to that form ([`materials::drawn_coarse`]; observed, not traced).
    pub coarse_bindings: usize,
    /// The coarse forms those bindings bind, loaded as textures of their own (counted in `textures`; they have no twin
    /// of their own, so `low_textures + missing_low_textures + coarse_textures` is `textures`).
    pub coarse_textures: usize,
    /// With `low_textures` (Elden Ring): the placements given `core.texture_switch` from their draw param, and those
    /// left without one, by reason ([`texture_switch`]).
    pub texture_switch: texture_switch::Report,
    /// With `states: Event`: the rules read and the parts decided or left at their default.
    pub states: Option<states::Report>,
    /// With `display_groups: Axes`: the axes made, the placements conditioned or left present, and how the layout's draw
    /// groups meet its collisions' display groups.
    pub display_groups: Option<groups::Report>,
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
    /// The sky read with the area ([`Options::sky`]).
    pub sky: sky::Report,
    /// The frame's render graph ([`Options::render_graph`]).
    pub render_graph: super::render_graph::Report,
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
        if let Some(why) = &self.texture_switch.unread {
            out.push(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "texture switch params not read: no placement switches its textures' form",
                    1,
                )
                .example(why.clone()),
            );
        }
        if self.texture_switch.pieces_unestablished > 0 {
            out.push(Notice::new(
                stage,
                NoticeKind::Unsupported,
                "map pieces without use_parts_draw_param: their texture switch row is not established, none is read",
                self.texture_switch.pieces_unestablished as u64,
            ));
        }
        if let Some(quality) = self.texture_switch.quality
            && self.texture_switch.switched > 0
        {
            out.push(
                Notice::new(
                    stage,
                    NoticeKind::Selection,
                    format!(
                        "texture switches read at texture quality {quality:?}, the one quality they are checked at \
                         (others are refused)"
                    ),
                    self.texture_switch.switched as u64,
                )
                .example(format!("open: {}", texture_switch::QUALITY_OPEN_POINT)),
            );
        }
        if self.coarse_bindings > 0 {
            out.push(Notice::new(
                stage,
                NoticeKind::Approximation,
                "bindings drawn in their texture's coarse form whatever the placement's level: an asset's binding to \
                 another asset group's texture (observed in two captures, 20 of 20 slots; not traced)",
                self.coarse_bindings as u64,
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
                self.collision.unpaired_f_files,
                NoticeKind::Selection,
                "f collision files whose model no read collision file names, not read",
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
            (
                self.display_groups.as_ref().map_or(0, |g| g.unconditioned),
                NoticeKind::Approximation,
                "placements under no draw group (a zero array or no draw block), present in every state: whether the \
                 game draws such a part is not known",
            ),
            (
                self.display_groups.as_ref().map_or(0, |g| g.unmet),
                NoticeKind::Approximation,
                "placements under draw groups none of the map's collisions displays: drawn only when other display \
                 groups are active",
            ),
            (
                self.extras.regions_dropped_outside,
                NoticeKind::Selection,
                "map regions outside the read region",
            ),
            (
                self.extras.region_types.values().sum::<usize>()
                    + self.extras.event_types.values().sum::<usize>(),
                NoticeKind::Unsupported,
                "map regions and events kept with their type's data as bytes (fromsoft.map_records): what a type's data \
                 holds (a sound's id, an effect, a message, a treasure's item lot) is not decoded, only the header, \
                 the shape and the references to a part and a region",
            ),
            (
                self.extras.part_kinds.values().sum::<usize>(),
                NoticeKind::Selection,
                "enemies, player starts, dummies and connect collisions kept as records at their stored place \
                 (fromsoft.map_records): their think, patrol and talk data is not read",
            ),
        ] {
            if count > 0 {
                out.push(Notice::new(stage, kind, reason, count as u64));
            }
        }
        out.extend(self.draw_param.notices());
        out.extend(self.sky.notices());
        out.extend(self.render_graph.notices());
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
            if animations.events > 0 {
                out.push(Notice::new(
                    stage,
                    NoticeKind::Selection,
                    "object animation events (.tae) put on their clips (fromsoft.clip_events)",
                    animations.events as u64,
                ));
            }
            if animations.tae_unmatched > 0 {
                out.push(Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "object event records (.tae) that no clip read is numbered as",
                    animations.tae_unmatched as u64,
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
        PartKind::MapPiece | PartKind::Object | PartKind::Asset | PartKind::Collision => {
            "placed or read parts"
        }
    }
}

/// The grass types the parts' configs name, resolved against the game's grass type param. A param that cannot be read
/// (Elden Ring's lives in `regulation.bin`, whose key the install may not supply) or is of another layout is said, not
/// an error of the area read: the parts are still counted.
fn grass_types(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    parts: &[fromsoft_formats::msb::PartGrass],
) -> GrassTypes {
    let mut out = GrassTypes {
        parts: parts.len(),
        ..GrassTypes::default()
    };
    for id in parts.iter().flat_map(|p| p.ids).filter(|id| *id >= 0) {
        *out.named.entry(id).or_default() += 1;
    }
    let param = fromsoft_formats::game_params::read_param(install, archive, decoder, "GrassTypeParam.param")
        .and_then(|param| fromsoft_formats::param::GrassParam::from_param(&param));
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

/// The draw-param file `choice` names for `area`, as [`read_game_area`] reads it when its region reaches no placement:
/// against the graphics configs of the area's collision parts alone (those are read whole, whatever the region). It
/// reads the area's layout and the file, no model, so a census of an area's draw-param files costs a layout read per
/// file rather than an area read.
pub fn read_area_draw_param(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    choice: &draw_param::DrawParams,
) -> Result<(Option<source::DrawParam>, draw_param::Report)> {
    let area = area_name(area)?;
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), decoder)?)?;
    let parts: BTreeSet<usize> = collision_parts(&layout).collect();
    let configs = environment_configs(&layout, choice, &parts)?;
    let collisions = collision_draw_params(&layout, choice)?;
    draw_param::read(
        archive,
        decoder,
        archive.game(),
        &area,
        choice,
        &configs,
        &collisions,
    )
}

/// The layout's collision parts, by part index.
fn collision_parts(layout: &MapLayout) -> impl Iterator<Item = usize> + '_ {
    layout
        .parts
        .iter()
        .filter(|p| p.kind == PartKind::Collision)
        .map(|p| p.index)
}

/// The draw-param file ids the collision parts of `layout` name through their scene configs, by part name (a name two
/// parts share keeps the later one): none when no file is read.
fn collision_draw_params(
    layout: &MapLayout,
    choice: &draw_param::DrawParams,
) -> Result<BTreeMap<String, fromsoft_formats::area::DrawParamId>> {
    if *choice == draw_param::DrawParams::None {
        return Ok(BTreeMap::new());
    }
    let configs: BTreeMap<usize, _> = layout
        .gparam_configs()?
        .into_iter()
        .map(|c| (c.part, c))
        .collect();
    Ok(layout
        .parts
        .iter()
        .filter(|p| p.kind == PartKind::Collision)
        .filter_map(|p| Some((p.name.clone(), configs.get(&p.index)?.draw_param_id()?)))
        .collect())
}

/// The graphics configs of `parts` (part indices of `layout`) that choose the value sets of the draw-param file
/// `choice` names: none when no file is read.
fn environment_configs(
    layout: &MapLayout,
    choice: &draw_param::DrawParams,
    parts: &BTreeSet<usize>,
) -> Result<Vec<fromsoft_formats::msb::PartGparam>> {
    if *choice == draw_param::DrawParams::None {
        return Ok(Vec::new());
    }
    Ok(layout
        .gparam_configs()?
        .into_iter()
        .filter(|c| parts.contains(&c.part))
        .collect())
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

/// A model key's binder as the archive holds it, and what is read from it without the reader's state: the parts of
/// a read that are the same whatever the keys before it did, so that several keys can be read at once. Errors a key's
/// read would meet after reading its binder are kept as values, to be raised in the order one thread would meet them.
struct Fetched {
    /// Whether the key names an object (an asset) model rather than a map piece.
    is_object: bool,
    /// `None`: the archive holds no binder for the key.
    binder: Option<FetchedBinder>,
}
struct FetchedBinder {
    binder: Binder,
    /// The object's animation binder, if asked for and present.
    animation: Result<Option<Vec<u8>>>,
    /// `None`: the binder holds no member for the model.
    model: Option<FetchedModel>,
}
struct FetchedModel {
    collision: Result<Vec<(String, fromsoft_formats::game::Resolution, Vec<u8>)>>,
    model: Result<Model>,
    bounds: Result<Option<[[f32; 3]; 2]>>,
}

/// The binder of the model `key` (`<kind>/<model>`), its animation binder, collision files, parsed model and extent.
fn fetch_model(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    object: &str,
    options: &Options,
    key: &str,
) -> Result<Fetched> {
    let (kind, model) = key.split_once('/').expect("constructed key");
    let is_object = kind == object;
    let (path, leaf, collision_files) = if is_object {
        let o = game.object_model(model)?;
        (o.binder, o.flver, o.collision)
    } else {
        let name = format!(
            "{kind}_{}",
            model
                .strip_prefix('m')
                .ok_or_else(|| invalid("FromSoftware model", 0, "map model prefix"))?
        );
        (
            format!("{}/{name}.mapbnd.dcx", game.map_folder(kind)?),
            format!("{name}.flver"),
            Vec::new(),
        )
    };
    if !archive.contains(&path) {
        return Ok(Fetched {
            is_object,
            binder: None,
        });
    }
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let animation = if is_object && options.object_animations {
        animation::object_binder(decoder, &binder)
    } else {
        Ok(None)
    };
    // a member by file name; `dcx`: also its compressed form
    let member = |leaf: &str, dcx: bool| {
        binder
            .member(leaf)
            .or_else(|| dcx.then(|| binder.member(&format!("{leaf}.dcx"))).flatten())
    };
    let Some(f) = member(&leaf, false) else {
        return Ok(Fetched {
            is_object,
            binder: Some(FetchedBinder {
                binder,
                animation,
                model: None,
            }),
        });
    };
    // each collision file where it is: a member of the model's binder, or of a binder of its own
    let collision = (|| {
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
                        fromsoft_formats::names::fold(leaf) == fromsoft_formats::names::fold(&source.member)
                    })
                    .map(|f| decoder.decode_if_dcx(&f.bytes))
                    .transpose()?,
                Some(_) => None,
            };
            if let Some(bytes) = bytes {
                collision.push((source.member.clone(), source.resolution, bytes));
            }
        }
        Ok(collision)
    })();
    let model = Model::read(&f.bytes).map_err(|e| invalid("FromSoftware model", 0, format!("{key}: {e}")));
    let bounds = match &model {
        Ok(model) => world_bounds(model).map_err(|e| invalid("FromSoftware model", 0, format!("{key}: {e}"))),
        Err(_) => Ok(None),
    };
    Ok(Fetched {
        is_object,
        binder: Some(FetchedBinder {
            binder,
            animation,
            model: Some(FetchedModel {
                collision,
                model,
                bounds,
            }),
        }),
    })
}

/// What spawning a prototype needs from it that depends on no other prototype, so that several can be worked out at
/// once: its materials' identity records and, for an object, its collision bodies with what they add to the report. A
/// failure is kept as a value and raised where the spawn would meet it.
struct Prepared {
    identities: Vec<Result<String>>,
    collision: Option<Result<(Vec<source::ObjectCollisionFile>, objects::Report)>>,
}

fn prepare(key: &str, proto: &Prototype) -> Prepared {
    let collision = (!proto.collision.is_empty()).then(|| {
        let mut tally = objects::Report::default();
        proto
            .collision
            .iter()
            .map(|(file, resolution, bytes)| {
                objects::bodies(file, *resolution, bytes, &mut tally)
                    .map_err(|e| invalid("object collision", 0, format!("{key}: {file}: {e}")))
            })
            .collect::<Result<Vec<_>>>()
            .map(|files| (files, tally))
    });
    Prepared {
        identities: proto
            .materials
            .iter()
            .map(materials::Material::identity)
            .collect(),
        collision,
    }
}

/// A read model: the FLVER as stored, its resolved materials, its posed extent in the world frame, and for an object
/// its collision files' names, resolutions and decoded bytes.
struct Prototype {
    model: Model,
    materials: Vec<materials::Material>,
    bounds: Option<[[f32; 3]; 2]>,
    collision: Vec<(String, fromsoft_formats::game::Resolution, Vec<u8>)>,
    /// An object model's animation binder, if it has one and the read was asked for it.
    animation: Option<Vec<u8>>,
}

/// Which placements (model key, transform) the read keeps, in order, by index; counted into `report` by why the
/// others are not: a model the archive does not hold, a model whose face sets use no vertex (`bounds` holds `None`),
/// an extent outside the region. A placement `everywhere` marks (the sky, which the game draws around the camera
/// wherever it is) is kept whatever the region.
fn select_placements(
    placements: &[(String, Transform)],
    bounds: &BTreeMap<String, Option<[[f32; 3]; 2]>>,
    options: &Options,
    everywhere: &[bool],
    report: &mut Report,
) -> Vec<usize> {
    let mut kept = Vec::new();
    for (i, (key, transform)) in placements.iter().enumerate() {
        match bounds.get(key) {
            None => report.skipped_missing += 1,
            Some(None) => report.skipped_empty += 1,
            Some(Some(b))
                if !everywhere.get(i).copied().unwrap_or(false) && !inside(b, transform, options) =>
            {
                report.skipped_outside += 1
            }
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
        if fromsoft_formats::names::fold(name).ends_with(".mtd") {
            definitions.insert(
                fromsoft_formats::names::stem(name),
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
            if !fromsoft_formats::names::fold(name).ends_with(".matbin") {
                continue;
            }
            let material = fromsoft_formats::matbin::Material::read(&decoder.decode_if_dcx(&f.bytes)?)
                .map_err(|e| invalid("material binary", 0, format!("{name}: {e}")))?;
            definitions
                .entry(fromsoft_formats::names::stem(name))
                .or_insert(materials::Definition::Matbin(material));
        }
    }
    Ok(definitions)
}

/// A placement the read considers: its model's key, the part as stored (from the map's layout or a larger map's),
/// the map whose layout holds it, the offset (source x, z) from that map's frame into the read map's, and whether it is
/// part of the sky ([`Options::sky`]).
struct Placed {
    key: String,
    part: fromsoft_formats::msb::Part,
    map: String,
    offset: [f64; 2],
    sky: bool,
}

/// Read the map `area` of `install` into `destination` as `options` says: what differs between the games is
/// [`fromsoft_formats::Game`]'s. Elden Ring's read does not read world states yet (`states` other than none is refused); materials without a definition are counted in the report's notices.
pub fn read_game_area(
    install: &Install,
    area: &str,
    decoder: &Decoder<'_>,
    options: &Options,
    destination: &mut World,
) -> Result<Report> {
    read_game_area_staged(install, area, decoder, options, destination, &mut |_| {})
}

/// [`read_game_area`], telling `stage` the name of each stage as it starts, in order: `definitions and texture
/// catalog`, `models of every placement`, `select placements`, `wanted textures and shader bundles`, `load textures`,
/// `spawn prototypes`, `spawn placements`, `collision`, `extras`, `effect lights` (with `effect_lights` asked), `environment`, and `done` when the read returns. A
/// caller can bracket the stages with it (a memory census, a progress display); it changes nothing in the read.
pub fn read_game_area_staged(
    install: &Install,
    area: &str,
    decoder: &Decoder<'_>,
    options: &Options,
    destination: &mut World,
    stage: &mut dyn FnMut(&str),
) -> Result<Report> {
    let game = install.game;
    if let Some(selection) = options.environment {
        if game != Game::EldenRing {
            return Err(invalid(
                "environment states",
                0,
                "explicit environment state pairs require Elden Ring",
            ));
        }
        selection.verify()?;
    }
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
    let weather = options
        .weather
        .as_ref()
        .map(|input| {
            if options.sky != sky::Sky::Include {
                return Err(invalid("weather", 0, "weather snapshots require sky=include"));
            }
            super::weather::read(install, &archive, decoder, input)
        })
        .transpose()?;
    let world_state = if let Some(input) = &options.world_state {
        if game != Game::Sekiro {
            return Err(invalid(
                "world-state",
                0,
                "common event 100 is understood only for Sekiro",
            ));
        }
        let bytes = archive.read("/event/common.emevd.dcx", decoder)?;
        let script = fromsoft_formats::emevd::Script::read(&bytes)?;
        let map = [
            area[1..3].parse::<u8>().expect("validated area"),
            area[4..6].parse::<u8>().expect("validated block"),
        ];
        let selection =
            super::world_state::evaluate(&script, input, map).map_err(|e| invalid("world-state", 0, e))?;
        Some(super::world_state::Report::new(input.clone(), &bytes, selection))
    } else {
        None
    };
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), decoder)?)?;
    let mut report = Report {
        weather,
        world_state,
        ..Report::default()
    };
    stage("definitions and texture catalog");
    // the definitions materials resolve against (Sekiro's MTDs, or the game's material binaries), and the texture
    // catalog: Sekiro's area discovery, or the game's common packages, then the packages its materials name
    let definitions = game_material_definitions(&archive, decoder, game)?;
    let mut catalog = if game.material_binders().is_empty() {
        textures::Catalog::discover(&archive, decoder, &area[..3], options.low_textures)?
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
    // the coarse forms bindings bind ([`materials::bind_coarse`]), by stem
    let mut coarse_twins = BTreeSet::new();
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
    let mut display_groups = match options.display_groups {
        groups::DisplayGroups::None => None,
        groups::DisplayGroups::Axes => Some(groups::Groups::read(
            maps.iter().map(|(name, layout, _, _)| (name.as_str(), layout)),
            &area,
        )?),
    };
    // an overlay layout (`_99`) has no collision of its own: its parts' groups are met by its base map's collisions
    if let Some(g) = display_groups.as_mut()
        && let Some(base) = area.strip_suffix("_99").map(|stem| format!("{stem}_00"))
    {
        let path = format!("/map/mapstudio/{base}.msb.dcx");
        if archive.contains(&path) {
            g.add_collisions_of(&base, &MapLayout::read(&archive.read(&path, decoder)?)?)?;
        }
    }
    // the sky map the read brings in, in the area's frame, and Elden Ring's weathers of the area
    let mut sky_map: Option<String> = None;
    let mut weathers: Option<sky::Weathers> = None;
    if options.sky == sky::Sky::Include {
        report.sky.asked = true;
        let found = match game {
            Game::Sekiro => Some((sky::SKY_MAP.to_owned(), [0.; 2])),
            Game::EldenRing => sky::elden_sky_map(&area).map_err(|e| invalid("FromSoftware sky", 0, e))?,
        };
        let path = found
            .as_ref()
            .map(|(map, _)| format!("/map/mapstudio/{map}.msb.dcx"));
        match (found, path) {
            (Some((map, offset)), Some(path)) if archive.contains(&path) => {
                if game == Game::EldenRing {
                    let param =
                        fromsoft_formats::game_params::read_weather_assets(install, &archive, decoder)?;
                    let limits =
                        fromsoft_formats::game_params::read_weather_asset_limits(install, &archive, decoder)?;
                    // a map with no row sets no limit
                    let limit = sky::map_number(&area).and_then(|n| limits.of(n)).unwrap_or(-1);
                    weathers = Some(
                        sky::Weathers::new(&map, limit, &param)
                            .map_err(|e| invalid("FromSoftware sky", 0, e))?,
                    );
                }
                let sky_layout = MapLayout::read(&archive.read(&path, decoder)?)?;
                maps.push((map.clone(), sky_layout, offset, None));
                sky_map = Some(map);
            }
            (found, _) => {
                report.sky.missing_sky_map =
                    Some(found.map_or_else(|| format!("{area}: none known"), |(map, _)| map));
            }
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
                let in_sky = sky_map.as_ref() == Some(map);
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
                        if in_sky {
                            report.sky.other_parts.push(p.name.clone());
                        } else {
                            *report.unplaced_parts.entry(unplaced_word(p.kind)).or_default() += 1;
                        }
                        continue;
                    }
                    _ => continue,
                };
                let Some(model) = p.model.map(|i| map_layout.models[i].name.as_str()) else {
                    continue;
                };
                // of Elden Ring's sky map, the assets some weather of the area shows
                if in_sky && let Some(w) = &weathers {
                    if !w.names(model) {
                        report.sky.unnamed_parts += 1;
                        let upper = model.to_ascii_uppercase();
                        if upper.starts_with("AEG096_") && !report.sky.unnamed_sky_models.contains(&upper) {
                            report.sky.unnamed_sky_models.push(upper);
                        }
                        continue;
                    }
                    if w.states_of(model) == 0 {
                        report.sky.unshown_parts += 1;
                        continue;
                    }
                }
                let key = format!("{kind}/{model}");
                requests.insert(key.clone());
                report.parent_placements += usize::from(prefix.is_some());
                placements.push(Placed {
                    key,
                    part: p.clone(),
                    map: map.clone(),
                    offset: *offset,
                    sky: in_sky,
                });
            }
        }
    }
    // Sekiro: the area's own domes, which no layout places, at the origin
    if options.sky == sky::Sky::Include && game == Game::Sekiro {
        let folder = game.map_folder(&area)?;
        for model in sky::area_domes(&layout, |digits| {
            archive.contains(&format!("{folder}/{area}_{digits}.mapbnd.dcx"))
        }) {
            let key = format!("{area}/{model}");
            requests.insert(key.clone());
            placements.push(Placed {
                key,
                part: sky::unplaced_part(&model),
                map: area.clone(),
                offset: [0.; 2],
                sky: true,
            });
        }
    }
    let sky_keys: BTreeSet<String> = placements
        .iter()
        .filter(|p| p.sky)
        .map(|p| p.key.clone())
        .collect();
    stage("models of every placement");
    let mut prototypes = BTreeMap::new();
    // The binders and models are read a few at a time, on the install's threads, and joined in key order: what one
    // key's read puts in the texture catalog, the report and the set of packages read decides what the keys after it
    // find there, so that part of each key's work stays here, in the order one thread would do it.
    let keys: Vec<String> = requests.into_iter().collect();
    let threads = Threads(install.threads);
    for wave in keys.chunks(threads.get().saturating_mul(2)) {
        let fetched = primase_foundations::parallel::map(threads, wave, |_, key| {
            fetch_model(&archive, decoder, game, object, options, key)
        })
        .map_err(|e| invalid("FromSoftware model", 0, e.to_string()))?;
        for (key, fetched) in wave.iter().zip(fetched) {
            let key = key.clone();
            let Fetched { is_object, binder } = fetched?;
            let Some(FetchedBinder {
                binder,
                animation,
                model,
            }) = binder
            else {
                report.missing_models.push(key);
                continue;
            };
            if is_object {
                for f in &binder.files {
                    if f.bytes.starts_with(b"TPF\0") {
                        catalog.add(&f.bytes)?;
                    }
                }
            }
            drop(binder);
            // the object's animation binder, held as bytes until the model is known to be kept
            let animation = animation?;
            let Some(FetchedModel {
                collision,
                model,
                bounds,
            }) = model
            else {
                report.missing_models.push(key);
                continue;
            };
            let collision = collision?;
            let model = model?;
            // the packages named after the textures the model's materials name, read once each
            for m in &model.materials {
                let defined = definitions.get(&fromsoft_formats::names::stem(&m.mtd));
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
                    let stem = fromsoft_formats::names::stem(&path);
                    if path.trim().is_empty() || catalog.entries.contains_key(&stem) {
                        continue;
                    }
                    if let Some(package) = game.texture_package(&stem)
                        && texture_packages.insert(package.clone())
                        && archive.contains(&package)
                    {
                        catalog.add(&archive.read(&package, decoder)?)?;
                        report.texture_packages += 1;
                        // its low-resolution textures, in the `_l` package beside it
                        if options.low_textures
                            && let Some(low) = package
                                .strip_suffix(".tpf.dcx")
                                .map(|stem| format!("{stem}_l.tpf.dcx"))
                            && archive.contains(&low)
                        {
                            catalog.add_low(&archive.read(&low, decoder)?)?;
                            report.low_texture_packages += 1;
                        }
                    }
                }
            }
            let mut materials: Vec<_> = model
                .materials
                .iter()
                .map(|m| {
                    materials::resolve_definition(
                        m,
                        definitions.get(&fromsoft_formats::names::stem(&m.mtd)),
                        game,
                        |n| catalog.entries.contains_key(n),
                    )
                })
                .collect();
            // the bindings the game draws in their texture's coarse form, where the read holds it (`low_textures`):
            // bound to the twin, a texture of its own the load takes like any other
            let name = key.split_once('/').map_or(key.as_str(), |k| k.1);
            for m in &mut materials {
                for twin in materials::bind_coarse(m, game, name, |stem| catalog.low.contains_key(stem)) {
                    report.coarse_bindings += 1;
                    let stored = twin.strip_suffix("_l").expect("a twin's stem");
                    if !catalog.entries.contains_key(&twin)
                        && let Some(location) = catalog.low.get(stored).cloned()
                    {
                        catalog.entries.insert(twin.clone(), location);
                    }
                    coarse_twins.insert(twin);
                }
            }
            let bounds = bounds?;
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
    }
    stage("select placements");
    // a larger map's part moves into the read map's frame by its centre's offset there, in f64 rounded once
    let placed = placements
        .iter()
        .map(|p| {
            let mut part = p.part.clone();
            part.position[0] = (f64::from(part.position[0]) - p.offset[0]) as f32;
            part.position[2] = (f64::from(part.position[2]) - p.offset[1]) as f32;
            Ok((p.key.clone(), crate::pose::placed(part.matrix()?)))
        })
        .collect::<Result<Vec<_>>>()?;
    let bounds = prototypes.iter().map(|(k, p)| (k.clone(), p.bounds)).collect();
    let everywhere: Vec<bool> = placements.iter().map(|p| p.sky).collect();
    let selected: Vec<(String, usize, Transform)> =
        select_placements(&placed, &bounds, options, &everywhere, &mut report)
            .into_iter()
            .map(|i| (placed[i].0.clone(), i, placed[i].1))
            .collect();
    let used: BTreeSet<&String> = selected.iter().map(|(key, _, _)| key).collect();
    prototypes.retain(|key, _| used.contains(key));
    // the sky's states and what each kept sky placement is, from its model and its materials
    let sky_plan = if let Some(w) = &weathers {
        // whether every material a prototype draws with (shadow proxies aside) is of the sky class
        let sky_programs = |key: &str| {
            prototypes.get(key).is_some_and(|proto| {
                let mut drawn = proto
                    .materials
                    .iter()
                    .filter(|m| m.skip != Some(materials::SkipClass::Shadow))
                    .peekable();
                drawn.peek().is_some() && drawn.all(|m| m.skip == Some(materials::SkipClass::Sky))
            })
        };
        let items: Vec<(usize, String, bool)> = selected
            .iter()
            .filter(|(_, index, _)| placements[*index].sky)
            .map(|(key, index, _)| {
                (
                    *index,
                    key.split_once('/').expect("key").1.to_owned(),
                    sky_programs(key.as_str()),
                )
            })
            .collect();
        Some(w.plan(&items, std::mem::take(&mut report.sky)))
    } else if options.sky == sky::Sky::Include && sky_map.is_some() {
        let items: Vec<sky::Item> = selected
            .iter()
            .filter(|(_, index, _)| placements[*index].sky)
            .map(|(key, index, _)| {
                let p = &placements[*index];
                let (_, model) = key.split_once('/').expect("key");
                sky::Item {
                    index: *index,
                    map: p.map.clone(),
                    model: model.to_owned(),
                    object: key.starts_with(&format!("{object}/")),
                    entity: p.part.entity_id,
                    mtds: prototypes
                        .get(key)
                        .map(|proto| proto.materials.iter().map(|m| m.mtd.clone()).collect())
                        .unwrap_or_default(),
                }
            })
            .collect();
        let mut plan = sky::plan(&items).map_err(|e| invalid("FromSoftware sky", 0, e))?;
        plan.report
            .other_parts
            .extend(std::mem::take(&mut report.sky.other_parts));
        Some(plan)
    } else {
        None
    };

    stage("wanted textures and shader bundles");
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
    stage("load textures");
    let payloads = catalog.load(&archive, decoder, &wanted)?;
    let low_payloads = if options.low_textures {
        // a coarse form a binding binds is a texture of its own, not a texture with a twin
        let with_twins: BTreeSet<String> = wanted.difference(&coarse_twins).cloned().collect();
        let (low, missing) = catalog.load_low(&archive, decoder, &with_twins)?;
        report.low_textures = low.len();
        report.missing_low_textures = missing;
        Some(low)
    } else {
        None
    };
    let programs = bundles.layered_programs(layered.into_iter())?;
    report.textures = payloads.len();
    report.coarse_textures = coarse_twins.iter().filter(|t| payloads.contains_key(*t)).count();
    report.shader_programs = programs.len();

    stage("spawn prototypes");
    let mut world = World::default();
    let mut states = match options.states {
        StateClasses::None => None,
        StateClasses::Event => {
            let script = fromsoft_formats::emevd::Script::read(
                &archive.read(&format!("/event/{area}.emevd.dcx"), decoder)?,
            )?;
            let mut s = states::States::read(&script, game);
            s.spawn_axes(&mut world);
            Some(s)
        }
    };
    let mut proto_entities = BTreeMap::new();
    // the dummy polygons of the object models, for the effect lights they spawn
    let mut object_dummies = BTreeMap::new();
    let mut material_entities = BTreeMap::new();
    let mut material_shaders = Vec::new();
    // a wave of prototypes at a time has what depends on no other worked out on the install's threads, and is spawned
    // in key order
    let mut queue: std::collections::VecDeque<_> = prototypes.into_iter().collect();
    while !queue.is_empty() {
        let wave: Vec<(String, Prototype)> = queue
            .drain(..threads.get().saturating_mul(2).min(queue.len()))
            .collect();
        let prepared =
            primase_foundations::parallel::map(threads, &wave, |_, (key, proto)| prepare(key, proto))
                .map_err(|e| invalid("FromSoftware model", 0, e.to_string()))?;
        for ((key, proto), prepared) in wave.into_iter().zip(prepared) {
            let Prepared {
                identities,
                collision,
            } = prepared;
            let mut slots = Vec::with_capacity(proto.materials.len());
            for (m, identity) in proto.materials.into_iter().zip(identities) {
                let identity = identity?;
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
            // the sky's models are of the backdrop series by name, and are the sky
            let far = !sky_keys.contains(&key)
                && key
                    .strip_prefix(&format!("{area}/"))
                    .is_some_and(|model| game.far_model(model));
            if (options.effect_lights || options.effects) && key.starts_with(&format!("{object}/")) {
                let (dummies, left_out) = effect_lights::ModelDummies::of(&proto.model)
                    .map_err(|e| invalid("FromSoftware model", 0, format!("{key}: {e}")))?;
                report.effect_lights.dummies_left_out += left_out;
                object_dummies.insert(key.clone(), dummies);
            }
            let e = world.spawn();
            world.insert(e, Name(key.split_once('/').expect("key").1.to_owned()));
            report.prototype_boxes.insert(e, proto.bounds);
            world.insert(e, crate::components::models::SourceModel::new(proto.model));
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
                            match animation::read_member(install.game, &bytes, decoder, model, &mut world) {
                                Ok(read) => {
                                    report.object_animation.bones += read.report.bones;
                                    report.object_animation.clips += read.report.animations;
                                    report.object_animation.events += read.report.events;
                                    report.object_animation.tae_unmatched += read.report.tae_unmatched;
                                    for (why, files) in read.report.refused {
                                        *report.object_animation.refused.entry(why).or_default() +=
                                            files.len();
                                    }
                                }
                                Err(why) => {
                                    report.object_animation.unreadable.push(format!("{model}: {why}"))
                                }
                            }
                        }
                    }
                }
            }
            if let Some(collision) = collision {
                let (files, tally) = collision?;
                report.object_collision.absorb(tally);
                report.object_collision.models_with_collision += 1;
                world.insert(e, source::ObjectBodies { files });
            }
            proto_entities.insert(key, (e, far));
            report.prototypes += 1;
            report.far_prototypes += usize::from(far);
        }
    }
    world.insert_resource(source::TexturePayloads(payloads));
    if let Some(low) = low_payloads {
        world.insert_resource(source::LowTexturePayloads(low));
    }
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
        .filter(|p| p.map == area && !p.sky)
        .map(|p| p.part.index)
        .chain(collision_parts(&layout))
        .collect();
    stage("spawn placements");
    // the kept placements of the object models, by placement index
    let selected_models: BTreeMap<usize, Transform> = if options.effect_lights || options.effects {
        selected
            .iter()
            .filter(|(key, _, _)| key.starts_with(&format!("{object}/")))
            .map(|(_, index, transform)| (*index, *transform))
            .collect()
    } else {
        BTreeMap::new()
    };
    let sky_axis = sky_plan
        .as_ref()
        .filter(|plan| !plan.values.is_empty())
        .map(|plan| {
            let source = match &weathers {
                Some(w) => format!(
                    "the weathers and fire-ash states of the WeatherAssetReplaceParam rows of the sky map {} that \
                     apply on {area} (creation limit {}); the game's choice of one (WeatherLotParam) is not read",
                    w.sky_map, w.limit
                ),
                None => format!(
                    "the domes of the sky map {} and the area's own; the game's choice of one is not read",
                    sky::SKY_MAP
                ),
            };
            sky::spawn_axis(&mut world, &plan.values, source)
        });
    if let Some(g) = display_groups.as_mut() {
        // one axis per draw group of a kept placement, in group order
        let numbers: BTreeSet<u8> = selected
            .iter()
            .flat_map(|(_, index, _)| {
                let p = &placements[*index];
                g.numbers(&p.map, p.part.index).iter().copied()
            })
            .collect();
        g.spawn_axes(&mut world, numbers);
    }
    // the texture switch's params, with the twins (Elden Ring's): a regulation the install cannot open is said, not an
    // error of the read
    let switch_params = if options.low_textures && game == Game::EldenRing {
        options.texture_quality.check()?;
        report.texture_switch.quality = Some(options.texture_quality);
        match texture_switch::Params::read(install, &archive, decoder) {
            Ok(p) => Some(p),
            Err(why) => {
                report.texture_switch.unread = Some(why.to_string());
                None
            }
        }
    } else {
        None
    };
    for (key, index, transform) in selected {
        let Placed {
            part,
            map,
            sky: in_sky,
            ..
        } = &placements[index];
        // a sky part of no known role, or a ring no dome pairs with, is not placed (counted in the plan's report)
        let sky_part = match (*in_sky, &sky_plan) {
            (true, Some(plan)) => match plan.parts.get(&index) {
                Some(&part) => Some(part),
                None => continue,
            },
            _ => None,
        };
        let (prototype, far) = proto_entities[&key];
        let e = world.spawn();
        let condition = if *in_sky {
            None
        } else {
            let event = states.as_mut().and_then(|s| s.clause(part));
            match display_groups.as_mut() {
                Some(g) => g.condition(map, part.index, event),
                None => event.map(|clause| primase_core::state::Condition(vec![clause])),
            }
        };
        if let Some(condition) = condition {
            world.insert(e, condition);
        }
        world.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        // an Elden Ring weather asset of sky programs stands relative to its sky map's origin, which the game moves to
        // the camera's x and z (the anchor supplies it), not where the origin lies in the area's frame
        let transform = match sky_part {
            Some((source::SkyRole::WeatherSky, _)) => crate::pose::placed(part.matrix()?),
            _ => transform,
        };
        world.insert(e, transform);
        world.insert(e, Source(format!("{map}:{}", part.name)));
        // the sky's placements stand relative to the camera; the switch is not read for them
        if let Some(params) = &switch_params
            && !*in_sky
        {
            let model = key.split_once('/').map_or(key.as_str(), |k| k.1);
            if let Some(switch) = params.switch(part.kind, &part.draw, model, &mut report.texture_switch)? {
                world.insert(e, switch);
            }
        }
        if let Some((role, state)) = sky_part {
            sky::attach(&mut world, e, sky_axis, role, state);
        }
        shadows::attach(&mut world, e, part.shadows, &mut report.shadows);
        report.placements += 1;
        report.far_placements += usize::from(far);
        report.object_placements += usize::from(key.starts_with(&format!("{object}/")));
    }
    if report.texture_switch.switched > 0 {
        world.insert_resource(texture_switch::view());
    }
    let grass = layout.grass_configs()?;
    report.grass_parts = grass.len();
    if !grass.is_empty() {
        report.grass = grass_types(install, &archive, decoder, &grass);
    }
    stage("collision");
    let folder = game.map_folder(&area)?;
    report.collision = collision::read(
        &archive,
        decoder,
        &folder,
        &area,
        &layout,
        states.as_mut(),
        options.f_collision,
        threads,
        &mut world,
    )?;
    stage("extras");
    if options.irradiance || options.environment.is_some() {
        if game == Game::EldenRing {
            let selection = options.environment.ok_or_else(|| invalid(
                "irradiance", 0,
                "Elden Ring environment requires explicit state A, state B, factor and tier; the clock selector is not traced",
            ))?;
            report.environment = Some(super::environment::read(
                &archive, decoder, &area, &layout, selection, &mut world,
            )?);
        } else {
            report.irradiance = super::irradiance::read(&archive, decoder, &area, &layout, &mut world)?;
        }
    }
    report.extras = extras::assemble(
        &archive,
        decoder,
        &folder,
        &area,
        &layout,
        options.region.map(|r| r.rect),
        &mut world,
    )?;
    if options.map_records {
        extras::records(
            &layout,
            options.region.map(|r| r.rect),
            &mut report.extras,
            &mut world,
        )?;
    }
    if options.effect_lights || options.effects {
        stage("effect lights");
        // every object or asset part of the layouts read, kept or not: the census of what the rows reach is of all of them
        let models: Vec<effect_lights::PlacedModel<'_>> = placements
            .iter()
            .enumerate()
            .filter(|(_, p)| p.key.starts_with(&format!("{object}/")) && !p.sky)
            .filter_map(|(index, p)| {
                let placement = match (selected_models.get(&index), object_dummies.get(&p.key)) {
                    (Some(transform), Some(dummies)) => effect_lights::Placement::Placed {
                        transform: *transform,
                        dummies,
                    },
                    _ => effect_lights::Placement::Unplaced(match bounds.get(&p.key) {
                        None => "model not in the archive",
                        Some(None) => "model uses no vertex",
                        Some(Some(_)) => "placement outside the region",
                    }),
                };
                Some(effect_lights::PlacedModel {
                    part: &p.part.name,
                    model: p.key.split_once('/')?.1,
                    relative_id: p.part.sfx_param_relative_id,
                    placement,
                })
            })
            .collect();
        let dummies_left_out = report.effect_lights.dummies_left_out;
        if options.effect_lights {
            report.effect_lights = effect_lights::assemble(
                install,
                &archive,
                decoder,
                &area,
                &layout,
                options.region.map(|r| r.rect),
                &models,
                &mut world,
            )?;
        }
        report.effect_lights.dummies_left_out = dummies_left_out;
        if options.effects {
            stage("effects");
            report.effects = effects::assemble(
                install,
                &archive,
                decoder,
                &area,
                &layout,
                options.region.map(|r| r.rect),
                &models,
                &mut world,
            )?;
        }
    }
    stage("environment");
    let configs = environment_configs(&layout, &options.draw_param, &environment_parts)?;
    let collisions = collision_draw_params(&layout, &options.draw_param)?;
    let (mut environment, mut draw_param_report) = draw_param::read(
        &archive,
        decoder,
        game,
        &area,
        &options.draw_param,
        &configs,
        &collisions,
    )?;
    if let Some(sub_id) = report
        .world_state
        .as_ref()
        .and_then(|s| s.selection.gparam_sub_ids.get(&area[..6]))
        && options.draw_param != draw_param::DrawParams::None
    {
        let stem = draw_param_report.file.as_ref().ok_or_else(|| {
            invalid(
                "world-state",
                0,
                "Set Area Gparam Sub ID: base draw-param file is unresolved",
            )
        })?;
        let (map, base) = fromsoft_formats::area::DrawParamId::from_stem(stem).ok_or_else(|| {
            invalid(
                "world-state",
                0,
                "Set Area Gparam Sub ID: draw-param slot is unresolved",
            )
        })?;
        let id = u8::try_from(*sub_id)
            .ok()
            .and_then(|sub| fromsoft_formats::area::DrawParamId::new(base.slot(), sub))
            .ok_or_else(|| invalid("world-state", 0, "Set Area Gparam Sub ID: invalid sub id"))?;
        let choice = draw_param::DrawParams::File(id.stem(map));
        (environment, draw_param_report) =
            draw_param::read(&archive, decoder, game, &area, &choice, &configs, &collisions)?;
        if environment.is_none() {
            return Err(invalid(
                "world-state",
                0,
                "Set Area Gparam Sub ID: selected file is missing or unreadable",
            ));
        }
    }
    if let Some(environment) = environment {
        if matches!(game, Game::Sekiro | Game::EldenRing) {
            super::post::lut::read_into(&mut world, &archive, decoder, &area);
        }
        world.insert_resource(environment);
    }
    report.draw_param = draw_param_report;
    if options.render_graph {
        stage("render graph");
        super::render_graph::read(&mut world, &archive, decoder, game, &mut report.render_graph)?;
    }
    world.insert_resource(source::ReadScope {
        game,
        area: area.clone(),
        region: options.region,
        states: states.is_some(),
    });
    report.states = states.map(|s| s.report);
    if let (Some(state), Some(plan)) = (report.world_state.as_mut(), sky_plan.as_ref()) {
        let masks: Vec<u64> = plan
            .parts
            .iter()
            .filter_map(|(&index, &(role, mask))| {
                let part = &placements[index].part;
                (role == source::SkyRole::Dome
                    && part
                        .entity_group_ids
                        .iter()
                        .any(|id| state.selection.groups.get(id) == Some(&true)))
                .then_some(mask)
            })
            .collect();
        state.sky = Some(
            super::world_state::selected_sky(&plan.values, &masks)
                .map_err(|e| invalid("world-state", 0, e))?,
        );
    }
    if let Some(plan) = sky_plan {
        report.sky = plan.report;
    }
    if let Some(weather) = &report.weather {
        super::weather::assign(&mut world, weather).map_err(|e| invalid("weather", 0, e))?;
        report.sky.selection = Some(format!(
            "{}: {} ({})",
            weather.input.name, weather.sky, weather.input.source
        ));
    }
    if let Some(state) = report.world_state.as_mut() {
        super::world_state::assign(&mut world, &area, state).map_err(|e| invalid("world-state", 0, e))?;
    }
    report.display_groups = display_groups.map(|g| g.report);
    report.area = area;
    *destination = world;
    stage("done");
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
            select_placements(&placements, &bounds, &options, &[false; 5], &mut report),
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
        let decoder = fromsoft_formats::dcx::Decoder {
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
            let e = super::read_game_area(
                &fromsoft_formats::Install::new(
                    "/nonexistent",
                    fromsoft_formats::Game::Sekiro,
                    fromsoft_formats::Keys::none(),
                ),
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
