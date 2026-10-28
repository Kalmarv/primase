//! Map collision as the area stores it: the reader keeps, whole, every file of the area's high-resolution collision
//! binder that MSB collision parts name (`fromsoft.map_collision`); `lower-fromsoft-collision` lowers it.
use crate::invalid;
use crate::pose::FRAME;
use crate::sekiro::source::{CollisionPart, MapCollision};
use fromsoft_formats::{
    Archive, Binder, Result, collision,
    dcx::Decoder,
    msb::{MapLayout, PartKind},
};
use havok_formats::Compendium;
use primase_core::{Name, Transform, World};
use primase_foundations::parallel::Threads;
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
    /// With `f_collision`: whether the area has an `f` collision binder.
    pub f_binder: bool,
    /// With `f_collision`: the `f` binder's files read (one per read `h` file that has a file of the same model).
    pub f_files: usize,
    /// With `f_collision`: the read `h` files whose `f` file holds no triangle (it counts in `f_files`).
    pub empty_f_files: Vec<String>,
    /// With `f_collision`: files of the `f` binder no read `h` file pairs with (no MSB part names their model): not
    /// read.
    pub unpaired_f_files: usize,
    /// With `f_collision`: the triangles of the `f` files read ([`MapCollision::f_triangles`]).
    pub f_triangles: usize,
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
    let name = fromsoft_formats::names::fold(name);
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
    let model = fromsoft_formats::names::fold(model);
    let suffix = model.strip_prefix('h')?;
    Some(fromsoft_formats::names::fold(&format!(
        "h{}_{suffix}.hkx",
        &area[1..]
    )))
}

/// Read every collision file the area's MSB collision parts name, of every hit filter, as `fromsoft.map_collision`
/// entities in file-name order; with `states`, each part's clause. An area whose MSB has no collision part has nothing
/// to read, and its binder is not opened: Elden Ring's m60_22_22_01 holds three map pieces and no collision part, and
/// no archive holds an `h60_22_22_01.hkxbhd`.
#[allow(clippy::too_many_arguments)]
pub fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    folder: &str,
    area: &str,
    layout: &MapLayout,
    mut states: Option<&mut super::states::States>,
    f_collision: bool,
    threads: Threads,
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
        .map(|o| o.matrix().map(crate::pose::placed))
        .transpose()?
        .unwrap_or(Transform::IDENTITY);
    let mut f_binder = if f_collision {
        FBinder::read(archive, decoder, folder, area)?
    } else {
        None
    };
    report.f_binder = f_binder.is_some();
    let mut files = binder
        .files
        .iter()
        .filter(|f| f.name.as_deref().is_some_and(|n| leaf(n).ends_with(".hkx")))
        .collect::<Vec<_>>();
    files.sort_by_key(|f| leaf(f.name.as_deref().unwrap_or("")));
    // the files some part names, in name order, each with the bytes of its `f` partner; a file whose name an earlier
    // one took is not named any more
    let mut wanted = Vec::new();
    for file in files {
        let name = leaf(file.name.as_deref().unwrap_or(""));
        let Some(parts) = named.remove(&name) else {
            report.unnamed_files += 1;
            continue;
        };
        let f_partner = f_binder.as_mut().and_then(|f| f.take_bytes(&name));
        wanted.push((file, name, parts, f_partner));
    }
    let f_compendium = f_binder.as_ref().and_then(|f| f.compendium.as_ref());
    // The files are decoded and read a few at a time, on the given threads, and put in the world in name order: what a
    // file adds to the report and the world is the same whatever the order they were read in.
    let mut wanted = wanted.into_iter();
    loop {
        let wave: Vec<_> = wanted.by_ref().take(threads.get().saturating_mul(2)).collect();
        if wave.is_empty() {
            break;
        }
        let read = primase_foundations::parallel::map(threads, &wave, |_, (file, _, _, f_partner)| {
            let raw = decoder
                .decode_if_dcx(&file.bytes)
                .and_then(|bytes| collision::read_map(&bytes, Some(&comp)));
            let f_soup = f_partner
                .as_ref()
                .map(|(f_name, bytes)| FBinder::soup(f_name, bytes, f_compendium))
                .transpose();
            (raw, f_soup)
        })
        .map_err(|e| invalid("collision", 0, e.to_string()))?;
        for ((_, name, parts, _), (raw, f_soup)) in wave.into_iter().zip(read) {
            let raw = raw?;
            report.files += 1;
            report.triangles += raw.triangles.len();
            report.unbound_triangles += raw.unmapped;
            report.unkeyed_triangle_data += raw.unkeyed_triangle_data;
            report.unread_custom_primitives += raw.unread_custom_primitives;
            for shape in &raw.unread_bodies {
                *report.unread_bodies.entry(shape.clone()).or_default() += 1;
            }
            let to_world = |triangles: Vec<[[f32; 3]; 3]>| -> Vec<[[f32; 3]; 3]> {
                triangles
                    .into_iter()
                    .map(|t| t.map(|v| transform.apply(FRAME.point(v))))
                    .collect()
            };
            let f_triangles = match f_soup? {
                Some(soup) => {
                    report.f_files += 1;
                    report.f_triangles += soup.triangles.len();
                    if soup.triangles.is_empty() {
                        report.empty_f_files.push(name.clone());
                    }
                    to_world(soup.triangles)
                }
                None => Vec::new(),
            };
            let e = world.spawn();
            world.insert(e, Name(name.clone()));
            world.insert(
                e,
                MapCollision {
                    file: name,
                    parts,
                    triangles: to_world(raw.triangles),
                    f_triangles,
                    materials: raw.materials,
                    invalid_material_keys: raw.invalid_material_keys,
                    dead_primitives: raw.dead_primitives,
                    unread_bodies: raw.unread_bodies,
                },
            );
        }
    }
    report.missing_files = named.into_keys().collect();
    report.unpaired_f_files = f_binder.map_or(0, |f| f.files.len());
    Ok(report)
}

