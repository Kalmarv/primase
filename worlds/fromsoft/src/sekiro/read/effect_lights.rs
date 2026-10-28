//! Effect lights where they stand (`fromsoft.effect_light`): the point and spot lights of FXR effect files, reached
//! through the map's SFX regions (the region's pose) and through placed objects and assets (their model effect param
//! row, and the dummy polygon of the model the row names), with the effects they spawn (`SFXReference`) at the place of
//! the node that spawns them. Their interpretation (light units, which lights are drawn, a spot's axis) belongs to
//! `lower-fromsoft-lights`. The routes follow the data: an effect is looked for in the binder of the area's
//! map (`sfxbnd_<mNN>`, and Elden Ring's `sfxbnd_<full map name>` where there is one), then `sfxbnd_commoneffects` and
//! Elden Ring's `commoneffects_dlc02`; the copies of an effect in several binders are byte-identical.
use crate::pose::{bone_matrices, dummy_in_model, placed};
use crate::sekiro::source::{EffectLight, EffectPlace, EffectShape, EffectValue};
use fromsoft_formats::affine::{Matrix, compose};
use fromsoft_formats::flver::Model;
use fromsoft_formats::fxr::{self, Fxr, Light, Property};
use fromsoft_formats::msb::{MapLayout, euler_matrix};
use fromsoft_formats::param::ModelSfxParam;
use fromsoft_formats::{Archive, Binder, Game, Install, Result, dcx::Decoder};
use primase_core::{Source, Transform, World};
use std::collections::BTreeMap;
use std::rc::Rc;

/// The deepest chain of spawned effects followed: no installed effect nests more than a few (the census of the
/// light-bearing ones reaches at most three), so a deeper one is a cycle or a mistake and is counted, not followed.
const MAX_SPAWN_DEPTH: usize = 8;

/// What the read of the effect lights found and left: the counts of both routes and the references that placed no light,
/// by reason.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Whether the read was asked for ([`crate::sekiro::Options::effect_lights`]).
    pub asked: bool,
    /// SFX regions inside the read region, those outside it, and those whose effect holds a light (the effect or one it
    /// spawns).
    pub regions: usize,
    pub regions_outside: usize,
    pub regions_with_light: usize,
    /// Placed objects (Sekiro) or assets (Elden Ring) whose model effect row was named, and the used slots of those rows.
    pub parts_with_row: usize,
    pub slots: usize,
    /// The lights the used slots reach, each slot's effect once with the effects it spawns: the census of the route, over
    /// every part with a row, placed or not. `lost_lights` are those of parts not placed and of slots whose dummy polygon the
    /// model lacks, by reason; `slot_lights = placed_slot_lights + the sum of lost_lights`.
    pub slot_lights: usize,
    pub placed_slot_lights: usize,
    pub lost_lights: BTreeMap<String, usize>,
    /// Parts with a row that are not placed, by reason (`model not in the archive`, `model uses no vertex`, `placement
    /// outside the region`).
    pub parts_not_placed: BTreeMap<String, usize>,
    /// Dummy polygons the used slots attach to (a slot attaches to every dummy of the model with its reference id).
    pub attachments: usize,
    /// Parts of models that cannot name a row (an object model not of the `oNNNNNN` form, an asset not of the
    /// `AEGnnn_mmm` form), and Elden Ring assets that name none (relative id -1).
    pub parts_without_row_name: usize,
    pub parts_naming_no_row: usize,
    /// Rows `num * 1000 + 1..=4` of Sekiro's `ModelSfxParam` that exist for placed objects: which one an object uses
    /// is not stored in the object, so only `num * 1000` is read and these are counted.
    pub unread_rows: usize,
    /// Lights placed through each route, and the effect files read.
    pub region_lights: usize,
    pub model_lights: usize,
    pub effects_read: usize,
    /// Of the lights placed: those reached through a spawned effect, those whose node's config the effect's state 0 does
    /// not choose, those of a region that starts disabled.
    pub spawned_lights: usize,
    pub unchosen_lights: usize,
    pub disabled_lights: usize,
    /// References that placed nothing, by reason (`effect not in the binders searched`, `row absent`, ...), counted in
    /// regions, parts or slots as the reason says.
    pub refused: BTreeMap<String, usize>,
    /// Dummy polygons of the placed object models left out of the model route: one that names a bone the model lacks.
    pub dummies_left_out: usize,
    /// Why the model effect param could not be read, if it could not (no key).
    pub param_unresolved: Option<String>,
}
impl Report {
    /// The lights placed.
    pub fn lights(&self) -> usize {
        self.region_lights + self.model_lights
    }
    fn refuse(&mut self, reason: impl ToString, n: usize) {
        *self.refused.entry(reason.to_string()).or_default() += n;
    }
}

