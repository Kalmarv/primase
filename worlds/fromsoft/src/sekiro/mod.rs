//! Sekiro area assembly: geometry, raw collision, ledges, lights and markers.
pub mod blockers;
pub mod collision;
pub mod extras;
pub mod ledges;
pub mod lights;
pub mod normals;
pub mod objects;
pub mod pbr;
mod shadows;
pub mod states;
mod textures;
use crate::{
    Archive, Binder, Result,
    dcx::Decoder,
    flver::{MaterialGeometry, Model},
    invalid, materials,
    msb::{MapLayout, PartKind},
    mtd::MaterialDefinition,
};
use primase_core::{
    ColorSpace, Instance, Mesh, Name, NormalGreen, NormalZ, Primitive, Semantic, Source, Stream, TexRole,
    Topology, Transform, World,
};
use primase_reskate::dialect::{
    FileRef, MinHeight, Part, SkateParts, SkatePlacement, SkatePrototype, SkateScene, SkateTexture, TableRef,
};
pub use primase_reskate::interchange::prepare_assets;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

struct AreaGeometry {
    area: String,
    archive: Archive,
    layout: MapLayout,
    catalog: textures::Catalog,
    report: Report,
    prototypes: BTreeMap<String, Prototype>,
    selected: Vec<(String, usize, Transform)>,
    used: BTreeSet<String>,
    blocker_cards: blockers::BlockerCards,
}

/// Read the same selected geometry as `read_area`, retaining only blocker records.
/// This inventory does not decode texture pixels or choose a consumer normal basis.
pub fn read_blocker_cards(
    install: impl AsRef<Path>,
    area: &str,
    decoder: &Decoder<'_>,
    options: &Options,
) -> Result<blockers::BlockerCards> {
    Ok(select_geometry(install.as_ref(), area, decoder, options)?.blocker_cards)
}

fn select_geometry(
    install: &Path,
    area: &str,
    decoder: &Decoder<'_>,
    options: &Options,
) -> Result<AreaGeometry> {
    if !options.region_margin.is_finite()
        || options.region_margin < 0.
        || options
            .region
            .is_some_and(|r| !r.iter().all(|v| v.is_finite()) || r[0] >= r[2] || r[1] >= r[3])
    {
        return Err(invalid("Sekiro area", 0, "invalid region"));
    }
    let area = area_name(area)?;
    let archive = Archive::open(install)?;
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
            if kind == PartKind::MapPiece && !options.include_far && model.starts_with("m9") {
                report.skipped_far += 1;
                continue;
            }
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
        let model = crate::rigid::lower_model(&model, &mut report.rigid_skinning)
            .map_err(|e| invalid("rigid skinning", 0, e))?;
        let resolved: Vec<_> = model
            .materials
            .iter()
            .map(|m| {
                materials::resolve(m, definitions.get(&materials::stem(&m.mtd)), |n| {
                    catalog.entries.contains_key(n)
                })
            })
            .collect();
        let choices: Vec<_> = resolved
            .iter()
            .map(|m| MaterialGeometry {
                skip: m.skip.is_some(),
                uv_scale: m.uv_scale,
            })
            .collect();
        let parts = model.geometry(&choices)?;
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut materials = Vec::new();
        let mut primitives = Vec::new();
        let mut cards = Vec::new();
        for part in parts {
            for p in &part.mesh.positions {
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
            }
            if blockers::is_card(&resolved[part.material]) {
                let material = resolved[part.material].clone();
                cards.push(blockers::Card::from_part(part, material));
                continue;
            }
            materials.push(resolved[part.material].clone());
            let tangents = part.tangents;
            primitives.push(Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (Semantic::Position, Stream::F32x3(part.mesh.positions)),
                    (Semantic::Normal, Stream::F32x3(part.mesh.normals)),
                    (Semantic::TexCoord(0), Stream::F32x2(part.mesh.uv)),
                ]),
                indices: Some(part.mesh.indices),
                material: None,
            });
            if !tangents.is_empty() {
                primitives
                    .last_mut()
                    .expect("inserted primitive")
                    .streams
                    .insert(Semantic::Tangent, Stream::F32x4(tangents));
            }
        }
        prototypes.insert(
            key,
            Prototype {
                cards,
                mesh: Mesh::new(primitives),
                materials,
                min,
                max,
            },
        );
    }
    let mut blocker_cards = blockers::BlockerCards::default();
    let mut selected = Vec::new();
    let mut used = BTreeSet::new();
    for (key, p) in placements {
        let Some(proto) = prototypes
            .get(&key)
            .filter(|p| !p.mesh.primitives().is_empty() || !p.cards.is_empty())
        else {
            report.skipped_empty += 1;
            continue;
        };
        let transform = p.transform()?;
        if !inside(proto, &transform, options) {
            report.skipped_outside += 1;
            continue;
        }
        blocker_cards.record(&key, format!("{area}:{}", p.name), &transform, &proto.cards);
        if proto.mesh.primitives().is_empty() {
            report.skipped_empty += 1;
            continue;
        }
        used.insert(key.clone());
        selected.push((key, p.index, transform));
    }
    Ok(AreaGeometry {
        area,
        archive,
        layout,
        catalog,
        report,
        prototypes,
        selected,
        used,
        blocker_cards,
    })
}

