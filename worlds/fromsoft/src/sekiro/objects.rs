//! Object collision lowering: main HKX only, dynamic bodies at rest, motion 2
//! omitted and named. Convex material is the dominant colliding render surface.
use crate::{Archive, Binder, Result, collision, dcx::Decoder, invalid, materials::Material};
use primase_core::state::Condition;
use primase_core::{
    Collision, Instance, Mesh, Semantic, Source, Stream, Surfaces, Transform, World,
    frame::{Axis, SourceFrame},
};
use primase_meshops::vector::Vec3;
use primase_skate_scene::SkatePrototype;
use std::collections::BTreeMap;
const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);
#[derive(Debug, Default)]
pub struct Report {
    /// Soups of objects present only in some world states (with states read).
    pub conditioned_soups: usize,
    pub models: usize,
    pub models_with_collision: usize,
    pub placements: usize,
    pub placements_with_collision: usize,
    pub triangles_by_shape: BTreeMap<String, usize>,
    pub bodies_by_motion: BTreeMap<u64, usize>,
    pub keyframed_left_out: Vec<String>,
    pub excluded_material_triangles: usize,
    pub outside_region_triangles: usize,
    pub written_triangles: usize,
    pub batches: Vec<Batch>,
}
#[derive(Debug)]
pub struct Batch {
    pub model: String,
    pub shape: String,
    pub start: usize,
    pub count: usize,
}
fn main_material(world: &World, prototype: primase_core::Entity) -> Result<i32> {
    let mesh = world
        .get::<Mesh>(prototype)
        .ok_or_else(|| invalid("object collision", 0, "missing prototype mesh"))?;
    let mut areas = BTreeMap::<String, f64>::new();
    for p in &mesh.primitives {
        let Some(m) = p.material.and_then(|e| world.get::<Material>(e)) else {
            continue;
        };
        if !["opaque", "cutout"].contains(&m.class.as_str()) {
            continue;
        }
        let Some(Stream::F32x3(pos)) = p.streams.get(&Semantic::Position) else {
            return Err(invalid("object collision", 0, "missing position stream"));
        };
        let indices = p
            .indices
            .clone()
            .unwrap_or_else(|| (0..pos.len() as u32).collect());
        for tri in indices.as_chunks::<3>().0 {
            let a = Vec3::from(pos[tri[0] as usize]);
            let b = Vec3::from(pos[tri[1] as usize]);
            let c = Vec3::from(pos[tri[2] as usize]);
            *areas.entry(m.surface.clone()).or_default() += f64::from((b - a).cross(c - a).length()) * 0.5;
        }
    }
    let dominant = areas
        .iter()
        .min_by(|(ka, a), (kb, b)| b.total_cmp(a).then(ka.cmp(kb)))
        .map(|(k, _)| k.as_str())
        .unwrap_or("default");
    Ok(match dominant {
        "concrete" => 1,
        "earth" => 3,
        "wood" => 4,
        "grass" => 5,
        "gravel" => 6,
        "metal" => 11,
        "sand" => 13,
        "roofing" => 16,
        "marble" => 34,
        _ => -1,
    })
}
pub fn assemble(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    region: Option<[f32; 4]>,
    world: &mut World,
) -> Result<Report> {
    let mut report = Report::default();
    let mut cache = BTreeMap::new();
    let mut surfaces = world.resource::<Surfaces>().cloned().unwrap_or_default();
    let mut soup = Collision::default();
    // collision of objects present only in some world states, by the placement's condition (states read)
    let mut conditioned: BTreeMap<String, (Condition, Collision)> = BTreeMap::new();
    for (e, instance) in world.iter::<Instance>() {
        let Some(proto) = world.get::<SkatePrototype>(instance.prototype) else {
            continue;
        };
        let Some(model) = proto.id.strip_prefix("obj/") else {
            continue;
        };
        report.placements += 1;
        if !cache.contains_key(model) {
            let binder = Binder::read(
                &archive.read(&format!("/obj/{model}.objbnd.dcx"), decoder)?,
                decoder,
            )?;
            let file = binder.files.iter().find(|f| {
                f.name
                    .as_deref()
                    .unwrap_or("")
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|s| s.trim_end_matches(".dcx") == format!("{model}.hkx"))
            });
            let bodies = match file {
                Some(f) => collision::read_bodies(
                    &decoder.decode_if_dcx(&f.bytes)?,
                    None,
                    main_material(world, instance.prototype)?,
                )?,
                None => Vec::new(),
            };
            report.models += 1;
            report.models_with_collision += usize::from(!bodies.is_empty());
            cache.insert(model.to_owned(), bodies);
        }
        let bodies = &cache[model];
        report.placements_with_collision += usize::from(!bodies.is_empty());
        let transform = world
            .get::<Transform>(e)
            .ok_or_else(|| invalid("object collision", 0, "missing placement transform"))?;
        let soup = match world.get::<Condition>(e) {
            None => &mut soup,
            Some(c) => {
                &mut conditioned
                    .entry(super::states::condition_key(c))
                    .or_insert_with(|| (c.clone(), Collision::default()))
                    .1
            }
        };
        for body in bodies {
            *report.bodies_by_motion.entry(body.motion_type).or_default() += 1;
            if body.motion_type == 2 {
                let source = world.get::<Source>(e).map(|s| s.0.as_str()).unwrap_or("");
                let source = source.split_once(':').map_or(source, |(_, part)| part);
                report
                    .keyframed_left_out
                    .push(format!("{area}:{source} {model} {}", body.name));
                continue;
            }
            *report.triangles_by_shape.entry(body.shape.clone()).or_default() += body.soup.triangles.len();
            let start = report.written_triangles;
            for (tri, &material) in body.soup.triangles.iter().zip(&body.soup.materials) {
                let v = tri.map(|p| super::point(transform, FRAME.point(p)));
                let x = (v[0][0] + v[1][0] + v[2][0]) / 3.;
                let z = -(v[0][2] + v[1][2] + v[2][2]) / 3.;
                if region
                    .is_some_and(|r| x < r[0] - 50. || x >= r[2] + 50. || z < r[1] - 50. || z >= r[3] + 50.)
                {
                    report.outside_region_triangles += 1;
                    continue;
                }
                let Some(surface) = super::collision::surface(material) else {
                    report.excluded_material_triangles += 1;
                    continue;
                };
                super::collision::push(soup, v, surfaces.index(surface))?;
                report.written_triangles += 1;
            }
            report.batches.push(Batch {
                model: model.into(),
                shape: body.shape.clone(),
                start,
                count: report.written_triangles - start,
            });
        }
    }
    world.insert_resource(surfaces);
    super::collision::attach(world, soup, "collision/objects.bin", false);
    for (n, (condition, soup)) in conditioned.into_values().enumerate() {
        report.conditioned_soups += 1;
        super::collision::attach_when(
            world,
            soup,
            &format!("collision/objects_state{n}.bin"),
            false,
            &condition,
        );
    }
    Ok(report)
}