/// The effect files of an area's binders, found lazily by id.
struct Library<'a> {
    archive: &'a Archive,
    decoder: &'a Decoder<'a>,
    /// The binders to search, in order, each read at the first search that reaches it.
    paths: Vec<String>,
    loaded: Vec<Option<Loaded>>,
    found: BTreeMap<i32, std::result::Result<Option<Rc<Fxr>>, String>>,
}
struct Loaded {
    binder: Binder,
    index: BTreeMap<i32, usize>,
}
impl<'a> Library<'a> {
    fn new(archive: &'a Archive, decoder: &'a Decoder<'a>, game: Game, area: &str) -> Self {
        let mut paths = vec![format!("/sfx/sfxbnd_{}.ffxbnd.dcx", &area[..3])];
        if game == Game::EldenRing {
            paths.push(format!("/sfx/sfxbnd_{area}.ffxbnd.dcx"));
        }
        paths.push("/sfx/sfxbnd_commoneffects.ffxbnd.dcx".to_owned());
        if game == Game::EldenRing {
            paths.push("/sfx/sfxbnd_commoneffects_dlc02.ffxbnd.dcx".to_owned());
        }
        paths.retain(|p| archive.contains(p));
        let loaded = paths.iter().map(|_| None).collect();
        Library {
            archive,
            decoder,
            paths,
            loaded,
            found: BTreeMap::new(),
        }
    }
    /// The effect `id`: the first binder searched that holds it, `Ok(None)` where none does, `Err` with the reason where
    /// the file does not read.
    fn get(&mut self, id: i32, report: &mut Report) -> Result<std::result::Result<Option<Rc<Fxr>>, String>> {
        if !self.found.contains_key(&id) {
            let mut result = Ok(None);
            for (i, path) in self.paths.iter().enumerate() {
                if self.loaded[i].is_none() {
                    let binder = Binder::read(&self.archive.read(path, self.decoder)?, self.decoder)?;
                    let index = binder
                        .files
                        .iter()
                        .enumerate()
                        .filter_map(|(k, f)| Some((fxr::member_id(f.name.as_deref()?)?, k)))
                        .collect();
                    self.loaded[i] = Some(Loaded { binder, index });
                }
                let loaded = self.loaded[i].as_ref().expect("loaded above");
                if let Some(&k) = loaded.index.get(&id) {
                    result = Fxr::read(&loaded.binder.files[k].bytes)
                        .map(|f| Some(Rc::new(f)))
                        .map_err(|e| e.to_string());
                    report.effects_read += 1;
                    break;
                }
            }
            self.found.insert(id, result);
        }
        Ok(self.found[&id].clone())
    }
}

fn value(p: &Property) -> Result<EffectValue> {
    let keyframes = p.keyframes();
    let initial = p
        .constant()
        .or_else(|| keyframes.as_ref().and_then(|k| k.values.first().cloned()))
        .ok_or_else(|| {
            crate::invalid(
                "effect light",
                0,
                "a property that is neither constant nor keyframed",
            )
        })?;
    Ok(EffectValue {
        initial,
        settled: p.settled(),
        keyframes: keyframes.map_or(0, |k| k.positions.len()),
        modifiers: p.modifiers.len(),
    })
}