#[derive(Clone, Debug)]
pub struct Options {
    /// East/north rectangle in the source map frame. None imports the whole area.
    pub region: Option<[f32; 4]>,
    pub region_margin: f32,
    pub include_far: bool,
    /// Pin the source floor in metres for a shared scene lift to 100.01 m.
    /// None emits no floor policy; geometry stays in the source world frame.
    pub floor: Option<f64>,
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
            include_far: false,
            floor: None,
            states: StateClasses::None,
        }
    }
}
impl Options {
    fn min_height(&self) -> Result<Option<MinHeight>> {
        self.floor
            .map(|floor| {
                if !floor.is_finite() || !(-1e6..=1e6).contains(&floor) {
                    return Err(invalid(
                        "Sekiro area",
                        0,
                        "floor must be finite and within +/-1000000 metres",
                    ));
                }
                Ok(MinHeight {
                    height: Some(100.01),
                    stub_below: Some(250.),
                    floor: Some(floor),
                    other: Default::default(),
                })
            })
            .transpose()
    }
}
#[derive(Default, Debug)]
pub struct Report {
    pub blocker_meshes: usize,
    pub blocker_placements: usize,
    pub blocker_triangles: usize,
    pub rigid_skinning: primase_core::pass::PassLog,
    /// Selected placements' source shadow controls mapped to `core.shadows`
    /// (units: placements). Counts contradictory cast/only controls left unknown
    /// and names the renderer-specific controls kept only as `fromsoft.shadow_flags`.
    pub shadows: primase_core::pass::PassLog,
    pub extras: extras::Report,
    pub collision: collision::Report,
    pub object_collision: objects::Report,
    pub prototypes: usize,
    pub placements: usize,
    pub materials: usize,
    pub textures: usize,
    pub missing_models: Vec<String>,
    /// With `states: Event`: the rules read and the parts decided or left at their default.
    pub states: Option<states::Report>,
    pub skipped_far: usize,
    pub skipped_empty: usize,
    pub skipped_outside: usize,
    pub normal_basis_assumptions: BTreeMap<String, usize>,
    pub shader_parts: BTreeMap<String, usize>,
}
struct Prototype {
    cards: Vec<blockers::Card>,
    mesh: Mesh,
    materials: Vec<materials::Material>,
    min: [f32; 3],
    max: [f32; 3],
}
fn file(path: String) -> FileRef {
    FileRef { path, sha256: None }
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
fn inside(proto: &Prototype, t: &Transform, o: &Options) -> bool {
    let Some(r) = o.region else { return true };
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for mask in 0..8 {
        let p = point(
            t,
            std::array::from_fn(|i| {
                if mask & (1 << i) == 0 {
                    proto.min[i]
                } else {
                    proto.max[i]
                }
            }),
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
/// Read an area directly from installed archives into an empty world. The caller
/// supplies its trusted decompressor; no environment variables or install paths
/// are guessed. On failure the destination world is unchanged.
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
    let min_height = options.min_height()?;
    let AreaGeometry {
        area,
        archive,
        layout,
        catalog,
        mut report,
        mut prototypes,
        selected,
        used,
        blocker_cards,
    } = select_geometry(install.as_ref(), area, decoder, options)?;
    let mut wanted = BTreeSet::new();
    let mut roles = BTreeSet::new();
    for key in &used {
        for m in &prototypes[key].materials {
            *report.shader_parts.entry(m.shader.clone()).or_default() += 1;
            if !normals::consumer_basis(&m.shader)? {
                *report
                    .normal_basis_assumptions
                    .entry(m.shader.clone())
                    .or_default() += 1;
            }
            for (name, normal) in [(&m.albedo, false), (&m.normal, true)] {
                if let Some(name) = name
                    && catalog.entries.contains_key(name)
                {
                    wanted.insert(name.clone());
                    roles.insert((name.clone(), normal));
                }
            }
        }
    }
    let payloads = catalog.load(&archive, decoder, &wanted)?;
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
    report.blocker_meshes = blocker_cards.meshes();
    report.blocker_placements = blocker_cards.placements.len();
    report.blocker_triangles = blocker_cards.triangles();
    world.insert_resource(blocker_cards);
    let mut texture_entities = BTreeMap::new();
    for (name, normal) in roles {
        let mut texture = primase_dds::read(&payloads[&name]).map_err(|e| invalid("Sekiro DDS", 0, e))?;
        texture.role = if normal { TexRole::Normal } else { TexRole::Color };
        texture.color_space = if normal {
            ColorSpace::Linear
        } else {
            ColorSpace::Srgb
        };
        texture.normal = normal.then_some(normals::SOURCE);
        let key = if normal {
            format!("{name}@normal")
        } else {
            name.clone()
        };
        let output = format!("textures/{key}.png");
        let e = world.spawn();
        world.insert(e, Name(key.clone()));
        world.insert(e, texture);
        world.insert(
            e,
            SkateTexture {
                key,
                file: file(output),
                stem: None,
                source_format: None,
                source_size: None,
                linear: Some(normal),
                green: normal.then_some(NormalGreen::Up),
                z: normal.then_some(NormalZ::Rebuild),
                other: Default::default(),
            },
        );
        texture_entities.insert((name, normal), e);
        report.textures += 1;
    }
    let mut proto_entities = BTreeMap::new();
    let mut material_entities = BTreeMap::new();
    for key in used {
        let mut proto = prototypes.remove(&key).expect("selected");
        let mut parts = Vec::new();
        for (primitive, m) in proto.mesh.primitives_mut().iter_mut().zip(proto.materials) {
            let identity = primase_core::json::to_value(&m)
                .map_err(|e| invalid("Sekiro material", 0, e))?
                .to_string();
            let e = if let Some(e) = material_entities.get(&identity) {
                *e
            } else {
                let e = world.spawn();
                let material_key = format!("material_{:06}", material_entities.len());
                world.insert(
                    e,
                    m.skate_material(material_key, |n, normal| {
                        texture_entities.get(&(n.to_owned(), normal)).copied()
                    }),
                );
                world.insert(e, Name(m.albedo.clone().unwrap_or_else(|| m.mtd.clone())));
                if let Some(layered) = &m.layered {
                    world.insert(e, layered.clone());
                }
                world.insert(e, m.clone());
                material_entities.insert(identity, e);
                report.materials += 1;
                e
            };
            primitive.material = Some(e);
            parts.push(Part {
                source: Some(serde_json::json!({"shader":format!("{} / {}",m.shader,m.mtd)})),
                ..Default::default()
            });
        }
        let e = world.spawn();
        world.insert(e, Name(key.split_once('/').expect("key").1.to_owned()));
        world.insert(e, proto.mesh);
        world.insert(
            e,
            SkatePrototype {
                id: key.clone(),
                glb: Some(file(format!("prototypes/{}.glb", key.replace('/', "_")))),
                key: None,
                foliage: None,
                ground: None,
                collision: None,
                other: Default::default(),
            },
        );
        world.insert(e, SkateParts(parts));
        proto_entities.insert(key, e);
        report.prototypes += 1;
    }
    for (key, index, transform) in selected {
        let part = &layout.parts[index];
        let name = &part.name;
        let e = world.spawn();
        if let Some(s) = states.as_mut()
            && let Some(clause) = s.clause(part)
        {
            world.insert(e, primase_core::state::Condition(vec![clause]));
        }
        world.insert(
            e,
            Instance {
                prototype: proto_entities[&key],
                variant: 0,
            },
        );
        world.insert(e, transform);
        world.insert(e, Source(format!("{area}:{name}")));
        shadows::attach(&mut world, e, part.shadows, &mut report.shadows);
        world.insert(e, SkatePlacement::default());
        report.placements += 1;
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
    let cards = world
        .resource::<blockers::BlockerCards>()
        .expect("recorded cards");
    let extension = primase_core::json::to_value(cards).map_err(|e| invalid("blocker cards", 0, e))?;
    let other = serde_json::Map::from_iter([(
        "extensions".into(),
        serde_json::json!({
            "sekiro": { "blocker_cards": {
                "rule": blockers::RULE, "drawn": false,
                "placement_count": report.blocker_placements,
                "triangles": report.blocker_triangles,
                "records": extension
            }}
        }),
    )]);
    world.insert_resource(SkateScene {
        name: format!("sekiro_{area}"),
        generator: Some("primase".into()),
        region: options.region,
        min_height,
        other,
        placements: Some(TableRef::single("tables/placements.bin")),
        lights: Some(TableRef::single("tables/lights.bin")),
        grind_curves: Some(TableRef::single("tables/grind.bin")),
        warnings: Some(vec![
            "Unbaked geometry and raw collision; roof smoothing and opened-up surfaces are not imported. Lights use the named sRGB/intensity/spot-axis conversion policy.".into(),
        ]),
        ..Default::default()
    });
    report.states = states.map(|s| s.report);
    *destination = world;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::Options;
    #[test]
    fn floor_policy_preserves_the_shared_height_without_rounding() {
        assert!(Options::default().min_height().unwrap().is_none());
        let policy = Options {
            floor: Some(-81.60212345),
            ..Options::default()
        }
        .min_height()
        .unwrap()
        .unwrap();
        assert_eq!(policy.floor, Some(-81.60212345));
        assert_eq!(policy.height, Some(100.01));
        assert_eq!(policy.stub_below, Some(250.));
        for floor in [f64::NAN, f64::INFINITY, -1000001., 1000001.] {
            assert!(
                Options {
                    floor: Some(floor),
                    ..Options::default()
                }
                .min_height()
                .is_err()
            );
        }
    }
}
