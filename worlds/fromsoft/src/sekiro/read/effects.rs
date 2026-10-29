//! Effects where they stand (`fromsoft.effect`): the effects the map's SFX regions name and the effects the kept objects and
//! assets spawn through their model effect row (the routes `fromsoft.effect_light` is read through, see
//! [`super::effect_lights`]), with the effect files as stored (`fromsoft.effect_file`) and the textures they name
//! (`fromsoft.effect_resource`, a `core.texture` each). Their interpretation belongs to `lower-fromsoft-effects`.
//!
//! The effect files are looked for in the binder of the area's map (`sfxbnd_<mNN>`, and Elden Ring's `sfxbnd_<full map name>`
//! where there is one), then `sfxbnd_commoneffects` and Elden Ring's `commoneffects_dlc02`; the copies of an effect in several
//! binders are byte-identical. A texture is looked for in the same binders, in that order.
use super::effect_lights::{ModelDummies, PlacedModel, Placement, map_transform, row_of};
use super::textures::tpf_image;
use crate::pose::placed;
use crate::sekiro::character_data::ClipEvents as SourceEvents;
use crate::sekiro::components::effect::{
    Effect, EffectFile, EffectResource, ResourceKind, TraceOffsets, apply_trace_offsets, clip_effect_uses,
};
use crate::sekiro::source::EffectPlace;
use fromsoft_formats::fxr::actions::Typed;
use fromsoft_formats::fxr::resources::{ResourceKind as FileKind, Resources};
use fromsoft_formats::fxr::{self, Fxr, TypedNode};
use fromsoft_formats::msb::{MapLayout, euler_matrix};
use fromsoft_formats::param::ModelSfxParam;
use fromsoft_formats::tpf::Package;
use fromsoft_formats::{Archive, Binder, Game, Install, Result, dcx::Decoder};
use primase_core::{Entity, Name, Source, Transform, World};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What the read of the effects found and left.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Whether the read was asked for ([`crate::sekiro::Options::effects`]).
    pub asked: bool,
    /// SFX regions inside the read region and those outside it.
    pub regions: usize,
    pub regions_outside: usize,
    /// Effects placed through the regions and through the model rows.
    pub region_effects: usize,
    pub model_effects: usize,
    /// Placed objects (Sekiro) or assets (Elden Ring) whose model effect row was named, and parts of models that cannot name
    /// one.
    pub parts_with_row: usize,
    pub parts_without_row: usize,
    /// Parts with a row that are not placed, by reason.
    pub parts_not_placed: BTreeMap<String, usize>,
    /// Effect files read (the placed ones and the ones they spawn), textures read into the world, and the numbers a file named
    /// that no binder searched held (by kind), and models and animations named (not read).
    pub files: usize,
    pub textures: usize,
    pub missing: BTreeMap<String, usize>,
    pub models_named: usize,
    pub animations_named: usize,
    /// References that placed nothing, by reason (counted once per effect file or row), and the placements left unplaced
    /// because their effect file was not found.
    pub refused: BTreeMap<String, usize>,
    pub unplaced: usize,
    /// Why the model effect param could not be read, if it could not.
    pub param_unresolved: Option<String>,
    /// The effects the events of a clip start ([`read_for_clip`]).
    pub clip_uses: usize,
}

impl Report {
    fn refuse(&mut self, reason: impl ToString) {
        *self.refused.entry(reason.to_string()).or_default() += 1;
    }
}

/// An effect to put where a transform says.
#[derive(Clone, Debug)]
pub struct Wanted {
    pub effect: i32,
    pub transform: Transform,
    pub place: EffectPlace,
    pub starts_disabled: bool,
    /// The label of the entity's `core.source`.
    pub label: String,
}

struct Loaded {
    binder: Binder,
    effects: BTreeMap<i32, usize>,
    resources: Resources,
}

