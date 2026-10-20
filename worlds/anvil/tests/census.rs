//! The readers' census (docs/DESIGN.md, Testing): every entry of every forge in an Anvil game's folder whose name-table
//! type is one the readers read, decoded and read, and every refusal counted by its reason with examples. A count, not
//! an oracle: it says what the readers accept and refuse across the whole game, not that what they accept is right.
//!
//! Run by hand (about 24 GB read for Riders Republic): `PRIMASE_ANVIL_DIR=<folder> cargo test -p primase-anvil --test
//! census -- --ignored --nocapture`.
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::lod::{LOD_SELECTOR, StoredLodSelector};
use primase_anvil::read::material::{MATERIAL, StoredReferences, TEXTURE_MAP, TEXTURE_SET};
use primase_anvil::read::mesh::{Layout, MESH, StoredMesh};
use primase_anvil::read::mesh_shape::{MESH_SHAPE, MeshShape};
use primase_anvil::read::mip::{COMPILED_MIP, StoredMip};
use primase_anvil::read::shapes::MAX_RECORD;
use primase_anvil::read::texture::{self, StoredTextureMap};
use primase_core::TexFormat;
use primase_testdata::real_data;

/// The kinds read, by their type ids.
const KINDS: [(u32, &str); 7] = [
    (MESH, "Mesh"),
    (TEXTURE_MAP, "TextureMap"),
    (COMPILED_MIP, "CompiledMip"),
    (MATERIAL, "Material"),
    (TEXTURE_SET, "TextureSet"),
    (LOD_SELECTOR, "LODSelector"),
    (MESH_SHAPE, "MeshShape"),
];

