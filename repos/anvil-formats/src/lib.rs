//! Readers of the Anvil engine's formats: bytes in, plain records out.
//!
//! - **Containers.** A `.forge` archive of version 26 or 27 ([`forge`]): its index, each entry's offset, size and
//!   ids as stored, and each entry's name record. An entry's data blocks ([`data`]): compression version 2 (zstd) or
//!   version 1 (LZO2A), the Adler-32 checksum checked, the resource's header parsed and its body kept as stored. Several
//!   forges searched as one ([`archive`]), the entries listed by kind ([`listing`]), and the objects of a version-26
//!   entry ([`objects`]).
//! - **Resources.** The bodies the forges hold, each read into a record as stored: Entities and EntityGroups
//!   ([`entity`]), meshes and their vertex layouts ([`mesh`], [`vertex`]), materials, texture sets and maps
//!   ([`material`], [`texture`], [`mip`]), material templates and their shader permutations ([`template`],
//!   [`shader_pack`], [`shader_source`]), LOD selectors ([`lod`]), collision shapes ([`shape`], [`mesh_shape`],
//!   [`terrain_material`]), skeletons, rigs and animation ([`skeleton`], [`rig`], [`animation`]), build tables
//!   ([`build_table`], [`rider`]), terrain node layers and their compressed code ([`tnl`], [`tnl_code`]), floor
//!   heights ([`floor`]), world grids ([`grid`]), grass ([`grass`]), water ([`water`]), decals ([`ddecal`]),
//!   impostors ([`impostor`]), lights, sky and looks ([`light`], [`sky`], [`look`], [`controller`], [`gidyn`]).
//! - **Shaun White Skateboarding** ([`shaunwhite`]): its scene entities, collision, markers, grind guidance,
//!   lights and materials as its version-26 forges store them, and its SDB2 shader databases.
//!
//! Every read is checked: a short or malformed body is refused with an error that names the field and its offset,
//! never a panic. Records keep what the reader doesn't interpret as stored bytes.
//!
//! Where a grammar depends on the game's class registry (an Entity's components without a length rule are found by
//! their headers' component types), the caller supplies that registry: see [`entity::StoredEntity::parse`].
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod animation;
pub mod archive;
pub mod build_table;
pub mod controller;
pub mod data;
pub mod ddecal;
pub mod entity;
pub mod fields;
pub mod floor;
pub mod forge;
pub mod gidyn;
pub mod grass;
pub mod grid;
pub mod impostor;
pub mod light;
pub mod listing;
pub mod lod;
pub mod look;
pub mod material;
pub mod mesh;
pub mod mesh_shape;
pub mod mip;
pub mod objects;
pub mod reference;
pub mod rider;
pub mod rig;
pub mod shader_pack;
pub mod shader_source;
pub mod shape;
pub mod shaunwhite;
pub mod skeleton;
pub mod sky;
pub mod template;
pub mod terrain_material;
pub mod texture;
pub mod tnl;
pub mod tnl_code;
pub mod vertex;
pub mod water;
