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
/// (the core texture's colour space is the format's own: [`crate::entry::format_colour_space`]), the fields the
/// reader does not interpret, and the GUID of the chunk the pixels were read from.
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

/// Where a `core.shader` came from in a CompiledBytecode resource, and the bytes around its container as stored: the
/// container's number among the resource's (`index` of `count`) and its offset in the payload, the tag the payload's
/// header carries at [`crate::bytecode::TAG_AT`] (`RC2S` for the raster and compute programs, `RPTR` for the raytracing
/// ones), the bytes before the first container (on container 0 only) and the bytes after this container up to the next
/// or the payload's end. The header's and the trailing fields are not interpreted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteBytecode {
    pub index: usize,
    pub count: usize,
    pub offset: usize,
    pub tag: Option<[u8; 4]>,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

impl Component for FrostbiteBytecode {
    const NAME: &'static str = "frostbite.bytecode";
    const GEOMETRY: Geometry = Geometry::None;
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.index >= self.count {
            return Err(format!("container {} of {}", self.index, self.count));
        }
        Ok(())
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        Ok(Some(json!({
            "index": self.index,
            "count": self.count,
            "offset": self.offset,
            "tag": self.tag.map(|t| String::from_utf8_lossy(&t).into_owned()),
            "before_bytes": self.before.len(),
            "after_bytes": self.after.len(),
        })))
    }
    fn payload_bytes(&self) -> u64 {
        (self.before.len() + self.after.len()) as u64
    }
}

/// A shader permutation read ([`crate::permutation`]): its kind, its own id, the ids it names (the shared data, the
/// input layout), the names of the CompiledBytecode resources its programs are, and the `core.shader` entities read
/// from them (in the same order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbitePermutation {
    pub permutation: crate::permutation::Permutation,
    pub shaders: Vec<Entity>,
}

impl Component for FrostbitePermutation {
    const NAME: &'static str = "frostbite.permutation";
    const GEOMETRY: Geometry = Geometry::None;
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.shaders.len() != self.permutation.bytecode.len() {
            return Err(format!(
                "{} shaders for {} programs",
                self.shaders.len(),
                self.permutation.bytecode.len()
            ));
        }
        Ok(())
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(&self.shaders);
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let p = &self.permutation;
        Ok(Some(json!({
            "kind": match p.kind {
                crate::permutation::Kind::Raster => "raster",
                crate::permutation::Kind::Compute => "compute",
            },
            "id": p.id.to_string(),
            "shared_data": p.shared_data.to_string(),
            "input_layout": p.input_layout.map(|i| i.to_string()),
            "unknown_18": p.unknown_18,
            "bytecode": p.bytecode.iter().map(crate::permutation::bytecode_name).collect::<Vec<_>>(),
        })))
    }
}

/// A program of a program database as a material key reaches it: the key the lookup row is under, the program's
/// index in the database's records, the record's own key, and its permutations (`frostbite.permutation` entities,
/// in the record's order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteProgram {
    pub material: u64,
    pub index: u32,
    pub record_key: u64,
    /// The program's 168-byte record as stored.
    pub record: [u8; crate::lookup::RECORD_LEN],
    pub permutations: Vec<Entity>,
}

impl Component for FrostbiteProgram {
    const NAME: &'static str = "frostbite.program";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(&self.permutations);
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        Ok(Some(json!({
            "material": format!("{:#018x}", self.material),
            "index": self.index,
            "record_key": format!("{:#018x}", self.record_key),
            "permutations": self.permutations.len(),
        })))
    }
}

/// A program database (ExpressionShaderDatabase) read: its program records and its lookup rows (material key to
/// program indices).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbitePrograms {
    pub records: Vec<crate::lookup::ProgramRecord>,
    pub lookup: crate::lookup::ProgramLookup,
}

