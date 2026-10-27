//! A compiled level read as what draws where: every placed MeshSet of a Patch Studio compiled (after every
//! post-compile step), each section with its material's program row, for the layering gate
//! ([`super::layers`]).
//!
//! - **Meshes.** Every MeshSet resource of a bundle the Patch places, every LOD and section: the material key, the
//!   stored positions (the position element as `Float3`, or `Half4` read as the half floats' values with `w` not
//!   used) and the triangle list (the LOD's 16- or 32-bit indices, counted from the section's first vertex).
//! - **Placements.** The level's world, walked from its roots: the world documents (first instance a `SubWorldData`
//!   or `LevelData` kind) of the Patch's own TOCs (those the game does not have) that no other world document of them
//!   references. Every instance of a document with a `Blueprint` import and a `BlueprintTransform` (the
//!   `ReferenceObjectData` kinds: objects, layers, sub-worlds, prefabs) places its blueprint there: a point `p` of the
//!   blueprint lies at `p.x right + p.y up + p.z forward + trans` (Frostbite's row-vector `LinearTransform`), composed
//!   down the walk. A blueprint whose `StaticModelEntityData` imports a mesh asset (a `RigidMeshAsset`, named as its
//!   MeshSet) places that mesh; any other blueprint the Patch holds is walked in turn. The asset's `LodGroup` import
//!   is a `MeshLodGroup`: `Lod1Distance` to `Lod6Distance` metres, each times the asset's `LodScale`, are where LODs 1
//!   to 6 take over. A group the Patch does not hold states no distances here ([`Mesh::lod_distances`] is `None`).
//!   A prefab's own placements are in its frame, so only the walk places them.
//! - **Rows.** A material key's program indices: its row in the Patch's program lookup (TOC [`ROOT_TOC`]), else in the
//!   game's.
//!
//! What is not resolved is counted, never guessed: placements whose blueprint, mesh asset or MeshSet the Patch does
//! not hold ([`Level::unresolved`]), and sections whose position element is neither layout ([`Level::unread`]).
//! Positions stay in the level's own frame (Frostbite's, metres): the gate measures distances, which any frame keeps.

use std::collections::{BTreeMap, BTreeSet};

use half::f16;

use crate::postcompile::fb::folder::Source;
use crate::postcompile::fb::{MESHSET, ebx, lookup};
use crate::postcompile::meshset::{self, format, usage};
use crate::postcompile::vblend::{lod_geometry, patch_bundles};
use crate::postcompile::{Result, bad};

const W: &str = "compiled level";

/// The TOC of the program lookup, relative to a data folder.
pub const ROOT_TOC: &str = "Win32/levels/game/dingolevel_root/dingolevel_root.toc";

/// One section of one LOD of a MeshSet: its material key and triangles, positions in the MeshSet's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub key: u64,
    /// Whether the positions were stored as half floats (`Half4`), else `Float3`.
    pub half: bool,
    /// Whether the section stores positions alone (a depth or shadow copy of a drawn section).
    pub position_only: bool,
    pub positions: Vec<[f32; 3]>,
    /// Triangles as indices into `positions`.
    pub triangles: Vec<[u32; 3]>,
}

/// A MeshSet: per LOD, its sections; and its LOD distances when its group states them.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub name: String,
    pub lods: Vec<Vec<Section>>,
    /// Where LODs 1, 2, ... take over, metres (already times the asset's `LodScale`); `None` when the mesh's group is
    /// not one the Patch holds.
    pub lod_distances: Option<Vec<f64>>,
    /// The LOD group asset's name, when the mesh asset names one.
    pub lod_group: Option<String>,
    /// Whether the game has a MeshSet of this name (a stock mesh the Patch carries or places).
    pub stock: bool,
}

/// A placement of a mesh: rows `[right, up, forward]` and `trans`.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// Index into [`Level::meshes`].
    pub mesh: usize,
    pub rows: [[f64; 3]; 3],
    pub trans: [f64; 3],
    /// The EBX document that places it.
    pub document: String,
}

impl Placement {
    /// `p` (the mesh's frame) in the level's frame, f64.
    pub fn apply(&self, p: [f32; 3]) -> [f64; 3] {
        let p = p.map(f64::from);
        std::array::from_fn(|a| {
            p[0] * self.rows[0][a] + p[1] * self.rows[1][a] + p[2] * self.rows[2][a] + self.trans[a]
        })
    }
}

