//! The Frostbite containers a post-compile step touches, as skate. (ReSkate) reads them: superbundle TOCs, bundle
//! regions and manifests, the native DB, data folders and CAS archives, shader block depots and the shader lookup
//! tables of `dingolevel_root`.

pub mod bundle;
pub mod db;
pub mod depot;
pub mod folder;
pub mod lookup;
pub mod toc;

/// Resource type of a MeshSet.
pub const MESHSET: u32 = 0x49B1_56D4;
/// Resource type of a texture.
pub const TEXTURE: u32 = 0x6BDE_20BA;