/// What libraries have read, kept from one to the next: the binders (decoded and indexed, by path) and the effect files found
/// (by number). A reader that loads many clips keeps one, so the common binder is read once, not once for each clip.
#[derive(Default)]
pub struct EffectCache {
    binders: BTreeMap<String, Arc<Loaded>>,
    /// The effects found or refused; one that no binder searched holds is not kept (a later library may search more).
    files: BTreeMap<i32, std::result::Result<Option<Arc<Fxr>>, String>>,
}

impl std::fmt::Debug for EffectCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectCache")
            .field("binders", &self.binders.len())
            .field("files", &self.files.len())
            .finish()
    }
}

/// The effect binders an area's effects and textures are looked for in.
pub struct EffectLibrary<'a> {
    archive: &'a Archive,
    decoder: &'a Decoder<'a>,
    game: Game,
    paths: Vec<String>,
    cache: EffectCache,
    /// The numbers none of this library's binders holds.
    missing: BTreeSet<i32>,
}

impl std::fmt::Debug for EffectLibrary<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectLibrary")
            .field("game", &self.game)
            .field("paths", &self.paths)
            .field("missing", &self.missing.len())
            .finish()
    }
}

/// The binders to search for the effects the animations of one character's `.tae` member (`c4130.tae`) start: Elden Ring's own
/// character binder (`sfxbnd_c4130`) first where there is one, then the common ones, and in Sekiro every map binder after them
/// (a Sekiro boss starts effects of the binder of the map it stands on, and no table of the files links the two). Only the
/// binders the archive holds are searched ([`EffectLibrary`]).
pub fn binders_for_clip(game: Game, file: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let digits = file
        .strip_prefix('c')
        .map(|rest| rest.chars().take_while(char::is_ascii_digit).count())
        .unwrap_or(0);
    if game == Game::EldenRing && digits == 4 {
        paths.push(format!("/sfx/sfxbnd_{}.ffxbnd.dcx", &file[..5]));
    }
    paths.push("/sfx/sfxbnd_commoneffects.ffxbnd.dcx".to_owned());
    if game == Game::EldenRing {
        paths.push("/sfx/sfxbnd_commoneffects_dlc02.ffxbnd.dcx".to_owned());
    } else {
        paths.extend((0..100).map(|n| format!("/sfx/sfxbnd_m{n:02}.ffxbnd.dcx")));
    }
    paths
}

/// The binders to search for an area's effects, those the archive holds: the area's own, then the common ones.
pub fn binders_for_area(game: Game, area: &str) -> Vec<String> {
    let mut paths = vec![format!(
        "/sfx/sfxbnd_{}.ffxbnd.dcx",
        area.get(..3).unwrap_or(area)
    )];
    if game == Game::EldenRing {
        paths.push(format!("/sfx/sfxbnd_{area}.ffxbnd.dcx"));
    }
    paths.push("/sfx/sfxbnd_commoneffects.ffxbnd.dcx".to_owned());
    if game == Game::EldenRing {
        paths.push("/sfx/sfxbnd_commoneffects_dlc02.ffxbnd.dcx".to_owned());
    }
    paths
}

impl<'a> EffectLibrary<'a> {
    /// A library over the binders `paths` (in search order) that the archive holds.
    pub fn new(archive: &'a Archive, decoder: &'a Decoder<'a>, game: Game, paths: Vec<String>) -> Self {
        Self::with_cache(archive, decoder, game, paths, EffectCache::default())
    }

    /// A library over the binders `paths` that the archive holds, starting from what `cache` already read ([`Self::into_cache`]).
    pub fn with_cache(
        archive: &'a Archive,
        decoder: &'a Decoder<'a>,
        game: Game,
        mut paths: Vec<String>,
        cache: EffectCache,
    ) -> Self {
        paths.retain(|p| archive.contains(p));
        EffectLibrary {
            archive,
            decoder,
            game,
            paths,
            cache,
            missing: BTreeSet::new(),
        }
    }

    /// What this library read, to start the next from.
    pub fn into_cache(self) -> EffectCache {
        self.cache
    }

