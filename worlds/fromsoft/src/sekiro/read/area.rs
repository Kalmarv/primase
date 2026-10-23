//! Reading one Sekiro area ([`read_area`]): its placed models, materials, texture files, layered-shader programs,
//! collision files and bodies, lights, hang edges and spawn points, selected and in the world frame, as source records.
use super::{collision, extras, objects, shaders, shadows, states, textures};
use crate::sekiro::source::{self, Region};
use crate::{
    Archive, Binder, Result,
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
    /// Distinct placed models (`obj/<model>`, `<area>/<model>`) the archive does not hold.
    pub missing_models: Vec<String>,
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
        return Err(invalid("Sekiro area", 0, "expected mNN_NN or mNN_NN_NN_NN"));
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
        let p = crate::pose::point(
            t,
            std::array::from_fn(|i| bounds[usize::from(mask & (1 << i) != 0)][i]),
        );
        let p = [p[0], p[1], -p[2]];
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    max[0] >= r[0] - margin && min[0] < r[2] + margin && max[2] >= r[1] - margin && min[2] < r[3] + margin
}

/// A read model: the FLVER as stored, its resolved materials, its posed extent in the world frame, and for an object
/// its main HKX's name and decoded bytes, if its binder holds one.
struct Prototype {
    model: Model,
    materials: Vec<materials::Material>,
    bounds: Option<[[f32; 3]; 2]>,
    collision: Option<(String, Vec<u8>)>,
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
        if name.to_lowercase().ends_with(".mtd") {
            definitions.insert(
                materials::stem(name),
                MaterialDefinition::read(&decoder.decode_if_dcx(&f.bytes)?)?,
            );
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
    if destination.entities().next().is_some() {
        return Err(invalid("Sekiro area", 0, "destination must be empty"));
    }
    if options.region.is_some_and(|Region { rect: r, margin }| {
        !(margin.is_finite() && margin >= 0.)
            || !r.iter().all(|v| v.is_finite())
            || r[0] >= r[2]
            || r[1] >= r[3]
    }) {
        return Err(invalid("Sekiro area", 0, "invalid region"));
    }
    let area = area_name(area)?;
    let archive = Archive::open(install.as_ref())?;
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), decoder)?)?;
    let mut catalog = textures::Catalog::discover(&archive, decoder, &area[..3])?;
    let definitions = material_definitions(&archive, decoder)?;
    let mut report = Report::default();
    let mut placements = Vec::new();
    let mut requests = BTreeSet::new();
    for kind in [PartKind::MapPiece, PartKind::Object] {
        for p in layout.parts.iter().filter(|p| p.kind == kind) {
            let Some(model) = p.model.map(|i| layout.models[i].name.as_str()) else {
                continue;
            };
            let key = format!("{}/{model}", if kind == PartKind::Object { "obj" } else { &area });
            requests.insert(key.clone());
            placements.push((key, p));
        }
    }
    let mut prototypes = BTreeMap::new();
    for key in requests {
        let (kind, model) = key.split_once('/').expect("constructed key");
        let (path, leaf) = if kind == "obj" {
            (format!("/obj/{model}.objbnd.dcx"), format!("{model}.flver"))
        } else {
            let name = format!(
                "{area}_{}",
                model
                    .strip_prefix('m')
                    .ok_or_else(|| invalid("Sekiro model", 0, "map model prefix"))?
            );
            (format!("/map/{area}/{name}.mapbnd.dcx"), format!("{name}.flver"))
        };
        if !textures::has(&archive, &path) {
            report.missing_models.push(key);
            continue;
        }
        let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
        if kind == "obj" {
            for f in &binder.files {
                if f.bytes.starts_with(b"TPF\0") {
                    catalog.add(&f.bytes)?;
                }
            }
        }
        // a member by file name; `dcx`: also its compressed form
        let member = |leaf: &str, dcx: bool| {
            binder.files.iter().find(|f| {
                f.name
                    .as_deref()
                    .unwrap_or("")
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|s| s == leaf || dcx && s.strip_suffix(".dcx") == Some(leaf))
            })
        };
        let Some(f) = member(&leaf, false) else {
            report.missing_models.push(key);
            continue;
        };
        let collision = if kind == "obj" {
            let file = format!("{model}.hkx");
            member(&file, true)
                .map(|f| decoder.decode_if_dcx(&f.bytes))
                .transpose()?
                .map(|bytes| (file, bytes))
        } else {
            None
        };
        let model = Model::read(&f.bytes)?;
        let materials = model
            .materials
            .iter()
            .map(|m| {
                materials::resolve(m, definitions.get(&materials::stem(&m.mtd)), |n| {
                    catalog.entries.contains_key(n)
                })
            })
            .collect();
        let bounds = world_bounds(&model).map_err(|e| invalid("Sekiro model", 0, format!("{key}: {e}")))?;
        prototypes.insert(
            key,
            Prototype {
                model,
                materials,
                bounds,
                collision,
            },
        );
    }
    let placed = placements
        .iter()
        .map(|(key, p)| Ok((key.clone(), p.transform()?)))
        .collect::<Result<Vec<_>>>()?;
    let bounds = prototypes.iter().map(|(k, p)| (k.clone(), p.bounds)).collect();
    let selected: Vec<(String, usize, Transform)> = select_placements(&placed, &bounds, options, &mut report)
        .into_iter()
        .map(|i| (placed[i].0.clone(), placements[i].1.index, placed[i].1))
        .collect();
    let used: BTreeSet<&String> = selected.iter().map(|(key, _, _)| key).collect();
    prototypes.retain(|key, _| used.contains(key));

    let mut wanted = BTreeSet::new();
    let mut wanted_shaders = BTreeSet::new();
    let mut layered = Vec::new();
    for proto in prototypes.values() {
        for m in &proto.materials {
            *report.shader_parts.entry(m.shader.clone()).or_default() += 1;
            for b in m.bindings.values() {
                if catalog.entries.contains_key(&b.texture) {
                    wanted.insert(b.texture.clone());
                }
            }
            layered.extend(m.layered.as_ref());
            if m.definition.is_some() {
                wanted_shaders.insert(m.shader.clone());
            }
        }
    }
    let payloads = catalog.load(&archive, decoder, &wanted)?;
    let mut bundles = shaders::Bundles::default();
    bundles.read(&archive, decoder, &wanted_shaders)?;
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
        let far = key.starts_with(&format!("{area}/m9"));
        let e = world.spawn();
        world.insert(e, Name(key.split_once('/').expect("key").1.to_owned()));
        world.insert(e, crate::components::models::SourceModel(proto.model));
        world.insert(e, source::MaterialSlots(slots));
        world.insert(e, source::PrototypeSource { id: key.clone(), far });
        if key.starts_with("obj/") {
            report.object_collision.models += 1;
        }
        if let Some((file, bytes)) = proto.collision {
            let bodies = objects::bodies(&file, &bytes, &mut report.object_collision)
                .map_err(|e| invalid("Sekiro object collision", 0, format!("{key}: {e}")))?;
            world.insert(e, bodies);
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
    for (key, index, transform) in selected {
        let part = &layout.parts[index];
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
        world.insert(e, Source(format!("{area}:{}", part.name)));
        shadows::attach(&mut world, e, part.shadows, &mut report.shadows);
        report.placements += 1;
        report.far_placements += usize::from(far);
    }
    report.collision = collision::read(&archive, decoder, &area, &layout, states.as_mut(), &mut world)?;
    report.extras = extras::assemble(
        &archive,
        decoder,
        &area,
        &layout,
        options.region.map(|r| r.rect),
        &mut world,
    )?;
    world.insert_resource(source::ReadScope {
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
