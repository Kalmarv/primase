//! Per-vertex blending after Studio's compile: retarget chosen material keys of a Studio Patch onto stock skate.
//! material rows and rewrite their sections to the stock vertex layout with per-vertex values.
//!
//! What the stock rows do with the values was measured in game (three controlled runs of test floors):
//! - the decal row (stock key [`DECAL_DONOR`], programs 1727, 2718): `Color0.r` is the decal's per-vertex opacity,
//!   with a mask-shaped dissolve; green, blue and alpha have no effect; a constant `r` is plain opacity. Exactly
//!   (0, 0, 0, 0) is a "no vertex colour" value that draws fully opaque, so opacity is written as
//!   [`decal_opacity`] and a decal section never receives [`NO_VERTEX_COLOUR`];
//! - the two-texture-set world-object row (stock key 0x872decc2be711a7f, stone planters of San Vansterdam, used by 1934 sections, the most common two-set row, programs 413, 2332, 2608, 3000): the
//!   sub-material index (usage 51, `.x`) picks the texture set per triangle (1: the second set, 0: the first), not
//!   interpolated; `Color0` does not blend between the sets.
//!
//! Both rows draw the stock declaration `[Pos:Half4, Color0:UByte4N, TexCoord0:Half2, SubMaterial:UByte4,
//! TangentSpace:UByte4N]`, one stream each. Studio's `[Pos:Float3, TexCoord0:Half2, TangentSpace:UByte4N]` sections
//! of a retargeted key are rewritten into it: positions to half floats (w = 1; a decal section's never behind its
//! own surface, see [`half_in_front`]), texture coordinates and tangent frames copied byte for byte, `Color0` and the sub-material index from the caller. A `vertex_blend` material's
//! sections have `TexCoord0:Half4` instead, the per-vertex weight in `z` (Studio 20261012 and later; see
//! [`crate::postcompile::vbrecord`]): the weight is handed to the caller ([`SectionInput::blend`]) and `x, y` are
//! written as the stock `Half2`. Everything else of the MeshSet is kept; vertex offsets, the stride byte, the
//! declarations and the LOD's vertex-buffer size follow the new layout.
//! Material keys are kept, so Studio's per-key section fields stay consistent. A retargeted key's program row and
//! streamable-texture-set row become the donor's (the texture-set value is a function of the program row in the
//! stock table: 2016 rows, no exception).
//!
//! The stock data (declaration bytes, parameter block, rows) is read from the installed game at run time
//! ([`Donor::find`]); nothing derived from it is part of this crate. The Patch and the game are read through the
//! Frostbite folder reader ([`Folder`]); this module only writes the output folder.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use half::f16;
use primase_compress::Compressor;
use primase_compress::oodle::Oodle;
use primase_frostbite::cas::CasRaw;
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

/// The decal row's opacity encoding, shared with the passes ([`crate::decal`]).
pub use crate::decal::{NO_VERTEX_COLOUR, decal_opacity};

