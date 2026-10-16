//! Per-vertex blending after Studio's compile: retarget chosen material keys of a Studio Patch onto stock skate.
//! material rows and rewrite their sections to the stock vertex layout with per-vertex values.
//!
//! What the stock rows do with the values was measured in game (three controlled runs of test floors):
//! - the decal row (stock key [`DECAL_DONOR`], programs 1727, 2718): `Color0.r` is the decal's per-vertex opacity,
//!   with a mask-shaped dissolve; green, blue and alpha have no effect; a constant `r` is plain opacity. Exactly
//!   (0, 0, 0, 0) is a "no vertex colour" value that draws fully opaque, so opacity is written as
//!   [`decal_opacity`] and a decal section never receives [`NO_VERTEX_COLOUR`];
//! - the two-texture-set world-object row (stock key [`TWOSET_DONOR`], programs 413, 2332, 2608, 3000): the
//!   sub-material index (usage 51, `.x`) picks the texture set per triangle (1: the second set, 0: the first), not
//!   interpolated; `Color0` does not blend between the sets.
//!
//! Both rows draw the stock declaration `[Pos:Half4, Color0:UByte4N, TexCoord0:Half2, SubMaterial:UByte4,
//! TangentSpace:UByte4N]`, one stream each. Studio's `[Pos:Float3, TexCoord0:Half2, TangentSpace:UByte4N]` sections
//! of a retargeted key are rewritten into it: positions to half floats (w = 1), texture coordinates and tangent
//! frames copied byte for byte, `Color0` and the sub-material index from the caller. Everything else of the MeshSet is
//! kept; vertex offsets, the stride byte, the declarations and the LOD's vertex-buffer size follow the new layout.
//! Material keys are kept, so Studio's per-key section fields stay consistent. A retargeted key's program row and
//! streamable-texture-set row become the donor's (the texture-set value is a function of the program row in the
//! stock table: 2016 rows, no exception).
//!
//! The stock data (declaration bytes, parameter block, rows) is read from the installed game at run time
//! ([`Donor::find`]); nothing derived from it is part of this crate.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use primase_compress::Compressor;
use primase_compress::cas::CasRaw;
use primase_compress::oodle::Oodle;
use sha1::{Digest, Sha1};

use crate::postcompile::fb::bundle::{self, FileRef, Manifest};
use crate::postcompile::fb::folder::{Folder, Source};
use crate::postcompile::fb::toc::{self, Loc};
use crate::postcompile::fb::{MESHSET, depot, lookup};
use crate::postcompile::meshset::{self, DECL_AT, DECL_SIZE, DECL2_AT, Element, format, usage};
use crate::postcompile::{Error, Result, bad};

const W: &str = "vertex blend";

/// The stock decal material whose program and texture-set rows decal sections take: hvac decals of San
/// Vansterdam (336 sections use its row).
pub const DECAL_DONOR: u64 = 0xca7a_fe63_5959_dd1e;
/// The stock two-texture-set world-object material: stone planters of San Vansterdam (its row is used by 1934
/// sections, the most common two-set row).
pub const TWOSET_DONOR: u64 = 0x872d_ecc2_be71_1a7f;

/// `Color0` = (0, 0, 0, 0) draws a decal fully opaque (measured): never written to a decal section.
pub const NO_VERTEX_COLOUR: [u8; 4] = [0; 4];

/// The `Color0` that gives a decal opacity `w` (0..1, clamped): (w, 0, 0, 1). Alpha is 1 so that `w = 0` is
/// transparent rather than [`NO_VERTEX_COLOUR`].
pub fn decal_opacity(w: f32) -> [u8; 4] {
    let r = (w.clamp(0.0, 1.0) * 255.0).round() as u8;
    [r, 0, 0, 255]
}

