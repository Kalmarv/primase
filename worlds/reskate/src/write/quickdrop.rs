//! A Quick Drop object pack: new items cloned from a stock one ([`crate::game::quickdrop::Donor`]), written as a
//! `.fbmod` for ReSkate Studio's `compile-mod` to package ([`pack`]).
//!
//! Each item is a clone of the donor's own documents (the item, its prefabs, mesh component, mesh asset, collision
//! asset and the bundle's mesh variation database), as the community packs clone theirs (KENNYoMKV's Black Chain,
//! Diplomatico's Skate 3 props: compared document by document, 2026-10-25):
//! - every GUID the clone defines is new: each document's file and instance GUIDs, the prefabs' entity GUIDs
//!   (`EntityGuid`) and the partitions they were cooked from (`SourceAsset.Partition`), each derived from the item's key and the old GUID ([`crate::hash::identity`]), and every place
//!   that names one follows ([`frostbite_formats::ebx::remap`]); every other GUID (component types, the stock content
//!   the item keeps using) stays, and so does an instance named by its component type's GUID (a prefab's component
//!   segments: 13 of the donor rail's 32 prefab instances, the 13 Black Chain keeps). A written clone holding any old
//!   GUID is refused.
//! - what names the item changes: the item's `Name`, `Key`, `HashedAssetKey` (djb2 of the key), `DisplayName` and
//!   `BuildKitPrefabName`; each document's `Name` (its entry's name); the mesh asset's `NameHash` (djb2 of its name)
//!   and `MeshSetResource`; the collision asset's `Resource` (and its `VolumeMoments`, when the item states them);
//!   the mesh component's `PartBoundingBoxes` (the MeshSet's bounds). `BuildKitBundleName` stays the donor's: the
//!   item's ECS documents join the donor's bundle.
//! - its MeshSet, geometry chunk and PhysicsResource are new resources, their ids derived from their names (bit 0
//!   set, as the game's: [`crate::hash::resource_id`]).
//! - registered: the item master collection's `Items` and the donor bundle's network registry's `Objects` gain an
//!   import of the new item and prefab.
//!
//! Entries are written as Studio writes the ones it adds (flags 8, handler 0) and the ones it edits (flags 0); a bundle
//! is named by the djb2 of its lowercased name; a chunk joins its bundle with Studio's mesh chunks' fields (range and
//! logical offset 0, logical size its own, first mip -1).

use std::collections::{BTreeSet, HashMap};

use frostbite_formats::ebx::{self, Document, ImportReference, Pointer, Value, remap};
use frostbite_formats::fbmod::{Body, ChunkInfo, Entry, Fbmod, ResInfo, Stored};
use frostbite_formats::meshset::MeshSet;
use frostbite_formats::physics::{self, PhysicsResource};
use frostbite_formats::{Error, MESHSET, Result, djb2, guid_text, meshset};

use crate::game::quickdrop::{Donor, ITEMS_BUNDLE, Named};
use crate::hash::{identity, resource_id};

/// The user string of the entries this writer adds.
pub const USER: &str = "primase-quickdrop:v1";
/// The fbmod's profile and game version, as Studio 20261012 writes them for skate.
pub const PROFILE: &str = "Skate";
pub const GAME_VERSION: u32 = 625_200;

/// One new item.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The item's key (`own_...`): its document is `<the donor's folder>/<key>`.
    pub key: String,
    /// The name the Quick Drop menu shows.
    pub title: String,
    /// The mesh asset's name (`world/buildkit/quickdrop/.../<x>_mesh`), which names the MeshSet too.
    pub mesh_name: String,
    /// The MeshSet (its name, LOD names and chunk GUID are set from `mesh_name`) and its LOD-0 geometry chunk.
    pub meshset: MeshSet,
    pub chunk: Vec<u8>,
    pub collision: PhysicsResource,
    /// The collision asset's `VolumeMoments` (row-major m11..m44); `None` keeps the donor's.
    pub volume_moments: Option<[f64; 16]>,
}

fn bad(what: impl Into<String>) -> Error {
    Error::Missing(what.into())
}

/// The bundle hash an entry names a bundle by.
pub fn bundle_hash(name: &str) -> u32 {
    djb2(&name.to_ascii_lowercase())
}

/// The object of the first instance of `doc` whose type is `ty`, mutably.
fn object_mut<'d>(doc: &'d mut Document, ty: &str) -> Result<&'d mut ebx::Object> {
    let i = (0..doc.instances.len())
        .find(|&i| doc.type_name(i) == Some(ty))
        .ok_or_else(|| bad(format!("{} has no {ty}", doc.root_type)))?;
    doc.instances[i]
        .object
        .as_mut()
        .ok_or_else(|| bad(format!("{ty} is not read")))
}