    fn load(&mut self, i: usize) -> Result<Arc<Loaded>> {
        let path = &self.paths[i];
        if let Some(loaded) = self.cache.binders.get(path) {
            return Ok(loaded.clone());
        }
        let bytes = self.archive.read(path, self.decoder)?;
        let binder = Binder::read(&bytes, self.decoder)?;
        let effects = binder
            .files
            .iter()
            .enumerate()
            .filter_map(|(k, f)| Some((fxr::member_id(f.name.as_deref()?)?, k)))
            .collect();
        let resources = Resources::index(&binder)?;
        let loaded = Arc::new(Loaded {
            binder,
            effects,
            resources,
        });
        self.cache.binders.insert(path.clone(), loaded.clone());
        Ok(loaded)
    }

    /// The effect `id`: the first binder searched that holds it, `Ok(None)` where none does, `Err` with the reason where the
    /// file does not read.
    pub fn effect(&mut self, id: i32) -> Result<std::result::Result<Option<Arc<Fxr>>, String>> {
        if let Some(found) = self.cache.files.get(&id) {
            return Ok(found.clone());
        }
        if self.missing.contains(&id) {
            return Ok(Ok(None));
        }
        let mut result = Ok(None);
        for i in 0..self.paths.len() {
            let loaded = self.load(i)?;
            if let Some(&k) = loaded.effects.get(&id) {
                result = Fxr::read(&loaded.binder.files[k].bytes)
                    .map(|f| Some(Arc::new(f)))
                    .map_err(|e| e.to_string());
                break;
            }
        }
        if matches!(result, Ok(None)) {
            self.missing.insert(id);
        } else {
            self.cache.files.insert(id, result.clone());
        }
        Ok(result)
    }

    /// The decoded image of the texture of `kind` and number `id` in the first binder that holds it.
    pub fn texture(
        &mut self,
        kind: fxr::resources::TextureKind,
        id: i32,
    ) -> Result<Option<primase_core::Texture>> {
        for i in 0..self.paths.len() {
            let game = self.game;
            let decoder = self.decoder;
            let loaded = self.load(i)?;
            if let Some(k) = loaded.resources.texture(game, kind, id) {
                let bytes = decoder.decode_if_dcx(&loaded.binder.files[k].bytes)?;
                let package = Package::read(&bytes)?;
                let Some(texture) = package.textures.first() else {
                    continue;
                };
                return tpf_image(texture).map(Some);
            }
        }
        Ok(None)
    }
}

/// Every effect named by the actions of `node`'s tree: its resources, by kind and number.
fn resources_of(node: &TypedNode, out: &mut BTreeSet<(ResourceKind, i32)>) {
    let mut note = |a: &Typed| {
        for r in a.resources() {
            let Some(id) = r.id.filter(|&id| id > 0) else {
                continue;
            };
            let kind = match r.kind {
                FileKind::Texture(k) => ResourceKind::of_texture(k),
                FileKind::Model => ResourceKind::Model,
                FileKind::Animation => ResourceKind::Animation,
                FileKind::Sound => continue,
            };
            out.insert((kind, id));
        }
    };
    for a in &node.actions {
        note(a);
    }
    for c in &node.configs {
        for a in &c.actions {
            note(a);
        }
    }
    for child in &node.children {
        resources_of(child, out);
    }
}