/// A refusal reason grouped: hexadecimal words and numbers of five or more digits (ids, offsets, lengths) masked as
/// `#`, smaller numbers (format codes, versions, strides, counts) kept so each variant is its own group.
fn mask(reason: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let hex = word.starts_with("0x");
        let long = word.len() >= 5 && word.chars().all(|c| c.is_ascii_digit());
        out.push_str(if hex || long { "#" } else { word.as_str() });
        word.clear();
    };
    for c in reason.chars() {
        if c.is_ascii_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// One kind's tally: what was read, and each refusal reason (ids and large numbers masked) with its count and first
/// examples.
#[derive(Default)]
struct Tally {
    read: u64,
    outcomes: BTreeMap<String, u64>,
    refused: BTreeMap<String, (u64, Vec<String>)>,
}

impl Tally {
    fn outcome(&mut self, what: impl Into<String>) {
        *self.outcomes.entry(what.into()).or_default() += 1;
    }
    fn refuse(&mut self, reason: &str, example: &str) {
        let key = mask(reason);
        let (n, examples) = self.refused.entry(key).or_default();
        *n += 1;
        if examples.len() < 3 {
            examples.push(format!("{example}: {reason}"));
        }
    }
}

/// The entry's resource, or why not.
fn resource(forge: &Forge, file: &mut File, k: usize) -> Result<Resource, String> {
    let payload = forge.payload(file, k).map_err(|e| format!("payload: {e}"))?;
    let entry = read_entry(&payload, MAX_RECORD).map_err(|e| format!("data: {e}"))?;
    Resource::parse(&entry.resource.data).map_err(|e| format!("resource: {e}"))
}

fn census_forge(path: &Path, tallies: &mut BTreeMap<&'static str, Tally>) {
    let forge = Forge::open(path).expect("forge");
    let mut file = File::open(path).expect("open");
    let label = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    for k in 0..forge.names.len() {
        let name_type = forge.names[k].type_id;
        let Some(&(type_id, kind)) = KINDS.iter().find(|(t, _)| *t == name_type) else {
            continue;
        };
        let tally = tallies.entry(kind).or_default();
        let example = format!("{label}:{k} {}", forge.names[k].name);
        let r = match resource(&forge, &mut file, k) {
            Ok(r) => r,
            Err(e) => {
                tally.refuse(&e, &example);
                continue;
            }
        };
        if r.type_id != type_id {
            tally.refuse(
                &format!(
                    "record type {:#010x} under name-table type {type_id:#010x}",
                    r.type_id
                ),
                &example,
            );
            continue;
        }
        let result: Result<(), String> = match type_id {
            MESH => StoredMesh::parse(&r.body)
                .and_then(|m| {
                    m.triangles()?;
                    m.materials()?;
                    Ok(m)
                })
                .map(|m| {
                    let (layout, mesh_data) = match &m.layout {
                        Layout::Ranged(r) => ("ranged", r.mesh_data),
                        Layout::Inline(i) => ("inline", i.mesh_data),
                    };
                    tally.outcome(layout);
                    // a measured relation, not a reading: MeshData's first byte against the vertex count
                    tally.outcome(format!(
                        "{layout}: MeshData first byte {}, more than 65,535 vertices {}",
                        mesh_data[0],
                        m.vertices.len() > 65_535
                    ));
                    tally.outcome(format!("{} ranges", m.ranges().map_or(0, |r| r.len())));
                })
                .map_err(|e| e.to_string()),
            TEXTURE_MAP => StoredTextureMap::parse(&r.body)
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    tally.outcome(format!("format code {}", t.format));
                    if texture::storage(t.format).is_none() {
                        // a measurement for codes not established: which storage classes the held levels fill exactly
                        let classes = [
                            ("8 bytes per 4x4 block", TexFormat::Bc1),
                            ("16 bytes per 4x4 block", TexFormat::Bc3),
                            ("1 byte per pixel", TexFormat::R8Unorm),
                            ("4 bytes per pixel", TexFormat::Rgba8Unorm),
                            ("8 bytes per pixel", TexFormat::Rgba16Float),
                        ];
                        let first = u32::try_from(t.references.len()).unwrap_or(u32::MAX);
                        let closes: Vec<&str> = classes
                            .iter()
                            .filter(|(_, f)| {
                                (first..t.mip_count)
                                    .map(|i| {
                                        let (w, h) = t.level_size(i);
                                        f.checked_level_size(w, h)
                                    })
                                    .try_fold(0usize, |sum, s| s.map(|s| sum + s))
                                    == Some(t.pixels.len())
                            })
                            .map(|(name, _)| *name)
                            .collect();
                        tally.outcome(format!("format code {} closes as {closes:?}", t.format));
                    }
                    t.levels().map_err(|e| e.to_string())?;
                    // each external level: a CompiledMip of this map, at its level, sized for it
                    let Some(f) = texture::storage(t.format) else {
                        return Ok(());
                    };
                    for (level, m) in (0u32..).zip(&t.references) {
                        let Some(e) = forge.find(m.id) else {
                            tally.outcome("external level in another forge");
                            continue;
                        };
                        let mip = resource(&forge, &mut file, e)?;
                        let mip = StoredMip::parse(&mip.body).map_err(|e| format!("external level: {e}"))?;
                        let (w, h) = t.level_size(level);
                        if (mip.parent, mip.level, Some(mip.image.len()))
                            != (r.class_id, level, f.level_size(w, h))
                        {
                            return Err(format!(
                                "external level {level}: parent, level, bytes {:?}, the map's {:?}",
                                (mip.parent, mip.level, mip.image.len()),
                                (r.class_id, level, f.level_size(w, h))
                            ));
                        }
                    }
                    Ok(())
                }),
            COMPILED_MIP => StoredMip::parse(&r.body).map(|_| ()).map_err(|e| e.to_string()),
            MATERIAL | TEXTURE_SET => StoredReferences::parse(&r.body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            LOD_SELECTOR => StoredLodSelector::parse(&r.body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            MESH_SHAPE => MeshShape::parse(&r.body)
                .and_then(|s| s.geometry().map(|_| ()))
                .map_err(|e| e.to_string()),
            _ => Ok(()),
        };
        match result {
            Ok(()) => tally.read += 1,
            Err(e) => tally.refuse(&e, &example),
        }
    }
}

#[test]
#[ignore = "reads every forge of a game's folder (about 24 GB for Riders Republic); run by hand"]
fn every_entry_of_the_kinds_read() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let mut forges: Vec<_> = std::fs::read_dir(&dir)
        .expect("folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "forge"))
        .collect();
    forges.sort();
    let mut tallies = BTreeMap::new();
    for path in &forges {
        eprintln!("census: {}", path.display());
        census_forge(path, &mut tallies);
    }
    for (kind, t) in &tallies {
        let refused: u64 = t.refused.values().map(|(n, _)| n).sum();
        println!("{kind}: {} read, {refused} refused", t.read);
        for (what, n) in &t.outcomes {
            println!("  {n:>8}  {what}");
        }
        for (reason, (n, examples)) in &t.refused {
            println!("  refused {n:>8}  {reason}");
            for e in examples {
                println!("              e.g. {e}");
            }
        }
    }
}
