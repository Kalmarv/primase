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

use primase_foundations::compress::oodle::Oodle;
use primase_frostbite::ebx::{Document, ImportReference, Value};
use primase_frostbite::fbmod::Fbmod;
use primase_frostbite::folder::{EbxEntry, Source};
use primase_frostbite::texture::{self, TextureHeader};
use std::collections::BTreeMap;

use super::asset::{Base, assign_identity, derived, derived_rid, read_donor, write_checked};
pub use super::asset::{HeaderUnknowns, guid_text};
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

/// What [`retexture`] did: the header choice it was given; per new asset, its name and the donor textures redirected
/// to it; per preset, how many imports it redirected; the donor texture imports left as they were, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retextured {
    pub header_unknowns: HeaderUnknowns,
    pub assets: Vec<(String, Vec<String>)>,
    pub presets: Vec<(String, usize)>,
    pub kept: Vec<String>,
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

/// The new asset for `r`, from the donor texture at `entry`: its EBX document, header, resource metadata and id.
struct Made {
    doc: Document,
    header: TextureHeader,
    meta: [u8; 16],
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
    let d = read_donor(src, donor, entry)?;
    let rid = derived_rid(name);
    let mut doc = d.doc;
    assign_identity(
        &mut doc,
        name,
        vec![
            ("Resource", Value::Resource(rid)),
            ("AuthoredWidth", Value::UInt(u64::from(r.width))),
            ("AuthoredHeight", Value::UInt(u64::from(r.height))),
        ],
    )?;

    let levels = r.levels.len();
    let mut mip_sizes = [0u32; 15];
    for (s, l) in mip_sizes.iter_mut().zip(&r.levels) {
        *s = l.len() as u32;
    }
    let mut header = TextureHeader {
        format: r.format,
        width: r.width,
        height: r.height,
        depth: 1,
        slices: 1,
        mip_count: levels as u8,
        first_mip: 0,
        chunk: derived(&format!("{name}/chunk")),
        mip_sizes,
        chunk_size: mip_sizes.iter().sum(),
        ..d.header.clone()
    };
    unknowns.apply(&mut header, &d.header, levels as u8);
    Ok(Made {
        doc,
        header,
        meta: d.meta,
        rid,
    })
}

/// Point the costume's presets at its own textures (see the module documentation). `costume` names the costume's
/// assets (Studio's item name, `c1010_first7`); `pick` maps a donor texture's name to its replacement, or `None` to
/// keep it; `unknowns` the header choice ([`HeaderUnknowns`]). Refused: a replacement whose levels are not the chain
/// of its format and size, a donor texture that is not a texture, a mod without presets or without a mesh chunk entry
/// (whose bundles the new chunks join).
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
    let preset = m.entries[*first].clone();
    let user = preset.stored.as_ref().map(|s| s.user.clone()).unwrap_or_default();
    let base = Base::new(m, &preset, &user)?;
    let dir = preset.name.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
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
            let ebx_bytes = write_checked(&a.doc)?;
            let pixels: Vec<u8> = r.levels.concat();
            new_entries.extend(base.texture_asset(
                m,
                &name,
                &ebx_bytes,
                (&a.header, a.rid, &a.meta),
                &pixels,
            )?);
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
