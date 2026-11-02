//! The placements of an x/z box in one season: every `.pgeo` record (placed by rule) and every `.pvs` instance
//! (placed by hand) whose origin is in the box and that the season shows, each an entity with `core.transform` (the
//! game's frame converted to the world's, [`crate::FRAME`]), `core.instance` and `forzatech.placement`.
//!
//! Prototypes, one per model template (rule) or per list of LOD rows (hand): each level of detail is a form whose
//! `forzatech.model_files` name the `.modelbin` files its mesh is made of, read later ([`super::models`]). A
//! template's files are grouped into levels by their `Modl` header, not by the cluster number in their names (the two
//! differ); a file whose header states no level (`has LOD` 0) is the template's only level when it is the only kind
//! of file the template has, and a smashable's fragment, not drawn whole, when the template also has levelled files.
//! A hand-placed instance is in the box by the placed centre of its first LOD row's model bound (`hand_in_area`). Its
//! levels are its LOD rows, in order; a row whose model the build stripped is a form with no files, so the band rows
//! still index the forms.
//!
//! What is not read is counted, not guessed: a block whose name is no model template of its library, or names two
//! templates with different models (which one the game draws is not known), and a template or row list with no
//! model in the install.
use super::open::Track;
use super::textures::{TextureScope, TextureScopes};
use crate::components::{
    DrawRange, ForzaLevels, LodProfile, LodProfiles, ModelFile, ModelFiles, Placement, PlacementOrigin,
    Template,
};
use crate::{Error, FRAME, Result};
use forzatech_formats::bundle::Bundle;
use forzatech_formats::modelbin::Header;
use forzatech_formats::pgeo::{self, Pgeo};
use forzatech_formats::plib::{Library, ModelTemplate};
use forzatech_formats::pvs::{CATALOGUE, NO_ROW, Pvs};
use forzatech_formats::track::Location;
use forzatech_formats::world::{Area, Cell, Season, SeasonMask, half};
use primase_core::{Entity, Instance, LodForm, Name, Transform, World};
use std::collections::{BTreeMap, HashMap};

/// What a read covers: the box the placements' origins are in (the game's x and z, metres), the season, and the stored
/// tiers of each texture it loads: of the far-field terrain textures (`terrain_textures`) and of every other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scope {
    pub area: Area,
    pub season: Season,
    pub textures: TextureScope,
    pub terrain_textures: TextureScope,
}

impl Scope {
    /// The texture scopes this read loads by.
    pub fn texture_scopes(&self) -> TextureScopes {
        TextureScopes {
            textures: self.textures,
            terrain: self.terrain_textures,
        }
    }
}

/// What a placement read found and left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlacementReport {
    /// `.pgeo` files of the track, those whose header box can hold a placement in the area, and those of them shown
    /// in the season.
    pub pgeo_files: usize,
    pub pgeo_in_area: usize,
    pub pgeo_in_season: usize,
    /// Rule-placed records placed, by library category.
    pub rule_placed: BTreeMap<String, usize>,
    /// Hand-placed instances placed, and those the season hides.
    pub hand_placed: usize,
    pub hand_out_of_season: usize,
    /// Records whose block names no model template of its library, by `category block`.
    pub no_template: BTreeMap<String, usize>,
    /// Records whose block names templates with different models, by `category block`.
    pub ambiguous_template: BTreeMap<String, usize>,
    /// Records or instances whose template or rows have no `.modelbin` at all, by name.
    pub no_model: BTreeMap<String, usize>,
    /// Prototypes made, and the files of smashable fragments not made forms.
    pub prototypes: usize,
    pub fragments: usize,
}

