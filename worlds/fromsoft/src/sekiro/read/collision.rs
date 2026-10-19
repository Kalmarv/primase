//! Map collision as the area stores it: the reader keeps, whole, every file of the area's high-resolution collision
//! binder that MSB collision parts name (`fromsoft.map_collision`); `lower-fromsoft-collision` lowers it.
use crate::pose::FRAME;
use crate::sekiro::source::{CollisionPart, MapCollision};
use crate::{
    Archive, Binder, Result, collision,
    dcx::Decoder,
    invalid,
    msb::{MapLayout, PartKind},
};
use primase_core::{Name, Transform, World};
use primase_havok::Compendium;
use std::collections::BTreeMap;

/// What the reader found of the area's map collision.
#[derive(Debug, Default)]
pub struct Report {
    /// Files read: those some MSB collision part names.
    pub files: usize,
    /// MSB collision parts by hit filter.
    pub parts_by_filter: BTreeMap<u8, usize>,
    /// Files of the binder no MSB collision part names: not read.
    pub unnamed_files: usize,
    pub triangles: usize,
    /// Triangles whose shape key binds no HitMtrl.
    pub unbound_triangles: usize,
    /// Collision files the MSB's parts name that the area's binder does not hold.
    pub missing_files: Vec<String>,
    /// Bodies of the read files whose shape class has no decoder, by class: listed on their file, not read.
    pub unread_bodies: BTreeMap<String, usize>,
}

fn leaf(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".dcx")
        .to_ascii_lowercase()
}

/// Read every collision file the area's MSB collision parts name, of every hit filter, as `fromsoft.map_collision`
/// entities in file-name order; with `states`, each part's clause.
pub fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    mut states: Option<&mut super::states::States>,
    world: &mut World,
) -> Result<Report> {
    let mut report = Report::default();
    let mut named = BTreeMap::<String, Vec<CollisionPart>>::new();
    for part in layout.parts.iter().filter(|p| p.kind == PartKind::Collision) {
        let filter = part
            .hit_filter
            .ok_or_else(|| invalid("collision", part.index, "missing hit filter"))?;
        let model = part
            .model
            .and_then(|i| layout.models.get(i))
            .ok_or_else(|| invalid("collision", part.index, "missing collision model"))?;
        let suffix = model
            .name
            .strip_prefix('h')
            .ok_or_else(|| invalid("collision", part.index, "unexpected collision model name"))?;
        *report.parts_by_filter.entry(filter).or_default() += 1;
        named
            .entry(format!("h{}_{suffix}.hkx", &area[1..]))
            .or_default()
            .push(CollisionPart {
                name: part.name.clone(),
                hit_filter: filter,
                condition: states.as_deref_mut().and_then(|s| s.clause(part)),
            });
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
        .unwrap_or(Transform::IDENTITY);
    let mut files = binder
        .files
        .iter()
        .filter(|f| f.name.as_deref().is_some_and(|n| leaf(n).ends_with(".hkx")))
        .collect::<Vec<_>>();
    files.sort_by_key(|f| leaf(f.name.as_deref().unwrap_or("")));
    for file in files {
        let name = leaf(file.name.as_deref().unwrap_or(""));
        let Some(parts) = named.remove(&name) else {
            report.unnamed_files += 1;
            continue;
        };
        let raw = collision::read_map(&decoder.decode_if_dcx(&file.bytes)?, Some(&comp))?;
        report.files += 1;
        report.triangles += raw.triangles.len();
        report.unbound_triangles += raw.unmapped;
        for shape in &raw.unread_bodies {
            *report.unread_bodies.entry(shape.clone()).or_default() += 1;
        }
        let e = world.spawn();
        world.insert(e, Name(name.clone()));
        world.insert(
            e,
            MapCollision {
                file: name,
                parts,
                triangles: raw
                    .triangles
                    .into_iter()
                    .map(|t| t.map(|v| crate::pose::point(&transform, FRAME.point(v))))
                    .collect(),
                materials: raw.materials,
                invalid_material_keys: raw.invalid_material_keys,
                dead_primitives: raw.dead_primitives,
                unread_bodies: raw.unread_bodies,
            },
        );
    }
    report.missing_files = named.into_keys().collect();
    Ok(report)
}
