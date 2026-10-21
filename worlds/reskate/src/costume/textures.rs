//! A costume's own textures in place of the donor's: Studio's presets draw with the donor costume's textures (its
//! colour, normal and mask texture arrays); [`retexture`] adds a texture of the costume's for each one the caller
//! replaces and points every preset of the costume at it.
//!
//! For each replaced donor texture (chosen by the caller, by its name) a new texture asset is made from the donor's
//! own documents, read from the game:
//! - its EBX (a `TextureAsset` or `TextureArrayAsset`) with a new file GUID, instance GUID, `Name`, `Resource` id and
//!   `AuthoredWidth`/`AuthoredHeight` (the replacement's size), everything else as the donor's;
//! - its texture resource: the donor's header with the replacement's format, size and levels, one slice (a texture
//!   array keeps its kind with one slice: an array index past the end is clamped to the last slice, so every UV tile
//!   samples it, and the order of levels and slices in the chunk is not in question), first mip 0 (every level in
//!   the chunk), byte 0x21 the level count (as in every game texture), `unknown_00` and byte 0x20 as the caller
//!   states ([`HeaderUnknowns`]: their meaning is not established), the rest as the donor's; the donor's resource
//!   metadata;
//! - a chunk of the levels, largest first, added as Studio adds its own chunks (range, logical offset and h32 0, the
//!   logical size the chunk's; checked on Studio's files) with first mip 0, the texture's (Frosty's chunk resources
//!   carry the texture's first mip; Studio's mesh chunks carry -1).
//!
//! Identities are derived from the costume's name and the role, so a rebuild makes the same ones; a resource id has
//! bit 0 set as every texture's in the game does (`Win32/items.toc`, all 12011) (the game treats an even id as a resolved pointer: an untagged id
//! crashed it, `Skate+0x3dc1b71`). The entries are added as Studio adds a costume's: the preset's bundles, its flags,
//! handler and user string; the chunk also to the bundles Studio adds its mesh chunks to.
//!
//! Replacements that share a role share one asset: every donor texture the caller maps to the same replacement is
//! redirected to the one asset made for it.

use primase_compress::oodle::Oodle;
use primase_frostbite::ebx::{Document, ImportReference, Value};
use primase_frostbite::fbmod::{Body, ChunkInfo, Entry, Fbmod, ResInfo, Stored};
use primase_frostbite::folder::{EbxEntry, Source};
use primase_frostbite::texture::{self, TextureHeader};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::{Error, Result, costume_presets, set_document};

/// A texture of the costume: one slice's levels, largest first, in format `format` (a Frostbite ordinal
/// [`texture::block`] knows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// The asset's name suffix (`c_ta`): the asset is `<costume>_<role>` beside the costume's presets.
    pub role: String,
    pub format: u32,
    pub width: u16,
    pub height: u16,
    pub levels: Vec<Vec<u8>>,
}

/// Where the donor's textures are: the TOC and bundle that hold them.
#[derive(Clone, Copy, Debug)]
pub struct Donor<'a> {
    pub toc: &'a str,
    pub bundle: &'a str,
}

/// What a new texture's header holds in the two fields whose meaning is not established and which ReSkate does not
/// read: `unknown_00` (8 bytes) and byte 0x20. The caller states it; there is no default. Context, not proof: among
/// the textures of `Win32/items.toc` whose first mip is 0 (every level in the chunk, as the costume's), 6654 of 7733
/// have `unknown_00` zero (elsewhere it looks like a pair of compressed sizes) and 7726 have byte 0x20 zero; the
/// donor's were written for its own chunk, not the costume's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderUnknowns {
    /// Both zero.
    Zero,
    /// Both the donor texture's.
    Donor,
}

/// What [`retexture`] did: the header choice it was given; per new asset, its name and the donor textures redirected
/// to it; per preset, how many imports it redirected; the donor texture imports left as they were, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retextured {
    pub header_unknowns: HeaderUnknowns,
    pub assets: Vec<(String, Vec<String>)>,
    pub presets: Vec<(String, usize)>,
    pub kept: Vec<String>,
}

/// 16 bytes derived from `identity`.
fn derived(identity: &str) -> [u8; 16] {
    let d = Sha256::digest(identity.as_bytes());
    let mut g = [0u8; 16];
    g.copy_from_slice(&d[..16]);
    g
}

/// A GUID's text form (`8-4-4-4-12`) from its stored bytes (the first three groups little-endian).
pub fn guid_text(g: &[u8; 16]) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let rev = |r: std::ops::Range<usize>| hex(&g[r].iter().rev().copied().collect::<Vec<_>>());
    format!(
        "{}-{}-{}-{}-{}",
        rev(0..4),
        rev(4..6),
        rev(6..8),
        hex(&g[8..10]),
        hex(&g[10..16])
    )
}