impl PlacementReport {
    /// The report's log lines.
    pub fn lines(&self) -> Vec<(String, String)> {
        let sum = |m: &BTreeMap<String, usize>| m.values().sum::<usize>();
        let mut lines = vec![
            (
                "forzatech placements".into(),
                format!(
                    "{} by rule ({}), {} by hand ({} hidden in the season); {} prototypes",
                    sum(&self.rule_placed),
                    self.rule_placed
                        .iter()
                        .map(|(c, n)| format!("{c} {n}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    self.hand_placed,
                    self.hand_out_of_season,
                    self.prototypes
                ),
            ),
            (
                "forzatech pgeo".into(),
                format!(
                    "{} files, {} can hold a placement in the area, {} of them shown in the season",
                    self.pgeo_files, self.pgeo_in_area, self.pgeo_in_season
                ),
            ),
        ];
        for (what, map) in [
            ("no model template", &self.no_template),
            ("two model templates", &self.ambiguous_template),
            ("no modelbin", &self.no_model),
        ] {
            if !map.is_empty() {
                lines.push((
                    format!("forzatech left out: {what}"),
                    format!("{} placements of {} names", sum(map), map.len()),
                ));
            }
        }
        if self.fragments > 0 {
            lines.push((
                "forzatech smashable fragments".into(),
                format!(
                    "{} files not read: fragments of a template that has levelled files",
                    self.fragments
                ),
            ));
        }
        lines
    }
}

/// The prototypes a read made, by what they are made from.
#[derive(Default)]
struct Prototypes {
    /// By template category and name.
    templates: HashMap<(String, String), Option<Entity>>,
    /// By LOD row models and the row drawn in the nearest band.
    rows: HashMap<(Vec<u32>, Option<u8>), Option<Entity>>,
    /// Model headers read, by entry.
    files: HashMap<Location, ModelFile>,
}

/// Read the placements of `scope` from `track` into `world`.
pub fn read_placements(track: &Track, scope: &Scope, world: &mut World) -> Result<PlacementReport> {
    let mut report = PlacementReport::default();
    let mut protos = Prototypes::default();
    let libraries = libraries(track, scope.season)?;
    read_rule(track, scope, &libraries, world, &mut protos, &mut report)?;
    read_hand(track, scope, world, &mut protos, &mut report)?;
    report.prototypes = protos
        .templates
        .values()
        .chain(protos.rows.values())
        .flatten()
        .count();
    Ok(report)
}

/// The season's template libraries, by category.
fn libraries(track: &Track, season: Season) -> Result<HashMap<String, Library>> {
    let dir = format!("Ribbon_00/instancetemplates/{}/objects", season.folder());
    let mut libs = HashMap::new();
    for (name, bytes) in track.files_in(&dir, "plib")? {
        let lib = Library::parse(&bytes).map_err(|e| Error::Track(format!("{dir}/{name}: {e}")))?;
        if let Some(first) = libs.insert(lib.category.clone(), lib) {
            return Err(Error::Track(format!("two {} libraries in {dir}", first.category)));
        }
    }
    Ok(libs)
}

fn read_rule(
    track: &Track,
    scope: &Scope,
    libraries: &HashMap<String, Library>,
    world: &mut World,
    protos: &mut Prototypes,
    report: &mut PlacementReport,
) -> Result<()> {
    let files: Vec<(Location, String)> = track
        .paths()
        .filter(|(_, p)| p.ends_with(".pgeo") && Cell::from_path(p).is_some())
        .map(|(at, p)| (at, p.to_owned()))
        .collect();
    report.pgeo_files = files.len();
    for (at, path) in files {
        let bytes = track.read(at)?;
        let header = pgeo::Header::parse(&bytes).map_err(|e| Error::Track(format!("{path}: {e}")))?;
        if !header.may_hold(&scope.area) {
            continue;
        }
        report.pgeo_in_area += 1;
        if !header.season_exclusion.shows(scope.season) {
            continue;
        }
        report.pgeo_in_season += 1;
        let file = Pgeo::parse(&bytes).map_err(|e| Error::Track(format!("{path}: {e}")))?;
        let lib = libraries.get(&header.category);
        for (block, k, record) in file.instances_in(&scope.area) {
            let key = format!("{} {}", header.category, block.name);
            let Some(lib) = lib else {
                *report.no_template.entry(key).or_default() += 1;
                continue;
            };
            let mut templates: Vec<&ModelTemplate> = Vec::new();
            for t in lib.model_templates_named(&block.name) {
                if !templates.iter().any(|d| d.models == t.models) {
                    templates.push(t);
                }
            }
            let template = match templates.as_slice() {
                [] => {
                    *report.no_template.entry(key).or_default() += 1;
                    continue;
                }
                [t] => *t,
                _ => {
                    *report.ambiguous_template.entry(key).or_default() += 1;
                    continue;
                }
            };
            let proto_key = (header.category.clone(), template.name.clone());
            let proto = match protos.templates.get(&proto_key) {
                Some(p) => *p,
                None => {
                    let p =
                        template_prototype(track, lib, &header.category, template, world, protos, report)?;
                    protos.templates.insert(proto_key, p);
                    p
                }
            };
            let Some(prototype) = proto else {
                *report.no_model.entry(key).or_default() += 1;
                continue;
            };
            let [x, y, z] = record.metres();
            let [r0, r1] = record.rows;
            let r2 = cross(r0, r1);
            let [sx, sy, sz] = record.scale;
            let local = Transform {
                cols: [
                    r0.map(|v| v * sx),
                    r1.map(|v| v * sy),
                    r2.map(|v| v * sz),
                    [x as f32, y as f32, z as f32],
                ],
            };
            let e = world.spawn();
            world.insert(e, FRAME.transform(&local));
            world.insert(
                e,
                Instance {
                    prototype,
                    variant: u32::from(record.material_variant),
                },
            );
            world.insert(
                e,
                Placement {
                    origin: PlacementOrigin::Rule {
                        file: path.clone(),
                        category: header.category.clone(),
                        block: block.name.clone(),
                        record: u32::try_from(k).map_err(|_| Error::Track(format!("{path}: record {k}")))?,
                        route: header.route.clone(),
                        gameplay_tag: header.gameplay_tag.clone(),
                        variation: record.variation,
                        frame: record.frame,
                    },
                    season_exclusion: header.season_exclusion.byte(),
                    material_variant: record.material_variant,
                },
            );
            *report.rule_placed.entry(header.category.clone()).or_default() += 1;
        }
    }
    Ok(())
}

/// `a × b`, the third row of a rotation stored as its first two.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let [a, b] = [a.map(f64::from), b.map(f64::from)];
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
    .map(|v| v as f32)
}

/// A template's prototype: its files grouped into levels; `None` when it has no file in the install.
fn template_prototype(
    track: &Track,
    library: &Library,
    category: &str,
    template: &ModelTemplate,
    world: &mut World,
    protos: &mut Prototypes,
    report: &mut PlacementReport,
) -> Result<Option<Entity>> {
    let mut files = Vec::new();
    for path in &template.models {
        for m in track.index().template_models(path) {
            files.push(model_file(track, m.location, protos)?);
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    let levelled = files.iter().any(|f| f.has_lod);
    let mut levels: BTreeMap<u8, Vec<ModelFile>> = BTreeMap::new();
    for f in files {
        match (f.has_lod, levelled) {
            (true, _) => {
                let range = |v: i8| u8::try_from(v).map_err(|_| Error::Track(format!("{}: LOD {v}", f.path)));
                for level in range(f.min_lod)?..=range(f.max_lod)? {
                    levels.entry(level).or_default().push(f.clone());
                }
            }
            (false, false) => levels.entry(0).or_default().push(f),
            (false, true) => report.fragments += 1,
        }
    }
    let ranges = draw_ranges(library, template)?;
    let forms: Vec<(u8, ModelFiles)> = levels
        .into_iter()
        .map(|(level, files)| {
            (
                level,
                ModelFiles {
                    level: Some(level),
                    files,
                },
            )
        })
        .collect();
    let e = prototype(world, &template.name, forms, None);
    world.insert(
        e,
        Template {
            name: template.name.clone(),
            category: category.to_owned(),
            draw_behaviour: template.draw_behaviour.clone(),
            models: template.models.clone(),
            ranges,
        },
    );
    Ok(Some(e))
}

/// The ranges of the template's draw behaviour in the slots it draws a LOD set in.
fn draw_ranges(library: &Library, template: &ModelTemplate) -> Result<Vec<DrawRange>> {
    let mut behaviours = library.draw_behaviours_named(&template.draw_behaviour);
    let behaviour = behaviours.next().ok_or_else(|| {
        Error::Track(format!(
            "{}: no draw behaviour {:?}",
            template.name, template.draw_behaviour
        ))
    })?;
    if behaviours.next().is_some_and(|other| other != behaviour) {
        return Err(Error::Track(format!(
            "two draw behaviours {:?} that differ",
            template.draw_behaviour
        )));
    }
    let mut ranges = Vec::new();
    for (slot, (s, set)) in behaviour.slots.iter().zip(template.slot_sets).enumerate() {
        if set == 0 {
            continue;
        }
        let slot = u8::try_from(slot).map_err(|_| Error::Track(format!("slot {slot}")))?;
        ranges.extend(s.ranges.iter().map(|r| DrawRange {
            slot,
            start: r.start,
            end: r.end,
        }));
    }
    Ok(ranges)
}

/// A prototype entity with its levels: the first is its own mesh, the rest its forms (`forzatech.levels`, made
/// `core.lods` when loaded). `default` indexes them.
fn prototype(world: &mut World, name: &str, forms: Vec<(u8, ModelFiles)>, default: Option<usize>) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(name.to_owned()));
    let mut levels = forms.into_iter();
    let (detail, own) = levels.next().unwrap_or_default();
    world.insert(e, own);
    let mut others = Vec::new();
    for (detail, files) in levels {
        let form = world.spawn();
        world.insert(form, files);
        others.push(LodForm { detail, form });
    }
    if !others.is_empty() || default.is_some() {
        world.insert(
            e,
            ForzaLevels {
                detail,
                forms: others,
                default,
            },
        );
    }
    e
}

/// A `.modelbin` entry's header, read once.
fn model_file(track: &Track, at: Location, protos: &mut Prototypes) -> Result<ModelFile> {
    if let Some(f) = protos.files.get(&at) {
        return Ok(f.clone());
    }
    let path = track.path(at).unwrap_or_default().to_owned();
    let bytes = track.read(at)?;
    let bundle = Bundle::parse(&bytes).map_err(|e| Error::Track(format!("{path}: {e}")))?;
    let modl = bundle
        .tagged(forzatech_formats::modelbin::MODL)
        .next()
        .ok_or_else(|| Error::Track(format!("{path}: no Modl blob")))?;
    let h = Header::parse(modl).map_err(|e| Error::Track(format!("{path}: {e}")))?;
    let f = ModelFile {
        chunk: at.chunk,
        entry: at.entry,
        path,
        lod_flags: h.lod_flags,
        has_lod: h.has_lod,
        min_lod: h.min_lod,
        max_lod: h.max_lod,
        min: h.bounds.min,
        max: h.bounds.max,
    };
    protos.files.insert(at, f.clone());
    Ok(f)
}

fn read_hand(
    track: &Track,
    scope: &Scope,
    world: &mut World,
    protos: &mut Prototypes,
    report: &mut PlacementReport,
) -> Result<()> {
    let name = track
        .folder()
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::Track(format!("{}: no track name", track.folder().display())))?
        .to_ascii_lowercase();
    let relative = format!("Ribbon_00/{name}_00.pvs");
    let pvs = Pvs::parse(&track.file(&relative)?).map_err(|e| Error::Track(format!("{relative}: {e}")))?;
    let w = pvs
        .world
        .as_ref()
        .ok_or_else(|| Error::Track(format!("{relative}: no world sections")))?;
    world.insert_resource(LodProfiles(
        w.lod_profiles
            .iter()
            .map(|g| {
                g.iter()
                    .map(|p| LodProfile {
                        tier: p.tier,
                        pass: p.pass,
                        distances: p.bands.map(|b| half(b.distance)),
                        fades: p.bands.map(|b| half(b.fade)),
                    })
                    .collect()
            })
            .collect(),
    ));
    for (k, inst) in w.instances.iter().enumerate() {
        if inst.flags & CATALOGUE != 0 || !hand_in_area(w, k, inst, &scope.area) {
            continue;
        }
        let record = inst.gameplay.and_then(|g| w.gameplay.records.get(usize::from(g)));
        let mask = record.map_or(SeasonMask::new(0), |r| Some(r.season_exclusion));
        let mask = mask.ok_or_else(|| Error::Track(format!("{relative}: instance {k}'s season mask")))?;
        if !mask.shows(scope.season) {
            report.hand_out_of_season += 1;
            continue;
        }
        let t = w
            .transform_of(inst)
            .ok_or_else(|| Error::Track(format!("{relative}: instance {k} has no transform")))?;
        let models: Vec<u32> = w.rows_of(k).iter().map(|r| r.model).collect();
        let band_rows = inst.band_rows.map(|r| (r != NO_ROW).then_some(r));
        let key = (models.clone(), band_rows[0]);
        let proto = match protos.rows.get(&key) {
            Some(p) => *p,
            None => {
                let p = rows_prototype(track, &models, band_rows[0], world, protos)?;
                protos.rows.insert(key, p);
                p
            }
        };
        let Some(prototype) = proto else {
            *report.no_model.entry(format!("pvs rows {models:?}")).or_default() += 1;
            continue;
        };
        let m = t.matrix.map(half);
        let local = Transform {
            cols: [
                [m[0], m[1], m[2]],
                [m[3], m[4], m[5]],
                [m[6], m[7], m[8]],
                t.position,
            ],
        };
        let e = world.spawn();
        world.insert(e, FRAME.transform(&local));
        world.insert(
            e,
            Instance {
                prototype,
                variant: u32::from(t.material_variant),
            },
        );
        world.insert(
            e,
            Placement {
                origin: PlacementOrigin::Hand {
                    instance: u32::try_from(k)
                        .map_err(|_| Error::Track(format!("{relative}: instance {k}")))?,
                    flags: inst.flags,
                    parent: inst.parent,
                    band_rows,
                    gameplay_tag: record.map(|r| r.tag.clone()),
                    routes: record.map(|r| r.routes.clone()).unwrap_or_default(),
                    tint: t.tint,
                    lod_group: t.lod_group,
                },
                season_exclusion: mask.byte(),
                material_variant: t.material_variant,
            },
        );
        report.hand_placed += 1;
    }
    Ok(())
}

