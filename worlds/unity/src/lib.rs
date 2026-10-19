//! Unity players' data files read into the world.
//!
//! The reader (`read/`, its modules re-exported here):
//! - [`serialized`]: the SerializedFile container (`globalgamemanagers`, `*.assets`, `levelN`): header, types,
//!   the object table, script references, external files. Strict, and version-gated: only container versions
//!   verified against real data are read.
//! - [`tpk`]: a type tree package (the TPK format), the class layouts per Unity version that a player
//!   build strips from its files. The caller supplies the package; none is bundled.
//! - [`typetree`]: decoding an object's bytes by its class's type tree into a [`Value`] tree.
//! - [`resource`]: streamed resources (`.resS`), the bulk data that objects point at.
//! - [`player`]: a player's data folder: its files, pointers resolved across them.
//! - [`scene`]: a scene's hierarchy (GameObjects, Transforms to world matrices) and its drawn mesh renderers.
//! - [`level`]: a scene into the world, Unity's frame converted; [`components`] the `unity.*` dialect.
//! - [`mesh`]: meshes (vertex streams, compressed meshes, index buffers, submeshes).
//! - [`texture`]: textures (formats, mip chains, rows flipped top first, normal map conventions).
//! - [`material`]: materials as stored (shader, saved properties, keywords).
//! - [`program`]: a shader's compiled programs (variants, constant-buffer layouts, bind slots, code).
//! - [`terrain`]: terrains as stored (heightmap, holes, splat layers and alphamaps, trees).
//!
//! The passes:
//! - [`drawn`]: what a renderer other than Unity's can draw of the scene's renderers (pass `unity-drawn`).
//! - [`lower`]: materials lowered to `pbr.material` by shader.
//! - [`lower_terrain`]: terrains lowered to geometry and a base colour image.
//! - [`lower_collision`]: colliders and terrain colliders lowered to collision geometry.
//! - [`lower_lights`]: lights lowered to `core.light`.
//! - [`register`]: the passes by name.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod drawn;
pub mod level;
pub mod lower;
pub mod lower_lights;
pub mod material;
pub mod passes;
mod read;

pub use passes::{lower_collision, lower_terrain};
pub use read::serialized::{ObjectInfo, SerializedFile};
pub use read::tpk::TypeTreePackage;
pub use read::typetree::TypeTree;
pub use read::value::Value;
pub use read::version::UnityVersion;
pub use read::{
    mesh, player, program, resource, scene, serialized, terrain, texture, tpk, typetree, value, version,
};

/// Registers the Unity passes by name: `unity-drawn`, `lower-unity-pbr`, `lower-unity-terrain`,
/// `lower-unity-collision` and `lower-unity-lights`. None takes an option: each lowers by Unity's own rules (its
/// module says which), not by a choice a program makes; an option is refused.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    use primase_core::pass::{Options, Pass};
    fn none(o: &Options, pass: impl Pass + 'static) -> std::result::Result<Box<dyn Pass>, String> {
        match o.keys().next() {
            Some(k) => Err(format!("unknown option {k}")),
            None => Ok(Box::new(pass)),
        }
    }
    reg.register("unity-drawn", |o| none(o, drawn::UnityDrawn));
    reg.register("lower-unity-pbr", |o| none(o, lower::LowerUnityPbr));
    reg.register("lower-unity-terrain", |o| {
        none(o, lower_terrain::LowerUnityTerrain)
    });
    reg.register("lower-unity-collision", |o| {
        none(o, lower_collision::LowerUnityCollision)
    });
    reg.register("lower-unity-lights", |o| none(o, lower_lights::LowerUnityLights));
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("{}: {source}", path.display())]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}