fn check(r: &Replacement) -> Result<()> {
    let bad = |d: String| Error::Missing(format!("replacement {}: {d}", r.role));
    let s = texture::block(r.format)
        .ok_or_else(|| bad(format!("format {} is not one ReSkate decodes", r.format)))?;
    if !(1..=texture::MAX_SIDE).contains(&r.width) || !(1..=texture::MAX_SIDE).contains(&r.height) {
        return Err(bad(format!("size {}x{}", r.width, r.height)));
    }
    let n = u8::try_from(r.levels.len())
        .ok()
        .filter(|n| (1..=15).contains(n))
        .ok_or_else(|| bad(format!("{} levels", r.levels.len())))?;
    let chain = texture::chain(s, u32::from(r.width), u32::from(r.height), n)
        .ok_or_else(|| bad("size overflows".into()))?;
    for (i, (l, want)) in r.levels.iter().zip(chain).enumerate() {
        if l.len() != want as usize {
            return Err(bad(format!(
                "level {i} is {} bytes, its size takes {want}",
                l.len()
            )));
        }
    }
    Ok(())
}

/// The new asset for `r`, from the donor texture at `entry`: its EBX document, header, resource metadata, chunk GUID
/// and id.
struct Made {
    doc: Document,
    header: Vec<u8>,
    meta: Vec<u8>,
    chunk: [u8; 16],
    rid: u64,
}

fn make(
    src: &Source,
    donor: &Donor,
    entry: &EbxEntry,
    name: &str,
    r: &Replacement,
    unknowns: HeaderUnknowns,
) -> Result<Made> {
    let mut doc = src.read_ebx(entry)?;
    let what = |d: &str| Error::Missing(format!("donor texture {}: {d}", entry.name));
    let rid_field = doc
        .instances
        .first()
        .and_then(|i| i.object.as_ref())
        .and_then(|o| o.field("Resource"))
        .cloned();
    let Some(Value::Resource(donor_rid)) = rid_field else {
        return Err(what("its first instance has no Resource"));
    };
    let res = src.resource(donor.toc, donor.bundle, donor_rid)?;
    if res.rtype != primase_frostbite::TEXTURE {
        return Err(what(&format!(
            "its resource is of type {:#x}, not a texture",
            res.rtype
        )));
    }
    let t: TextureHeader = texture::read(&res.bytes)?;

    let chunk = derived(&format!("{name}/chunk"));
    let rid = u64::from_le_bytes(
        derived(&format!("{name}/rid"))[..8]
            .try_into()
            .unwrap_or_default(),
    ) | 1;
    doc.file_guid = derived(&format!("{name}/file"));
    let inst = doc.instances.first_mut().ok_or_else(|| what("no instance"))?;
    inst.guid = derived(&format!("{name}/instance"));
    let o = inst
        .object
        .as_mut()
        .ok_or_else(|| what("its first instance is not read"))?;
    for (field, value) in [
        ("Name", Value::Str(name.as_bytes().to_vec())),
        ("Resource", Value::Resource(rid)),
        ("AuthoredWidth", Value::UInt(u64::from(r.width))),
        ("AuthoredHeight", Value::UInt(u64::from(r.height))),
    ] {
        *o.field_mut(field).ok_or_else(|| what(&format!("no {field}")))? = value;
    }

    let levels = r.levels.len();
    let mut mip_sizes = [0u32; 15];
    for (s, l) in mip_sizes.iter_mut().zip(&r.levels) {
        *s = l.len() as u32;
    }
    let mut unknown_20 = t.unknown_20;
    let unknown_00 = match unknowns {
        HeaderUnknowns::Zero => {
            unknown_20[0] = 0;
            [0; 8]
        }
        HeaderUnknowns::Donor => t.unknown_00,
    };
    unknown_20[1] = levels as u8;
    let header = TextureHeader {
        unknown_00,
        format: r.format,
        width: r.width,
        height: r.height,
        depth: 1,
        slices: 1,
        mip_count: levels as u8,
        first_mip: 0,
        unknown_20,
        chunk,
        mip_sizes,
        chunk_size: mip_sizes.iter().sum(),
        ..t
    };
    Ok(Made {
        doc,
        header: texture::write(&header)?,
        meta: res.meta.to_vec(),
        chunk,
        rid,
    })
}