/// The stored layout Studio writes for the surfaces a retarget takes (`TexCoord0` as `uv`: [`format::HALF2`], or
/// [`format::HALF4`] for a `vertex_blend` material), and the stock layout it writes.
fn studio_layout(uv: u8) -> [Element; 3] {
    [
        Element {
            usage: usage::POSITION,
            format: format::FLOAT3,
            offset: 0,
            stream: 0,
        },
        Element {
            usage: usage::TEXCOORD0,
            format: uv,
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
        let t = game.folder.toc(root_toc)?;
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
        let t = game.folder.toc(level_toc)?;
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

/// The installed game's program records of `dingolevel_root`'s program lookup: what a lookup row's program indices
/// mean. Studio writes its own copy of the records with a private program appended for `vertex_blend` (3,620
/// programs against the game's 3,619); once every row is retargeted onto stock programs the copy is replaced by
/// these, so the Patch carries no program the game has no compiled variants of. A runtime that loads the Patch's
/// table whole (ReSkate 1.1.5, and 2.1.0-pre since it stopped carrying the built-in fpv_drone, so a lone map is no
/// longer combined over the game's table) builds the private program at boot and crashes at `Skate+0x3db8d40`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StockPrograms {
    /// The program and relocation sections (everything before the rows).
    pub head: Vec<u8>,
    /// The resource metadata's program and relocation sizes (its first 8 bytes).
    pub sizes: [u8; 8],
    /// Programs in the table: the program section's first `u32` (3,619 in the game; every stock row's indices are
    /// below it).
    pub count: u32,
}

impl StockPrograms {
    /// Reads the program records of the program lookup in TOC `root_toc` (relative to the data folder).
    pub fn find(game: &Source, root_toc: &str) -> Result<StockPrograms> {
        let t = game.folder.toc(root_toc)?;
        for (_, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = game.manifest(&files, &inline)? else {
                continue;
            };
            for (i, a) in m.res.iter().enumerate() {
                if a.rtype != lookup::PROGRAM_LOOKUP {
                    continue;
                }
                let d = game.payload(files[first + m.ebx.len() + i], a.size as usize)?;
                let p = lookup::read_programs(&d, &a.rmeta)?;
                let count = u32::from_le_bytes(
                    p.head
                        .get(..4)
                        .and_then(|b| b.try_into().ok())
                        .ok_or_else(|| bad(W, 0, "stock program lookup without programs"))?,
                );
                let past = p.rows.values().flatten().filter(|&&v| v >= count).count();
                if past > 0 {
                    return Err(bad(
                        W,
                        0,
                        format!("the stock program lookup has {past} indices past its {count} programs"),
                    ));
                }
                let sizes: [u8; 8] = a
                    .rmeta
                    .get(..8)
                    .and_then(|s| s.try_into().ok())
                    .ok_or_else(|| bad(W, 0, format!("{root_toc}: program lookup metadata too short")))?;
                return Ok(StockPrograms {
                    head: p.head,
                    sizes,
                    count,
                });
            }
        }
        Err(Error::Missing(format!("{root_toc}: no program lookup")))
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
    /// Studio's per-vertex weight (`TexCoord0.z`) of a `vertex_blend` section; `None` for a plain surface.
    pub blend: Option<&'a [f32]>,
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
    /// Decal vertices whose nearest half-float position lay behind their surface and were stepped in front of it.
    pub stepped_in_front: usize,
    /// Per retargeted key: (old program row, new program row).
    pub rows: BTreeMap<u64, (Vec<u32>, Vec<u32>)>,
    /// The program records replaced by the game's: (Studio's program count, the game's).
    pub programs: Option<(u32, u32)>,
}

#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: writes the patched files"
)]
fn write_file(p: &Path, d: &[u8]) -> Result<()> {
    fs::write(p, d).map_err(|source| Error::Io {
        path: p.to_path_buf(),
        source,
    })
}

#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: copies the Patch folder to the output"
)]
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

/// A bundle the Patch places (its manifest in the Patch's own archives), as read.
pub(crate) struct Bundle {
    pub(crate) files: Vec<FileRef>,
    pub(crate) manifest: Manifest,
    framed_manifest: bool,
    changed: bool,
}

/// Every TOC of the Patch with its bundles, `Some` for each bundle the Patch places.
type PatchBundles = Vec<(String, toc::Toc, Vec<Option<Bundle>>)>;