/// Whether a hand-placed instance is in `area`: the centre of its first LOD row's model bound, placed by its transform,
/// or its position when that model has no bound (the bound record is the sentinel: centre 0, half extents -1). Not its
/// position alone: the terrain clusters are placed at the origin with an identity transform, their geometry where it
/// stands in the world.
fn hand_in_area(
    w: &forzatech_formats::pvs::World,
    k: usize,
    inst: &forzatech_formats::pvs::Instance,
    area: &Area,
) -> bool {
    let Some(t) = w.transform_of(inst) else {
        return false;
    };
    let bound = w
        .rows_of(k)
        .first()
        .and_then(|r| w.model_bounds.get(usize::try_from(r.model).ok()?))
        .filter(|b| b.half_extent.map(half).iter().all(|h| *h >= 0.0));
    let [x, z] = match bound {
        Some(b) => {
            let m = t.matrix.map(half).map(f64::from);
            let c = b.centre.map(f64::from);
            let p = t.position.map(f64::from);
            // row vectors: placed = c · M + p
            [
                c[0] * m[0] + c[1] * m[3] + c[2] * m[6] + p[0],
                c[0] * m[2] + c[1] * m[5] + c[2] * m[8] + p[2],
            ]
        }
        None => [f64::from(t.position[0]), f64::from(t.position[2])],
    };
    area.contains(x, z)
}