/// Reads the files the effects `roots` need into `world`: the effect files (once each, with the effects they spawn, an
/// `fromsoft.effect_file` entity each) and the textures they name (a `core.texture` each). The files read, by number.
pub fn read_files(
    library: &mut EffectLibrary<'_>,
    roots: Vec<i32>,
    world: &mut World,
    report: &mut Report,
) -> Result<BTreeMap<i32, Arc<Fxr>>> {
    let game = library.game;
    // the effect files: the roots with the effects they spawn
    let mut files: BTreeMap<i32, Arc<Fxr>> = BTreeMap::new();
    let mut stack: Vec<i32> = roots;
    stack.sort_unstable();
    stack.dedup();
    let mut queued: BTreeSet<i32> = stack.iter().copied().collect();
    while let Some(id) = stack.pop() {
        match library.effect(id)? {
            Ok(Some(fx)) => {
                for spawned in fx.spawned() {
                    if queued.insert(spawned) {
                        stack.push(spawned);
                    }
                }
                files.insert(id, fx);
            }
            Ok(None) => report.refuse("effect not in the binders searched (the area's, the common ones)"),
            Err(why) => report.refuse(format!("effect file refused: {why}")),
        }
    }
    for (id, fx) in &files {
        let e = world.spawn();
        world.insert(
            e,
            EffectFile {
                effect: *id,
                game,
                file: Arc::clone(fx),
            },
        );
        world.insert(e, Source(format!("fxr {id}")));
    }
    report.files += files.len();
    // the files the effects name
    let mut named: BTreeSet<(ResourceKind, i32)> = BTreeSet::new();
    for fx in files.values() {
        let tree = fx
            .typed(game)
            .map_err(|e| crate::invalid("effect file", 0, format!("effect {}: {e}", fx.id)))?;
        resources_of(&tree, &mut named);
    }
    for (kind, id) in named {
        match kind.texture() {
            Some(texture_kind) => match library.texture(texture_kind, id)? {
                Some(texture) => {
                    let e = world.spawn();
                    world.insert(e, texture);
                    world.insert(e, EffectResource { kind, id });
                    world.insert(e, Name(format!("sfx texture {id} ({kind:?})")));
                    report.textures += 1;
                }
                None => {
                    *report
                        .missing
                        .entry(format!("{kind:?} not in the binders searched"))
                        .or_default() += 1
                }
            },
            None if kind == ResourceKind::Model => report.models_named += 1,
            None => report.animations_named += 1,
        }
    }
    Ok(files)
}

/// Reads the files the `wanted` effects need into `world` ([`read_files`]) and places the effects: one `fromsoft.effect`
/// entity per wanted effect at its transform.
pub fn place_all(
    library: &mut EffectLibrary<'_>,
    wanted: Vec<Wanted>,
    world: &mut World,
    report: &mut Report,
) -> Result<()> {
    let files = read_files(library, wanted.iter().map(|w| w.effect).collect(), world, report)?;
    for w in wanted {
        if !files.contains_key(&w.effect) {
            report.unplaced += 1;
            continue;
        }
        let e = world.spawn();
        world.insert(e, w.transform);
        world.insert(e, Source(w.label));
        world.insert(
            e,
            Effect {
                effect: w.effect,
                place: w.place,
                starts_disabled: w.starts_disabled,
            },
        );
    }
    Ok(())
}

