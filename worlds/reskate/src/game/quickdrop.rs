//! A stock Quick Drop item and the documents it is made of, read from the game ([`crate::read::quickdrop::read_donor`]): what a new item is
//! cloned from ([`crate::write::quickdrop`]).
//!
//! An item is a `DelMarBaseItemAsset` in `Win32/items.toc` (bundle `win32/characters/customization/configs/
//! cas_main_sharedbundle`) whose `BuildKitItemData` names the ECS bundle in `Win32/buildkititems.toc` it lives in
//! (`BuildKitBundleName`) and its prefab there (`BuildKitPrefabName`). From that prefab, through the documents'
//! imports, the item's own documents are (as the stock items and the community packs that clone them have them):
//! - the prefab, and the `EcsRuntimePrefabAsset`s it reaches whose names hold the item's key (its `.nopolicy`
//!   variant and base prefabs: under `items/` for a rail, the base one beside the mesh for a pad);
//! - the `EcsMeshComponentAsset` the prefab imports, and the `RigidMeshAsset` that names (its `MeshSetResource`, the
//!   MeshSet, and the MeshSet's geometry chunk);
//! - the item's collision: the `EcsPhysicsAsset` the prefab imports when it imports one (a pad's convex box), else
//!   the one whose PhysicsResource holds a triangle mesh (kind 5; a rail's, beside convex boxes along it). The
//!   prefab's other physics assets and every other import (shared prefabs, the object variation, the mesh's LOD
//!   group, textures) are stock content a clone keeps pointing at.
//!
//! With them come what an item's registration edits: the bundle's `MeshVariationDatabase` (`<bundle>/
//! meshvariationdb_win32`), its `NetworkRegistryAsset` (`<bundle>_networkregistry_win32`), and the item master
//! collection (`items/itemmastercollection`).

use frostbite_formats::ebx::Document;

/// The TOCs and the bundle the item documents lie in.
pub const ITEMS_TOC: &str = "Win32/items.toc";
pub const ITEMS_BUNDLE: &str = "win32/characters/customization/configs/cas_main_sharedbundle";
pub const ECS_TOC: &str = "Win32/buildkititems.toc";
pub const MASTER_COLLECTION: &str = "items/itemmastercollection";

/// A document of the game, by its name in its bundle's manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub name: String,
    pub doc: Document,
}

/// A resource of the game: its manifest name, id, metadata and bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Res {
    pub name: String,
    pub rid: u64,
    pub meta: [u8; 16],
    pub bytes: Vec<u8>,
}

/// A stock Quick Drop item read whole (the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct Donor {
    /// The item document (`items/<folder>/<key>`).
    pub item: Named,
    /// The item's `Key`.
    pub key: String,
    /// The ECS bundle, as the TOC names it (`win32/items/...`).
    pub ecs_bundle: String,
    /// The prefab (`BuildKitPrefabName`), then the item's other prefabs, in the order the imports reach them.
    pub prefabs: Vec<Named>,
    pub mesh_component: Named,
    pub mesh: Named,
    pub meshset: Res,
    /// The MeshSet's LOD-0 geometry chunk: its GUID and bytes.
    pub chunk: ([u8; 16], Vec<u8>),
    pub collision: Named,
    pub collision_resource: Res,
    pub variation_db: Named,
    pub registry: Named,
    pub collection: Named,
}