fn record(light: &Light, effect: i32, place: &EffectPlace) -> Result<EffectLight> {
    Ok(match light {
        Light::Point(l) => EffectLight {
            effect,
            place: place.clone(),
            shape: EffectShape::Point {
                radius: value(&l.radius)?,
            },
            diffuse: value(&l.diffuse_color)?,
            diffuse_multiplier: l.diffuse_multiplier.as_ref().map(value).transpose()?,
            specular: value(&l.specular_color)?,
            specular_multiplier: l.specular_multiplier.as_ref().map(value).transpose()?,
            separate_specular: l.separate_specular,
            shadows: l.shadows,
            flickers: l.flicker.enabled,
            fade_out_frames: l.fade_out_frames,
            starts_disabled: false,
            chosen_in_state_0: true,
            static_transforms: 0,
            via: Vec::new(),
        },
        Light::Spot(l) => EffectLight {
            effect,
            place: place.clone(),
            shape: EffectShape::Spot {
                near: value(&l.near)?,
                far: value(&l.far)?,
                radius_x: value(&l.radius_x)?,
                radius_y: value(&l.radius_y)?,
            },
            diffuse: value(&l.diffuse_color)?,
            diffuse_multiplier: Some(value(&l.diffuse_multiplier)?),
            specular: value(&l.specular_color)?,
            specular_multiplier: Some(value(&l.specular_multiplier)?),
            separate_specular: l.separate_specular,
            shadows: l.shadows,
            flickers: l.flicker.enabled,
            fade_out_frames: l.fade_out_frames,
            starts_disabled: false,
            chosen_in_state_0: true,
            static_transforms: 0,
            via: Vec::new(),
        },
    })
}

/// A light found under an effect: its record, and where it stands in the frame of the effect first named.
struct Found {
    record: EffectLight,
    matrix: Matrix,
}

/// The lights of effect `id` and of the effects it spawns, each with its place in the frame of `id` composed under
/// `base` (the file frame), in tree order.
fn collect(
    library: &mut Library<'_>,
    id: i32,
    base: &Matrix,
    place: &EffectPlace,
    via: &mut Vec<i32>,
    report: &mut Report,
    out: &mut Vec<Found>,
) -> Result<()> {
    if via.len() > MAX_SPAWN_DEPTH || via.contains(&id) {
        report.refuse("effects spawned too deep or in a cycle", 1);
        return Ok(());
    }
    let fx = match library.get(id, report)? {
        Ok(Some(fx)) => fx,
        Ok(None) => {
            report.refuse(
                "effect not in the binders searched (the area's, the common ones)",
                1,
            );
            return Ok(());
        }
        Err(why) => {
            report.refuse(format!("effect file refused: {why}"), 1);
            return Ok(());
        }
    };
    let lights = fx
        .lights()
        .map_err(|e| crate::invalid("effect light", 0, format!("effect {id}: {e}")))?;
    for pl in &lights {
        let transforms = pl.static_transforms(0)?;
        let mut record = record(&pl.light, id, place)?;
        record.static_transforms = transforms.len();
        record.chosen_in_state_0 = pl.active_in_state(0);
        record.via = via.clone();
        out.push(Found {
            record,
            matrix: compose(base, &pl.local_matrix(0)?),
        });
    }
    via.push(id);
    for spawn in fx.spawns()? {
        if !spawn.active_in_state(0) {
            continue;
        }
        let at = compose(base, &spawn.local_matrix(0)?);
        collect(library, spawn.effect, &at, place, via, report, out)?;
    }
    via.pop();
    Ok(())
}

fn core(m: &Matrix) -> Transform {
    placed(m.map(|c| c.map(|x| x as f32)))
}