/// The stored layout Studio writes for the surfaces a retarget takes, and the stock layout it writes.
fn studio_layout() -> [Element; 3] {
    [
        Element {
            usage: usage::POSITION,
            format: format::FLOAT3,
            offset: 0,
            stream: 0,
        },
        Element {
            usage: usage::TEXCOORD0,
            format: format::HALF2,
            offset: 0,
            stream: 1,
        },
        Element {
            usage: usage::TANGENT_SPACE,
            format: format::UBYTE4N,
            offset: 0,
            stream: 2,
        },
    ]
}
fn stock_layout() -> [Element; 5] {
    [
        Element {
            usage: usage::POSITION,
            format: format::HALF4,
            offset: 0,
            stream: 0,
        },
        Element {
            usage: usage::COLOR0,
            format: format::UBYTE4N,
            offset: 0,
            stream: 1,
        },
        Element {
            usage: usage::TEXCOORD0,
            format: format::HALF2,
            offset: 0,
            stream: 2,
        },
        Element {
            usage: usage::SUBMATERIAL,
            format: format::UBYTE4,
            offset: 0,
            stream: 3,
        },
        Element {
            usage: usage::TANGENT_SPACE,
            format: format::UBYTE4N,
            offset: 0,
            stream: 4,
        },
    ]
}
const STOCK_STRIDES: [u8; 5] = [8, 4, 4, 4, 4];

/// What a stock material contributes to a retarget, as read from the installed game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Donor {
    pub key: u64,
    pub program_row: Vec<u32>,
    pub texture_set: u64,
    /// Its parameter block in its depot.
    pub block: Vec<u8>,
    /// The declaration of one of its sections (stored and draw are equal).
    pub decl: [u8; DECL_SIZE],
    /// The section flags byte (+0x1b).
    pub flags: u8,
}

impl Donor {
    /// Reads stock material `key` from the game data: a section using it and the parameter block of the same
    /// bundle in TOC `level_toc`, its rows in TOC `root_toc` (both relative to the data folder).
    pub fn find(game: &Source, level_toc: &str, root_toc: &str, key: u64) -> Result<Donor> {
        let (mut program_row, mut texture_set) = (None, None);
        let t = toc::read(&read_file(&game.folder.toc_path(root_toc))?)?;
        for (_, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = game.manifest(&files, &inline)? else {
                continue;
            };
            for (i, a) in m.res.iter().enumerate() {
                let f = files[first + m.ebx.len() + i];
                if a.rtype == lookup::PROGRAM_LOOKUP {
                    let p = lookup::read_programs(&game.payload(f, a.size as usize)?, &a.rmeta)?;
                    program_row = program_row.or_else(|| p.rows.get(&key).cloned());
                } else if a.rtype == lookup::TEXTURE_LOOKUP {
                    let rows = lookup::read_texture_sets(&game.payload(f, a.size as usize)?)?;
                    texture_set = texture_set.or_else(|| rows.iter().find(|r| r.0 == key).map(|r| r.1));
                }
            }
        }
        let missing = |what: &str| Error::Missing(format!("stock key {key:016x}: no {what}"));
        let program_row = program_row.ok_or_else(|| missing("program row"))?;
        let texture_set = texture_set.ok_or_else(|| missing("texture-set row"))?;
        let t = toc::read(&read_file(&game.folder.toc_path(level_toc))?)?;
        for (_, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = game.manifest(&files, &inline)? else {
                continue;
            };
            let mut section = None;
            for (i, a) in m.res.iter().enumerate().filter(|(_, a)| a.rtype == MESHSET) {
                let d = game.payload(files[first + m.ebx.len() + i], a.size as usize)?;
                let ms = meshset::read(&d)?;
                if let Some(s) = ms
                    .lods
                    .iter()
                    .flat_map(|l| &l.sections)
                    .find(|s| s.key == key && s.decl == s.draw_decl)
                {
                    section = Some((s.decl.bytes, d[s.at + 0x1b]));
                    break;
                }
            }
            let Some((decl, flags)) = section else { continue };
            for (i, a) in m.res.iter().enumerate().filter(|(_, a)| a.rtype == depot::RTYPE) {
                let dep = depot::read(
                    &game.payload(files[first + m.ebx.len() + i], a.size as usize)?,
                    &a.rmeta,
                )?;
                if let Some(&(_, bi)) = dep.entries.iter().find(|e| e.0 == key) {
                    let block = dep.blocks[bi].data.clone();
                    return Ok(Donor {
                        key,
                        program_row,
                        texture_set,
                        block,
                        decl,
                        flags,
                    });
                }
            }
        }
        Err(missing("section with a parameter block in the same bundle"))
    }

