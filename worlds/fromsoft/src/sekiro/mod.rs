//! Sekiro area assembly: unbaked geometry and raw map collision. Lights,
//! ledges and object collision remain separate reader steps.
mod assets;
pub mod collision;
pub mod normals;
mod textures;
use crate::{
    Archive, Binder, Result,
    dcx::Decoder,
    flver::{MaterialGeometry, Model},
    invalid, materials,
    msb::{MapLayout, PartKind},
    mtd::MaterialDefinition,
};
pub use assets::prepare_scene_assets;
use primase_core::{
    ColorSpace, Instance, Mesh, Name, NormalConvention, NormalGreen, NormalX, NormalZ, Primitive, Semantic,
    Source, Stream, TexRole, Topology, Transform, World,
};
use primase_skate_scene::{
    FileRef, Part, SkatePlacement, SkatePrototype, SkateScene, SkateTexture, TableRef,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct Options {
    /// East/north rectangle in the source map frame. None imports the whole area.
    pub region: Option<[f32; 4]>,
    pub region_margin: f32,
    pub include_far: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            region: None,
            region_margin: 30.,
            include_far: false,
        }
    }
}
#[derive(Default, Debug)]
pub struct Report {
    pub collision: collision::Report,
    pub prototypes: usize,
    pub placements: usize,
    pub materials: usize,
    pub textures: usize,
    pub missing_models: Vec<String>,
    pub skipped_far: usize,
    pub skipped_empty: usize,
    pub skipped_outside: usize,
    pub normal_basis_assumptions: BTreeMap<String, usize>,
    pub shader_parts: BTreeMap<String, usize>,
}
struct Prototype {
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
        let mut mesh = Mesh::default();
        for part in parts {
            for p in &part.mesh.positions {
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
            }
            materials.push(resolved[part.material].clone());
            let tangents = part.tangents;
            mesh.primitives.push(Primitive {
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
                mesh.primitives
                    .last_mut()
                    .expect("inserted primitive")
                    .streams
                    .insert(Semantic::Tangent, Stream::F32x4(tangents));
            }
        }
        prototypes.insert(
            key,
            Prototype {
                mesh,
                materials,
                min,
                max,
            },
        );
    }
    let mut selected = Vec::new();
    let mut used = BTreeSet::new();
    for (key, p) in placements {
        let Some(proto) = prototypes.get(&key).filter(|p| !p.mesh.primitives.is_empty()) else {
            report.skipped_empty += 1;
            continue;
        };
        let transform = p.transform()?;
        if !inside(proto, &transform, options) {
            report.skipped_outside += 1;
            continue;
        }
        used.insert(key.clone());
        selected.push((key, p.name.clone(), transform));
    }
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
    let mut texture_entities = BTreeMap::new();
    for (name, normal) in roles {
        let mut texture = primase_dds::read(&payloads[&name]).map_err(|e| invalid("Sekiro DDS", 0, e))?;
        texture.role = if normal { TexRole::Normal } else { TexRole::Color };
        texture.color_space = if normal {
            ColorSpace::Linear
        } else {
            ColorSpace::Srgb
        };
        texture.normal = normal.then_some(NormalConvention {
            green: NormalGreen::Up,
            z: NormalZ::Rebuild,
            x: NormalX::Red,
            basis: primase_core::NormalBasis::TangentBitangent,
        });
        if normal {
            normals::swap_normal_axes(&mut texture)?;
        }
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
                z: normal.then_some(NormalZ::Stored),
                other: Default::default(),
            },
        );
        texture_entities.insert((name, normal), e);
        report.textures += 1;
    }
    primase_image::normalize_normals(&mut world).map_err(|e| invalid("Sekiro normal conversion", 0, e))?;
    let mut proto_entities = BTreeMap::new();
    let mut material_entities = BTreeMap::new();
    for key in used {
        let mut proto = prototypes.remove(&key).expect("selected");
        let mut parts = Vec::new();
        for (primitive, m) in proto.mesh.primitives.iter_mut().zip(proto.materials) {
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
                parts,
                collision: None,
                other: Default::default(),
            },
        );
        proto_entities.insert(key, e);
        report.prototypes += 1;
    }
    for (key, name, transform) in selected {
        let e = world.spawn();
        world.insert(
            e,
            Instance {
                prototype: proto_entities[&key],
                variant: 0,
            },
        );
        world.insert(e, transform);
        world.insert(
            e,
            Source(format!("{}:{name}", key.split_once('/').expect("key").0)),
        );
        world.insert(e, SkatePlacement::default());
        report.placements += 1;
    }
    report.collision = collision::assemble(&archive, decoder, &area, &layout, options.region, &mut world)?;
    world.insert_resource(SkateScene {
        name: format!("sekiro_{area}"),
        generator: Some("primase".into()),
        region: options.region,
        placements: Some(TableRef::single("tables/placements.bin")),
        warnings: Some(vec![
            "Unbaked geometry and raw map collision; object collision, lights, ledges, roof smoothing and opened-up surfaces are not imported.".into(),
        ]),
        ..Default::default()
    });
    *destination = world;
    Ok(report)
}