/// What the model route needs of a placed model: its dummy polygons' places in the model, by reference id.
pub struct ModelDummies(pub BTreeMap<i32, Vec<Matrix>>);
impl ModelDummies {
    /// The dummies of `model` at rest; those that name a bone the model lacks are left out, counted.
    pub fn of(model: &Model) -> std::result::Result<(Self, usize), String> {
        let worlds = bone_matrices(&model.bones)?;
        let mut by_id: BTreeMap<i32, Vec<Matrix>> = BTreeMap::new();
        let mut left_out = 0;
        for d in &model.dummies {
            match dummy_in_model(&worlds, d) {
                Some(m) => by_id.entry(i32::from(d.reference_id)).or_default().push(m),
                None => left_out += 1,
            }
        }
        Ok((ModelDummies(by_id), left_out))
    }
}

/// Whether a part of the layouts is placed in the read: where it stands (core frame) and its model's dummies, or why it
/// is not.
pub enum Placement<'a> {
    Placed {
        transform: Transform,
        dummies: &'a ModelDummies,
    },
    Unplaced(&'static str),
}

/// An object or asset part of the layouts read, for the model route: its part and model names, the relative id an
/// Elden Ring asset stores, and where it stands. A part that is not placed still counts for what its row reaches.
pub struct PlacedModel<'a> {
    pub part: &'a str,
    pub model: &'a str,
    pub relative_id: Option<i16>,
    pub placement: Placement<'a>,
}

/// The row of `model` in the model effect param: Sekiro's `oNNNNNN` is `NNNNNN * 1000`, Elden Ring's `AEGnnn_mmm` with
/// relative id `r` is `(nnn * 1000 + mmm) * 1000 + r`. `Ok(None)`: the asset names no row (-1); `Err`: the model's name is
/// of no such form.
fn row_of(game: Game, model: &str, relative_id: Option<i16>) -> std::result::Result<Option<i32>, ()> {
    match game {
        Game::Sekiro => {
            let digits = model.strip_prefix('o').and_then(|m| m.get(..6)).ok_or(())?;
            let rest = &model[7..];
            if !digits.bytes().all(|b| b.is_ascii_digit()) || !(rest.is_empty() || rest.starts_with('_')) {
                return Err(());
            }
            Ok(digits.parse::<i32>().ok().map(|n| n * 1000))
        }
        Game::EldenRing => {
            let (n, m) = model
                .strip_prefix("AEG")
                .and_then(|m| m.split_once('_'))
                .ok_or(())?;
            if n.len() != 3 || m.len() != 3 {
                return Err(());
            }
            let key = n.parse::<i32>().map_err(|_| ())? * 1000 + m.parse::<i32>().map_err(|_| ())?;
            let relative = relative_id.ok_or(())?;
            Ok((relative >= 0).then(|| key * 1000 + i32::from(relative)))
        }
    }
}

/// The map's offset into the common frame, as the lights of bank zero take it.
pub(super) fn map_transform(layout: &MapLayout) -> Result<Transform> {
    if layout.map_offsets.len() > 1 {
        return Err(crate::invalid("effect lights", 0, "multiple map offsets"));
    }
    Ok(layout
        .map_offsets
        .first()
        .map(|o| o.matrix().map(placed))
        .transpose()?
        .unwrap_or(Transform {
            cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
        }))
}