    fn check_layout(&self) -> Result<()> {
        let d = meshset::Decl::read(&self.decl)?;
        if d.elements != stock_layout() || d.strides != STOCK_STRIDES {
            return Err(bad(
                W,
                0,
                format!(
                    "stock key {:016x} has layout {:?}, not the stock blend layout",
                    self.key, d.elements
                ),
            ));
        }
        Ok(())
    }
}

/// What a Studio material key becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The decal row: per-vertex opacity in `Color0.r`.
    Decal,
    /// The two-set row; the second texture set (colour, mask, normal) is taken from the Studio material `second`
    /// (compiled into the level by an object that uses it).
    TwoSet { second: u64 },
}

#[derive(Clone, Debug)]
pub struct Request {
    /// A Studio material key of the Patch.
    pub key: u64,
    pub target: Target,
    pub donor: Donor,
}

/// One section to fill, as Studio compiled it (positions in the MeshSet's space).
#[derive(Debug)]
pub struct SectionInput<'a> {
    pub mesh: &'a str,
    pub lod: usize,
    pub section: usize,
    pub key: u64,
    pub positions: &'a [[f32; 3]],
    pub uv0: &'a [[f32; 2]],
}

/// Per-vertex values for a section: `Color0` and the sub-material index (usage 51).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionValues {
    pub color0: Vec<[u8; 4]>,
    pub submaterial: Vec<[u8; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub sections: usize,
    pub meshsets: usize,
    /// Largest |position - half-float position| over all rewritten vertices, in metres.
    pub max_position_error: f32,
    /// Per retargeted key: (old program row, new program row).
    pub rows: BTreeMap<u64, (Vec<u32>, Vec<u32>)>,
}

fn read_file(p: &Path) -> Result<Vec<u8>> {
    fs::read(p).map_err(|source| Error::Io {
        path: p.to_path_buf(),
        source,
    })
}

fn write_file(p: &Path, d: &[u8]) -> Result<()> {
    fs::write(p, d).map_err(|source| Error::Io {
        path: p.to_path_buf(),
        source,
    })
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    let io = |p: &Path| {
        let p = p.to_path_buf();
        move |source| Error::Io { path: p, source }
    };
    fs::create_dir_all(to).map_err(io(to))?;
    let mut entries: Vec<_> = fs::read_dir(from)
        .map_err(io(from))?
        .collect::<Result<_, _>>()
        .map_err(io(from))?;
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(io(&src))?;
        }
    }
    Ok(())
}

/// New payloads go to one new archive per install directory, appended with 16-byte alignment.
struct Archives<'a> {
    folder: &'a Folder,
    open: BTreeMap<u32, (u16, PathBuf, Vec<u8>)>,
}