/// Sets `field` of `obj`, keeping the stored value's kind (a string stays a string, an integer its signedness).
fn set(obj: &mut ebx::Object, field: &str, v: Value) -> Result<()> {
    let slot = obj
        .field_mut(field)
        .ok_or_else(|| bad(format!("no field {field}")))?;
    *slot = match (&*slot, v) {
        (Value::Int(_), Value::UInt(u)) => Value::Int(u as i64),
        (Value::UInt(_), Value::Int(i)) => Value::UInt(i as u64),
        (_, v) => v,
    };
    Ok(())
}

fn text(s: &str) -> Value {
    Value::Str(s.as_bytes().to_vec())
}

/// The GUID values of fields named `field` anywhere in `doc`.
fn guids_named(doc: &Document, field: &str) -> Vec<[u8; 16]> {
    fn walk(v: &Value, name: &str, field: &str, out: &mut Vec<[u8; 16]>) {
        match v {
            Value::Guid(g) if name == field => out.push(*g),
            Value::Object(o) => o.fields.iter().for_each(|f| walk(&f.value, &f.name, field, out)),
            Value::Array(items, _) => items.iter().for_each(|x| walk(x, name, field, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for o in doc.instances.iter().filter_map(|i| i.object.as_ref()) {
        o.fields
            .iter()
            .for_each(|f| walk(&f.value, &f.name, field, &mut out));
    }
    out
}

/// An object without where its boxed values lay (their offsets are the layout's, which a write lays out anew).
fn layoutless(mut o: ebx::Object) -> ebx::Object {
    fn value(v: &mut Value) {
        match v {
            Value::Boxed(b) => {
                b.data_offset = 0;
                b.relative_offset = 0;
            }
            Value::Object(o) => o.fields.iter_mut().for_each(|f| value(&mut f.value)),
            Value::Array(items, _) => items.iter_mut().for_each(value),
            _ => {}
        }
    }
    o.fields.iter_mut().for_each(|f| value(&mut f.value));
    o
}

/// The partition a prefab names as its source (`SourceAsset.Partition` of its root instance): the identity of the
/// document it was cooked from, which its entity path records repeat (`PartitionGuid`).
fn source_partition(doc: &Document) -> Option<[u8; 16]> {
    let root = doc.instances.first()?.object.as_ref()?;
    let Value::Object(r) = root.field("SourceAsset")? else {
        return None;
    };
    match r.field("Partition")? {
        Value::Guid(g) => Some(*g),
        _ => None,
    }
}

/// A document written, read back to the same values, and holding none of `old` (the GUIDs a clone replaced).
fn written(doc: &Document, old: &BTreeSet<[u8; 16]>, what: &str) -> Result<Vec<u8>> {
    let bytes = ebx::write(doc)?;
    let back = ebx::read(&bytes)?;
    if back.instances.len() != doc.instances.len() {
        return Err(bad(format!(
            "{what}: written document reads back with another instance count"
        )));
    }
    let (back_objects, objects): (Vec<_>, Vec<_>) = (
        back.instances
            .iter()
            .map(|i| i.object.clone().map(layoutless))
            .collect(),
        doc.instances
            .iter()
            .map(|i| i.object.clone().map(layoutless))
            .collect(),
    );
    let differs = back_objects
        .iter()
        .zip(&objects)
        .enumerate()
        .find(|(_, (x, y))| x != y);
    if let Some((i, (x, y))) = differs {
        let field = match (x, y) {
            (Some(x), Some(y)) => x.fields.iter().zip(&y.fields).find(|(a, b)| a != b).map_or_else(
                || " (its fields differ in number)".to_owned(),
                |(a, b)| format!(" field {}: {:?} written, {:?} meant", a.name, a.value, b.value),
            ),
            _ => " (one side not read)".to_owned(),
        };
        return Err(bad(format!(
            "{what}: written instance {i} reads back otherwise{field}"
        )));
    }
    if let Some(at) = bytes
        .windows(16)
        .position(|w| w.try_into().is_ok_and(|g: [u8; 16]| old.contains(&g)))
    {
        return Err(bad(format!(
            "{what}: an old GUID {} is left at byte {at:#x}",
            guid_text(bytes[at..at + 16].try_into().unwrap_or(&[0; 16]))
        )));
    }
    Ok(bytes)
}

/// The root instance's `[file GUID, instance GUID]` (an import of it).
fn import_of(doc: &Document) -> Result<ImportReference> {
    let root = doc
        .instances
        .iter()
        .find(|i| i.exported)
        .ok_or_else(|| bad(format!("{} has no exported instance", doc.root_type)))?;
    Ok(ImportReference {
        file_guid: doc.file_guid,
        class_guid: root.guid,
    })
}

/// Appends a pointer to `imp` (imported) to the array `field` of `doc`'s instance of type `ty`.
fn append_import(doc: &mut Document, ty: &str, field: &str, imp: ImportReference) -> Result<()> {
    let index = doc.imports.len();
    doc.imports.push(imp);
    match object_mut(doc, ty)?.field_mut(field) {
        Some(Value::Array(items, _)) => {
            items.push(Value::Pointer(Pointer::External(index)));
            Ok(())
        }
        _ => Err(bad(format!("{ty}.{field} is not an array"))),
    }
}

struct Writer<'a> {
    m: Fbmod,
    donor: &'a Donor,
    ecs: Vec<u32>,
    items: Vec<u32>,
}

impl Writer<'_> {
    fn entry(&mut self, name: &str, bytes: &[u8], bundles: Vec<u32>, added: bool, body: Body) -> Result<()> {
        let index = self.m.add_payload(bytes)?;
        self.m.entries.push(Entry {
            name: name.to_owned(),
            stored: Some(Stored {
                index,
                size: bytes.len() as u64,
                flags: if added { 8 } else { 0 },
                handler: 0,
                user: if added { USER.to_owned() } else { String::new() },
            }),
            bundles,
            body,
        });
        Ok(())
    }

    /// Adds one item: its clones, resources and chunk. Returns the item's and its prefab's imports.
    fn item(&mut self, item: &Item) -> Result<(ImportReference, ImportReference)> {
        let d = self.donor;
        let folder = d.item.name.rsplit_once('/').map_or("", |(f, _)| f);
        let item_name = format!("{folder}/{}", item.key);
        let renamed = |n: &str| {
            let lower = n.to_ascii_lowercase();
            lower.replacen(&d.key.to_ascii_lowercase(), &item.key, 1)
        };
        // the GUIDs the clone defines, and their new values
        let mut docs: Vec<Named> = std::iter::once(d.item.clone())
            .chain(d.prefabs.iter().cloned())
            .chain([
                d.mesh_component.clone(),
                d.mesh.clone(),
                d.collision.clone(),
                d.variation_db.clone(),
            ])
            .collect();
        let mut old = BTreeSet::new();
        for n in &docs {
            old.extend(remap::defined(&n.doc));
        }
        for n in &d.prefabs {
            old.extend(guids_named(&n.doc, "EntityGuid"));
            old.extend(source_partition(&n.doc));
        }
        // a component segment is named by its component type's GUID: that stays
        for n in &docs {
            for g in guids_named(&n.doc, "ComponentTypeGuid") {
                old.remove(&g);
            }
        }
        let map: HashMap<[u8; 16], [u8; 16]> = old
            .iter()
            .map(|g| {
                (
                    *g,
                    identity(&format!("primase-quickdrop/{}/{}", item.key, guid_text(g))),
                )
            })
            .collect();
        for n in &mut docs {
            remap::remap(&mut n.doc, &map);
        }
        let [it, rest @ ..] = docs.as_mut_slice() else {
            return Err(bad("no item document"));
        };
        let (prefabs, rest) = rest.split_at_mut(d.prefabs.len());
        let [component, mesh, collision, variation_db] = rest else {
            return Err(bad("clone set"));
        };
        // names
        let meshset_rid = resource_id(&item.mesh_name);
        let collision_name = format!(
            "{}.{}.ecsphysics",
            item.mesh_name.trim_end_matches("_mesh"),
            guid_text(&collision.doc.file_guid)
        );
        let collision_rid = resource_id(&collision_name);
        it.name = item_name.clone();
        let o = object_mut(&mut it.doc, "DelMarBaseItemAsset")?;
        set(o, "Name", text(&item_name))?;
        set(o, "Key", text(&item.key))?;
        set(o, "HashedAssetKey", Value::UInt(u64::from(djb2(&item.key))))?;
        let o = object_mut(&mut it.doc, "BuildKitItemData")?;
        set(o, "DisplayName", text(&item.title))?;
        set(o, "BuildKitPrefabName", text(&renamed(&d.prefabs[0].name)))?;
        for p in prefabs.iter_mut() {
            p.name = renamed(&p.name);
            set(
                object_mut(&mut p.doc, "EcsRuntimePrefabAsset")?,
                "Name",
                text(&p.name),
            )?;
        }
        component.name = format!("{}_ecsmeshcomponentasset", item.key);
        let o = object_mut(&mut component.doc, "EcsMeshComponentAsset")?;
        set(o, "Name", text(&component.name))?;
        let (lo, hi) = bounds(&item.meshset);
        match o.field_mut("PartBoundingBoxes") {
            Some(Value::Array(boxes, _)) if boxes.len() == 1 => set_box(&mut boxes[0], lo, hi)?,
            _ => return Err(bad("the mesh component has not one part bounding box")),
        }
        mesh.name = item.mesh_name.clone();
        let o = object_mut(&mut mesh.doc, "RigidMeshAsset")?;
        set(o, "Name", text(&item.mesh_name))?;
        set(o, "NameHash", Value::UInt(u64::from(djb2(&item.mesh_name))))?;
        set(o, "MeshSetResource", Value::Resource(meshset_rid))?;
        collision.name = collision_name.clone();
        let o = object_mut(&mut collision.doc, "EcsPhysicsAsset")?;
        set(o, "Name", text(&collision_name))?;
        set(o, "Resource", Value::Resource(collision_rid))?;
        if let Some(m) = item.volume_moments {
            set_moments(o, &m)?;
        }
        variation_db.name = format!("{folder}/{}_ecs_bundle/meshvariationdb_win32", item.key);
        set(
            object_mut(&mut variation_db.doc, "MeshVariationDatabase")?,
            "Name",
            text(&variation_db.name),
        )?;
        // the MeshSet named after its asset, its chunk named from it
        let mut ms = item.meshset.clone();
        let chunk_guid = identity(&format!("{}/chunk", item.mesh_name));
        name_meshset(&mut ms, &item.mesh_name, chunk_guid)?;
        let (ms_bytes, ms_meta) = meshset::write(&ms)?;
        let collision_bytes = physics::write(&item.collision)?;
        // entries
        let (ecs, items) = (self.ecs.clone(), self.items.clone());
        let item_import = import_of(&it.doc)?;
        let prefab_import = import_of(&prefabs[0].doc)?;
        let bytes = written(&it.doc, &old, &it.name)?;
        self.entry(&it.name.clone(), &bytes, items, true, Body::Ebx)?;
        for n in prefabs
            .iter()
            .chain([&*component, &*mesh, &*collision, &*variation_db])
        {
            let bytes = written(&n.doc, &old, &n.name)?;
            self.entry(&n.name, &bytes, ecs.clone(), true, Body::Ebx)?;
        }
        let res = |res_type, rid, meta: &[u8]| {
            Body::Res(ResInfo {
                res_type,
                rid,
                meta: meta.to_vec(),
            })
        };
        self.entry(
            &item.mesh_name,
            &ms_bytes,
            ecs.clone(),
            true,
            res(MESHSET, meshset_rid, &ms_meta),
        )?;
        self.entry(
            &collision_name,
            &collision_bytes,
            ecs.clone(),
            true,
            res(physics::RTYPE, collision_rid, &[0; 16]),
        )?;
        let chunk = Body::Chunk(ChunkInfo {
            range_start: 0,
            range_end: 0,
            logical_offset: 0,
            logical_size: u32::try_from(item.chunk.len()).map_err(|_| bad("chunk past 4 GiB"))?,
            h32: 0,
            first_mip: -1,
            added_bundles: Vec::new(),
        });
        self.entry(&guid_text(&chunk_guid), &item.chunk, ecs, true, chunk)?;
        Ok((item_import, prefab_import))
    }
}

/// The MeshSet's bounds (header 0x10 and 0x20).
fn bounds(m: &MeshSet) -> ([f64; 3], [f64; 3]) {
    let f = |at: usize| {
        f64::from(f32::from_le_bytes(
            m.header
                .get(at..at + 4)
                .and_then(|b| b.try_into().ok())
                .unwrap_or_default(),
        ))
    };
    ([f(0x10), f(0x14), f(0x18)], [f(0x20), f(0x24), f(0x28)])
}

/// Sets an `AxisAlignedBox` object's `min` and `max`.
fn set_box(b: &mut Value, lo: [f64; 3], hi: [f64; 3]) -> Result<()> {
    let Value::Object(o) = b else {
        return Err(bad("a bounding box is not an object"));
    };
    for (field, v) in [("min", lo), ("max", hi)] {
        let Some(Value::Object(p)) = o.field_mut(field) else {
            return Err(bad(format!("bounding box has no {field}")));
        };
        for (k, c) in ["x", "y", "z"].iter().zip(v) {
            set(p, k, Value::Float(c))?;
        }
    }
    Ok(())
}

/// Sets a `VolumeMoments` Mat4 (`m11`..`m44`, row-major).
fn set_moments(o: &mut ebx::Object, m: &[f64; 16]) -> Result<()> {
    let Some(Value::Object(mat)) = o.field_mut("VolumeMoments") else {
        return Err(bad("no VolumeMoments"));
    };
    for (i, v) in m.iter().enumerate() {
        set(mat, &format!("m{}{}", i / 4 + 1, i % 4 + 1), Value::Float(*v))?;
    }
    Ok(())
}

/// Names the MeshSet after its asset `name` as the game names its own (the header's name and last part, each LOD's
/// `Mesh:<name>_lod<n>` with its path and last part, the name hashes) and points its LOD at the chunk `chunk`. Only a
/// one-LOD MeshSet: the item writes one chunk.
fn name_meshset(m: &mut MeshSet, name: &str, chunk: [u8; 16]) -> Result<()> {
    if m.lods.len() != 1 {
        return Err(bad(format!(
            "a MeshSet of {} LODs: the item writes one chunk",
            m.lods.len()
        )));
    }
    let short = name.rfind('/').map_or(0, |i| i + 1);
    m.name = name.to_owned();
    m.short_name = meshset::NamePart::At(short);
    m.header[0x78..0x7c].copy_from_slice(&djb2(name).to_le_bytes());
    for (l, lod) in m.lods.iter_mut().enumerate() {
        lod.name = format!("Mesh:{name}_lod{l}");
        lod.name_parts = [meshset::NamePart::At(5), meshset::NamePart::At(5 + short)];
        lod.record[0xa4..0xa8].copy_from_slice(&djb2(&lod.name[5..]).to_le_bytes());
        lod.chunk = chunk;
    }
    Ok(())
}

/// The pack: `items` cloned from `donor`, and the donor's item master collection and bundle network registry with
/// each item and its prefab registered. `title` and `author` head the mod. Refused: no item, two items with one key or
/// one mesh name, an item whose clone cannot be written (the module documentation's checks).
pub fn pack(donor: &Donor, items: &[Item], title: &str, author: &str) -> Result<Fbmod> {
    if items.is_empty() {
        return Err(bad("a pack of no items"));
    }
    let keys: BTreeSet<_> = items.iter().map(|i| i.key.as_str()).collect();
    let meshes: BTreeSet<_> = items.iter().map(|i| i.mesh_name.as_str()).collect();
    if keys.len() != items.len() || meshes.len() != items.len() {
        return Err(bad("two items share a key or a mesh name"));
    }
    let embedded = ["Icon", "Screenshot0", "Screenshot1", "Screenshot2", "Screenshot3"].map(|n| Entry {
        name: n.to_owned(),
        stored: None,
        bundles: Vec::new(),
        body: Body::Embedded,
    });
    let mut w = Writer {
        m: Fbmod {
            version: frostbite_formats::fbmod::VERSION,
            profile: PROFILE.to_owned(),
            game_version: GAME_VERSION,
            title: title.to_owned(),
            author: author.to_owned(),
            category: String::new(),
            mod_version: "1.0".to_owned(),
            description: String::new(),
            link: String::new(),
            entries: embedded.into(),
            payloads: Vec::new(),
        },
        donor,
        ecs: vec![bundle_hash(&donor.ecs_bundle)],
        items: vec![bundle_hash(ITEMS_BUNDLE)],
    };
    let mut collection = donor.collection.clone();
    let mut registry = donor.registry.clone();
    for item in items {
        let (item_import, prefab_import) = w.item(item)?;
        append_import(
            &mut collection.doc,
            "DelMarItemCollectionAsset",
            "Items",
            item_import,
        )?;
        append_import(
            &mut registry.doc,
            "NetworkRegistryAsset",
            "Objects",
            prefab_import,
        )?;
    }
    let none = BTreeSet::new();
    let bytes = written(&collection.doc, &none, &collection.name)?;
    let items_bundle = w.items.clone();
    w.entry(&collection.name, &bytes, items_bundle, false, Body::Ebx)?;
    let bytes = written(&registry.doc, &none, &registry.name)?;
    let ecs = w.ecs.clone();
    w.entry(&registry.name, &bytes, ecs, false, Body::Ebx)?;
    Ok(w.m)
}