/// Read the effect lights of the area into `world`: the SFX regions of `layout` inside `region` (by position, as the
/// bank-zero lights are), then the lights of `models`. `param` is the model effect param (`None`: unread, said in the
/// report).
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
    let mut library = Library::new(archive, decoder, game, area);
    let map = map_transform(layout)?;
    let inside =
        |p: [f32; 3]| region.is_none_or(|r| p[0] >= r[0] && p[0] < r[2] && -p[2] >= r[1] && -p[2] < r[3]);
    // the SFX regions
    for r in layout.map_regions()? {
        let Some(sfx) = layout.sfx_region(&r)? else {
            continue;
        };
        if !inside(r.position) {
            report.regions_outside += 1;
            continue;
        }
        report.regions += 1;
        let pose = placed(euler_matrix(r.position, r.rotation_degrees, [1.; 3], 0)?);
        let place = EffectPlace::Region {
            region: r.name.clone(),
        };
        let mut found = Vec::new();
        collect(
            &mut library,
            sfx.effect_id,
            &IDENTITY,
            &place,
            &mut Vec::new(),
            &mut report,
            &mut found,
        )?;
        let n = spawn_found(
            world,
            found,
            &map.compose(&pose),
            &r.name,
            sfx.start_disabled,
            area,
            &mut report,
        );
        report.region_lights += n;
        report.regions_with_light += usize::from(n > 0);
    }
    // the placed objects and assets, through their model effect row
    if !models.is_empty() {
        let param = match fromsoft_formats::game_params::read_model_sfx_param(install, archive, decoder) {
            Ok(p) => Some(p),
            Err(e) => {
                report.param_unresolved = Some(e.to_string());
                None
            }
        };
        if let Some(param) = &param {
            for m in models {
                model_lights(game, param, &mut library, m, area, world, &mut report)?;
            }
        }
    }
    Ok(report)
}

const IDENTITY: Matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]];

/// Spawn `found` as entities placed at `at` (core frame) composed with each light's place, counting them into `report`.
fn spawn_found(
    world: &mut World,
    found: Vec<Found>,
    at: &Transform,
    label: &str,
    starts_disabled: bool,
    area: &str,
    report: &mut Report,
) -> usize {
    let n = found.len();
    for (k, f) in found.into_iter().enumerate() {
        let mut record = f.record;
        record.starts_disabled = starts_disabled;
        report.spawned_lights += usize::from(!record.via.is_empty());
        report.unchosen_lights += usize::from(!record.chosen_in_state_0);
        report.disabled_lights += usize::from(starts_disabled);
        let e = world.spawn();
        world.insert(e, at.compose(&core(&f.matrix)));
        world.insert(
            e,
            Source(format!("{area}:{label}/fxr {} light {k}", record.effect)),
        );
        world.insert(e, record);
    }
    n
}