impl Archives<'_> {
    fn put(&mut self, near: Loc, data: &[u8]) -> Result<FileRef> {
        if !self.open.contains_key(&near.chunk) {
            let (num, path) = self.folder.new_archive(near)?;
            self.open.insert(near.chunk, (num, path, Vec::new()));
        }
        let (num, _, buf) = self
            .open
            .get_mut(&near.chunk)
            .ok_or_else(|| bad(W, 0, "archive"))?;
        buf.resize(buf.len().div_ceil(16) * 16, 0);
        let at = u32::try_from(buf.len()).map_err(|_| bad(W, 0, "new archive exceeds 4 GiB"))?;
        buf.extend_from_slice(data);
        Ok((
            Loc {
                patch: true,
                chunk: near.chunk,
                archive: *num,
            },
            at,
            data.len() as u32,
        ))
    }

    fn finish(self) -> Result<()> {
        for (_, (_, path, buf)) in self.open {
            write_file(&path, &buf)?;
        }
        Ok(())
    }
}

fn encode(data: &[u8]) -> Result<Vec<u8>> {
    Ok(CasRaw.compress(data)?)
}

fn sha1(data: &[u8]) -> [u8; 20] {
    Sha1::digest(data).into()
}

/// Refuses values a section cannot take: wrong length, or [`NO_VERTEX_COLOUR`] on a decal.
pub fn check_values(target: &Target, vertices: usize, v: &SectionValues) -> Result<()> {
    if v.color0.len() != vertices || v.submaterial.len() != vertices {
        return Err(bad(
            W,
            0,
            format!(
                "{} / {} values for {vertices} vertices",
                v.color0.len(),
                v.submaterial.len()
            ),
        ));
    }
    if *target == Target::Decal
        && let Some(i) = v.color0.iter().position(|&c| c == NO_VERTEX_COLOUR)
    {
        return Err(bad(
            W,
            i,
            "Color0 (0, 0, 0, 0) on a decal draws it fully opaque; write decal_opacity(0.0)",
        ));
    }
    Ok(())
}

fn texture_refs(block: &[u8]) -> Result<BTreeMap<u16, Vec<u8>>> {
    Ok(depot::params(block)?
        .iter()
        .filter_map(|p| p.texture_ref(block).map(|r| (p.slot(), r.to_vec())))
        .collect())
}

/// The donor's two-set block with the six texture-set slots swapped: first set (colour 0x79a3, mask 0x9695, normal
/// 0xaf17) from `first`, second set (colour 0xba61, mask 0x0fd7, normal 0x0795) from `second`'s first-set slots.
/// The other references (grime, texel detail, neutral) stay the donor's.
pub fn twoset_block(donor: &[u8], first: &[u8], second: &[u8]) -> Result<Vec<u8>> {
    let (a, b) = (texture_refs(first)?, texture_refs(second)?);
    let get = |m: &BTreeMap<u16, Vec<u8>>, s: u16| {
        m.get(&s)
            .cloned()
            .ok_or_else(|| bad(W, 0, format!("block has no texture slot {s:04x}")))
    };
    let swaps = [
        (0x79a3, get(&a, 0x79a3)?),
        (0x9695, get(&a, 0x9695)?),
        (0xaf17, get(&a, 0xaf17)?),
        (0xba61, get(&b, 0x79a3)?),
        (0x0fd7, get(&b, 0x9695)?),
        (0x0795, get(&b, 0xaf17)?),
    ];
    let mut out = donor.to_vec();
    let params = depot::params(donor)?;
    for (slot, r) in swaps {
        let p = params
            .iter()
            .find(|p| p.ty == depot::TEXTURE && p.slot() == slot)
            .ok_or_else(|| bad(W, 0, format!("stock block has no slot {slot:04x}")))?;
        out[p.at + 16..p.at + 49].copy_from_slice(&r);
    }
    Ok(out)
}

/// Parameter (name, type, count) of a block, sorted: two blocks of one template have equal signatures.
fn signature(block: &[u8]) -> Result<Vec<(u64, u32, u16)>> {
    let mut s: Vec<_> = depot::params(block)?
        .iter()
        .map(|p| (p.name, p.ty, p.count))
        .collect();
    s.sort_unstable();
    Ok(s)
}

struct Bundle {
    files: Vec<FileRef>,
    manifest: Manifest,
    framed_manifest: bool,
    changed: bool,
}