/// What the Patch draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Level {
    pub meshes: Vec<Mesh>,
    pub placements: Vec<Placement>,
    /// Program indices of each material key a section uses (the Patch's row, else the game's); a key in neither is
    /// absent.
    pub rows: BTreeMap<u64, Vec<u32>>,
    /// Placements not resolved to a MeshSet, by what was missing.
    pub unresolved: BTreeMap<String, u64>,
    /// Sections not read, by why.
    pub unread: BTreeMap<String, u64>,
}

/// The import an EBX pointer field names: its file GUID.
fn import(doc: &ebx::Document, v: Option<&ebx::Value>) -> Option<[u8; 16]> {
    match v {
        Some(ebx::Value::Pointer(ebx::Pointer::External(i))) => doc.imports.get(*i).map(|r| r.file_guid),
        _ => None,
    }
}

fn number(v: Option<&ebx::Value>) -> Option<f64> {
    match v {
        Some(ebx::Value::Float(f)) => Some(*f),
        Some(ebx::Value::Int(i)) => Some(*i as f64),
        Some(ebx::Value::UInt(u)) => Some(*u as f64),
        _ => None,
    }
}

fn vec3(v: Option<&ebx::Value>) -> Option<[f64; 3]> {
    let Some(ebx::Value::Object(o)) = v else {
        return None;
    };
    Some([
        number(o.field("x"))?,
        number(o.field("y"))?,
        number(o.field("z"))?,
    ])
}

/// A `LinearTransform`: rows `[right, up, forward]` and `trans`.
type Affine = ([[f64; 3]; 3], [f64; 3]);

const IDENTITY: Affine = ([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [0.0; 3]);

/// `child` placed in `parent`'s frame: a point goes through `child`, then `parent` (row vectors).
fn compose(child: &Affine, parent: &Affine) -> Affine {
    let (rc, tc) = child;
    let (rp, tp) = parent;
    let mul = |v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|a| v[0] * rp[0][a] + v[1] * rp[1][a] + v[2] * rp[2][a])
    };
    let t = mul(*tc);
    (rc.map(mul), std::array::from_fn(|a| t[a] + tp[a]))
}

/// What one EBX document of the Patch declares.
#[derive(Default)]
struct Doc {
    name: String,
    /// The type of its first instance.
    kind: String,
    /// Whether it is in a TOC the game does not have.
    own: bool,
    /// The mesh asset its `StaticModelEntityData` imports.
    mesh: Option<[u8; 16]>,
    /// What it places: (blueprint GUID, transform).
    references: Vec<([u8; 16], Affine)>,
}

impl Doc {
    /// Whether it is a world (a sub-world or a level).
    fn world(&self) -> bool {
        self.kind.ends_with("SubWorldData") || self.kind == "LevelData"
    }
}

/// What the Patch's EBX documents declare.
#[derive(Default)]
struct Found {
    docs: BTreeMap<[u8; 16], Doc>,
    /// mesh asset document GUID -> (its LOD group document GUID, LodScale)
    mesh_assets: BTreeMap<[u8; 16], (Option<[u8; 16]>, f64)>,
    /// LOD group document GUID -> Lod1Distance..Lod6Distance
    lod_groups: BTreeMap<[u8; 16], Vec<f64>>,
}

impl Found {
    fn add(&mut self, name: &str, doc: &ebx::Document, own: bool) {
        let mut d = Doc {
            name: name.to_string(),
            own,
            ..Doc::default()
        };
        for (k, inst) in doc.instances.iter().enumerate() {
            let (Some(o), Some(t)) = (&inst.object, inst.descriptor.and_then(|d| doc.types.get(d))) else {
                continue;
            };
            if k == 0 {
                d.kind = t.name.clone();
            }
            match t.name.as_str() {
                "StaticModelEntityData" => {
                    if let Some(m) = import(doc, o.field("Mesh")) {
                        d.mesh.get_or_insert(m);
                    }
                }
                "RigidMeshAsset" => {
                    let scale = number(o.field("LodScale")).unwrap_or(1.0);
                    self.mesh_assets
                        .insert(doc.file_guid, (import(doc, o.field("LodGroup")), scale));
                }
                "MeshLodGroup" => {
                    let l: Option<Vec<f64>> = (1..=6)
                        .map(|k| number(o.field(&format!("Lod{k}Distance"))))
                        .collect();
                    if let Some(l) = l {
                        self.lod_groups.insert(doc.file_guid, l);
                    }
                }
                _ => {
                    let Some(bp) = import(doc, o.field("Blueprint")) else {
                        continue;
                    };
                    let Some(ebx::Value::Object(x)) = o.field("BlueprintTransform") else {
                        continue;
                    };
                    let rows = [
                        vec3(x.field("right")),
                        vec3(x.field("up")),
                        vec3(x.field("forward")),
                    ];
                    let (Some(r), Some(u), Some(f), Some(tr)) =
                        (rows[0], rows[1], rows[2], vec3(x.field("trans")))
                    else {
                        continue;
                    };
                    d.references.push((bp, ([r, u, f], tr)));
                }
            }
        }
        self.docs.insert(doc.file_guid, d);
    }
}