/// The lights of one placed model: its row, each used slot's effect at each dummy polygon the slot names.
fn model_lights(
    game: Game,
    param: &ModelSfxParam,
    library: &mut Library<'_>,
    m: &PlacedModel<'_>,
    area: &str,
    world: &mut World,
    report: &mut Report,
) -> Result<()> {
    let row_id = match row_of(game, m.model, m.relative_id) {
        Err(()) => {
            report.parts_without_row_name += 1;
            return Ok(());
        }
        Ok(None) => {
            report.parts_naming_no_row += 1;
            return Ok(());
        }
        Ok(Some(id)) => id,
    };
    if game == Game::Sekiro {
        report.unread_rows += (1..=4)
            .filter(|k| param.rows_with(row_id + k).next().is_some())
            .count();
    }
    let mut rows = param.rows_with(row_id);
    let (Some(row), None) = (rows.next(), rows.next()) else {
        if param.rows_with(row_id).next().is_some() {
            report.refuse("model effect row stored twice", 1);
        } else if game == Game::Sekiro {
            // most objects spawn nothing: Sekiro's param holds a row for 1,582 of the 10,538 placed ones
            report.parts_naming_no_row += 1;
        } else {
            report.refuse("model effect row named but absent from the param", 1);
        }
        return Ok(());
    };
    report.parts_with_row += 1;
    if let Placement::Unplaced(why) = &m.placement {
        *report.parts_not_placed.entry((*why).to_owned()).or_default() += 1;
    }
    for (slot, s) in row.slots.iter().enumerate() {
        if s.effect_id <= 0 {
            continue;
        }
        report.slots += 1;
        // what the slot's effect reaches, once
        let mut reached = Vec::new();
        let place = EffectPlace::Model {
            part: m.part.to_owned(),
            model: m.model.to_owned(),
            row: row_id,
            slot,
            dummy: s.dummy_id,
        };
        collect(
            library,
            s.effect_id,
            &IDENTITY,
            &place,
            &mut Vec::new(),
            report,
            &mut reached,
        )?;
        let lights = reached.len();
        report.slot_lights += lights;
        let (transform, dummies) = match &m.placement {
            Placement::Placed { transform, dummies } => (transform, dummies),
            Placement::Unplaced(why) => {
                if lights > 0 {
                    *report.lost_lights.entry((*why).to_owned()).or_default() += lights;
                }
                continue;
            }
        };
        let Some(places) = dummies.0.get(&s.dummy_id) else {
            report.refuse("slot's dummy polygon absent from the model", 1);
            if lights > 0 {
                *report
                    .lost_lights
                    .entry("dummy polygon absent from the model".to_owned())
                    .or_default() += lights;
            }
            continue;
        };
        report.placed_slot_lights += lights;
        for dummy in places {
            report.attachments += 1;
            let mut found = Vec::new();
            // already counted above: its refusals and reads are the same
            collect(
                library,
                s.effect_id,
                dummy,
                &place,
                &mut Vec::new(),
                &mut Report::default(),
                &mut found,
            )?;
            let label = format!("{}/slot {slot}", m.part);
            let n = spawn_found(world, found, transform, &label, false, area, report);
            report.model_lights += n;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A place composed in the core frame is the place composed in the file frame and converted once, which is how the
    /// lights are placed (the map and the region or dummy in the core frame, the effect's chain converted by itself).
    #[test]
    fn composing_in_the_core_frame_is_composing_in_the_file_frame() {
        let region = euler_matrix([1., 2., 3.], [10., 20., 30.], [1.; 3], 0).unwrap();
        let chain: Matrix = [[0., 1., 0.], [-1., 0., 0.], [0., 0., 1.], [0.5, 0., 5.]];
        let both = placed(compose(&region.map(|c| c.map(f64::from)), &chain).map(|c| c.map(|x| x as f32)));
        let parts = placed(region).compose(&core(&chain));
        for (a, b) in both.cols.iter().flatten().zip(parts.cols.iter().flatten()) {
            assert!((a - b).abs() < 1e-5, "{both:?} {parts:?}");
        }
        // a plain offset along the file's +Z is the core frame's -Z
        let straight = placed(euler_matrix([1., 2., 3.], [0.; 3], [1.; 3], 0).unwrap()).compose(&core(&[
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [0., 0., 5.],
        ]));
        assert_eq!(straight.cols[3], [1., 2., -8.]);
    }

    /// The model's key in the effect param: Sekiro's `oNNNNNN` is `NNNNNN * 1000`, Elden Ring's asset adds its relative id.
    #[test]
    fn a_model_names_its_row() {
        assert_eq!(row_of(Game::Sekiro, "o001230", None), Ok(Some(1_230_000)));
        assert_eq!(row_of(Game::Sekiro, "o001230_0001", None), Ok(Some(1_230_000)));
        assert_eq!(row_of(Game::Sekiro, "o12345", None), Err(()));
        assert_eq!(row_of(Game::Sekiro, "m11_00", None), Err(()));
        assert_eq!(
            row_of(Game::EldenRing, "AEG003_316", Some(1)),
            Ok(Some(3_316_001))
        );
        assert_eq!(row_of(Game::EldenRing, "AEG003_316", Some(-1)), Ok(None));
        assert_eq!(row_of(Game::EldenRing, "AEG003_316", None), Err(()));
        assert_eq!(row_of(Game::EldenRing, "AEG03_316", Some(0)), Err(()));
    }
}
