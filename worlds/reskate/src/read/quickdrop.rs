//! A stock Quick Drop item read from the game's data folder ([`frostbite_formats::folder`]): the I/O of
//! [`crate::game::quickdrop`], which describes what is read.

use frostbite_formats::ebx::{self, Document, Value};
use frostbite_formats::folder::Source;
use frostbite_formats::listing::Bundle;
use frostbite_formats::{Error, Result, meshset, physics};

use crate::game::quickdrop::{Donor, ECS_TOC, ITEMS_BUNDLE, ITEMS_TOC, MASTER_COLLECTION, Named, Res};

fn missing(what: impl Into<String>) -> Error {
    Error::Missing(what.into())
}

/// The string value of `field` of the first instance of type `ty` in `doc`.
fn string(doc: &Document, ty: &str, field: &str) -> Result<String> {
    doc.instances
        .iter()
        .enumerate()
        .filter(|(i, _)| doc.type_name(*i) == Some(ty))
        .find_map(|(_, inst)| match inst.object.as_ref()?.field(field)? {
            Value::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
            _ => None,
        })
        .ok_or_else(|| missing(format!("{ty}.{field} in {}", doc.root_type)))
}

/// The resource id of `field` of `doc`'s root instance.
fn resource_of(doc: &Document, field: &str) -> Result<u64> {
    doc.instances
        .first()
        .and_then(|i| i.object.as_ref())
        .and_then(|o| match o.field(field)? {
            Value::Resource(r) => Some(*r),
            _ => None,
        })
        .ok_or_else(|| missing(format!("{}.{field}", doc.root_type)))
}

/// One bundle's EBX documents and resources, read on demand.
struct Opened<'a, 's> {
    src: &'a Source<'s>,
    bundle: Bundle,
}

impl Opened<'_, '_> {
    fn ebx(&self, name: &str) -> Result<Named> {
        let m = &self.bundle.manifest;
        let i = (m.ebx.iter())
            .position(|e| e.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| missing(format!("{} has no EBX {name}", self.bundle.name)))?;
        let e = self.bundle.ebx(i);
        Ok(Named {
            name: e.name.clone(),
            doc: ebx::read(&self.src.payload(e.file, e.size as usize)?)?,
        })
    }

    /// The EBX document of file GUID `guid`, if the bundle holds it.
    fn by_guid(&self, guid: &[u8; 16]) -> Result<Option<Named>> {
        for i in 0..self.bundle.manifest.ebx.len() {
            let e = self.bundle.ebx(i);
            let bytes = self.src.payload(e.file, e.size as usize)?;
            if ebx::read_file_guid(&bytes)? == *guid {
                return Ok(Some(Named {
                    name: e.name,
                    doc: ebx::read(&bytes)?,
                }));
            }
        }
        Ok(None)
    }

    fn resource(&self, rid: u64) -> Result<Res> {
        let m = &self.bundle.manifest;
        let i = (m.res.iter())
            .position(|r| r.rid == rid)
            .ok_or_else(|| missing(format!("{} has no resource {rid:#x}", self.bundle.name)))?;
        let e = self.bundle.resource(i);
        Ok(Res {
            name: e.name.clone(),
            rid,
            meta: m.res[i].rmeta,
            bytes: self.src.payload(e.file, e.size as usize)?,
        })
    }
}

/// The stock item `item` (its document's name, `items/<folder>/<key>`) and its documents, from the game's data.
/// Refused, naming what: an item without its build-kit data, a closure member missing from its bundle, a prefab
/// importing no mesh component or no physics asset holding a triangle mesh (or more than one).
pub fn read_donor(src: &Source<'_>, item: &str) -> Result<Donor> {
    let items = Opened {
        src,
        bundle: src.bundle(ITEMS_TOC, ITEMS_BUNDLE)?,
    };
    let item = items.ebx(item)?;
    let collection = items.ebx(MASTER_COLLECTION)?;
    let key = string(&item.doc, "DelMarBaseItemAsset", "Key")?;
    let ecs_bundle = format!(
        "win32/{}",
        string(&item.doc, "BuildKitItemData", "BuildKitBundleName")?.to_ascii_lowercase()
    );
    let prefab_name = string(&item.doc, "BuildKitItemData", "BuildKitPrefabName")?;
    let ecs = Opened {
        src,
        bundle: src.bundle(ECS_TOC, &ecs_bundle)?,
    };
    let stem = key.to_ascii_lowercase();
    let mut prefabs = vec![ecs.ebx(&prefab_name)?];
    let (mut mesh_component, mut collision) = (None, Vec::new());
    let mut i = 0;
    while i < prefabs.len() {
        let imports = prefabs[i].doc.imports.clone();
        for imp in imports {
            if prefabs.iter().any(|p| p.doc.file_guid == imp.file_guid) {
                continue;
            }
            let Some(d) = ecs.by_guid(&imp.file_guid)? else {
                continue;
            };
            match d.doc.root_type.as_str() {
                "EcsRuntimePrefabAsset" if d.name.to_ascii_lowercase().contains(&stem) => prefabs.push(d),
                "EcsMeshComponentAsset" if i == 0 => mesh_component = Some(d),
                "EcsPhysicsAsset" if i == 0 => {
                    let r = ecs.resource(resource_of(&d.doc, "Resource")?)?;
                    let p = physics::read(&r.bytes).map_err(|e| missing(format!("{}: {e}", d.name)))?;
                    let mesh = p.parts.iter().any(|q| q.kind == physics::KIND_MESH);
                    collision.push((d, r, mesh));
                }
                _ => {}
            }
        }
        i += 1;
    }
    let mesh_component =
        mesh_component.ok_or_else(|| missing(format!("{prefab_name} imports no mesh component")))?;
    // the only physics asset, or of several the one holding a triangle mesh
    if collision.len() > 1 {
        collision.retain(|c| c.2);
    }
    let [(collision, collision_resource, _)]: [(Named, Res, bool); 1] =
        collision.try_into().map_err(|c: Vec<_>| {
            missing(format!(
                "{prefab_name} imports {} physics assets that could be its collision, not one",
                c.len()
            ))
        })?;
    let mesh = mesh_component
        .doc
        .imports
        .iter()
        .map(|imp| ecs.by_guid(&imp.file_guid))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .find(|d| d.doc.root_type == "RigidMeshAsset")
        .ok_or_else(|| missing(format!("{} names no RigidMeshAsset", mesh_component.name)))?;
    let meshset = ecs.resource(resource_of(&mesh.doc, "MeshSetResource")?)?;
    let m = meshset::read_resource(&meshset.bytes, &meshset.meta)?;
    let lod0 = m
        .lods
        .first()
        .ok_or_else(|| missing(format!("{}: no LOD", meshset.name)))?;
    let (file, size) = ecs.bundle.chunk(src, &lod0.chunk)?;
    let chunk = (lod0.chunk, src.payload(file, size)?);
    let bundle_path = ecs_bundle.trim_start_matches("win32/");
    let variation_db = ecs.ebx(&format!("{bundle_path}/meshvariationdb_win32"))?;
    let registry = ecs.ebx(&format!("{bundle_path}_networkregistry_win32"))?;
    Ok(Donor {
        item,
        key,
        ecs_bundle,
        prefabs,
        mesh_component,
        mesh,
        meshset,
        chunk,
        collision,
        collision_resource,
        variation_db,
        registry,
        collection,
    })
}