impl Component for FrostbitePrograms {
    const NAME: &'static str = "frostbite.programs";
    const GEOMETRY: Geometry = Geometry::None;
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let n = self.records.len();
        match (self.lookup.rows.iter()).find(|(_, row)| row.iter().any(|&i| i as usize >= n)) {
            Some((key, _)) => Err(format!("row {key:#x} names a program past the {n} records")),
            None => Ok(()),
        }
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let records: Vec<Value> = (self.records.iter())
            .map(|r| {
                json!({
                    "key": format!("{:#018x}", r.key),
                    "permutations": r.permutations.iter().map(u64::to_string).collect::<Vec<_>>(),
                })
            })
            .collect();
        let rows: serde_json::Map<String, Value> = (self.lookup.rows.iter())
            .map(|(k, row)| (format!("{k:#018x}"), json!(row)))
            .collect();
        Ok(Some(json!({ "records": records, "rows": rows })))
    }
    fn payload_bytes(&self) -> u64 {
        (self.records.len() * crate::lookup::RECORD_LEN + self.lookup.head.len()) as u64
    }
}

/// A MeshSet read ([`crate::meshset`]): its LODs and sections as stored (vertex layouts, where each section's vertices
/// and indices lie in its LOD's chunk, the section's material key), and the material of each distinct key where one
/// was resolved ([`crate::program::Scope`]). The vertices themselves are not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteMeshSet {
    pub meshset: crate::meshset::MeshSet,
    /// Each distinct section key's `frostbite.material` entity, once resolved.
    pub materials: std::collections::BTreeMap<u64, Entity>,
}

impl Component for FrostbiteMeshSet {
    const NAME: &'static str = "frostbite.meshset";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.materials.values());
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let element = |e: &crate::meshset::Element| json!({ "usage": e.usage, "format": e.format, "offset": e.offset, "stream": e.stream });
        let lods: Vec<Value> = (self.meshset.lods.iter())
            .map(|l| {
                json!({
                    "chunk": hex(&l.chunk),
                    "index_size": l.index_size,
                    "index_bytes": l.index_bytes,
                    "vertex_bytes": l.vertex_bytes,
                    "sections": l.sections.iter().map(|s| json!({
                        "key": format!("{:#018x}", s.key),
                        "vertices": s.vertices,
                        "primitives": s.primitives,
                        "start_index": s.start_index,
                        "vertex_offset": s.vertex_offset,
                        "flags": s.flags,
                        "stride": s.stride,
                        "stored_declaration": s.decl.elements.iter().map(element).collect::<Vec<_>>(),
                        "draw_declaration": s.draw_decl.elements.iter().map(element).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(Some(json!({
            "lods": lods,
            "materials": self.materials.keys().map(|k| format!("{k:#018x}")).collect::<Vec<_>>(),
        })))
    }
}

/// A material key resolved the way the game resolves it: in the ShaderBlockDepots, the program database's rows and the
/// streamable texture set lookup of the bundles loaded. What none of them held is named in `unresolved`; nothing is
/// guessed in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteMaterial {
    pub key: u64,
    /// The depot that holds the key: its resource name, the block the key names and the block's parameters.
    pub depot: Option<MaterialDepot>,
    /// The `frostbite.program` entities of the key's program row, in the row's order.
    pub programs: Vec<Entity>,
    /// The streamable texture set the key's lookup row names, and its resource name where a bundle holds it.
    pub texture_set: Option<(u64, Option<String>)>,
    /// What each texture parameter of the depot block leads to, in the block's order, once
    /// [`Source::resolve_textures`](crate::folder::Source::resolve_textures) has run (empty before it).
    pub textures: Vec<MaterialTexture>,
    pub unresolved: Vec<String>,
}

/// A texture parameter of a material's depot block: its name hash, the GUID the block holds for it (the file GUID of
/// a texture asset) and where the GUID led.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialTexture {
    pub name: u64,
    pub guid: [u8; 16],
    pub link: TextureLink,
}

/// Where a texture parameter's GUID led.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureLink {
    /// The texture entity of the world (a `core.texture` with its `frostbite.texture` header).
    Texture(Entity),
    /// Nowhere, and why (the reason without the name of any entry: the material's `unresolved` has that).
    Missing(String),
}

/// The depot block of a material key ([`FrostbiteMaterial::depot`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialDepot {
    pub resource: String,
    pub bundle: String,
    pub block: crate::depot::Block,
    pub parameters: Vec<Param>,
}

impl Component for FrostbiteMaterial {
    const NAME: &'static str = "frostbite.material";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(&self.programs);
        out.extend(self.textures.iter().filter_map(|t| match t.link {
            TextureLink::Texture(e) => Some(e),
            TextureLink::Missing(_) => None,
        }));
    }
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        for t in &self.textures {
            if let TextureLink::Texture(e) = t.link
                && !(w.get::<primase_core::Texture>(e).is_some() && w.get::<FrostbiteTexture>(e).is_some())
            {
                return Err(format!(
                    "texture parameter {:#018x} names an entity that is not a frostbite texture",
                    t.name
                ));
            }
        }
        Ok(())
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let depot = self.depot.as_ref().map(|d| {
            let texture_slots: Vec<u16> = (d.parameters.iter())
                .filter(|p| p.texture_ref(&d.block.data).is_some())
                .map(Param::slot)
                .collect();
            json!({
                "resource": d.resource,
                "bundle": d.bundle,
                "block_hash": format!("{:#018x}", d.block.hash),
                "block_bytes": d.block.data.len(),
                "parameters": d.parameters.len(),
                "texture_slots": texture_slots,
            })
        });
        Ok(Some(json!({
            "key": format!("{:#018x}", self.key),
            "depot": depot,
            "programs": self.programs.len(),
            "texture_set": self.texture_set.as_ref().map(|(id, name)| json!({ "id": id.to_string(), "resource": name })),
            "textures": self.textures.iter().map(|t| json!({
                "name": format!("{:#018x}", t.name),
                "guid": hex(&t.guid),
                "texture": match &t.link {
                    TextureLink::Texture(e) => json!(e.index()),
                    TextureLink::Missing(why) => json!({ "missing": why }),
                },
            })).collect::<Vec<_>>(),
            "unresolved": self.unresolved,
        })))
    }
    fn payload_bytes(&self) -> u64 {
        self.depot.as_ref().map_or(0, |d| d.block.data.len() as u64)
    }
}

