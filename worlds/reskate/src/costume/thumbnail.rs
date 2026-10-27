//! The costume's own menu thumbnail in place of the donor's: Studio's item (the `DelMarBaseItemAsset` it adds,
//! `items/cust_fullbodycostume/own_<name>`) points its `Thumbnail` (the small picture) and `ThumbnailLarge` at the donor
//! costume's `DingoCdnTexture` documents, each of which refers to a UI `TextureAsset`. [`set_thumbnail`] makes a
//! thumbnail of the costume's for each of the two and points the item at them.
//!
//! For each of the two pictures, from the donor's documents read from the game:
//! - a texture asset: the donor's EBX with a new file GUID, instance GUID, `Name` (the asset's name) and `Resource` id,
//!   everything else (the crop and authored size are the slot's layout) as the donor's; its resource, the donor's
//!   header with a new chunk GUID and the new picture's byte size, byte 0x21 the level count (as in every game
//!   texture), `unknown_00` and byte 0x20 as the caller states ([`Choices::header_unknowns`]), the rest (the format,
//!   size, one level, the texture group name) as the donor's, the donor's resource metadata; and a chunk of the
//!   encoded picture, added as [`super::textures`] adds its chunks;
//! - a CDN document: the donor's with a new file GUID, instance GUID, `Name`, a `Ref` import of the new texture asset,
//!   `NameHash` (the DJB2-XOR hash of the new texture asset's `Name` that [`name_hash`] computes, which the game's own
//!   CDN documents carry for theirs: checked on every stock costume thumbnail) and `ContentHash` as the caller states
//!   ([`CdnContentHash`]: its meaning is not established);
//! - the item's import of the donor's CDN document is replaced by one of the new document (the item's `Thumbnail` and
//!   `ThumbnailLarge` are different imports; the import is replaced in place, as [`super::textures::retexture`] does).
//!
//! The pictures are encoded in the donor texture's own format; BC7 (ordinals 66 and 67) is encoded, any other format
//! is refused. A picture must be the donor texture's size (256 and 768 pixels square in the game's stock thumbnails):
//! resizing is the caller's. The assets are named `thumbnail/tool/own_<costume>[_lrg]` and
//! `thumbnail/cdn/img_own_<costume>[_lrg]` after the stock pattern; their identities derive from those names, so a
//! rebuild makes the same ones, and a resource id has bit 0 set as every texture's in the game does (see
//! [`super::textures`]). The entries are added as Studio adds a costume's: the item's bundles, its flags and handler
//! with the user string `reskate-cosmetic-addition`; the chunks also to the bundles Studio adds its mesh chunks to.
//! `Name` of both documents is the entry name in lower case, as [`super::textures`] names its assets; the game's stock
//! documents carry the same text in mixed case (`Thumbnail/Tool/Own_...`), and nothing established that the game reads
//! the difference: that is a choice of this module, not a finding. Nothing else of the mod changes.

use frostbite_formats::ebx::{self, Document, ImportReference, Pointer, Value};
use frostbite_formats::fbmod::{Body, Fbmod};
use frostbite_formats::folder::{EbxEntry, Source};
use frostbite_formats::texture::TextureHeader;
use primase_foundations::compress::oodle::Oodle;
use primase_core::TexFormat;
use primase_texcodec::{Encoder, TexEncoder};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;

use super::asset::{Base, HeaderUnknowns, assign_identity, derived, derived_rid, read_donor, write_checked};
use super::textures::Donor;
use super::{ADDITION, Error, Result, set_document};

/// The root type of the item Studio adds for a costume.
pub const ITEM: &str = "DelMarBaseItemAsset";

/// A picture: straight RGBA8, rows top first (`width * height * 4` bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The costume's two pictures: the item's `Thumbnail` and `ThumbnailLarge`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnails {
    pub small: Picture,
    pub large: Picture,
}

/// One thumbnail made: the item field it serves, its texture asset's and CDN document's names, and the donor texture
/// it was made from, with its format ordinal and size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MadeThumbnail {
    pub field: &'static str,
    pub texture: String,
    pub cdn: String,
    pub donor: String,
    pub format: u32,
    pub width: u16,
    pub height: u16,
}

/// What a CDN document's `ContentHash` holds. Its meaning is not established, so the caller states it; there is no
/// default. What was checked, on the game's two stock costume thumbnails (`Win32/items.toc`, the bundle that holds
/// them): the value is not the SHA-1 recorded in the bundle manifest for any EBX, resource or chunk of the bundle (both
/// documents' values checked), and for the large thumbnail it is not the SHA-1 of its stock chunk's 589,824 bytes (the
/// small one's chunk was not checked). It may be a hash of the source image the game's tools made the thumbnail from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CdnContentHash {
    /// The donor document's value, kept: it names the donor's picture, not the costume's.
    Donor,
    /// The SHA-1 of the encoded picture's chunk: unique to the new content, which is a guess at what is hashed.
    PixelsSha1,
}

