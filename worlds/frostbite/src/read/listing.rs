//! What a data folder holds, by superbundle, bundle and entry: each TOC's bundles, and a bundle's EBX documents,
//! resources and chunks with their names, the resource types, their sizes and the CAS archive and offset the bytes
//! lie at. Only the TOC and the bundle manifests are read (a manifest is a CAS file of its own): no entry's payload
//! is decoded, except where a caller asks for an EBX document's type ([`Source::ebx_type`]).

use std::sync::Arc;

use super::folder::Source;
use crate::bundle::{self, Asset, FileRef, Kind, Manifest};
use crate::toc::{Toc, TocChunk};
use crate::{Error, Result, depot, hex, lookup};

/// One entry of a bundle, as its manifest lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// The superbundle TOC, relative to the data folder (as in [`Folder::tocs`](super::folder::Folder::tocs)), and the
    /// bundle in it.
    pub toc: String,
    pub bundle: String,
    pub kind: Kind,
    /// The entry's place among the manifest's entries of its kind.
    pub index: usize,
    /// An EBX document's or a resource's name as the manifest states it; a chunk's is its GUID as stored, in hex.
    pub name: String,
    /// A resource's type id and its id.
    pub resource_type: Option<u32>,
    pub resource_id: Option<u64>,
    /// The entry's size once decoded.
    pub size: u32,
    /// Where its stored bytes lie: the CAS archive ([`Loc`](crate::toc::Loc)), the offset and the stored size.
    pub file: FileRef,
}

/// A bundle opened: its TOC's tables, its file table and its manifest.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub toc_path: String,
    pub name: String,
    pub(super) toc: Arc<Toc>,
    pub(super) files: Vec<FileRef>,
    /// The index in `files` of the manifest's first asset.
    pub(super) first: usize,
    pub manifest: Manifest,
}

/// What [`Source::listing`] found in a TOC: its bundles opened, and the bundles whose manifest could not be found or
/// read, by name with the reason.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub bundles: Vec<Bundle>,
    pub unread: Vec<(String, String)>,
}

impl Listing {
    /// Every entry of every bundle, in bundle order.
    pub fn entries(&self) -> impl Iterator<Item = Listed> + '_ {
        self.bundles.iter().flat_map(Bundle::entries)
    }
}

impl Bundle {
    fn listed(&self, kind: Kind, index: usize, a: &Asset, file_index: usize) -> Listed {
        let res = kind == Kind::Res;
        Listed {
            toc: self.toc_path.clone(),
            bundle: self.name.clone(),
            kind,
            index,
            name: match kind {
                Kind::Chunk => hex(&a.guid),
                _ => a.name.clone(),
            },
            resource_type: res.then_some(a.rtype),
            resource_id: res.then_some(a.rid),
            size: if kind == Kind::Chunk { a.lsize } else { a.size },
            file: self.files[file_index],
        }
    }

    /// Every entry: the EBX documents, then the resources, then the chunks, each in manifest order.
    pub fn entries(&self) -> impl Iterator<Item = Listed> + '_ {
        let m = &self.manifest;
        let base = self.first;
        let ebx = (m.ebx.iter().enumerate()).map(move |(i, a)| self.listed(Kind::Ebx, i, a, base + i));
        let res = (m.res.iter().enumerate())
            .map(move |(i, a)| self.listed(Kind::Res, i, a, base + m.ebx.len() + i));
        let chunks = (m.chunks.iter().enumerate())
            .map(move |(i, a)| self.listed(Kind::Chunk, i, a, base + m.ebx.len() + m.res.len() + i));
        ebx.chain(res).chain(chunks)
    }

    /// Resource `i` of the manifest, as an entry.
    pub fn resource(&self, i: usize) -> Listed {
        let m = &self.manifest;
        self.listed(Kind::Res, i, &m.res[i], self.first + m.ebx.len() + i)
    }

    /// The chunk with GUID `guid` in this bundle's TOC chunk table: its bytes' place and decoded size (the whole chunk:
    /// a bundle's manifest may hold a tail of it, as for a streamed texture), `None` when the table has none; an
    /// error when the table lists it as removed.
    pub fn toc_chunk(&self, src: &Source<'_>, guid: &[u8; 16]) -> Result<Option<(FileRef, usize)>> {
        let Some(TocChunk { file, .. }) = self.toc.chunks.iter().find(|c| &c.guid == guid) else {
            return Ok(None);
        };
        let file =
            file.ok_or_else(|| Error::Missing(format!("{}: chunk {} is removed", self.name, hex(guid))))?;
        let raw = src.raw(file)?;
        let blocks = crate::cas::blocks(&raw)?;
        Ok(Some((file, blocks.iter().map(|b| b.decoded_size).sum())))
    }

    /// The chunk with GUID `guid` (as stored): its bytes' place and decoded size, from this bundle or else from the
    /// TOC's chunk table (content copied from the base game lies there).
    pub fn chunk(&self, src: &Source<'_>, guid: &[u8; 16]) -> Result<(FileRef, usize)> {
        let m = &self.manifest;
        if let Some(i) = m.chunks.iter().position(|c| &c.guid == guid) {
            return Ok((
                self.files[self.first + m.ebx.len() + m.res.len() + i],
                m.chunks[i].lsize as usize,
            ));
        }
        let TocChunk { file, .. } = self.toc.chunks.iter().find(|c| &c.guid == guid).ok_or_else(|| {
            Error::Missing(format!(
                "{}: chunk {} is in neither the bundle nor its TOC",
                self.name,
                hex(guid)
            ))
        })?;
        let file =
            file.ok_or_else(|| Error::Missing(format!("{}: chunk {} is removed", self.name, hex(guid))))?;
        let raw = src.raw(file)?;
        let blocks = crate::cas::blocks(&raw)?;
        Ok((file, blocks.iter().map(|b| b.decoded_size).sum()))
    }
}