pub(crate) fn patch_bundles(src: &Source) -> Result<PatchBundles> {
    let mut tocs = Vec::new();
    for rel in &src.folder.tocs {
        let t = src.folder.toc(rel)?;
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
    Ok(tocs)
}

/// LOD `li`'s geometry chunk of MeshSet `name` in bundle `b`: (its index in the manifest's chunks, its payload),
/// checked to hold the LOD's vertex and index buffers.
pub(crate) fn lod_geometry(
    src: &Source,
    b: &Bundle,
    name: &str,
    li: usize,
    lod: &meshset::Lod,
) -> Result<(usize, Vec<u8>)> {
    let ci = b
        .manifest
        .chunks
        .iter()
        .position(|c| c.guid == lod.chunk)
        .ok_or_else(|| bad(W, lod.at, format!("{name}: LOD {li} chunk is not in its bundle")))?;
    let geo = src.payload(
        b.files[b.manifest.chunk_file(ci)],
        b.manifest.chunks[ci].lsize as usize,
    )?;
    let (vsize, isize) = (lod.vertex_bytes as usize, lod.index_bytes as usize);
    if geo.len() < vsize + isize {
        return Err(bad(
            W,
            lod.at,
            format!("{name}: chunk of {} bytes for {vsize} + {isize}", geo.len()),
        ));
    }
    Ok((ci, geo))
}

/// A Studio surface section's vertices: positions, texture coordinates and, for a `vertex_blend` section
/// (`TexCoord0:Half4`), the weight in `z`. `None` when the section has neither of Studio's surface layouts.
pub(crate) type Surface = (Vec<[f32; 3]>, Vec<[f32; 2]>, Option<Vec<f32>>);

pub(crate) fn surface(sec: &meshset::Section, geo: &[u8]) -> Option<Surface> {
    let blend = if sec.decl.elements == studio_layout(format::HALF2) {
        false
    } else if sec.decl.elements == studio_layout(format::HALF4) {
        true
    } else {
        return None;
    };
    // one stream each, unpadded: Float3 positions, the texture coordinates, the tangent frame
    if sec.decl != sec.draw_decl || sec.decl.strides != [12, if blend { 8 } else { 4 }, 4] {
        return None;
    }
    let n = sec.vertices as usize;
    let stream = |j: usize| {
        let o = sec.stream_offset(j);
        &geo[o..o + sec.decl.strides[j] as usize * n]
    };
    let h = |c: &[u8], k: usize| half::f16::from_le_bytes([c[k], c[k + 1]]).to_f32();
    let positions = stream(0)
        .as_chunks::<12>()
        .0
        .iter()
        .map(|c| {
            let f = |k: usize| f32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
            [f(0), f(4), f(8)]
        })
        .collect();
    let width = if blend { 8 } else { 4 };
    let uv = stream(1).chunks_exact(width);
    let uv0 = uv.clone().map(|c| [h(c, 0), h(c, 2)]).collect();
    let weights = blend.then(|| uv.map(|c| h(c, 4)).collect());
    Some((positions, uv0, weights))
}

/// Rewrites Patch `patch` into `out` (which must not exist) with every request applied. `values` gives each
/// rewritten section's per-vertex values; it is called once per section, in TOC, bundle, MeshSet, LOD and section
/// order.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: refuses an existing output folder"
)]
pub fn apply(
    patch: &Path,
    out: &Path,
    base: Option<&Folder>,
    oodle: Option<&Oodle>,
    stock: &StockPrograms,
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
    let mut tocs = patch_bundles(&src)?;
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
                            // the game's program records instead of Studio's copy (see [`StockPrograms`]): every
                            // row must index them
                            if let Some((k, v)) = t
                                .rows
                                .iter()
                                .find_map(|(k, r)| r.iter().find(|&&v| v >= stock.count).map(|v| (k, v)))
                            {
                                return Err(bad(
                                    W,
                                    0,
                                    format!(
                                        "key {k:016x}'s row uses program {v}, past the game's {}: a Studio program this \
                                         stage does not replace",
                                        stock.count
                                    ),
                                ));
                            }
                            let studio = t
                                .head
                                .get(..4)
                                .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("four bytes")));
                            report.programs = Some((studio, stock.count));
                            t.head.clone_from(&stock.head);
                            let mut m = a.rmeta;
                            m[..8].copy_from_slice(&stock.sizes);
                            let (d, meta) = lookup::write_programs(&t, &m);
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

/// A position as half floats, each coordinate rounded to nearest.
fn half_nearest(p: [f32; 3]) -> Result<[f16; 3]> {
    let h = p.map(f16::from_f32);
    if h.iter().any(|c| !c.is_finite()) {
        return Err(bad(W, 0, format!("position {p:?} does not fit half floats")));
    }
    Ok(h)
}

/// The next half float from `h` toward +infinity (`up`) or -infinity.
fn half_step(h: f16, up: bool) -> f16 {
    let b = h.to_bits();
    let negative = b & 0x8000 != 0;
    let bits = if b & 0x7fff == 0 {
        if up { 0x0001 } else { 0x8001 }
    } else if negative != up {
        b + 1
    } else {
        b - 1
    };
    f16::from_bits(bits)
}

/// A decal vertex as half floats, never behind its own surface: the nearest half-float position, or, when that lies
/// behind the plane through `p` with normal `n`, the normal's dominant coordinate moved out by the shortfall and
/// rounded away from the surface (so the vertex ends at most one half-float step of that coordinate in front).
/// Rounding to nearest moves a vertex up to half a step per axis (7.8 mm at 16..32 m from the mesh origin), enough
/// to hide a coplanar overlay under its base. A zero normal (a vertex no triangle uses) keeps the nearest position.
/// Returns whether the vertex was moved.
fn half_in_front(p: [f32; 3], n: [f32; 3]) -> Result<([f16; 3], bool)> {
    let mut h = half_nearest(p)?;
    let depth = |h: &[f16; 3]| (0..3).map(|i| (h[i].to_f32() - p[i]) * n[i]).sum::<f32>();
    let axis = (0..3).fold(0, |a, i| if n[i].abs() > n[a].abs() { i } else { a });
    let d = depth(&h);
    if n[axis] == 0.0 || d >= 0.0 {
        return Ok((h, false));
    }
    let up = n[axis] > 0.0;
    let target = h[axis].to_f32() - d / n[axis];
    let mut c = f16::from_f32(target);
    if (up && c.to_f32() < target) || (!up && c.to_f32() > target) {
        c = half_step(c, up);
    }
    h[axis] = c;
    // the depth sum rounds in f32: at most one more step
    for _ in 0..2 {
        if !h[axis].is_finite() {
            break;
        }
        if depth(&h) >= 0.0 {
            return Ok((h, true));
        }
        h[axis] = half_step(h[axis], up);
    }
    Err(bad(
        W,
        0,
        format!("position {p:?}: no half-float position in front of its surface {n:?}"),
    ))
}

