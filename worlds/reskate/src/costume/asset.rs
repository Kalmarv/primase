//! What the steps that add a texture asset to the mod share ([`super::textures`], [`super::thumbnail`]): the identities
//! derived from an asset's name, the donor texture read from the game, the new document's identity fields, the header
//! fields the caller chooses, a document written once and checked, and the three entries (EBX, texture resource, chunk)
//! added as Studio adds a costume's, so the Studio contract for them lives here once.

use primase_frostbite::ebx::{self, Document, Value};
use primase_frostbite::fbmod::{Body, ChunkInfo, Entry, Fbmod, ResInfo, Stored};
use primase_frostbite::folder::{EbxEntry, Source};
use primase_frostbite::texture::{self, TextureHeader};
use sha2::{Digest, Sha256};

use super::textures::Donor;
use super::{Error, Result};

/// 16 bytes derived from `identity`.
pub(super) fn derived(identity: &str) -> [u8; 16] {
    let d = Sha256::digest(identity.as_bytes());
    let mut g = [0u8; 16];
    g.copy_from_slice(&d[..16]);
    g
}

/// The resource id of the texture asset `name`: derived from the name, with bit 0 set as every texture's in the game
/// (`Win32/items.toc`, all 12011); the game treats an even id as a resolved pointer, and an untagged id crashed it
/// (`Skate+0x3dc1b71`).
pub(super) fn derived_rid(name: &str) -> u64 {
    u64::from_le_bytes(
        derived(&format!("{name}/rid"))[..8]
            .try_into()
            .unwrap_or_default(),
    ) | 1
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

impl HeaderUnknowns {
    /// Set `header`'s `unknown_00` and `unknown_20` from this choice and the donor texture's header, and byte 0x21
    /// (`unknown_20[1]`) to the level count, as in every game texture.
    pub(super) fn apply(self, header: &mut TextureHeader, donor: &TextureHeader, levels: u8) {
        header.unknown_20 = donor.unknown_20;
        header.unknown_00 = match self {
            Self::Zero => {
                header.unknown_20[0] = 0;
                [0; 8]
            }
            Self::Donor => donor.unknown_00,
        };
        header.unknown_20[1] = levels;
    }
}

/// A donor texture read from the game: its EBX document, its header and its resource's metadata.
pub(super) struct DonorTexture {
    pub doc: Document,
    pub header: TextureHeader,
    pub meta: [u8; 16],
}

/// The donor texture at `entry`, read; refused when its document has no `Resource` or the resource is not a texture.
pub(super) fn read_donor(src: &Source, donor: &Donor, entry: &EbxEntry) -> Result<DonorTexture> {
    let doc = src.read_ebx(entry)?;
    let what = |d: &str| Error::Missing(format!("donor texture {}: {d}", entry.name));
    let Some(Value::Resource(rid)) = doc
        .instances
        .first()
        .and_then(|i| i.object.as_ref())
        .and_then(|o| o.field("Resource"))
        .cloned()
    else {
        return Err(what("its first instance has no Resource"));
    };
    let res = src.resource(donor.toc, donor.bundle, rid)?;
    if res.rtype != primase_frostbite::TEXTURE {
        return Err(what(&format!(
            "its resource is of type {:#x}, not a texture",
            res.rtype
        )));
    }
    Ok(DonorTexture {
        doc,
        header: texture::read(&res.bytes)?,
        meta: res.meta,
    })
}

/// Give `doc` the identity of the asset `name`: file and instance GUIDs derived from it, and its first instance's
/// `Name` and the further fields `set`; every field must exist.
pub(super) fn assign_identity(doc: &mut Document, name: &str, set: Vec<(&str, Value)>) -> Result<()> {
    let what = |d: String| Error::Missing(format!("document {name}: {d}"));
    doc.file_guid = derived(&format!("{name}/file"));
    let inst = doc
        .instances
        .first_mut()
        .ok_or_else(|| what("no instance".into()))?;
    inst.guid = derived(&format!("{name}/instance"));
    let o = inst
        .object
        .as_mut()
        .ok_or_else(|| what("its first instance is not read".into()))?;
    let fields = std::iter::once(("Name", Value::Str(name.as_bytes().to_vec()))).chain(set);
    for (field, value) in fields {
        *o.field_mut(field).ok_or_else(|| what(format!("no {field}")))? = value;
    }
    Ok(())
}

/// `doc` written, once, and read back to check that what is written reads; the bytes written.
pub(super) fn write_checked(doc: &Document) -> Result<Vec<u8>> {
    let bytes = ebx::write(doc)?;
    ebx::read(&bytes)?;
    Ok(bytes)
}

/// Where a step adds its entries: the stored-payload template (flags, handler and user string of the entries it adds),
/// the bundles the entries join, and the bundles Studio adds its chunks to (those of the mod's first chunk entry).
pub(super) struct Base {
    stored: Stored,
    bundles: Vec<u32>,
    added_bundles: Vec<u32>,
}

impl Base {
    /// The base for the entries of a step that adds beside `entry` (a document Studio added, with a payload): its
    /// bundles and stored flags, `user` the user string of the entries added. Refused: an entry without a payload, a
    /// mod without a chunk entry.
    pub fn new(m: &Fbmod, entry: &Entry, user: &str) -> Result<Self> {
        let stored = entry
            .stored
            .clone()
            .ok_or_else(|| Error::Missing(format!("{} has no payload", entry.name)))?;
        let added_bundles = m
            .entries
            .iter()
            .find_map(|e| match &e.body {
                Body::Chunk(c) => Some(c.added_bundles.clone()),
                _ => None,
            })
            .ok_or_else(|| Error::Missing("the mod has no chunk entry".into()))?;
        Ok(Self {
            stored: Stored {
                user: user.into(),
                ..stored
            },
            bundles: entry.bundles.clone(),
            added_bundles,
        })
    }

    /// An entry `name` of `body` whose payload `bytes` is added to `m`.
    fn entry(&self, m: &mut Fbmod, name: &str, bytes: &[u8], body: Body) -> Result<Entry> {
        Ok(Entry {
            name: name.into(),
            stored: Some(Stored {
                index: m.add_payload(bytes)?,
                size: bytes.len() as u64,
                ..self.stored.clone()
            }),
            bundles: self.bundles.clone(),
            body,
        })
    }

    /// An EBX entry `name` of `bytes` (already written).
    pub fn ebx(&self, m: &mut Fbmod, name: &str, bytes: &[u8]) -> Result<Entry> {
        self.entry(m, name, bytes, Body::Ebx)
    }

    /// The entries of the texture asset `name`, their payloads added to `m`: its EBX (`ebx_bytes`, already written),
    /// its texture resource (`header` written, with resource id `rid` and metadata `meta`) and its chunk of `pixels`,
    /// named by the header's chunk GUID. The chunk is added as Studio adds its own chunks (range, logical offset and
    /// h32 0, the logical size the chunk's; checked on Studio's files) with first mip 0, the texture's (Frosty's
    /// chunk resources carry the texture's first mip; Studio's mesh chunks carry -1).
    pub fn texture_asset(
        &self,
        m: &mut Fbmod,
        name: &str,
        ebx_bytes: &[u8],
        (header, rid, meta): (&TextureHeader, u64, &[u8]),
        pixels: &[u8],
    ) -> Result<[Entry; 3]> {
        let resource = Body::Res(ResInfo {
            res_type: primase_frostbite::TEXTURE,
            rid,
            meta: meta.to_vec(),
        });
        let chunk = Body::Chunk(ChunkInfo {
            range_start: 0,
            range_end: 0,
            logical_offset: 0,
            logical_size: pixels.len() as u32,
            h32: 0,
            first_mip: 0,
            added_bundles: self.added_bundles.clone(),
        });
        Ok([
            self.ebx(m, name, ebx_bytes)?,
            self.entry(m, name, &texture::write(header)?, resource)?,
            self.entry(m, &guid_text(&header.chunk), pixels, chunk)?,
        ])
    }
}
