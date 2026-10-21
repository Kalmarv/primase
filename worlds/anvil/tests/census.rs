//! The readers' census (docs/DESIGN.md, Testing): every entry of every forge in an Anvil game's folder whose name-table
//! type is one the readers read, decoded and read, and every refusal counted by its reason with examples. A count, not
//! an oracle: it says what the readers accept and refuse across the whole game, not that what they accept is right.
//!
//! Run by hand (about 24 GB read for Riders Republic): `PRIMASE_ANVIL_DIR=<folder> cargo test -p primase-anvil --test
//! census -- --ignored --nocapture`.
//!
//! With `PRIMASE_ANVIL_RRAS_MESHES` (RRAssetStudio's read of every Mesh), every Mesh primase reads is also compared
//! with the third-party tool's: vertex and triangle counts, the submesh partition and material ids, and the
//! SHA-256s of its positions (f32 little-endian, in stored order, no frame) and absolute indices (u32
//! little-endian, in stored order and winding). Each difference is counted with examples, under `vs RRAssetStudio`.

/// RRAssetStudio's row for a mesh, by forge file name and entry.
type Oracle = std::collections::HashMap<(String, usize), Value>;

fn oracle() -> Option<Oracle> {
    let dir = real_data("PRIMASE_ANVIL_RRAS_MESHES")?;
    let text = std::fs::read_to_string(dir.join("summary.jsonl")).expect("summary.jsonl");
    Some(
        text.lines()
            .map(|l| {
                let v: Value = serde_json::from_str(l).expect("a summary row");
                let key = (
                    v["forge"].as_str().expect("forge").to_owned(),
                    usize::try_from(v["ordinal"].as_u64().expect("ordinal")).expect("small"),
                );
                (key, v)
            })
            .collect(),
    )
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Where a mesh primase read differs from RRAssetStudio's row (none: they agree on everything compared).
fn against(m: &StoredMesh, row: &Value) -> Vec<String> {
    let mut differ = Vec::new();
    let triangles = m.triangles().unwrap_or_default();
    let counts = (m.vertices.len() as u64, triangles.len() as u64);
    let theirs = (
        row["vertex_count"].as_u64().unwrap_or(0),
        row["triangle_count"].as_u64().unwrap_or(0),
    );
    if counts != theirs {
        differ.push(format!("counts {counts:?}, RRAssetStudio's {theirs:?}"));
    }
    let materials = m.materials().unwrap_or_default();
    let ours: Vec<[u64; 5]> = m
        .ranges()
        .unwrap_or_default()
        .iter()
        .zip(&materials)
        .map(|(r, &id)| {
            [
                r.indices.start as u64,
                r.indices.len() as u64,
                r.vertices.start as u64,
                r.vertices.len() as u64,
                id,
            ]
        })
        .collect();
    let theirs: Vec<[u64; 5]> = row["submeshes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    [
                        "first_index",
                        "index_count",
                        "vertex_base",
                        "vertex_count",
                        "material_id",
                    ]
                    // RRAssetStudio writes null for a material whose stored ClassID is 0
                    .map(|k| match &s[k] {
                        Value::Null if k == "material_id" => 0,
                        v => v.as_u64().unwrap_or(u64::MAX),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if ours != theirs {
        differ.push(format!("submeshes {ours:?}, RRAssetStudio's {theirs:?}"));
    }
    let positions: Vec<u8> = m
        .positions()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect();
    if Some(sha(&positions).as_str()) != row["positions_sha256"].as_str() {
        differ.push("positions".into());
    }
    let indices: Vec<u8> = triangles.iter().flatten().flat_map(|i| i.to_le_bytes()).collect();
    if Some(sha(&indices).as_str()) != row["indices_sha256"].as_str() {
        differ.push("indices".into());
    }
    differ
}
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::entity::{
    BoundingVolume, ComponentBody, ENTITY, ENTITY_GROUP, Instance, Slot, StoredEntity, StoredEntityGroup,
    Visual,
};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::grid::{
    self, GRID_CELL_DATA_BLOCK, GRID_PARTITION, StoredGridCellDataBlock, StoredGridPartition, WORLD,
};
use primase_anvil::read::lod::{LOD_SELECTOR, StoredLodSelector};
use primase_anvil::read::material::{
    MATERIAL, Payload, StoredMaterial, StoredTextureSet, TEXTURE_MAP, TEXTURE_SET,
};
use primase_anvil::read::mesh::{Layout, MESH, StoredMesh};
use primase_anvil::read::mesh_shape::{MESH_SHAPE, MeshShape};
use primase_anvil::read::mip::{COMPILED_MIP, StoredMip};
use primase_anvil::read::shape::{
    BOX_SHAPE, CAPSULE_SHAPE, REFERENCE_LIST_SHAPE, SPHERE_SHAPE, StoredBox, StoredCapsule,
    StoredReferenceList, StoredSphere,
};
use primase_anvil::read::shapes::MAX_RECORD;
use primase_anvil::read::template::{self, MATERIAL_TEMPLATE};
use primase_anvil::read::terrain_material::{
    COLLISION_MATERIAL, StoredMaterialBank, StoredTerrainMaterial, TERRAIN_MATERIAL, TERRAIN_MATERIAL_BANK,
};
use primase_anvil::read::texture::{self, StoredTextureMap};
use primase_core::TexFormat;
use primase_testdata::real_data;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The kinds read, by their type ids.
const KINDS: [(u32, &str); 19] = [
    (TERRAIN_MATERIAL_BANK, "TerrainMaterialBank"),
    (TERRAIN_MATERIAL, "TerrainMaterial"),
    (SPHERE_SHAPE, "SphereShape"),
    (CAPSULE_SHAPE, "CapsuleShape"),
    (BOX_SHAPE, "BoxShape"),
    (REFERENCE_LIST_SHAPE, "ReferenceListShape"),
    (ENTITY, "Entity"),
    (ENTITY_GROUP, "EntityGroup"),
    (GRID_PARTITION, "GridPartition"),
    (GRID_CELL_DATA_BLOCK, "GridCellDataBlock"),
    (WORLD, "World"),
    (MATERIAL_TEMPLATE, "MaterialTemplate"),
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
    fn count_by(&mut self, what: &str, n: usize) {
        *self.outcomes.entry(what.to_owned()).or_default() += u64::try_from(n).unwrap_or(u64::MAX);
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

/// An Entity's (or a group's own) Visuals tallied: each's instance data and slots, a slot holding instance matrices
/// in lower case; whether any is instanced, and whether the placement reader would refuse it for its scale.
/// The axis-aligned box, in f64, around the box `min`..`max`'s 8 corners under the column-major affine `inner`, then
/// the rotation (upper 3x3) of the column-major `outer`, without its translation.
fn transformed_box(
    outer: &[f32; 16],
    inner: &[f32; 16],
    (min, max): ([f32; 3], [f32; 3]),
) -> ([f64; 3], [f64; 3]) {
    let (o, i) = (outer.map(f64::from), inner.map(f64::from));
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for corner in 0..8 {
        let p = [0, 1, 2].map(|a| f64::from(if corner >> a & 1 == 0 { min[a] } else { max[a] }));
        let q = [0, 1, 2].map(|r| i[r] * p[0] + i[4 + r] * p[1] + i[8 + r] * p[2] + i[12 + r]);
        for r in 0..3 {
            let v = o[r] * q[0] + o[4 + r] * q[1] + o[8 + r] * q[2];
            lo[r] = lo[r].min(v);
            hi[r] = hi[r].max(v);
        }
    }
    (lo, hi)
}

/// The largest absolute difference, over a stored bounding volume's 6 coordinates, from a box computed in f64, as a
/// bucket: "0" where every coordinate equals the computed one rounded to f32, else its power of ten.
fn bound_fit(stored: &BoundingVolume, (lo, hi): ([f64; 3], [f64; 3])) -> String {
    let pairs: Vec<(f32, f64)> = stored
        .min
        .iter()
        .copied()
        .zip(lo)
        .chain(stored.max.iter().copied().zip(hi))
        .collect();
    if pairs.iter().all(|&(s, c)| s == c as f32) {
        return "0".into();
    }
    let worst = pairs
        .iter()
        .map(|&(s, c)| (f64::from(s) - c).abs())
        .fold(0.0, f64::max);
    format!("at most 1e{}", worst.log10().ceil() as i32)
}

/// Each instanced Visual's per-instance bounds against its instance matrix, then its owner's rotation, applied to the
/// LODSelectorInstance's bounds and to its slot's MeshInstanceData bounds.
fn instance_bound_outcomes(tally: &mut Tally, v: &Visual, owner: &[f32; 16]) {
    let Instance::Lod { bounds, slots, .. } = &v.instance else {
        return;
    };
    for s in slots {
        let Slot::Mesh(m) = s else { continue };
        for (i, b) in m.instances.iter().zip(&m.instance_bounds) {
            // in the owner's rotated frame, not translated (the Riders Republic notes, docs/world.md §7)
            for (what, of) in [("LSI", bounds), ("MID", &m.bounds)] {
                let fit = bound_fit(b, transformed_box(owner, i, (of.min, of.max)));
                tally.outcome(format!("instance bounds vs R I ({what} bounds): {fit}"));
            }
        }
        if m.instances.len() != m.instance_bounds.len() {
            tally.outcome("instance matrices and bounds counts differ");
        }
    }
}

fn entity_outcomes(tally: &mut Tally, e: &StoredEntity) {
    for v in e.visuals() {
        instance_bound_outcomes(tally, v, &e.matrix);
    }
    for v in e.visuals() {
        tally.outcome(match &v.instance {
            // a slot holding instance matrices in lower case
            Instance::Lod { instances, slots, .. } => {
                let slots: String = slots
                    .iter()
                    .map(|s| match s {
                        Slot::None => '-',
                        Slot::Mesh(m) if m.instances.is_empty() => 'M',
                        Slot::Mesh(_) => 'm',
                        Slot::Impostor(i) if i.instances.is_empty() => 'I',
                        Slot::Impostor(_) => 'i',
                    })
                    .collect();
                let own = if instances.is_empty() {
                    ""
                } else {
                    ", instanced itself"
                };
                format!("Visual LODSelectorInstance, slots {slots}{own}")
            }
            Instance::Mesh(_) => "Visual MeshInstanceData".into(),
            Instance::Raw { kind, .. } => format!("Visual instance {kind:08X} kept"),
            Instance::None => "Visual without instance data".into(),
        });
    }
    let instanced = e.visuals().any(|v| match &v.instance {
        Instance::Lod { slots, .. } => slots
            .iter()
            .any(|s| matches!(s, Slot::Mesh(m) if !m.instances.is_empty())),
        Instance::Mesh(m) => !m.instances.is_empty(),
        Instance::Raw { .. } | Instance::None => false,
    });
    tally.outcome(if instanced { "instanced" } else { "not instanced" });
    // refused by the placement reader: whether the scale applies to instances isn't established
    if instanced && e.scale != 1.0 {
        tally.outcome("instanced, scale not 1");
    }
}

/// A box as two corners.
type Corners = ([f32; 3], [f32; 3]);

/// An instanced EntityGroup Visual: its object, the group's matrix, each instance's matrix and stored bounds.
type GroupInstances = (u64, [f32; 16], Vec<([f32; 16], BoundingVolume)>);

/// What the cross-forge bounds measurements gather: every Mesh's stored bounds and LODSelector's level references by
/// ClassID (a later forge's copy replacing an earlier's: each patch sorts after its base), and the instanced Visuals.
#[derive(Default)]
struct Levels {
    meshes: std::collections::HashMap<u64, [[f32; 3]; 2]>,
    selectors: std::collections::HashMap<u64, Vec<u64>>,
    groups: Vec<GroupInstances>,
    /// Instanced Entity Visuals: the object and the LODSelectorInstance's bounds.
    entities: Vec<(u64, BoundingVolume)>,
}

impl Levels {
    /// The instanced Visuals of an Entity (or group).
    fn gather(&mut self, e: &StoredEntity, group: bool) {
        for v in e.visuals() {
            let (Some(object), Instance::Lod { bounds, slots, .. }) = (v.object, &v.instance) else {
                continue;
            };
            let instances: Vec<([f32; 16], BoundingVolume)> = slots
                .iter()
                .filter_map(|s| match s {
                    Slot::Mesh(m) => Some(m.instances.iter().copied().zip(m.instance_bounds.iter().copied())),
                    _ => None,
                })
                .flatten()
                .collect();
            if instances.is_empty() {
                continue;
            }
            if group {
                self.groups.push((object.id, e.matrix, instances));
            } else {
                self.entities.push((object.id, *bounds));
            }
        }
    }

    /// The union of the object's mesh levels' stored bounds, and its finest's alone; `None` where none is read.
    fn union(&self, object: u64) -> Option<(Corners, Corners)> {
        let levels = self.selectors.get(&object)?;
        let mut found = levels.iter().filter_map(|id| self.meshes.get(id));
        let first = *found.next()?;
        let mut u = (first[0], first[1]);
        for b in found {
            u.0 = [0, 1, 2].map(|a| u.0[a].min(b[0][a]));
            u.1 = [0, 1, 2].map(|a| u.1[a].max(b[1][a]));
        }
        Some((u, (first[0], first[1])))
    }

    /// The measurements, as tallies.
    fn measure(&self, tallies: &mut BTreeMap<&'static str, Tally>) {
        let t = tallies
            .entry("EntityGroup instance bounds vs R I (its levels' mesh bounds)")
            .or_default();
        for (object, owner, instances) in &self.groups {
            let Some((all, finest)) = self.union(*object) else {
                t.outcome("object's mesh levels not read");
                continue;
            };
            for (i, b) in instances {
                for (what, of) in [("every level", all), ("finest level", finest)] {
                    let (lo, hi) = transformed_box(owner, i, of);
                    t.outcome(format!("{what}: {}", bound_fit(b, (lo, hi))));
                    let inside = (0..3)
                        .all(|a| f64::from(b.min[a]) <= lo[a] + 1e-3 && f64::from(b.max[a]) >= hi[a] - 1e-3);
                    if inside {
                        t.outcome(format!("{what}: contained (1e-3)"));
                    }
                }
            }
        }
        let t = tallies
            .entry("Entity LODSelectorInstance bounds vs its levels' mesh bounds")
            .or_default();
        for (object, bounds) in &self.entities {
            let Some((all, finest)) = self.union(*object) else {
                t.outcome("object's mesh levels not read");
                continue;
            };
            for (what, (lo, hi)) in [("every level", all), ("finest level", finest)] {
                let fit = bound_fit(bounds, (lo.map(f64::from), hi.map(f64::from)));
                t.outcome(format!("{what}: {fit}"));
            }
        }
    }
}

/// Each RigidBodyComponent's body length and the type its forge's index gives its shape (the reference at its offset
/// 748, which the reader requires), named where the type is.
fn rigid_bodies(tally: &mut Tally, e: &StoredEntity, forge: &Forge) {
    for c in &e.components {
        let ComponentBody::RigidBody { shape, bytes } = &c.body else {
            continue;
        };
        let to = match shape {
            None => "none (03)".to_owned(),
            Some(r) => match forge.find(r.id) {
                None => format!("mode {}, an id in no entry of this forge", r.mode),
                Some(k) => {
                    let t = forge.names[k].type_id;
                    let name = match t {
                        0xB22B_3E61 => "MeshShape".to_owned(),
                        0x2D67_5BA2 => "ReferenceListShape".to_owned(),
                        0xFA3F_7A18 => "SphereShape".to_owned(),
                        0xB859_9052 => "CapsuleShape".to_owned(),
                        0x4EC6_8E98 => "BoxShape".to_owned(),
                        _ => format!("{t:08X}"),
                    };
                    format!("mode {}, a {name}", r.mode)
                }
            },
        };
        tally.outcome(format!("RigidBody {} B, shape: {to}", bytes.len()));
    }
}

fn census_forge(
    path: &Path,
    tallies: &mut BTreeMap<&'static str, Tally>,
    oracle: Option<&Oracle>,
    rras: &mut Tally,
    levels: &mut Levels,
) {
    let forge = Forge::open(path).expect("forge");
    let mut file = File::open(path).expect("open");
    let label = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut textured: Vec<(Option<u64>, Vec<u32>)> = Vec::new();
    let mut binding_keys: std::collections::HashMap<u64, std::collections::HashSet<u32>> =
        std::collections::HashMap::new();
    let mut labels: std::collections::HashMap<u64, std::collections::HashSet<u32>> =
        std::collections::HashMap::new();
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
                    levels.meshes.insert(r.class_id, m.bounds);
                    if m.stale_entries().is_ok_and(|e| e.is_some()) {
                        tally.outcome("MeshPrimitive entries other than the ranges");
                    }
                    let (layout, first_byte) = match &m.layout {
                        Layout::Ranged(r) => ("ranged", r.mesh_data[0]),
                        Layout::Inline(i) => ("inline", i.declaration[0]),
                    };
                    if let Some(o) = oracle {
                        match o.get(&(label.clone(), k)) {
                            None => rras.refuse("no RRAssetStudio row", &example),
                            Some(row) => {
                                let differ = against(&m, row);
                                if differ.is_empty() {
                                    rras.read += 1;
                                }
                                for d in differ {
                                    rras.refuse(&d, &example);
                                }
                            }
                        }
                    }
                    tally.outcome(layout);
                    // a measured relation: the decoded extrema against the bounds block's triples, and against the
                    // triples at body 20 and 36 (where the first 81-byte record lies when there are any)
                    let at = |o: usize| -> [f32; 3] {
                        [0, 4, 8].map(|i| {
                            let w = r.body.get(o + i..o + i + 4).and_then(|w| w.try_into().ok());
                            w.map_or(f32::NAN, f32::from_le_bytes)
                        })
                    };
                    // a quantization step: the split layouts' scale over 32768, otherwise the largest w over 32768
                    let step = if m.positions_stream.is_empty() {
                        Some(
                            m.vertices
                                .ws()
                                .map(|w| f32::from(w).abs() / 32768.0)
                                .fold(0.0f32, f32::max),
                        )
                    } else {
                        m.material_list.descriptors.last().map(|d| {
                            f32::from_le_bytes([d.rest[4], d.rest[5], d.rest[6], d.rest[7]]).abs() / 32768.0
                        })
                    };
                    let records = if m.records_81.is_empty() {
                        "no records"
                    } else {
                        "records"
                    };
                    if let Ok(p) = m.positions() {
                        let lo = p
                            .iter()
                            .fold([f32::INFINITY; 3], |a, v| [0, 1, 2].map(|i| a[i].min(v[i])));
                        let hi = p
                            .iter()
                            .fold([f32::NEG_INFINITY; 3], |a, v| [0, 1, 2].map(|i| a[i].max(v[i])));
                        for (which, b) in [("bounds block", m.bounds), ("body 20, 36", [at(20), at(36)])] {
                            let d = (0..3)
                                .flat_map(|i| [(lo[i] - b[0][i]).abs(), (hi[i] - b[1][i]).abs()])
                                .fold(0.0f32, |a, x| {
                                    if x.is_nan() || a.is_nan() {
                                        f32::NAN
                                    } else {
                                        a.max(x)
                                    }
                                });
                            let fit = if p.is_empty() {
                                "no vertices".to_owned()
                            } else if d == 0.0 {
                                "equal".to_owned()
                            } else if let Some(s) = step.filter(|s| *s > 0.0) {
                                match d / s {
                                    x if x <= 1.0 => "within 1 step".to_owned(),
                                    x if x <= 3.0 => "within 3 steps".to_owned(),
                                    _ => "beyond 3 steps".to_owned(),
                                }
                            } else {
                                "unequal, no step".to_owned()
                            };
                            tally.outcome(format!("extrema vs {which} ({layout}, {records}): {fit}"));
                        }
                    }
                    // a measured relation, not a reading: MeshData's first byte against the vertex count
                    tally.outcome(format!(
                        "{layout}: MeshData first byte {}, more than 65,535 vertices {}",
                        first_byte,
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
            TEXTURE_SET => StoredTextureSet::parse(&r.body)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            MATERIAL => StoredMaterial::parse(&r.body)
                .map(|m| {
                    // the join measured after the forge: each texture record's key against its template's labels
                    let template = m.references[0].map(|t| t.id);
                    let keys = m
                        .properties
                        .iter()
                        .filter(|p| matches!(p.payload, Payload::Texture { .. }))
                        .map(|p| p.key)
                        .collect::<Vec<_>>();
                    textured.push((template, keys));
                })
                .map_err(|e| e.to_string()),
            ENTITY => StoredEntity::parse(&r.body)
                .map(|e| {
                    entity_outcomes(tally, &e);
                    rigid_bodies(tally, &e, &forge);
                    levels.gather(&e, false);
                })
                .map_err(|e| e.to_string()),
            ENTITY_GROUP => StoredEntityGroup::parse(&r.body)
                .map(|g| {
                    entity_outcomes(tally, &g.entity);
                    rigid_bodies(tally, &g.entity, &forge);
                    levels.gather(&g.entity, true);
                    tally.outcome(format!(
                        "{} members, last word {}",
                        if g.members.is_empty() { "no" } else { "with" },
                        g.word
                    ));
                    for v in g.entity.visuals() {
                        tally.outcome(format!("Visual head {:02x?}", v.head));
                    }
                })
                .map_err(|e| e.to_string()),
            SPHERE_SHAPE => StoredSphere::parse(&r.body)
                .map(|x| {
                    tally.outcome(if x.material.is_some() {
                        "with a CollisionMaterial"
                    } else {
                        "without"
                    })
                })
                .map_err(|e| e.to_string()),
            CAPSULE_SHAPE => StoredCapsule::parse(&r.body)
                .map(|x| {
                    tally.outcome(if x.material.is_some() {
                        "with a CollisionMaterial"
                    } else {
                        "without"
                    })
                })
                .map_err(|e| e.to_string()),
            BOX_SHAPE => StoredBox::parse(&r.body)
                .map(|x| tally.outcome(format!("byte {:#04x}", x.byte)))
                .map_err(|e| e.to_string()),
            REFERENCE_LIST_SHAPE => StoredReferenceList::parse(&r.body)
                .map(|x| {
                    for c in &x.children {
                        tally.outcome(match forge.find(c.id).map(|k| forge.names[k].type_id) {
                            Some(0xB22B_3E61) => "child: a MeshShape",
                            Some(SPHERE_SHAPE) => "child: a SphereShape",
                            Some(CAPSULE_SHAPE) => "child: a CapsuleShape",
                            Some(BOX_SHAPE) => "child: a BoxShape",
                            Some(REFERENCE_LIST_SHAPE) => "child: a ReferenceListShape",
                            Some(_) => "child: another type",
                            None => "child: an id in no entry of this forge",
                        });
                    }
                })
                .map_err(|e| e.to_string()),
            TERRAIN_MATERIAL_BANK => StoredMaterialBank::parse(&r.body)
                .map(|b| {
                    tally.outcome(format!(
                        "{} details, {} materials, {} regions",
                        b.details.len(),
                        b.materials.len(),
                        b.regions.len()
                    ));
                })
                .map_err(|e| e.to_string()),
            TERRAIN_MATERIAL => StoredTerrainMaterial::parse(&r.body)
                .map(|m| {
                    tally.outcome(
                        match m
                            .collision_material
                            .map(|r| forge.find(r.id).map(|k| forge.names[k].type_id))
                        {
                            None => "collision material: none",
                            Some(Some(COLLISION_MATERIAL)) => "collision material: a CollisionMaterial",
                            Some(Some(_)) => "collision material: another type",
                            Some(None) => "collision material: an id in no entry of this forge",
                        },
                    );
                })
                .map_err(|e| e.to_string()),
            GRID_PARTITION => StoredGridPartition::parse(&r.body)
                .map(|p| {
                    let blocks = p.cells.iter().filter(|&&c| c != 0).count();
                    tally.outcome(format!("{} levels, {} cells", p.levels, p.cells.len()));
                    tally.count_by("cells holding a block", blocks);
                })
                .map_err(|e| e.to_string()),
            GRID_CELL_DATA_BLOCK => StoredGridCellDataBlock::parse(&r.body)
                .map(|b| {
                    tally.count_by("listed references", b.objects.len());
                    if usize::try_from(b.word).is_ok_and(|w| w <= b.objects.len()) {
                        tally.outcome("word at most the reference count");
                    }
                })
                .map_err(|e| e.to_string()),
            // the GridLayout run's length is the program's stated choice: 5 u16 in a base forge, 4 in a patch
            WORLD => grid::grid_descriptions(&r.body, if label.contains("_patch_") { 4 } else { 5 })
                .map_err(|e| e.to_string())
                .and_then(|grids| {
                    for g in &grids {
                        let l = &g.layout;
                        tally.outcome(format!(
                            "GridLayout {} {:?} {}, after {:02x?}",
                            l.cell_size, l.bottom_left, l.dimension, l.after
                        ));
                        let Some(partition) = g.partition else {
                            tally.outcome("GridDescription without a partition");
                            continue;
                        };
                        let Some(k) = forge.find(partition.id) else {
                            tally.outcome("GridPartition in another forge");
                            continue;
                        };
                        let p = resource(&forge, &mut file, k)
                            .and_then(|r| StoredGridPartition::parse(&r.body).map_err(|e| e.to_string()))?;
                        if l.cell_count(p.levels) != Some(p.cells.len()) {
                            return Err(format!(
                                "GridPartition of {} cells, not its layout's at {} levels",
                                p.cells.len(),
                                p.levels
                            ));
                        }
                        tally.outcome("GridPartition cells = its layout's squared dimensions summed");
                    }
                    Ok(())
                }),
            MATERIAL_TEMPLATE => template::descriptors(&r.body)
                .map_err(|e| e.to_string())
                .and_then(|d| {
                    labels.insert(r.class_id, d.iter().map(template::Descriptor::key).collect());
                    let b = template::bindings(&r.body).map_err(|e| e.to_string())?;
                    for x in &b {
                        tally.outcome(match x.kind() {
                            template::BindingKind::Texture { .. } => "binding: texture",
                            template::BindingKind::Scalar { .. } => "binding: scalar",
                            template::BindingKind::Vector { .. } => "binding: vec4",
                            template::BindingKind::UvTransform { .. } => "binding: uv transform",
                            template::BindingKind::Other => "binding: another (f3, f4)",
                        });
                    }
                    tally.outcome(match b.len().cmp(&d.len()) {
                        std::cmp::Ordering::Equal => "bindings as many as descriptors",
                        std::cmp::Ordering::Less => "bindings fewer than descriptors",
                        std::cmp::Ordering::Greater => "bindings more than descriptors",
                    });
                    binding_keys.insert(r.class_id, b.iter().map(|x| x.key).collect());
                    Ok(())
                }),
            LOD_SELECTOR => StoredLodSelector::parse(&r.body)
                .map(|s| {
                    let ids = s
                        .levels
                        .iter()
                        .filter_map(|l| l.reference.map(|x| x.id))
                        .collect();
                    levels.selectors.insert(r.class_id, ids);
                })
                .map_err(|e| e.to_string()),
            MESH_SHAPE => MeshShape::parse(&r.body)
                .and_then(|s| s.surfaces().map(|_| ()))
                .map_err(|e| e.to_string()),
            _ => Ok(()),
        };
        match result {
            Ok(()) => tally.read += 1,
            Err(e) => tally.refuse(&e, &example),
        }
    }
    // a measurement: Material texture keys against their own template's descriptor labels, within this forge
    let join = tallies
        .entry("Material texture key vs its template's labels")
        .or_default();
    for (template, keys) in &textured {
        for key in keys {
            match template.and_then(|t| labels.get(&t)) {
                None => join.outcome("template not read in this forge"),
                Some(set) if set.contains(key) => join.read += 1,
                Some(_) => join.refuse(&format!("key {key:#010x} no label of its template"), &label),
            }
        }
    }
    // and against their own template's binding records' keys
    let join = tallies
        .entry("Material texture key vs its template's binding keys")
        .or_default();
    for (template, keys) in textured {
        for key in keys {
            match template.and_then(|t| binding_keys.get(&t)) {
                None => join.outcome("template not read in this forge"),
                Some(set) if set.contains(&key) => join.read += 1,
                Some(_) => join.refuse(&format!("key {key:#010x} no binding of its template"), &label),
            }
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
    let oracle = oracle();
    let mut rras = Tally::default();
    let mut levels = Levels::default();
    for path in &forges {
        eprintln!("census: {}", path.display());
        census_forge(path, &mut tallies, oracle.as_ref(), &mut rras, &mut levels);
    }
    levels.measure(&mut tallies);
    if oracle.is_some() {
        tallies.insert("Mesh vs RRAssetStudio", rras);
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