/// Area-weighted vertex normals of a section from its own triangles (its indices count from its first vertex),
/// in the triangles' winding; zero for a vertex no triangle uses.
fn vertex_normals(
    sec: &meshset::Section,
    lod: &meshset::Lod,
    geo: &[u8],
    positions: &[[f32; 3]],
    mesh: &str,
) -> Result<Vec<[f32; 3]>> {
    let size = usize::from(lod.index_size);
    let start = lod.vertex_bytes as usize + sec.start_index as usize * size;
    let count = sec.primitives as usize * 3;
    let span = primase_bytes::checked_span(start, count, size, geo.len())
        .map_err(|e| bad(W, sec.at, format!("{mesh}: section indices: {e}")))?;
    let idx: Vec<usize> = geo[span]
        .chunks_exact(size)
        .map(|c| {
            if size == 2 {
                usize::from(u16::from_le_bytes([c[0], c[1]]))
            } else {
                u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize
            }
        })
        .collect();
    if let Some(&i) = idx.iter().find(|&&i| i >= positions.len()) {
        return Err(bad(
            W,
            sec.at,
            format!(
                "{mesh}: index {i} is past the section's {} vertices",
                positions.len()
            ),
        ));
    }
    let mut out = vec![[0f32; 3]; positions.len()];
    for t in idx.as_chunks::<3>().0 {
        let [a, b, c] = [positions[t[0]], positions[t[1]], positions[t[2]]];
        let (u, v) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let f = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &k in t {
            for j in 0..3 {
                out[k][j] += f[j];
            }
        }
    }
    Ok(out)
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
        let (ci, geo) = lod_geometry(src, b, &a.name, li, lod)?;
        let cf = b.files[b.manifest.chunk_file(ci)];
        let vsize = lod.vertex_bytes as usize;
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
                    let Some((positions, uv0, blend)) = surface(sec, &geo) else {
                        return Err(bad(
                            W,
                            sec.at,
                            format!(
                                "{}: section layout {:?} is not one of Studio's surface layouts",
                                a.name, sec.decl.elements
                            ),
                        ));
                    };
                    let input = SectionInput {
                        mesh: &a.name,
                        lod: li,
                        section: s,
                        key: sec.key,
                        positions: &positions,
                        uv0: &uv0,
                        blend: blend.as_deref(),
                    };
                    let v = values(&input)?;
                    check_values(&r.target, n, &v)?;
                    // a decal is drawn over another surface and hidden where it lies behind it (measured: 4 mm
                    // behind is invisible), so its vertices are never rounded behind their own surface
                    let normals = match r.target {
                        Target::Decal => Some(vertex_normals(sec, lod, &geo, &positions, &a.name)?),
                        Target::TwoSet { .. } => None,
                    };
                    for (vi, p) in positions.iter().enumerate() {
                        let (h, stepped) = match &normals {
                            Some(nrm) => half_in_front(*p, nrm[vi])?,
                            None => (half_nearest(*p)?, false),
                        };
                        report.stepped_in_front += usize::from(stepped);
                        for (c, hc) in p.iter().zip(h) {
                            vb.extend(hc.to_le_bytes());
                            report.max_position_error =
                                report.max_position_error.max((hc.to_f32() - c).abs());
                        }
                        vb.extend(half::f16::ONE.to_le_bytes());
                    }
                    for c in &v.color0 {
                        vb.extend(c);
                    }
                    // TexCoord0 as the stock Half2: a vertex-blend section's Half4 loses its weight (z) and w
                    let width = if blend.is_some() { 8 } else { 4 };
                    for c in streams[1].chunks_exact(width) {
                        vb.extend_from_slice(&c[..4]);
                    }
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

    /// A one-vertex section with Studio's surface elements (`uv`: Half2 or Half4) and the given stream strides.
    fn studio_section(uv: u8, strides: Vec<u8>) -> meshset::Section {
        let decl = meshset::Decl {
            elements: studio_layout(uv).to_vec(),
            strides,
            bytes: [0; DECL_SIZE],
        };
        meshset::Section {
            at: 0,
            key: 1,
            draw_decl: decl.clone(),
            stride: decl.strides.iter().sum(),
            decl,
            flags: 0,
            primitives: 0,
            start_index: 0,
            vertex_offset: 0,
            vertices: 1,
            bones: Vec::new(),
        }
    }

    #[test]
    fn only_unpadded_studio_streams_are_surfaces() {
        let geo = [0u8; 64];
        assert!(surface(&studio_section(format::HALF2, vec![12, 4, 4]), &geo).is_some());
        assert!(surface(&studio_section(format::HALF4, vec![12, 8, 4]), &geo).is_some());
        // the elements match, but a stream is padded (or the Half4 stride paired with Half2): refused
        assert!(surface(&studio_section(format::HALF2, vec![16, 4, 4]), &geo).is_none());
        assert!(surface(&studio_section(format::HALF2, vec![12, 8, 4]), &geo).is_none());
        assert!(surface(&studio_section(format::HALF4, vec![12, 8, 8]), &geo).is_none());
    }

    #[test]
    fn half_steps_cross_zero_and_both_signs() {
        let h = |v: f32| f16::from_f32(v);
        assert_eq!(half_step(h(1.0), true).to_f32(), 1.0 + 2f32.powi(-10));
        assert_eq!(half_step(h(1.0), false).to_f32(), 1.0 - 2f32.powi(-11));
        assert_eq!(half_step(h(-1.0), true).to_f32(), -1.0 + 2f32.powi(-11));
        assert_eq!(half_step(h(-1.0), false).to_f32(), -1.0 - 2f32.powi(-10));
        assert!(half_step(h(0.0), true).to_f32() > 0.0 && half_step(h(-0.0), false).to_f32() < 0.0);
        assert_eq!(half_step(half_step(h(0.0), true), false).to_f32(), 0.0);
    }

    #[test]
    fn decal_vertices_never_round_behind_their_surface() {
        // a deterministic spread of positions up to 120 m from the origin (y also near 0, where its steps are tiny
        // against x and z's) and tilted normals
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut moved_count = 0;
        for _ in 0..20_000 {
            let p = [
                next() * 240.0 - 120.0,
                next() * 64.0 - 32.0,
                next() * 240.0 - 120.0,
            ];
            let n = [
                next() * 0.6 - 0.3,
                if next() < 0.5 { 1.0 } else { -1.0 },
                next() * 0.6 - 0.3,
            ];
            let (h, moved) = half_in_front(p, n).unwrap();
            let d: f32 = (0..3).map(|i| (h[i].to_f32() - p[i]) * n[i]).sum();
            assert!(d >= 0.0, "{p:?} {n:?} -> {h:?} is {d} behind");
            if moved {
                // minimal: one step back toward the surface would put it behind
                let mut back = h;
                back[1] = half_step(h[1], n[1] < 0.0);
                let db: f32 = (0..3).map(|i| (back[i].to_f32() - p[i]) * n[i]).sum();
                assert!(db < 0.0, "{p:?} {n:?} -> {h:?} is further out than needed");
                moved_count += 1;
            } else {
                assert_eq!(h, half_nearest(p).unwrap());
            }
        }
        // about half the vertices round behind their surface to nearest; all of those are moved
        assert!((5_000..15_000).contains(&moved_count), "{moved_count} moved");
        // in front already, and without a normal: the nearest position
        assert_eq!(
            half_in_front([0.0, 30.4976, 0.0], [0.0, 1.0, 0.0]).unwrap().0[1].to_f32(),
            30.5
        );
        assert!(!half_in_front([0.0, 30.4976, 0.0], [0.0; 3]).unwrap().1);
        // the legion S1 desground vertex lifted 12 mm: 30.5096 rounds to 30.515625 (in front)
        assert!(!half_in_front([0.0, 30.5096, 0.0], [0.0, 1.0, 0.0]).unwrap().1);
        // just above a representable value, facing up: the next one up, not the nearest below
        let (h, moved) = half_in_front([0.0, 30.501, 0.0], [0.0, 1.0, 0.0]).unwrap();
        assert!(moved && h[1].to_f32() == 30.515625, "{h:?}");
        // facing down: the next one down
        let (h, moved) = half_in_front([0.0, 30.514, 0.0], [0.0, -1.0, 0.0]).unwrap();
        assert!(moved && h[1].to_f32() == 30.5, "{h:?}");
    }
}