/// The members of an area's `f` collision binder (`f<area>.hkxbhd`, absent from some areas), decoded, by file name:
/// each is the extern mesh of the `h` file of the same model ([`collision::read_map`] reads it).
struct FBinder {
    files: BTreeMap<String, Vec<u8>>,
    compendium: Option<Compendium>,
}

impl FBinder {
    /// The area's `f` binder, or `None` where the archive holds none.
    fn read(archive: &Archive, decoder: &Decoder<'_>, folder: &str, area: &str) -> Result<Option<Self>> {
        let base = format!("{folder}/f{}", &area[1..]);
        let (header, payload) = (format!("{base}.hkxbhd"), format!("{base}.hkxbdt"));
        if !archive.contains(&header) {
            return Ok(None);
        }
        let binder = Binder::read_split(
            &archive.read(&header, decoder)?,
            &archive.read(&payload, decoder)?,
            decoder,
        )?;
        let mut files = BTreeMap::new();
        let mut compendium = None;
        for member in &binder.files {
            let name = leaf(member.name.as_deref().unwrap_or(""));
            if name.ends_with(".compendium") {
                compendium = Some(Compendium::read(&decoder.decode_if_dcx(&member.bytes)?)?);
            } else if name.ends_with(".hkx") {
                files.insert(name, decoder.decode_if_dcx(&member.bytes)?);
            }
        }
        Ok(Some(FBinder { files, compendium }))
    }

    /// The name and bytes of the file for the `h` file `h_name` (`h<area>_<model>.hkx`: `f<area>_<model>.hkx`), if the
    /// binder holds one; taken out of the binder.
    fn take_bytes(&mut self, h_name: &str) -> Option<(String, Vec<u8>)> {
        let f_name = format!("f{}", h_name.strip_prefix('h').unwrap_or(h_name));
        let bytes = self.files.remove(&f_name)?;
        Some((f_name, bytes))
    }

    /// The triangles of the `f` file `f_name` that `bytes` holds; every body of it must be an extern mesh.
    fn soup(f_name: &str, bytes: &[u8], compendium: Option<&Compendium>) -> Result<collision::Soup> {
        let soup = collision::read_map(bytes, compendium)?;
        if let Some(shape) = soup.unread_bodies.first() {
            return Err(invalid(
                "collision",
                0,
                format!("{f_name}: a body of {shape}, not an extern mesh"),
            ));
        }
        Ok(soup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::msb::Generation;

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
            &Archive::empty(fromsoft_formats::Game::EldenRing),
            &decoder,
            "/map/m60/m60_22_22_01",
            "m60_22_22_01",
            &layout,
            None,
            false,
            Threads::ONE,
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