/// The effects of an area to place: the SFX regions of `layout` inside `region` (by position, as the lights are), then the
/// effects of the model rows of `models`. `param` is the model effect param.
fn wanted_for_area(
    game: Game,
    layout: &MapLayout,
    region: Option<[f32; 4]>,
    models: &[PlacedModel<'_>],
    param: Option<&ModelSfxParam>,
    report: &mut Report,
) -> Result<Vec<Wanted>> {
    let mut out = Vec::new();
    let map = map_transform(layout)?;
    let inside =
        |p: [f32; 3]| region.is_none_or(|r| p[0] >= r[0] && p[0] < r[2] && -p[2] >= r[1] && -p[2] < r[3]);
    for r in layout.map_regions()? {
        let Some(sfx) = layout.sfx_region(&r)? else {
            continue;
        };
        if !inside(r.position) {
            report.regions_outside += 1;
            continue;
        }
        report.regions += 1;
        report.region_effects += 1;
        let pose = placed(euler_matrix(r.position, r.rotation_degrees, [1.; 3], 0)?);
        out.push(Wanted {
            effect: sfx.effect_id,
            transform: map.compose(&pose),
            place: EffectPlace::Region {
                region: r.name.clone(),
            },
            starts_disabled: sfx.start_disabled,
            label: format!("{}/fxr {}", r.name, sfx.effect_id),
        });
    }
    if let Some(param) = param {
        for m in models {
            model_effects(game, param, m, &mut out, report);
        }
    }
    Ok(out)
}

/// The effects of one placed model: each used slot of its row at each dummy polygon the slot names.
fn model_effects(
    game: Game,
    param: &ModelSfxParam,
    m: &PlacedModel<'_>,
    out: &mut Vec<Wanted>,
    report: &mut Report,
) {
    let row_id = match row_of(game, m.model, m.relative_id) {
        Err(()) => {
            report.parts_without_row += 1;
            return;
        }
        Ok(None) => return,
        Ok(Some(id)) => id,
    };
    let mut rows = param.rows_with(row_id);
    let (Some(row), None) = (rows.next(), rows.next()) else {
        if param.rows_with(row_id).next().is_some() {
            report.refuse("model effect row stored twice");
        } else if game == Game::EldenRing {
            report.refuse("model effect row named but absent from the param");
        }
        return;
    };
    report.parts_with_row += 1;
    let (transform, dummies): (&Transform, &ModelDummies) = match &m.placement {
        Placement::Placed { transform, dummies } => (transform, dummies),
        Placement::Unplaced(why) => {
            *report.parts_not_placed.entry((*why).to_owned()).or_default() += 1;
            return;
        }
    };
    for (slot, s) in row.slots.iter().enumerate() {
        if s.effect_id <= 0 {
            continue;
        }
        let Some(places) = dummies.0.get(&s.dummy_id) else {
            report.refuse("slot's dummy polygon absent from the model");
            continue;
        };
        for dummy in places {
            report.model_effects += 1;
            out.push(Wanted {
                effect: s.effect_id,
                transform: transform.compose(&placed(dummy.map(|c| c.map(|x| x as f32)))),
                place: EffectPlace::Model {
                    part: m.part.to_owned(),
                    model: m.model.to_owned(),
                    row: row_id,
                    slot,
                    dummy: s.dummy_id,
                },
                starts_disabled: false,
                label: format!("{}/slot {slot}/fxr {}", m.part, s.effect_id),
            });
        }
    }
}

/// Reads the effects of the area into `world`: the SFX regions of `layout` inside `region`, then the effects of `models`.
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    region: Option<[f32; 4]>,
    models: &[PlacedModel<'_>],
    world: &mut World,
) -> Result<Report> {
    let mut report = Report {
        asked: true,
        ..Report::default()
    };
    let game = install.game;
    let param = if models.is_empty() {
        None
    } else {
        match fromsoft_formats::game_params::read_model_sfx_param(install, archive, decoder) {
            Ok(p) => Some(p),
            Err(e) => {
                report.param_unresolved = Some(e.to_string());
                None
            }
        }
    };
    let wanted = wanted_for_area(game, layout, region, models, param.as_ref(), &mut report)?;
    let mut library = EffectLibrary::new(archive, decoder, game, binders_for_area(game, area));
    place_all(&mut library, wanted, world, &mut report)?;
    Ok(report)
}

/// Reads the effects the events of clip `clip` start (its `fromsoft.clip_events` in `world`) from the effect binders of its
/// character ([`binders_for_clip`]): the effect files and the textures they name ([`read_files`]), and on the clip a
/// `fromsoft.clip_effect_uses` saying which events start which effect. A clip with no events, or whose events start no effect,
/// is left as it is. `cache` keeps the binders read from one clip to the next.
///
/// An event's number is the effect's number, as the game takes it: an event whose number is no file of the binders searched is
/// counted as lost ("no effect file with this id"), not read as another. The one number the game changes is a blade's
/// (event 118) in Elden Ring: the handler adds the offset the character's data keeps for the hand ([`TraceOffsets`], stated
/// by the caller; none active is 0) to the event's number before it starts the effect, and that is the number read here.
pub fn read_for_clip(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    world: &mut World,
    clip: Entity,
    trace_offsets: TraceOffsets,
    cache: &mut EffectCache,
) -> Result<Report> {
    let mut report = Report {
        asked: true,
        ..Report::default()
    };
    let Some(source) = world.get::<SourceEvents>(clip) else {
        return Ok(report);
    };
    let mut found = clip_effect_uses(source);
    if found.uses.is_empty() && found.lost.is_empty() {
        return Ok(report);
    }
    if game == Game::EldenRing {
        apply_trace_offsets(&mut found.uses, trace_offsets);
    }
    let binders = binders_for_clip(game, &source.file);
    if !found.uses.is_empty() {
        let mut library = EffectLibrary::with_cache(archive, decoder, game, binders, std::mem::take(cache));
        let roots: Vec<i32> = found.uses.iter().map(|u| u.effect).collect();
        let read = read_files(&mut library, roots, world, &mut report);
        *cache = library.into_cache();
        let files = read?;
        let before = found.uses.len();
        found.uses.retain(|u| files.contains_key(&u.effect));
        let unmatched = u32::try_from(before - found.uses.len()).unwrap_or(u32::MAX);
        if unmatched > 0 {
            found
                .lost
                .push(("no effect file with this id".to_owned(), unmatched));
        }
    }
    report.clip_uses = found.uses.len();
    world.insert(clip, found);
    Ok(report)
}

/// One effect by its number, standing at the origin (an effect viewer's world): the effect file, the effects it spawns and the
/// textures they name, and an entity placing the effect at the identity transform. `binders` are searched in order
/// ([`binders_for_area`] for a map's, or the one binder the caller knows holds the effect).
pub fn read_effect(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    binders: Vec<String>,
    effect: i32,
    world: &mut World,
) -> Result<(Report, Option<Entity>)> {
    let mut report = Report {
        asked: true,
        ..Report::default()
    };
    let before: BTreeSet<Entity> = world.entities().collect();
    let mut library = EffectLibrary::new(archive, decoder, game, binders);
    let wanted = vec![Wanted {
        effect,
        transform: Transform::IDENTITY,
        place: EffectPlace::Standalone,
        starts_disabled: false,
        label: format!("fxr {effect}"),
    }];
    place_all(&mut library, wanted, world, &mut report)?;
    let placed = world
        .iter::<Effect>()
        .map(|(e, _)| e)
        .find(|e| !before.contains(e));
    Ok((report, placed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binders searched for a character's clip effects, by the names of the install's binders: Elden Ring's own
    /// `sfxbnd_c4130` before the common ones (and none for a member that is not a character's), Sekiro's common binder before
    /// every map binder `sfxbnd_m00` to `sfxbnd_m99`.
    #[test]
    fn the_binders_of_a_characters_clip_effects_follow_the_games_naming() {
        assert_eq!(
            binders_for_clip(Game::EldenRing, "c4130.tae"),
            [
                "/sfx/sfxbnd_c4130.ffxbnd.dcx",
                "/sfx/sfxbnd_commoneffects.ffxbnd.dcx",
                "/sfx/sfxbnd_commoneffects_dlc02.ffxbnd.dcx",
            ]
        );
        assert_eq!(
            binders_for_clip(Game::EldenRing, "c0000_a00.tae")[0],
            "/sfx/sfxbnd_c0000.ffxbnd.dcx"
        );
        assert_eq!(
            binders_for_clip(Game::EldenRing, "").first().map(String::as_str),
            Some("/sfx/sfxbnd_commoneffects.ffxbnd.dcx")
        );
        let sekiro = binders_for_clip(Game::Sekiro, "c1010.tae");
        assert_eq!(sekiro.len(), 101);
        assert_eq!(sekiro[0], "/sfx/sfxbnd_commoneffects.ffxbnd.dcx");
        assert_eq!(sekiro[1], "/sfx/sfxbnd_m00.ffxbnd.dcx");
        assert_eq!(sekiro[100], "/sfx/sfxbnd_m99.ffxbnd.dcx");
    }
}