/// How deep the walk goes before it counts a blueprint as unresolved (a cycle, or a nesting no level has).
const DEPTH: usize = 32;

/// The positions of a section as stored: its position element, `Float3` or `Half4`; `None` for another layout.
fn positions(sec: &meshset::Section, geo: &[u8]) -> Option<(bool, Vec<[f32; 3]>)> {
    let e = sec.decl.element(usage::POSITION)?;
    let j = usize::from(e.stream);
    let stride = usize::from(*sec.decl.strides.get(j)?);
    let half = match e.format {
        format::FLOAT3 => false,
        format::HALF4 => true,
        _ => return None,
    };
    let width = if half { 8 } else { 12 };
    let start = sec.stream_offset(j)? + usize::from(e.offset);
    let n = sec.vertices as usize;
    if n > 0 && (stride < usize::from(e.offset) + width || start + (n - 1) * stride + width > geo.len()) {
        return None;
    }
    let out = (0..n)
        .map(|v| {
            let c = &geo[start + v * stride..start + v * stride + width];
            if half {
                std::array::from_fn(|k| f16::from_le_bytes([c[2 * k], c[2 * k + 1]]).to_f32())
            } else {
                std::array::from_fn(|k| {
                    f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]])
                })
            }
        })
        .collect();
    Some((half, out))
}

/// A section's triangles: its indices, counted from its first vertex.
fn triangles(sec: &meshset::Section, lod: &meshset::Lod, geo: &[u8], name: &str) -> Result<Vec<[u32; 3]>> {
    let size = usize::from(lod.index_size);
    if size != 2 && size != 4 {
        return Err(bad(W, lod.at, format!("{name}: {size}-byte indices")));
    }
    let start = lod.vertex_bytes as usize + sec.start_index as usize * size;
    let count = sec.primitives as usize * 3;
    let span = primase_foundations::bytes::checked_span(start, count, size, geo.len())
        .map_err(|e| bad(W, sec.at, format!("{name}: section indices: {e}")))?;
    let idx: Vec<u32> = geo[span]
        .chunks_exact(size)
        .map(|c| {
            if size == 2 {
                u32::from(u16::from_le_bytes([c[0], c[1]]))
            } else {
                u32::from_le_bytes([c[0], c[1], c[2], c[3]])
            }
        })
        .collect();
    if let Some(&i) = idx.iter().find(|&&i| i >= sec.vertices) {
        return Err(bad(
            W,
            sec.at,
            format!(
                "{name}: index {i} is past the section's {} vertices",
                sec.vertices
            ),
        ));
    }
    Ok(idx.as_chunks::<3>().0.to_vec())
}

