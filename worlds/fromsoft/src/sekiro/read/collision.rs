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
use havok_formats::Compendium;
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
    /// Triangle-data rows beyond their shape's key table (Elden Ring stores some tables shorter than their data):
    /// the triangles they would key bind no HitMtrl and are among `unbound_triangles`.
    pub unkeyed_triangle_data: usize,
    /// Collision files the MSB's parts name that the area's binder does not hold.
    pub missing_files: Vec<String>,
    /// Bodies of the read files whose shape class has no decoder, by class: listed on their file, not read.
    pub unread_bodies: BTreeMap<String, usize>,
    /// Compressed-mesh custom primitives of an unknown kind ([`havok_formats::CustomKind::Unread`]): not read.
    pub unread_custom_primitives: usize,
}

fn leaf(name: &str) -> String {
    let name = crate::names::fold(name);
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(&name)
        .trim_end_matches(".dcx")
        .to_owned()
}

/// The file of the area's collision binder that collision model `model` names (`h533700` in `m60_53_37_00`:
/// `h60_53_37_00_533700.hkx`, lowercase as [`leaf`] compares the binder's names), `None` for a model not named `h<n>`.
/// The `h` is matched in either case, as the game's names are not case-sensitive: Elden Ring's m60_53_37_00 names one
/// of its six collision models `H533700`.
fn collision_file(area: &str, model: &str) -> Option<String> {
    let model = crate::names::fold(model);
    let suffix = model.strip_prefix('h')?;
    Some(crate::names::fold(&format!("h{}_{suffix}.hkx", &area[1..])))
}

/// Read every collision file the area's MSB collision parts name, of every hit filter, as `fromsoft.map_collision`
/// entities in file-name order; with `states`, each part's clause. An area whose MSB has no collision part has nothing
/// to read, and its binder is not opened: Elden Ring's m60_22_22_01 holds three map pieces and no collision part, and
/// no archive holds an `h60_22_22_01.hkxbhd`.
pub fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    folder: &str,
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
        let file = collision_file(area, &model.name)
            .ok_or_else(|| invalid("collision", part.index, "unexpected collision model name"))?;
        *report.parts_by_filter.entry(filter).or_default() += 1;
        named.entry(file).or_default().push(CollisionPart {
            name: part.name.clone(),
            hit_filter: filter,
            condition: states.as_deref_mut().and_then(|s| s.clause(part)),
        });
    }
    if named.is_empty() {
        return Ok(report);
    }
    let base = format!("{folder}/h{}", &area[1..]);
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
        report.unkeyed_triangle_data += raw.unkeyed_triangle_data;
        report.unread_custom_primitives += raw.unread_custom_primitives;
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
                    .map(|t| t.map(|v| transform.apply(FRAME.point(v))))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msb::Generation;

    /// An area with no collision part reads no collision and opens no binder: the archive holds none.
    #[test]
    fn an_area_with_no_collision_part_reads_no_binder() {
        let layout = MapLayout {
            generation: &Generation::SEKIRO,
            models: Vec::new(),
            parts: Vec::new(),
            map_offsets: Vec::new(),
            sections: Vec::new(),
        };
        let mut world = World::default();
        let decoder = Decoder {
            oodle: None,
            max_output: 1 << 20,
        };
        let report = read(
            &Archive::empty(crate::Game::EldenRing),
            &decoder,
            "/map/m60/m60_22_22_01",
            "m60_22_22_01",
            &layout,
            None,
            &mut world,
        )
        .unwrap();
        assert_eq!((report.files, report.missing_files.len()), (0, 0));
        assert_eq!(world.entities().count(), 0);
    }

    /// A collision model names its file in the area's binder whatever the case of its `h`; a model named otherwise
    /// names none.
    #[test]
    fn a_collision_models_file_is_named_whatever_the_case_of_its_h() {
        let area = "m60_53_37_00";
        assert_eq!(
            collision_file(area, "h533750").as_deref(),
            Some("h60_53_37_00_533750.hkx")
        );
        assert_eq!(
            collision_file(area, "H533700").as_deref(),
            Some("h60_53_37_00_533700.hkx")
        );
        assert_eq!(collision_file(area, "m533700"), None);
        assert_eq!(collision_file(area, ""), None);
    }
}