/// Rewrites Patch `patch` into `out` (which must not exist) with every request applied. `values` gives each
/// rewritten section's per-vertex values; it is called once per section, in TOC, bundle, MeshSet, LOD and section
/// order.
pub fn apply(
    patch: &Path,
    out: &Path,
    base: Option<&Folder>,
    oodle: Option<&Oodle>,
    requests: &[Request],
    values: &mut dyn FnMut(&SectionInput) -> Result<SectionValues>,
) -> Result<Report> {
    if out.exists() {
        return Err(Error::Missing(format!(
            "{} exists; give a new folder",
            out.display()
        )));
    }
    let wanted: BTreeMap<u64, &Request> = requests.iter().map(|r| (r.key, r)).collect();
    if wanted.len() != requests.len() {
        return Err(bad(W, 0, "a key is requested twice"));
    }
    for r in requests {
        r.donor.check_layout()?;
    }
    copy_dir(patch, out)?;
    let folder = Folder::open(out)?;
    let src = Source {
        folder: &folder,
        base,
        oodle,
    };
    let mut report = Report::default();
    // ---- every patch-placed bundle of every TOC
    let mut tocs: Vec<(String, toc::Toc, Vec<Option<Bundle>>)> = Vec::new();
    for rel in &folder.tocs {
        let t = toc::read(&read_file(&folder.toc_path(rel))?)?;
        let mut bundles = Vec::with_capacity(t.bundles.len());
        for (_, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let entry = match src.manifest(&files, &inline)? {
                Some((manifest, 1, _)) if files[0].0.patch => {
                    let framed_manifest = !bundle::is_manifest(&src.raw(files[0])?);
                    Some(Bundle {
                        files,
                        manifest,
                        framed_manifest,
                        changed: false,
                    })
                }
                _ => None,
            };
            bundles.push(entry);
        }
        tocs.push((rel.clone(), t, bundles));
    }
    // ---- blocks of the requested keys and the two-set second-set carriers
    let mut blocks: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    let mut need: BTreeSet<u64> = wanted.keys().copied().collect();
    for r in requests {
        if let Target::TwoSet { second } = r.target {
            need.insert(second);
        }
    }
    for (_, _, bundles) in &tocs {
        for b in bundles.iter().flatten() {
            for (i, a) in b
                .manifest
                .res
                .iter()
                .enumerate()
                .filter(|(_, a)| a.rtype == depot::RTYPE)
            {
                let dep = depot::read(
                    &src.payload(b.files[b.manifest.res_file(i)], a.size as usize)?,
                    &a.rmeta,
                )?;
                for &(k, bi) in &dep.entries {
                    if need.contains(&k) {
                        let prev = blocks.insert(k, dep.blocks[bi].data.clone());
                        if prev.is_some_and(|p| p != dep.blocks[bi].data) {
                            return Err(bad(W, 0, format!("key {k:016x} has two different blocks")));
                        }
                    }
                }
            }
        }
    }
    let block = |k: u64| {
        blocks
            .get(&k)
            .ok_or_else(|| Error::Missing(format!("key {k:016x} has no parameter block")))
    };
    let mut new_blocks: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    for r in requests {
        match r.target {
            Target::Decal => {
                if signature(block(r.key)?)? != signature(&r.donor.block)? {
                    return Err(bad(
                        W,
                        0,
                        format!("key {:016x}: its block is not the stock decal template", r.key),
                    ));
                }
            }
            Target::TwoSet { second } => {
                new_blocks.insert(
                    r.key,
                    twoset_block(&r.donor.block, block(r.key)?, block(second)?)?,
                );
            }
        }
    }
    let mut archives = Archives {
        folder: &folder,
        open: BTreeMap::new(),
    };
    // ---- depots, MeshSets and lookups
    for (_, _, bundles) in &mut tocs {
        for b in bundles.iter_mut().flatten() {
            let ne = b.manifest.ebx.len();
            for i in 0..b.manifest.res.len() {
                let a = b.manifest.res[i].clone();
                let f = b.files[1 + ne + i];
                let new_payload = match a.rtype {
                    depot::RTYPE if !new_blocks.is_empty() => {
                        let mut dep = depot::read(&src.payload(f, a.size as usize)?, &a.rmeta)?;
                        let mut hit = false;
                        for (&k, nb) in &new_blocks {
                            let Some(pos) = dep.entries.iter().position(|e| e.0 == k) else {
                                continue;
                            };
                            let bi = dep.entries[pos].1;
                            let shared = dep.entries.iter().filter(|e| e.1 == bi).count() > 1;
                            if shared {
                                // the key leaves the shared block for one of its own (hash of the new contents)
                                let h = u64::from_le_bytes(
                                    sha1(nb)[..8].try_into().map_err(|_| bad(W, 0, "hash"))?,
                                );
                                dep.blocks.push(depot::Block {
                                    hash: h,
                                    data: nb.clone(),
                                });
                                dep.entries[pos].1 = dep.blocks.len() - 1;
                            } else {
                                dep.blocks[bi].data = nb.clone();
                            }
                            hit = true;
                        }
                        if hit {
                            let (d, meta) = depot::write(&dep);
                            b.manifest.res[i].rmeta = meta;
                            Some(d)
                        } else {
                            None
                        }
                    }
                    lookup::PROGRAM_LOOKUP => {
                        let mut t = lookup::read_programs(&src.payload(f, a.size as usize)?, &a.rmeta)?;
                        let mut hit = false;
                        for r in requests {
                            if let Some(row) = t.rows.get_mut(&r.key) {
                                report
                                    .rows
                                    .insert(r.key, (row.clone(), r.donor.program_row.clone()));
                                *row = r.donor.program_row.clone();
                                hit = true;
                            }
                        }
                        if hit {
                            let (d, meta) = lookup::write_programs(&t, &a.rmeta);
                            b.manifest.res[i].rmeta = meta;
                            Some(d)
                        } else {
                            None
                        }
                    }
                    lookup::TEXTURE_LOOKUP => {
                        let mut rows = lookup::read_texture_sets(&src.payload(f, a.size as usize)?)?;
                        let mut hit = false;
                        for row in &mut rows {
                            if let Some(r) = wanted.get(&row.0) {
                                row.1 = r.donor.texture_set;
                                hit = true;
                            }
                        }
                        hit.then(|| lookup::write_texture_sets(&rows))
                    }
                    MESHSET => rewrite_meshset(&src, b, i, &wanted, values, &mut archives, &mut report)?,
                    _ => None,
                };
                if let Some(d) = new_payload {
                    let enc = encode(&d)?;
                    b.manifest.res[i].size = d.len() as u32;
                    b.manifest.res[i].sha1 = sha1(&enc);
                    b.files[1 + ne + i] = archives.put(f.0, &enc)?;
                    b.changed = true;
                }
            }
        }
    }
    for k in wanted.keys() {
        if !report.rows.contains_key(k) {
            return Err(Error::Missing(format!(
                "key {k:016x} has no program row in the Patch"
            )));
        }
    }
    // ---- manifests, regions, TOCs
    for (rel, t, bundles) in &mut tocs {
        let mut toc_changed = false;
        for (j, b) in bundles.iter_mut().enumerate() {
            let Some(b) = b.as_mut().filter(|b| b.changed) else {
                continue;
            };
            let m = bundle::write_manifest(&b.manifest)?;
            let stored = if b.framed_manifest { encode(&m)? } else { m };
            b.files[0] = archives.put(b.files[0].0, &stored)?;
            t.bundles[j].1 = bundle::write_region(&b.files, &[]);
            toc_changed = true;
        }
        if toc_changed {
            let bytes = toc::write(&t.bundles, &t.chunks, t.flags)?;
            toc::verify(&toc::read(&bytes)?)?;
            write_file(&folder.toc_path(rel), &bytes)?;
        }
    }
    archives.finish()?;
    Ok(report)
}