/// A hand-placed instance's prototype: one level per LOD row; `None` when no row has a model in the install.
fn rows_prototype(
    track: &Track,
    models: &[u32],
    near: Option<u8>,
    world: &mut World,
    protos: &mut Prototypes,
) -> Result<Option<Entity>> {
    let mut forms = Vec::with_capacity(models.len());
    let mut name = None;
    for (row, &model) in models.iter().enumerate() {
        let detail = u8::try_from(row).map_err(|_| Error::Track(format!("{} LOD rows", models.len())))?;
        let files = match track.index().pvs_model(model) {
            Some(m) => {
                let f = model_file(track, m.location, protos)?;
                name.get_or_insert_with(|| stem(&f.path).to_owned());
                // a file of one level gives that level's meshes; one of several levels (or none stated) gives all
                let level = (f.has_lod && f.min_lod == f.max_lod)
                    .then(|| u8::try_from(f.min_lod).ok())
                    .flatten();
                ModelFiles {
                    level,
                    files: vec![f],
                }
            }
            None => ModelFiles::default(),
        };
        forms.push((detail, files));
    }
    let Some(name) = name else {
        return Ok(None);
    };
    Ok(Some(prototype(world, &name, forms, near.map(usize::from))))
}

/// A zipcache path's file name less `.modelbin`.
fn stem(path: &str) -> &str {
    let name = path.rsplit('\\').next().unwrap_or(path);
    name.strip_suffix(".modelbin").unwrap_or(name)
}
