//! The Frostbite dialect: what the reader puts in a world that has no core meaning yet, as stored.

use primase_core::{Component, Entity, Geometry, World};
use serde::Serialize;
use serde_json::{Value, json};

use crate::bundle::{FileRef, Kind};
use crate::depot::{Depot, Param};
use crate::ebx::Document;
use crate::hex;
use crate::texture::TextureHeader;

/// Where an entry was read from: its superbundle TOC (relative to the data folder), its bundle, the kind of entry and
/// its name as the bundle's manifest states it (a chunk's is its GUID as stored), its type where it is a resource,
/// its size once decoded, and where its stored bytes lie (the CAS archive's install chunk and number, offset and
/// stored size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteEntry {
    pub toc: String,
    pub bundle: String,
    pub kind: Kind,
    pub name: String,
    pub resource_type: Option<u32>,
    pub size: u32,
    pub file: FileRef,
}

/// The name of an entry kind, as a listing shows it.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Ebx => "ebx",
        Kind::Res => "resource",
        Kind::Chunk => "chunk",
    }
}

impl Component for FrostbiteEntry {
    const NAME: &'static str = "frostbite.entry";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let (loc, offset, stored) = self.file;
        Ok(Some(json!({
            "toc": self.toc,
            "bundle": self.bundle,
            "kind": kind_name(self.kind),
            "name": self.name,
            "resource_type": self.resource_type.map(|t| format!("{t:#010x}")),
            "size": self.size,
            "cas": {
                "base_game": !loc.patch,
                "install_chunk": loc.chunk,
                "archive": loc.archive,
                "offset": offset,
                "stored_size": stored,
            },
        })))
    }
}

/// An EBX document read into typed values, as stored ([`Document::to_json`] shows it).
#[derive(Clone, Debug, PartialEq)]
pub struct FrostbiteEbx {
    pub document: Document,
}

impl Component for FrostbiteEbx {
    const NAME: &'static str = "frostbite.ebx";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        self.document.to_json().map(Some)
    }
    fn payload_bytes(&self) -> u64 {
        self.document.payload.len() as u64
    }
}

/// A texture resource's header as stored, on the entity that holds its `core.texture`: the Frostbite format ordinal
/// (the colour space of the format is not established, so the core texture's stays linear), the fields the reader
/// does not interpret, and the GUID of the chunk the pixels were read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteTexture {
    pub header: TextureHeader,
}

impl Component for FrostbiteTexture {
    const NAME: &'static str = "frostbite.texture";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let h = &self.header;
        Ok(Some(json!({
            "kind": h.kind,
            "format": h.format,
            "flags": h.flags,
            "width": h.width,
            "height": h.height,
            "depth": h.depth,
            "slices": h.slices,
            "mip_count": h.mip_count,
            "first_mip": h.first_mip,
            "chunk": hex(&h.chunk),
            "chunk_size": h.chunk_size,
            "mip_sizes": &h.mip_sizes[..usize::from(h.mip_count)],
            "unknown_00": hex(&h.unknown_00),
            "unknown_10": h.unknown_10,
            "unknown_20": hex(&h.unknown_20),
            "unknown_78": hex(&h.unknown_78),
        })))
    }
}

/// A ShaderBlockDepot read: material key to parameter block, and each block's parameters. A block is a material's
/// named parameters (its texture slots among them), not shader programs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteDepot {
    pub depot: Depot,
    /// The parameters of each of `depot.blocks`, in stored order.
    pub parameters: Vec<Vec<Param>>,
}

#[derive(Serialize)]
struct ParamView {
    name: String,
    slot: u16,
    #[serde(rename = "type")]
    ty: String,
    count: u16,
    extra: u16,
    /// A texture reference's flag byte and its two GUIDs as stored, else the value bytes.
    value: Value,
}

impl Component for FrostbiteDepot {
    const NAME: &'static str = "frostbite.depot";
    const GEOMETRY: Geometry = Geometry::None;
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.parameters.len() != self.depot.blocks.len() {
            return Err(format!(
                "{} parameter lists for {} blocks",
                self.parameters.len(),
                self.depot.blocks.len()
            ));
        }
        match self
            .depot
            .entries
            .iter()
            .find(|(_, b)| *b >= self.depot.blocks.len())
        {
            Some((key, b)) => Err(format!(
                "key {key:#x} names block {b} of {}",
                self.depot.blocks.len()
            )),
            None => Ok(()),
        }
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let blocks: Vec<Value> = self
            .depot
            .blocks
            .iter()
            .zip(&self.parameters)
            .map(|(block, params)| {
                let params: Vec<ParamView> = params
                    .iter()
                    .map(|p| ParamView {
                        name: format!("{:#018x}", p.name),
                        slot: p.slot(),
                        ty: format!("{:#010x}", p.ty),
                        count: p.count,
                        extra: p.extra,
                        value: match p.texture_ref(&block.data) {
                            Some(t) => json!({
                                "flag": t[0],
                                "guid_a": hex(&t[1..17]),
                                "guid_b": hex(&t[17..33]),
                            }),
                            None => Value::String(hex(&block.data[p.at + 20..p.at + p.len])),
                        },
                    })
                    .collect();
                json!({
                    "hash": format!("{:#018x}", block.hash),
                    "bytes": block.data.len(),
                    "parameters": params,
                })
            })
            .collect();
        let entries: Vec<Value> = (self.depot.entries.iter())
            .map(|(key, block)| json!({ "key": format!("{key:#018x}"), "block": block }))
            .collect();
        checked(json!({
            "head": self.depot.head,
            "version": self.depot.version,
            "entries": entries,
            "blocks": blocks,
        }))
    }
    fn payload_bytes(&self) -> u64 {
        self.depot.blocks.iter().map(|b| b.data.len() as u64).sum()
    }
}

/// A JSON view with nothing in it that JSON cannot hold (every number here is an integer).
fn checked(v: Value) -> Result<Option<Value>, primase_core::JsonError> {
    primase_core::json::to_value(&v).map(Some)
}