fn half_bits(v: f32, what: &'static str) -> Result<[u8; 2]> {
    let h = half::f16::from_f32(v);
    if !h.is_finite() {
        return Err(bad(what, 0, format!("{v} does not fit a half float")));
    }
    Ok(h.to_le_bytes())
}

/// Rewrites resource `i` of bundle `b` when one of its sections uses a requested key: new geometry chunks (one per
/// LOD) put into `archives` and recorded in the manifest, and the new MeshSet payload returned.
fn rewrite_meshset(
    src: &Source,
    b: &mut Bundle,
    i: usize,
    wanted: &BTreeMap<u64, &Request>,
    values: &mut dyn FnMut(&SectionInput) -> Result<SectionValues>,
    archives: &mut Archives,
    report: &mut Report,
) -> Result<Option<Vec<u8>>> {
    let a = b.manifest.res[i].clone();
    let mut d = src.payload(b.files[b.manifest.res_file(i)], a.size as usize)?;
    let ms = meshset::read(&d)?;
    if !ms
        .lods
        .iter()
        .flat_map(|l| &l.sections)
        .any(|s| wanted.contains_key(&s.key))
    {
        return Ok(None);
    }
    for (li, lod) in ms.lods.iter().enumerate() {
        let ci = b
            .manifest
            .chunks
            .iter()
            .position(|c| c.guid == lod.chunk)
            .ok_or_else(|| {
                bad(
                    W,
                    lod.at,
                    format!("{}: LOD {li} chunk is not in its bundle", a.name),
                )
            })?;
        let cf = b.files[b.manifest.chunk_file(ci)];
        let geo = src.payload(cf, b.manifest.chunks[ci].lsize as usize)?;
        let (vsize, isize) = (lod.vertex_bytes as usize, lod.index_bytes as usize);
        if geo.len() < vsize + isize {
            return Err(bad(
                W,
                lod.at,
                format!("{}: chunk of {} bytes for {vsize} + {isize}", a.name, geo.len()),
            ));
        }
        let mut order: Vec<usize> = (0..lod.sections.len()).collect();
        order.sort_by_key(|&s| lod.sections[s].vertex_offset);
        let mut vb: Vec<u8> = Vec::with_capacity(vsize * 2);
        let mut changed = false;
        for s in order {
            let sec = &lod.sections[s];
            let start = u32::try_from(vb.len()).map_err(|_| bad(W, sec.at, "vertex buffer exceeds 4 GiB"))?;
            let n = sec.vertices as usize;
            let streams: Vec<&[u8]> = (0..sec.decl.strides.len())
                .map(|j| {
                    let o = sec.stream_offset(j);
                    &geo[o..o + sec.decl.strides[j] as usize * n]
                })
                .collect();
            match wanted.get(&sec.key) {
                None => {
                    for st in &streams {
                        vb.extend_from_slice(st);
                    }
                }
                Some(r) => {
                    if sec.decl.elements != studio_layout() || sec.decl != sec.draw_decl {
                        return Err(bad(
                            W,
                            sec.at,
                            format!(
                                "{}: section layout {:?} is not Studio's surface layout",
                                a.name, sec.decl.elements
                            ),
                        ));
                    }
                    let positions: Vec<[f32; 3]> = streams[0]
                        .as_chunks::<12>()
                        .0
                        .iter()
                        .map(|c| {
                            let f = |k: usize| f32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
                            [f(0), f(4), f(8)]
                        })
                        .collect();
                    let uv0: Vec<[f32; 2]> = streams[1]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| {
                            [
                                half::f16::from_le_bytes([c[0], c[1]]).to_f32(),
                                half::f16::from_le_bytes([c[2], c[3]]).to_f32(),
                            ]
                        })
                        .collect();
                    let input = SectionInput {
                        mesh: &a.name,
                        lod: li,
                        section: s,
                        key: sec.key,
                        positions: &positions,
                        uv0: &uv0,
                    };
                    let v = values(&input)?;
                    check_values(&r.target, n, &v)?;
                    for p in &positions {
                        for c in p {
                            vb.extend(half_bits(*c, "position")?);
                            let back = half::f16::from_f32(*c).to_f32();
                            report.max_position_error = report.max_position_error.max((back - c).abs());
                        }
                        vb.extend(half::f16::ONE.to_le_bytes());
                    }
                    for c in &v.color0 {
                        vb.extend(c);
                    }
                    vb.extend_from_slice(streams[1]);
                    for u in &v.submaterial {
                        vb.extend(u);
                    }
                    vb.extend_from_slice(streams[2]);
                    let o = sec.at;
                    d[o + DECL_AT..o + DECL_AT + DECL_SIZE].copy_from_slice(&r.donor.decl);
                    d[o + DECL2_AT..o + DECL2_AT + DECL_SIZE].copy_from_slice(&r.donor.decl);
                    d[o + 0x1e] = STOCK_STRIDES.iter().sum();
                    d[o + 0x1b] = r.donor.flags;
                    report.sections += 1;
                    changed = true;
                }
            }
            d[sec.at + 0x28..sec.at + 0x2c].copy_from_slice(&start.to_le_bytes());
            d[sec.at + 0x54..sec.at + 0x58].copy_from_slice(&start.to_le_bytes());
        }
        if !changed {
            continue;
        }
        vb.resize(vb.len().div_ceil(16) * 16, 0);
        let vb_len = u32::try_from(vb.len()).map_err(|_| bad(W, lod.at, "vertex buffer exceeds 4 GiB"))?;
        d[lod.at + 0x5c..lod.at + 0x60].copy_from_slice(&vb_len.to_le_bytes());
        vb.extend_from_slice(&geo[vsize..]);
        let enc = encode(&vb)?;
        let c = &mut b.manifest.chunks[ci];
        if c.loff != 0 {
            return Err(bad(W, lod.at, format!("{}: chunk has a logical offset", a.name)));
        }
        c.lsize = vb.len() as u32;
        c.size = c.lsize;
        c.sha1 = sha1(&enc);
        b.files[b.manifest.chunk_file(ci)] = archives.put(cf.0, &enc)?;
    }
    meshset::read(&d)?;
    report.meshsets += 1;
    Ok(Some(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decal_opacity_is_never_the_opaque_sentinel() {
        for i in 0..=1000 {
            let w = i as f32 / 1000.0;
            assert_ne!(decal_opacity(w), NO_VERTEX_COLOUR, "w = {w}");
        }
        assert_eq!(decal_opacity(0.0), [0, 0, 0, 255]);
        assert_eq!(decal_opacity(1.0), [255, 0, 0, 255]);
        assert_eq!(decal_opacity(-3.0), [0, 0, 0, 255]);
    }

    #[test]
    fn an_all_zero_decal_emit_is_refused() {
        let v = SectionValues {
            color0: vec![decal_opacity(0.5), NO_VERTEX_COLOUR],
            submaterial: vec![[0; 4]; 2],
        };
        assert!(check_values(&Target::Decal, 2, &v).is_err());
        // the same values are fine on a two-set section (no sentinel there) and a decal without the sentinel passes
        assert!(check_values(&Target::TwoSet { second: 1 }, 2, &v).is_ok());
        let v = SectionValues {
            color0: vec![decal_opacity(0.0); 2],
            submaterial: vec![[0; 4]; 2],
        };
        assert!(check_values(&Target::Decal, 2, &v).is_ok());
        assert!(check_values(&Target::Decal, 3, &v).is_err());
    }
}