/// Point the costume's presets at its own textures (see the module documentation). `costume` names the costume's
/// assets (Studio's item name, `c1010_first7`); `pick` maps a donor texture's name to its replacement, or `None` to
/// keep it; `unknowns` the header choice ([`HeaderUnknowns`]). Refused: a replacement whose levels are not the chain of its format and size, a donor texture that is not
/// a texture, a mod without presets or without a mesh chunk entry (whose bundles the new chunks join).
pub fn retexture<'r>(
    m: &mut Fbmod,
    src: &Source,
    oodle: Option<&Oodle>,
    donor: &Donor,
    costume: &str,
    pick: &dyn Fn(&str) -> Option<&'r Replacement>,
    unknowns: HeaderUnknowns,
) -> Result<Retextured> {
    let presets = costume_presets(m, oodle)?;
    let (first, _) = presets
        .first()
        .ok_or_else(|| Error::Missing("the mod has no costume presets".into()))?;
    let base = m.entries[*first].clone();
    let base_stored = base
        .stored
        .clone()
        .ok_or_else(|| Error::Missing("a preset without a payload".into()))?;
    let dir = base.name.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
    let added_bundles = m
        .entries
        .iter()
        .find_map(|e| match &e.body {
            Body::Chunk(c) => Some(c.added_bundles.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::Missing("the mod has no chunk entry".into()))?;
    let index = src.ebx_index(donor.toc, donor.bundle)?;

    let mut report = Retextured {
        header_unknowns: unknowns,
        assets: Vec::new(),
        presets: Vec::new(),
        kept: Vec::new(),
    };
    // donor file GUID -> the import the presets get instead
    let mut redirect: BTreeMap<[u8; 16], ImportReference> = BTreeMap::new();
    // role -> (asset name, its import)
    let mut made: BTreeMap<String, (String, ImportReference)> = BTreeMap::new();
    let mut new_entries = Vec::new();
    for (_, doc) in &presets {
        for im in &doc.imports {
            if redirect.contains_key(&im.file_guid) {
                continue;
            }
            let Some(entry) = index.get(&im.file_guid) else {
                continue;
            };
            let Some(r) = pick(&entry.name) else {
                if !report.kept.contains(&entry.name) {
                    report.kept.push(entry.name.clone());
                }
                continue;
            };
            if let Some((asset, to)) = made.get(&r.role) {
                redirect.insert(im.file_guid, *to);
                if let Some(a) = report.assets.iter_mut().find(|(n, _)| n == asset) {
                    a.1.push(entry.name.clone());
                }
                continue;
            }
            check(r)?;
            let name = format!("{dir}/{costume}_{}", r.role);
            let a = make(src, donor, entry, &name, r, unknowns)?;
            let to = ImportReference {
                file_guid: a.doc.file_guid,
                class_guid: a.doc.instances[0].guid,
            };
            let ebx_bytes = primase_frostbite::ebx::write(&a.doc)?;
            primase_frostbite::ebx::read(&ebx_bytes)?;
            let pixels: Vec<u8> = r.levels.concat();
            let stored = |m: &mut Fbmod, raw: &[u8]| -> Result<Option<Stored>> {
                Ok(Some(Stored {
                    index: m.add_payload(raw)?,
                    size: raw.len() as u64,
                    ..base_stored.clone()
                }))
            };
            new_entries.push(Entry {
                name: name.clone(),
                stored: stored(m, &ebx_bytes)?,
                bundles: base.bundles.clone(),
                body: Body::Ebx,
            });
            new_entries.push(Entry {
                name: name.clone(),
                stored: stored(m, &a.header)?,
                bundles: base.bundles.clone(),
                body: Body::Res(ResInfo {
                    res_type: primase_frostbite::TEXTURE,
                    rid: a.rid,
                    meta: a.meta,
                }),
            });
            new_entries.push(Entry {
                name: guid_text(&a.chunk),
                stored: stored(m, &pixels)?,
                bundles: base.bundles.clone(),
                body: Body::Chunk(ChunkInfo {
                    range_start: 0,
                    range_end: 0,
                    logical_offset: 0,
                    logical_size: pixels.len() as u32,
                    h32: 0,
                    first_mip: 0,
                    added_bundles: added_bundles.clone(),
                }),
            });
            made.insert(r.role.clone(), (name.clone(), to));
            redirect.insert(im.file_guid, to);
            report.assets.push((name, vec![entry.name.clone()]));
        }
    }

    for (i, mut doc) in presets {
        let mut n = 0;
        for im in &mut doc.imports {
            if let Some(to) = redirect.get(&im.file_guid) {
                *im = *to;
                n += 1;
            }
        }
        if n > 0 {
            set_document(m, i, &doc)?;
        }
        report.presets.push((m.entries[i].name.clone(), n));
    }
    m.entries.extend(new_entries);
    m.drop_unused_payloads();
    Ok(report)
}