/// The vertex elements of a mesh's sections that [`crate::geometry`] does not decode, as stored, one list per primitive
/// (a section) in the mesh's order, on the entity that holds the mesh. What they mean is not established: blend weights
/// and indices, a sub-material index, the tangent space, radiosity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteVertexElements {
    pub primitives: Vec<Vec<crate::geometry::Raw>>,
}

impl Component for FrostbiteVertexElements {
    const NAME: &'static str = "frostbite.vertex_elements";
    const GEOMETRY: Geometry = Geometry::PerVertex;
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let Some(mesh) = w.get::<primase_core::Mesh>(e) else {
            return Err("no mesh on the entity".to_owned());
        };
        let prims = mesh.primitives();
        if prims.len() != self.primitives.len() {
            return Err(format!(
                "{} lists for {} primitives",
                self.primitives.len(),
                prims.len()
            ));
        }
        for (i, (p, elements)) in prims.iter().zip(&self.primitives).enumerate() {
            let n = p.vertex_count();
            if let Some(r) = elements.iter().find(|r| r.data.len() != r.size * n) {
                return Err(format!(
                    "primitive {i}: usage {} holds {} bytes for {n} vertices of {}",
                    r.usage,
                    r.data.len(),
                    r.size
                ));
            }
        }
        Ok(())
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(
            primase_core::remap_list(&self.primitives, from)
                .map(|primitives| FrostbiteVertexElements { primitives }),
        )
    }
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        let Some(elements) = self.primitives.get(primitive) else {
            return Some(Err(format!("primitive {primitive} of {}", self.primitives.len())));
        };
        let gathered: Result<Vec<_>, String> = elements.iter().map(|r| r.gather(from)).collect();
        Some(gathered.map(|g| {
            let mut out = self.clone();
            out.primitives[primitive] = g;
            out
        }))
    }
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let primitives: Vec<Vec<Value>> = (self.primitives.iter())
            .map(|p| {
                p.iter()
                    .map(|r| {
                        json!({
                            "usage": r.usage,
                            "format": r.format,
                            "bytes_per_vertex": r.size,
                            "vertices": r.data.len().checked_div(r.size).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .collect();
        Ok(Some(json!(primitives)))
    }
    fn payload_bytes(&self) -> u64 {
        (self.primitives.iter().flatten())
            .map(|r| r.data.len() as u64)
            .sum()
    }
}

/// A permutation's binding sets as its shared data names them ([`crate::permutation::SharedData`]): the pass binding and
/// the three sets read, so a shader's registers resolve to parameters ([`crate::binding::Bindings`]). On the
/// permutation's entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteBindings {
    pub shared: crate::permutation::SharedData,
    pub pass: Option<crate::binding::Set>,
    pub sets: [Option<crate::binding::Set>; 3],
}

impl FrostbiteBindings {
    /// The register rule's view of the sets ([`crate::binding::Bindings`]).
    pub fn bindings(&self) -> crate::binding::Bindings<'_> {
        crate::binding::Bindings {
            pass: self.pass.as_ref(),
            sets: [
                self.sets[0].as_ref(),
                self.sets[1].as_ref(),
                self.sets[2].as_ref(),
            ],
        }
    }
}

impl Component for FrostbiteBindings {
    const NAME: &'static str = "frostbite.bindings";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let set = |s: &Option<crate::binding::Set>| {
            s.as_ref().map(|s| {
                json!({
                    "id": s.id.to_string(),
                    "groups": s.groups.iter().map(|g| json!({
                        "name": format!("{:#018x}", g.name),
                        "flags": hex(&g.flags),
                        "parameters": g.params.len(),
                        "buffer_bytes": g.buffer_end(),
                    })).collect::<Vec<_>>(),
                })
            })
        };
        Ok(Some(json!({
            "pass_binding": set(&self.pass),
            "sets": self.sets.iter().map(set).collect::<Vec<_>>(),
            "signature": self.shared.signature.to_string(),
            "root_slots": self.shared.root_slots.to_string(),
        })))
    }
    fn payload_bytes(&self) -> u64 {
        let size = |s: &crate::binding::Set| s.groups.iter().map(|g| 24 * g.params.len() as u64).sum::<u64>();
        (self.pass.iter().chain(self.sets.iter().flatten()))
            .map(size)
            .sum()
    }
}

