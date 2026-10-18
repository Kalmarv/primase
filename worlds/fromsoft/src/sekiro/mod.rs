//! Sekiro area reading: each placed model as its FLVER as stored, its materials as resolved source records, the
//! texture files and layered-shader programs they bind, raw collision, lights, hang edges and spawn points, with
//! typed source identities ([`source`]). Reading lowers nothing: geometry, materials, lights and extras become core
//! data through the passes in [`passes`], which a program chooses and orders.
pub mod blockers;
pub mod collision;
pub mod extras;
pub mod lights;
pub mod normals;
pub mod objects;
pub mod passes;
pub mod pbr;
mod shadows;
pub mod source;
pub mod states;
mod textures;
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

#[derive(Clone, Debug)]
pub struct Options {
    /// East/north rectangle in the source map frame that the read keeps: placements whose model's extent reaches it
    /// within `region_margin`, collision triangles whose centre is within 50 m of it, lights and spawn points inside
    /// it. None reads the whole area.
    pub region: Option<[f32; 4]>,
    /// Metres around the region within which a placement's extent must reach (the caller's choice; 30 m by default).
    pub region_margin: f32,
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
impl Default for Options {
    fn default() -> Self {
        Self {
            region: None,
            region_margin: 30.,
            states: StateClasses::None,
        }
    }
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
    pub collision: collision::Report,
    pub object_collision: objects::Report,
    pub prototypes: usize,
    /// Prototypes of the distant-backdrop series (`fromsoft.prototype` `far`), and their placements.
    pub far_prototypes: usize,
    pub far_placements: usize,
    pub placements: usize,
    pub materials: usize,
    /// Texture files kept as source payloads.
    pub textures: usize,
    /// Layered-shader programs kept.
    pub shader_programs: usize,
    pub missing_models: Vec<String>,
    /// With `states: Event`: the rules read and the parts decided or left at their default.
    pub states: Option<states::Report>,
    /// Placements of models whose face sets use no vertex.
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
        if !self.missing_models.is_empty() {
            let mut n = Notice::new(
                stage,
                NoticeKind::Unsupported,
                "placed models not found in the archive",
                self.missing_models.len() as u64,
            );
            for m in &self.missing_models {
                n = n.example(m.clone());
            }
            out.push(n);
        }
        for (count, reason) in [
            (
                self.skipped_outside,
                "placements outside the region and its margin",
            ),
            (
                self.collision.outside_region_triangles,
                "map collision triangles more than 50 m outside the region",
            ),
            (
                self.object_collision.outside_region_triangles,
                "object collision triangles more than 50 m outside the region",
            ),
        ] {
            if count > 0 {
                out.push(Notice::new(stage, NoticeKind::Selection, reason, count as u64));
            }
        }
        for (count, reason) in [
            (
                self.skipped_empty,
                "placements of models whose face sets use no vertex",
            ),
            (
                self.collision.excluded_material_triangles,
                "map collision triangles of blocker, cloud or water-surface materials not kept as ground",
            ),
            (
                self.object_collision.excluded_material_triangles,
                "object collision triangles of blocker, cloud or water-surface materials not kept",
            ),
            (
                self.object_collision.keyframed_left_out.len(),
                "keyframed object bodies (motion type 2) not kept",
            ),
            (
                self.collision.omitted_filters.values().sum::<usize>(),
                "collision parts of other hit filters than 8 and 27 not kept",
            ),
        ] {
            if count > 0 {
                out.push(Notice::new(stage, NoticeKind::Unsupported, reason, count as u64));
            }
        }
        if !self.collision.missing_files.is_empty() {
            let mut n = Notice::new(
                stage,
                NoticeKind::Unsupported,
                "collision files named by the MSB but not in the area's binder",
                self.collision.missing_files.len() as u64,
            );
            for f in &self.collision.missing_files {
                n = n.example(f.clone());
            }
            out.push(n);
        }
        if self.extras.missing_light_bank {
            out.push(Notice::new(stage, NoticeKind::Unsupported, "no light bank 0", 1));
        }
        if self.extras.missing_edge_binder {
            out.push(Notice::new(
                stage,
                NoticeKind::Unsupported,
                "no hang-edge binder",
                1,
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
fn point(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| t.cols[0][i] * p[0] + t.cols[1][i] * p[1] + t.cols[2][i] * p[2] + t.cols[3][i])
}
/// Whether a placement of a model with world-frame box `[min, max]` reaches the region within its margin.
fn inside(bounds: &[[f32; 3]; 2], t: &Transform, o: &Options) -> bool {
    let Some(r) = o.region else { return true };
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for mask in 0..8 {
        let p = point(
            t,
            std::array::from_fn(|i| bounds[usize::from(mask & (1 << i) != 0)][i]),
        );
        let p = [p[0], p[1], -p[2]];
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    max[0] >= r[0] - o.region_margin
        && min[0] < r[2] + o.region_margin
        && max[2] >= r[1] - o.region_margin
        && min[2] < r[3] + o.region_margin
}

/// A read model: the FLVER as stored, its resolved materials, and its posed extent in the world frame.
struct Prototype {
    model: Model,
    materials: Vec<materials::Material>,
    bounds: Option<[[f32; 3]; 2]>,
}

/// Read an area directly from installed archives into an empty world. The caller supplies its trusted
/// decompressor; no environment variables or install paths are guessed. On failure the destination world is
/// unchanged.
///
/// Each prototype holds its model as stored (`fromsoft.flver`), its source identity (`fromsoft.prototype`, with the
/// distant-backdrop class) and the material entity of each of its FLVER materials (`fromsoft.material_slots`, each
/// holding `fromsoft.material`). Placements are `core.instance` with their MSB transform, source label, shadow
/// controls and, with states read, conditions. Every texture a kept material binds is kept as stored
/// (`fromsoft.texture_payloads`), and the G-buffer programs of its layered materials (`fromsoft.shader_programs`).
/// Collision soups carry `fromsoft.collision`; lights are `fromsoft.light` records where they stand; hang edges and
/// spawn points are `fromsoft.area_extras`.
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
    if !options.region_margin.is_finite()
        || options.region_margin < 0.
        || options
            .region
            .is_some_and(|r| !r.iter().all(|v| v.is_finite()) || r[0] >= r[2] || r[1] >= r[3])
    {
        return Err(invalid("Sekiro area", 0, "invalid region"));
    }
    let area = area_name(area)?;
    let archive = Archive::open(install.as_ref())?;
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), decoder)?)?;
    let mut catalog = textures::Catalog::discover(&archive, decoder, &area[..3])?;
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
        let Some(f) = binder
            .files
            .iter()
            .find(|f| f.name.as_deref().unwrap_or("").rsplit(['/', '\\']).next() == Some(leaf.as_str()))
        else {
            report.missing_models.push(key);
            continue;
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
        let bounds = crate::rigid::posed_bounds(&model)
            .map_err(|e| invalid("Sekiro model", 0, format!("{key}: {e}")))?
            .map(|[min, max]| [[min[0], min[1], -max[2]], [max[0], max[1], -min[2]]]);
        prototypes.insert(
            key,
            Prototype {
                model,
                materials,
                bounds,
            },
        );
    }
    let mut selected = Vec::new();
    let mut used = BTreeSet::new();
    for (key, p) in placements {
        let Some(bounds) = prototypes.get(&key).and_then(|p| p.bounds.as_ref()) else {
            report.skipped_empty += 1;
            continue;
        };
        let transform = p.transform()?;
        if !inside(bounds, &transform, options) {
            report.skipped_outside += 1;
            continue;
        }
        used.insert(key.clone());
        selected.push((key, p.index, transform));
    }
    prototypes.retain(|key, _| used.contains(key));