impl Level {
    /// Reads the Patch of `src`; a material key the Patch's program lookup does not carry takes its row from
    /// `game_rows` (the game's lookup), and a mesh is stock when `stock` (the game's MeshSet names, lower case) holds
    /// its name ([`super::layers::Stock`]).
    pub fn read(
        src: &Source,
        game_rows: &BTreeMap<u64, Vec<u32>>,
        stock: &BTreeSet<String>,
    ) -> Result<Level> {
        let mut level = Level::default();
        let mut found = Found::default();
        let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
        let mut rows: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
        for (rel, _, bundles) in patch_bundles(src)? {
            let own = !src
                .base
                .is_some_and(|g| g.tocs.iter().any(|t| t.eq_ignore_ascii_case(&rel)));
            for b in bundles.iter().flatten() {
                for (i, e) in b.manifest.ebx.iter().enumerate() {
                    let bytes = src.payload(b.files[1 + i], e.size as usize)?;
                    found.add(&e.name, &ebx::read(&bytes)?, own);
                }
                for (i, a) in b.manifest.res.iter().enumerate() {
                    let file = b.files[b.manifest.res_file(i)];
                    if a.rtype == lookup::PROGRAM_LOOKUP && rel.eq_ignore_ascii_case(ROOT_TOC) {
                        let p = lookup::read_programs(&src.payload(file, a.size as usize)?, &a.rmeta)?;
                        rows.extend(p.rows);
                        continue;
                    }
                    if a.rtype != MESHSET {
                        continue;
                    }
                    let d = src.payload(file, a.size as usize)?;
                    let ms = meshset::read(&d)?;
                    // a stock MeshSet a Patch bundle carries keeps its geometry in the game's chunks
                    if !ms
                        .lods
                        .iter()
                        .all(|l| b.manifest.chunks.iter().any(|c| c.guid == l.chunk))
                    {
                        *level
                            .unread
                            .entry("MeshSet whose geometry its bundle does not hold".into())
                            .or_default() += 1;
                        continue;
                    }
                    let mut lods = Vec::with_capacity(ms.lods.len());
                    for (li, lod) in ms.lods.iter().enumerate() {
                        let (_, geo) = lod_geometry(src, b, &a.name, li, lod)?;
                        let mut sections = Vec::with_capacity(lod.sections.len());
                        for sec in &lod.sections {
                            let Some((half, pos)) = positions(sec, &geo) else {
                                *level
                                    .unread
                                    .entry("position element neither Float3 nor Half4".into())
                                    .or_default() += 1;
                                continue;
                            };
                            sections.push(Section {
                                key: sec.key,
                                half,
                                position_only: sec.decl.elements.len() == 1,
                                positions: pos,
                                triangles: triangles(sec, lod, &geo, &a.name)?,
                            });
                        }
                        lods.push(sections);
                    }
                    by_name.insert(a.name.to_ascii_lowercase(), level.meshes.len());
                    level.meshes.push(Mesh {
                        name: a.name.clone(),
                        lods,
                        lod_distances: None,
                        lod_group: None,
                        stock: stock.contains(&a.name.to_ascii_lowercase()),
                    });
                }
            }
        }
        // LOD distances from the mesh assets the Patch holds
        for (guid, (group, scale)) in &found.mesh_assets {
            let Some(&m) = found
                .docs
                .get(guid)
                .and_then(|d| by_name.get(&d.name.to_ascii_lowercase()))
            else {
                continue;
            };
            let mesh = &mut level.meshes[m];
            mesh.lod_group = group.and_then(|g| found.docs.get(&g).map(|d| d.name.clone()));
            mesh.lod_distances = group
                .and_then(|g| found.lod_groups.get(&g))
                .map(|d| d.iter().map(|x| x * scale).collect());
        }
        // the world, walked from its roots
        let referenced: BTreeSet<[u8; 16]> = found
            .docs
            .values()
            .filter(|d| d.own && d.world())
            .flat_map(|d| d.references.iter().map(|r| r.0))
            .collect();
        let mut stack: Vec<([u8; 16], Affine, usize)> = found
            .docs
            .iter()
            .filter(|(g, d)| d.own && d.world() && !referenced.contains(*g))
            .map(|(g, _)| (*g, IDENTITY, 0))
            .collect();
        while let Some((guid, at, depth)) = stack.pop() {
            let doc = &found.docs[&guid];
            for (bp, t) in &doc.references {
                let here = compose(t, &at);
                let why = match found.docs.get(bp) {
                    None => Some("blueprint not in the Patch"),
                    Some(target) => match target.mesh {
                        Some(asset) => {
                            match found
                                .docs
                                .get(&asset)
                                .and_then(|a| by_name.get(&a.name.to_ascii_lowercase()))
                            {
                                Some(&mesh) => {
                                    level.placements.push(Placement {
                                        mesh,
                                        rows: here.0,
                                        trans: here.1,
                                        document: doc.name.clone(),
                                    });
                                    None
                                }
                                None => Some("mesh asset without a MeshSet in the Patch"),
                            }
                        }
                        None if target.references.is_empty() => Some("blueprint that places nothing"),
                        None if depth + 1 >= DEPTH => Some("blueprint nested past the walk's depth"),
                        None => {
                            stack.push((*bp, here, depth + 1));
                            None
                        }
                    },
                };
                if let Some(why) = why {
                    *level.unresolved.entry(why.into()).or_default() += 1;
                }
            }
        }
        // a stable order whatever the walk's
        level.placements.sort_by(|a, b| {
            (
                a.document.as_str(),
                a.mesh,
                a.trans.map(f64::to_bits),
                a.rows.map(|r| r.map(f64::to_bits)),
            )
                .cmp(&(
                    b.document.as_str(),
                    b.mesh,
                    b.trans.map(f64::to_bits),
                    b.rows.map(|r| r.map(f64::to_bits)),
                ))
        });
        level.rows = level
            .meshes
            .iter()
            .flat_map(|m| m.lods.iter().flatten().map(|s| s.key))
            .filter_map(|k| rows.get(&k).or_else(|| game_rows.get(&k)).map(|r| (k, r.clone())))
            .collect();
        Ok(level)
    }
}