/// A raster permutation's input layout ([`crate::inputlayout`]): the vertex elements its vertex program reads. On the
/// permutation's entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrostbiteInputLayout(pub crate::inputlayout::InputLayout);

impl Component for FrostbiteInputLayout {
    const NAME: &'static str = "frostbite.input_layout";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, primase_core::JsonError> {
        let l = &self.0;
        Ok(Some(json!({
            "id": l.id.to_string(),
            "elements": l.elements.iter().zip(&l.usages).map(|(e, &usage)| json!({
                "usage": usage,
                "dxgi_format": e.dxgi(),
                "first": e.first,
                "last": e.last,
            })).collect::<Vec<_>>(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(textures: Vec<MaterialTexture>) -> FrostbiteMaterial {
        FrostbiteMaterial {
            key: 1,
            depot: None,
            programs: Vec::new(),
            texture_set: None,
            textures,
            unresolved: Vec::new(),
        }
    }

    #[test]
    fn a_texture_link_must_lead_to_a_frostbite_texture() {
        let mut w = World::new();
        let plain = w.spawn();
        let link = |link| MaterialTexture {
            name: 0xC1,
            guid: [3; 16],
            link,
        };
        let m = material(vec![link(TextureLink::Missing("why".into()))]);
        assert_eq!(m.verify(plain, &w), Ok(()));
        let m = material(vec![link(TextureLink::Texture(plain))]);
        assert!(
            m.verify(plain, &w)
                .unwrap_err()
                .contains("not a frostbite texture")
        );
        // the entities a material links are the ones it references, a missing link none
        let mut out = Vec::new();
        m.references(&mut out);
        assert_eq!(out, [plain]);
    }
}