/// The choices the caller states: what [`set_thumbnail`] writes where a value's meaning is not established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choices {
    /// The new textures' header fields `unknown_00` and byte 0x20 (see [`HeaderUnknowns`]). The donor thumbnails have
    /// both zero, so the two choices make the same bytes for them; the choice is still the caller's.
    pub header_unknowns: HeaderUnknowns,
    pub content_hash: CdnContentHash,
}

/// What [`set_thumbnail`] did: the choices it was given, the item's name and the thumbnails made (small first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnailed {
    pub choices: Choices,
    pub item: String,
    pub thumbnails: Vec<MadeThumbnail>,
}

/// The `NameHash` of a CDN document: the case-sensitive [`djb2`](frostbite_formats::djb2) of the name of the texture
/// asset it refers to. The game's stock CDN documents carry it for their textures' `Name`.
pub fn name_hash(name: &str) -> u32 {
    frostbite_formats::djb2(name)
}

/// The item document Studio added: its entry's index and the document.
fn find_item(m: &Fbmod, oodle: Option<&Oodle>) -> Result<(usize, Document)> {
    let mut found = None;
    for (i, e) in m.entries.iter().enumerate() {
        let (Body::Ebx, Some(s)) = (&e.body, &e.stored) else {
            continue;
        };
        if !s.user.starts_with(ADDITION) || !e.name.starts_with("items/") {
            continue;
        }
        let doc = ebx::read(&m.payload(s.index, oodle)?)?;
        if doc.root_type != ITEM {
            continue;
        }
        if found.replace((i, doc)).is_some() {
            return Err(Error::Missing(
                "the mod has more than one item Studio added".into(),
            ));
        }
    }
    found.ok_or_else(|| Error::Missing("the mod has no item Studio added".into()))
}

/// The import slot that field `field` of `doc`'s first instance points at.
fn slot(doc: &Document, field: &str, what: &str) -> Result<usize> {
    let p = doc
        .instances
        .first()
        .and_then(|i| i.object.as_ref())
        .and_then(|o| o.field(field));
    match p {
        Some(Value::Pointer(Pointer::External(i))) if *i < doc.imports.len() => Ok(*i),
        _ => Err(Error::Missing(format!("{what}: {field} is not an import"))),
    }
}

/// What one [`set_thumbnail`] call reads from: the game's donor documents and the caller's choices.
struct Context<'a> {
    src: &'a Source<'a>,
    donor: &'a Donor<'a>,
    /// The donor bundle's documents by file GUID.
    index: BTreeMap<[u8; 16], EbxEntry>,
    costume: &'a str,
    choices: Choices,
}

/// The new documents and payloads of one thumbnail.
struct Made {
    texture_doc: Document,
    header: TextureHeader,
    meta: [u8; 16],
    rid: u64,
    pixels: Vec<u8>,
    cdn_doc: Document,
    report: MadeThumbnail,
}