    let mut wanted = BTreeSet::new();
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
        }
    }
    let payloads = catalog.load(&archive, decoder, &wanted)?;
    let programs = shader_programs(&archive, decoder, layered.into_iter())?;
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
    for (key, proto) in prototypes {
        let mut slots = Vec::with_capacity(proto.materials.len());
        for m in proto.materials {
            let identity = primase_core::json::to_value(&m)
                .map_err(|e| invalid("Sekiro material", 0, e))?
                .to_string();
            let e = if let Some(e) = material_entities.get(&identity) {
                *e
            } else {
                let e = world.spawn();
                world.insert(e, Name(m.albedo.clone().unwrap_or_else(|| m.mtd.clone())));
                if let Some(layered) = &m.layered {
                    world.insert(e, layered.clone());
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
        world.insert(e, crate::rigid::SourceModel(proto.model));
        world.insert(e, source::MaterialSlots(slots));
        world.insert(e, source::PrototypeSource { id: key.clone(), far });
        proto_entities.insert(key, (e, far));
        report.prototypes += 1;
        report.far_prototypes += usize::from(far);
    }
    world.insert_resource(source::TexturePayloads(payloads));
    world.insert_resource(source::ShaderPrograms(programs));
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
    report.collision = collision::assemble(
        &archive,
        decoder,
        &area,
        &layout,
        options.region,
        states.as_mut(),
        &mut world,
    )?;
    report.extras = extras::assemble(&archive, decoder, &area, &layout, options.region, &mut world)?;
    report.object_collision = objects::assemble(&archive, decoder, &area, options.region, &mut world)?;
    report.states = states.map(|s| s.report);
    report.area = area;
    *destination = world;
    Ok(report)
}

/// The G-buffer programs of the given layered materials, from the installed shader bundles.
fn shader_programs<'a>(
    archive: &Archive,
    decoder: &Decoder<'_>,
    materials: impl Iterator<Item = &'a materials::LayeredMaterial>,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let wanted = materials
        .map(materials::LayeredMaterial::program_name)
        .collect::<Result<BTreeSet<_>>>()?;
    let mut programs = BTreeMap::new();
    if wanted.is_empty() {
        return Ok(programs);
    }
    let outer = Binder::read(
        &archive.read("/shader/shaderbdle.shaderbdlebnd.dcx", decoder)?,
        decoder,
    )?;
    let leaf = |name: Option<&str>| {
        name.unwrap_or("")
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_owned()
    };
    for file in outer.files {
        if !matches!(
            leaf(file.name.as_deref()).as_str(),
            "M[Multiple].shaderbdle" | "M[MultipleGround].shaderbdle" | "M[Multiple]_Emissive.shaderbdle"
        ) {
            continue;
        }
        for shader in Binder::read(&file.bytes, decoder)?.files {
            let name = leaf(shader.name.as_deref());
            if wanted.contains(&name) {
                programs.insert(name, shader.bytes);
            }
        }
    }
    Ok(programs)
}

#[cfg(test)]
mod tests {
    use super::Report;
    use primase_core::convert::NoticeKind;

    #[test]
    fn notices_name_what_the_read_left_out_by_kind() {
        let mut report = Report {
            missing_models: vec!["obj/o000001".into()],
            skipped_outside: 7,
            ..Report::default()
        };
        report.collision.missing_files = vec!["h11_00_00_00_009500.hkx".into()];
        report.extras.missing_light_bank = true;
        let notices = report.notices();
        let find = |reason: &str| notices.iter().find(|n| n.reason.starts_with(reason)).unwrap();
        let models = find("placed models not found");
        assert_eq!((models.kind, models.count), (NoticeKind::Unsupported, 1));
        assert_eq!(models.examples, ["obj/o000001"]);
        let outside = find("placements outside the region");
        assert_eq!((outside.kind, outside.count), (NoticeKind::Selection, 7));
        assert_eq!(
            find("collision files named").examples,
            ["h11_00_00_00_009500.hkx"]
        );
        assert_eq!(find("no light bank").count, 1);
        assert_eq!(notices.len(), 4, "{notices:?}");
        assert!(Report::default().notices().is_empty());
    }
}