/// The name of a resource type the game's executable names and this crate knows, `None` for any other.
pub fn resource_type_name(rtype: u32) -> Option<&'static str> {
    TYPE_NAMES
        .iter()
        .find(|(id, _)| *id == rtype)
        .map(|(_, name)| *name)
}

/// The resource types this crate names and the names the game's executable holds for them (the id is the djb2-xor hash
/// of the lowercased name: [`type_id`]). A texture, a ShaderBlockDepotResource and a CompiledBytecode are read
/// ([`Source::read_entry`]); the others are named only.
pub const TYPE_NAMES: &[(u32, &str)] = &[
    (crate::TEXTURE, "Texture"),
    (crate::MESHSET, "MeshSet"),
    (depot::RTYPE, "ShaderBlockDepotResource"),
    (crate::bytecode::RTYPE, "CompiledBytecode"),
    (lookup::PROGRAM_LOOKUP, "ExpressionShaderDatabase"),
    (
        lookup::TEXTURE_LOOKUP,
        "ExpressionShaderStreamableTextureSetLookupTable",
    ),
    (0x2FEC_6526, "ShaderStateAssetId"),
    (0xD489_FACB, "ExpressionShaderRasterPermutation"),
    (0x95CE_62DD, "ExpressionShaderComputePermutation"),
    (0xA19B_025D, "ExpressionShaderBindingSet"),
    (0x7D98_F632, "ExpressionShaderInputLayout"),
    (0x2500_47B2, "ExpressionShaderRasterPermutationSharedData"),
    (0x879B_A711, "ExpressionShaderPermutationSharedData"),
    (0x7A63_0735, "ExpressionShaderStreamableTextureSet"),
    (0xA895_7ED6, "ExpressionShaderRootSlotInitData"),
    (0x589B_B36E, "ExpressionShaderPassBinding"),
    (0x6A5B_E5DB, "ExpressionShaderBindingSignature"),
    (0xF434_940D, "ShaderParameterCollectionTable"),
];

/// A resource type's id from its name: the lowercased name hashed with djb2 and xor (`h = 0x1505`, then
/// `h = h * 33 ^ byte`).
pub fn type_id(name: &str) -> u32 {
    name.to_ascii_lowercase()
        .bytes()
        .fold(0x1505u32, |h, b| h.wrapping_mul(0x21) ^ u32::from(b))
}

impl Source<'_> {
    /// The bundles of the TOC at `toc` (relative to the data folder), by name, in stored order. Reads the TOC only.
    pub fn bundle_names(&self, toc: &str) -> Result<Vec<String>> {
        Ok(self
            .folder
            .toc(toc)?
            .bundles
            .into_iter()
            .map(|(n, _)| n)
            .collect())
    }

    fn open_bundle(&self, toc_path: &str, toc: &Arc<Toc>, name: &str, region: &[u8]) -> Result<Bundle> {
        let (files, inline) = bundle::read_region(region)?;
        let (manifest, first, _) = self
            .manifest(&files, &inline)?
            .ok_or_else(|| Error::Missing(format!("{name} has no manifest")))?;
        Ok(Bundle {
            toc_path: toc_path.to_owned(),
            name: name.to_owned(),
            toc: Arc::clone(toc),
            files,
            first,
            manifest,
        })
    }

    /// The bundle named `name` (ASCII case-insensitive) of the TOC at `toc`.
    pub fn bundle(&self, toc: &str, name: &str) -> Result<Bundle> {
        let t = Arc::new(self.folder.toc(toc)?);
        let (n, region) = t
            .bundles
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::Missing(format!("{toc} has no bundle {name}")))?;
        self.open_bundle(toc, &t, n, region)
    }

    /// Every bundle of the TOC at `toc`, opened (the TOC is read once, each manifest once).
    pub fn listing(&self, toc: &str) -> Result<Listing> {
        let t = Arc::new(self.folder.toc(toc)?);
        let mut out = Listing::default();
        for (name, region) in &t.bundles {
            match self.open_bundle(toc, &t, name, region) {
                Ok(b) => out.bundles.push(b),
                Err(e) => out.unread.push((name.clone(), e.to_string())),
            }
        }
        Ok(out)
    }

    /// The type name of EBX document `entry`'s first instance. Decodes the document: its cost is the document's.
    pub fn ebx_type(&self, entry: &Listed) -> Result<String> {
        if entry.kind != Kind::Ebx {
            return Err(Error::Unsupported(format!("{}: not an EBX document", entry.name)));
        }
        let bytes = self.payload(entry.file, entry.size as usize)?;
        Ok(crate::ebx::read(&bytes)?.root_type)
    }
}