/// The thumbnail serving item field `field` (`suffix` the asset names' size suffix) made from `picture`, and the
/// item's import slot it replaces.
fn make(
    cx: &Context,
    item: &Document,
    field: &'static str,
    suffix: &str,
    picture: &Picture,
) -> Result<(usize, Made)> {
    let what = |d: String| Error::Missing(format!("thumbnail {field}: {d}"));
    let item_slot = slot(item, field, "the item")?;
    let cdn_entry = cx
        .index
        .get(&item.imports[item_slot].file_guid)
        .ok_or_else(|| what("the item's import is not a document of the donor bundle".into()))?;
    let mut cdn = cx.src.read_ebx(cdn_entry)?;
    if cdn.root_type != "DingoCdnTexture" {
        return Err(what(format!(
            "{} is a {}, not a DingoCdnTexture",
            cdn_entry.name, cdn.root_type
        )));
    }
    let ref_slot = slot(&cdn, "Ref", &cdn_entry.name)?;
    let tex_entry = cx
        .index
        .get(&cdn.imports[ref_slot].file_guid)
        .ok_or_else(|| what(format!("{}'s Ref is not in the donor bundle", cdn_entry.name)))?;
    let texture_name = format!("thumbnail/tool/own_{}{suffix}", cx.costume);
    let cdn_name = format!("thumbnail/cdn/img_own_{}{suffix}", cx.costume);

    let d = read_donor(cx.src, cx.donor, tex_entry)?;
    let t = &d.header;
    if !matches!(t.format, 66 | 67) {
        return Err(what(format!(
            "{} is format {}; only BC7 (66, 67) is encoded",
            tex_entry.name, t.format
        )));
    }
    if (t.slices, t.depth, t.mip_count) != (1, 1, 1) {
        return Err(what(format!(
            "{} has {} slices, depth {} and {} levels, not one picture",
            tex_entry.name, t.slices, t.depth, t.mip_count
        )));
    }
    if (picture.width, picture.height) != (u32::from(t.width), u32::from(t.height)) {
        return Err(what(format!(
            "the picture is {}x{}, the donor texture {} is {}x{}",
            picture.width, picture.height, tex_entry.name, t.width, t.height
        )));
    }
    let pixels = Encoder::default()
        .encode_rgba8(TexFormat::Bc7, picture.width, picture.height, &picture.rgba)
        .map_err(|e| what(format!("encoding the picture: {e}")))?;

    let rid = derived_rid(&texture_name);
    let mut tex = d.doc;
    assign_identity(&mut tex, &texture_name, vec![("Resource", Value::Resource(rid))])?;
    let mut mip_sizes = [0u32; 15];
    mip_sizes[0] = pixels.len() as u32;
    let mut header = TextureHeader {
        chunk: derived(&format!("{texture_name}/chunk")),
        mip_sizes,
        chunk_size: pixels.len() as u32,
        ..t.clone()
    };
    cx.choices.header_unknowns.apply(&mut header, t, 1);

    cdn.imports[ref_slot] = ImportReference {
        file_guid: tex.file_guid,
        class_guid: tex.instances[0].guid,
    };
    let mut set = vec![("NameHash", Value::UInt(u64::from(name_hash(&texture_name))))];
    if cx.choices.content_hash == CdnContentHash::PixelsSha1 {
        set.push(("ContentHash", Value::Sha1(Sha1::digest(&pixels).into())));
    }
    assign_identity(&mut cdn, &cdn_name, set)?;
    let report = MadeThumbnail {
        field,
        texture: texture_name,
        cdn: cdn_name,
        donor: tex_entry.name.clone(),
        format: t.format,
        width: t.width,
        height: t.height,
    };
    Ok((
        item_slot,
        Made {
            texture_doc: tex,
            header,
            meta: d.meta,
            rid,
            pixels,
            cdn_doc: cdn,
            report,
        },
    ))
}

/// Give the costume `costume` (Studio's item name, `c1010_first7`) its own thumbnails (see the module documentation).
/// `donor` names the bundle that holds the donor costume's thumbnail documents. Refused: a mod without exactly one
/// item Studio added or without a chunk entry (whose bundles the new chunks join), an item whose thumbnails are not
/// imports of the donor bundle's CDN documents, a donor texture that is not one BC7 picture, a picture that is not
/// its size.
pub fn set_thumbnail(
    m: &mut Fbmod,
    src: &Source,
    oodle: Option<&Oodle>,
    donor: &Donor,
    costume: &str,
    thumbnails: &Thumbnails,
    choices: Choices,
) -> Result<Thumbnailed> {
    let (item_at, mut item) = find_item(m, oodle)?;
    let item_entry = m.entries[item_at].clone();
    // Studio's user string for what it adds is the item's up to its `|donor=...` suffix
    let user = item_entry
        .stored
        .as_ref()
        .map_or(ADDITION, |s| s.user.split('|').next().unwrap_or(ADDITION));
    let base = Base::new(m, &item_entry, user)?;
    let cx = Context {
        src,
        donor,
        index: src.ebx_index(donor.toc, donor.bundle)?,
        costume,
        choices,
    };

    let mut made = Vec::new();
    for (field, suffix, picture) in [
        ("Thumbnail", "", &thumbnails.small),
        ("ThumbnailLarge", "_lrg", &thumbnails.large),
    ] {
        made.push(make(&cx, &item, field, suffix, picture)?);
    }
    if made[0].0 == made[1].0 {
        return Err(Error::Missing(
            "the item's Thumbnail and ThumbnailLarge are the same import".into(),
        ));
    }

    let mut report = Thumbnailed {
        choices,
        item: item_entry.name.clone(),
        thumbnails: Vec::new(),
    };
    let mut new_entries = Vec::new();
    for (item_slot, a) in made {
        let texture_ebx = write_checked(&a.texture_doc)?;
        let cdn_ebx = write_checked(&a.cdn_doc)?;
        new_entries.extend(base.texture_asset(
            m,
            &a.report.texture,
            &texture_ebx,
            (&a.header, a.rid, &a.meta),
            &a.pixels,
        )?);
        new_entries.push(base.ebx(m, &a.report.cdn, &cdn_ebx)?);
        item.imports[item_slot] = ImportReference {
            file_guid: a.cdn_doc.file_guid,
            class_guid: a.cdn_doc.instances[0].guid,
        };
        report.thumbnails.push(a.report);
    }
    set_document(m, item_at, &item)?;
    m.entries.extend(new_entries);
    m.drop_unused_payloads();
    Ok(report)
}
