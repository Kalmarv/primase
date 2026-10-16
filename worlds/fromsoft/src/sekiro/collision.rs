//! Collision selection and HitMtrl-to-surface mapping are explicit lowering
//! policy, not part of Havok parsing. Raw collision is never roof-smoothed here.
use crate::{
    Archive, Binder, Result, collision,
    dcx::Decoder,
    invalid,
    msb::{MapLayout, PartKind},
};
use primase_core::state::Condition;
use primase_core::{
    Collision, Surfaces, Transform, World,
    frame::{Axis, SourceFrame},
};
use primase_havok::Compendium;
use primase_skate_scene::{CollisionMode, SkateCollision, SoupRef, TableRef};
use std::collections::{BTreeMap, BTreeSet};
const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);
#[derive(Debug, Default)]
pub struct Report {
    /// Soups present only in some world states (with states read).
    pub conditioned_soups: usize,
    pub files: usize,
    pub omitted_filters: BTreeMap<u8, usize>,
    pub unselected_files: usize,
    pub solid_triangles: usize,
    pub water_triangles: usize,
    pub excluded_material_triangles: usize,
    pub outside_region_triangles: usize,
    pub unmapped_materials: usize,
}
/// Rideable-map lowering policy: hundreds digits retain the base material;
/// blockers/cloud/water do not become solid surfaces. Water has its own soup.
pub fn surface(id: i32) -> Option<&'static str> {
    if id < 0 {
        return Some("default");
    }
    Some(match id % 100 {
        22 | 29 | 36 | 40 => return None,
        1 | 46 => "concrete",
        3 | 15 | 18 | 27 | 30 | 33 | 45 => "earth",
        4 | 7 | 8 | 28 | 32 | 47 => "wood",
        5 | 9 | 10 | 31 | 37 | 38 => "grass",
        6 => "gravel",
        11 | 19 => "metal",
        13 => "sand",
        16 => "roofing",
        34 => "marble",
        _ => "default",
    })
}
fn leaf(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".dcx")
        .to_ascii_lowercase()
}
pub(super) fn push(soup: &mut Collision, vertices: [[f32; 3]; 3], surface: u16) -> Result<()> {
    let start =
        u32::try_from(soup.positions.len()).map_err(|_| invalid("collision", 0, "too many soup vertices"))?;
    if start > u32::MAX - 3 {
        return Err(invalid("collision", 0, "soup index overflow"));
    }
    soup.positions.extend(vertices);
    soup.triangles.push([start, start + 2, start + 1]);
    soup.surfaces.push(surface);
    Ok(())
}
/// [`attach`] for collision present only while `condition` holds: the entity carries the condition and an identity
/// transform (state sits on placed things).
pub(super) fn attach_when(
    world: &mut World,
    soup: Collision,
    path: &str,
    water: bool,
    condition: &Condition,
) {
    let Some(e) = attach(world, soup, path, water) else {
        return;
    };
    world.insert(e, condition.clone());
    world.insert(
        e,
        Transform {
            cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
        },
    );
}
pub(super) fn attach(
    world: &mut World,
    soup: Collision,
    path: &str,
    water: bool,
) -> Option<primase_core::Entity> {
    if soup.triangles.is_empty() {
        return None;
    }
    let entity = world.spawn();
    world.insert(entity, soup);
    world.insert(
        entity,
        SkateCollision(SoupRef {
            table: TableRef::single(path),
            mode: Some(if water {
                CollisionMode::Water
            } else {
                CollisionMode::TriangleMesh
            }),
            ground: Some(!water),
            region_cut: Some(true),
            other: Default::default(),
        }),
    );
    Some(entity)
}
pub fn assemble(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    region: Option<[f32; 4]>,
    mut states: Option<&mut super::states::States>,
    world: &mut World,
) -> Result<Report> {
    let mut report = Report::default();
    let mut selected = BTreeSet::new();
    // with states: each file's parts' clauses; the file is present when any of its parts is
    let mut clauses: BTreeMap<String, Vec<Option<primase_core::state::Clause>>> = BTreeMap::new();
    for part in layout.parts.iter().filter(|p| p.kind == PartKind::Collision) {
        let filter = part
            .hit_filter
            .ok_or_else(|| invalid("collision", part.index, "missing hit filter"))?;
        if ![8, 27].contains(&filter) {
            *report.omitted_filters.entry(filter).or_default() += 1;
            continue;
        }
        let model = part
            .model
            .and_then(|i| layout.models.get(i))
            .ok_or_else(|| invalid("collision", part.index, "missing collision model"))?;
        let suffix = model
            .name
            .strip_prefix('h')
            .ok_or_else(|| invalid("collision", part.index, "unexpected collision model name"))?;
        let file = format!("h{}_{suffix}.hkx", &area[1..]);
        if let Some(s) = states.as_deref_mut() {
            clauses.entry(file.clone()).or_default().push(s.clause(part));
        }
        selected.insert(file);
    }
    let base = format!("/map/{area}/h{}", &area[1..]);
    let binder = Binder::read_split(
        &archive.read(&format!("{base}.hkxbhd"), decoder)?,
        &archive.read(&format!("{base}.hkxbdt"), decoder)?,
        decoder,
    )?;
    let comp = binder
        .files
        .iter()
        .find(|f| {
            f.name
                .as_deref()
                .is_some_and(|n| leaf(n).ends_with(".compendium"))
        })
        .ok_or_else(|| invalid("collision", 0, "missing compendium"))?;
    let comp = Compendium::read(&decoder.decode_if_dcx(&comp.bytes)?)?;
    if layout.map_offsets.len() > 1 {
        return Err(invalid("collision", 0, "multiple map offsets"));
    }
    let transform = layout
        .map_offsets
        .first()
        .map(|o| o.transform())
        .transpose()?
        .unwrap_or(Transform {
            cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
        });
    let mut files = binder
        .files
        .iter()
        .filter(|f| f.name.as_deref().is_some_and(|n| leaf(n).ends_with(".hkx")))
        .collect::<Vec<_>>();
    files.sort_by_key(|f| leaf(f.name.as_deref().unwrap_or("")));
    let mut surfaces = world.resource::<Surfaces>().cloned().unwrap_or_default();
    let mut solid = Collision::default();
    let mut water = Collision::default();
    // soups of files present only in some states, by condition (states only)
    let mut conditioned: BTreeMap<String, (Condition, Collision, Collision)> = BTreeMap::new();
    for file in files {
        let name = leaf(file.name.as_deref().unwrap_or(""));
        if !selected.remove(&name) {
            report.unselected_files += 1;
            continue;
        }
        let condition = clauses.get(&name).and_then(|c| super::states::States::any(c));
        let (solid, water) = match &condition {
            None => (&mut solid, &mut water),
            Some(c) => {
                let entry = conditioned
                    .entry(super::states::condition_key(c))
                    .or_insert_with(|| (c.clone(), Collision::default(), Collision::default()));
                (&mut entry.1, &mut entry.2)
            }
        };
        let raw = collision::read_map(&decoder.decode_if_dcx(&file.bytes)?, Some(&comp))?;
        report.files += 1;
        report.unmapped_materials += raw.unmapped;
        for (triangle, material) in raw.triangles.into_iter().zip(raw.materials) {
            let vertices = triangle.map(|v| super::point(&transform, FRAME.point(v)));
            let x = (vertices[0][0] + vertices[1][0] + vertices[2][0]) / 3.;
            let z = -(vertices[0][2] + vertices[1][2] + vertices[2][2]) / 3.;
            if region.is_some_and(|r| x < r[0] - 50. || x >= r[2] + 50. || z < r[1] - 50. || z >= r[3] + 50.)
            {
                report.outside_region_triangles += 1;
                continue;
            }
            if material >= 0 && [22, 36].contains(&(material % 100)) {
                push(water, vertices, surfaces.index("default"))?;
                report.water_triangles += 1;
            } else if let Some(surface) = surface(material) {
                push(solid, vertices, surfaces.index(surface))?;
                report.solid_triangles += 1;
            } else {
                report.excluded_material_triangles += 1;
            }
        }
    }
    if !selected.is_empty() {
        return Err(invalid(
            "collision",
            0,
            format!("missing selected files {selected:?}"),
        ));
    }
    world.insert_resource(surfaces);
    attach(world, solid, "collision/game.bin", false);
    attach(world, water, "collision/water.bin", true);
    for (n, (condition, solid, water)) in conditioned.into_values().enumerate() {
        report.conditioned_soups += 1;
        attach_when(
            world,
            solid,
            &format!("collision/game_state{n}.bin"),
            false,
            &condition,
        );
        attach_when(
            world,
            water,
            &format!("collision/water_state{n}.bin"),
            true,
            &condition,
        );
    }
    Ok(report)
}
