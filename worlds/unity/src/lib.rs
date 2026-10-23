//! Unity players' data files read into the world.
//!
//! The reader (`read/`, its modules re-exported here):
//! - [`serialized`]: the SerializedFile container (`globalgamemanagers`, `*.assets`, `levelN`): header, types, the
//!   object table, script references, external files. Strict, and version-gated: only container versions verified
//!   against real data are read.
//! - [`tpk`]: a type tree package (the TPK format), the class layouts per Unity version that a player build strips
//!   from its files. The caller supplies the package; none is bundled.
//! - [`typetree`]: decoding an object's bytes by its class's type tree into a [`Value`] tree.
//! - [`resource`]: streamed resources (`.resS`), the bulk data that objects point at.
//! - [`player`]: a player's data folder: its files, pointers resolved across them.
//! - [`scene`]: a scene's hierarchy (GameObjects, Transforms to world matrices) and its drawn mesh renderers.
//! - [`level`]: a scene into the world, Unity's frame converted; [`components`] the `unity.*` dialect. Its report
//!   serialises ([`level::LevelReport::to_json`]).
//! - [`convert`]: a scene as a conversion's reader ([`convert::UnityReader`]: the level read, a part its roots' closure).
//! - [`listing`]: a player's objects by class and name from its object tables, the names read from the objects' first
//!   bytes; [`player::Player::scenes`] the build's scene list (`levelN` to scene name).
//! - [`asset`]: one mesh, texture or material read into a world as a preview.
//! - [`mesh`]: meshes (vertex streams, compressed meshes, index buffers, submeshes).
//! - [`texture`]: textures (formats, mip chains, rows flipped top first, normal map conventions).
//! - [`material`]: materials as stored (shader, saved properties, keywords).
//! - [`program`]: a shader's compiled programs (variants, constant-buffer layouts, bind slots, code).
//! - [`terrain`]: terrains as stored (heightmap, holes, splat layers and alphamaps, trees).
//!
//! [`shader`]: a Shader's render state as stored, which the reader decodes, `unity.material` carries and the passes
//! choose by.
//!
//! [`bind`]: a variant of a material's compiled programs (`unity.programs`, [`program::read_programs`]) bound for
//! evaluation on the CPU: its constant layout, the material's properties and Unity's built-in lighting globals in it,
//! its textures as Unity samples them.
//!
//! The passes ([`passes`], each module re-exported here):
//! - [`drawn`]: what a renderer other than Unity's can draw of the scene's renderers (pass `unity-drawn`).
//! - [`lower`]: materials lowered to `pbr.material` by shader.
//! - [`lower_terrain`]: terrains lowered to geometry and a base colour image.
//! - [`lower_collision`]: colliders and terrain colliders lowered to collision geometry.
//! - [`lower_lights`]: lights lowered to `core.light`.
//! - [`lower_ambient`]: the scene's ambient lighting lowered to `core.environment`'s ambient.
//! - [`resolve_shaders`]: a material's programs and what they read, as `core.shader` and `core.shader_binding`.
//! - [`register`]: the passes by name.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bind;
pub mod components;
pub mod passes;
mod read;
pub mod shader;

pub use passes::{
    drawn, lower, lower_ambient, lower_collision, lower_lights, lower_terrain, resolve_shaders,
};
pub use read::serialized::{ObjectInfo, SerializedFile};
pub use read::tpk::TypeTreePackage;
pub use read::typetree::TypeTree;
pub use read::value::Value;
pub use read::version::UnityVersion;
pub use read::{
    asset, clip, convert, level, listing, material, mesh, player, program, resource, scene, serialized,
    terrain, texture, tpk, typetree, value, version,
};

/// Registers the Unity passes by name: `unity-drawn`, `lower-unity-pbr`, `lower-unity-terrain`,
/// `lower-unity-collision`, `lower-unity-lights` and `lower-unity-ambient`. All but the last take no option: each
/// lowers by Unity's own rules (its module says which), not by a choice a program makes; an option is refused.
/// `lower-unity-ambient` requires the program's choice for an ambient whose probe is not read (`probe`, no default).
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
    reg.register_requiring(
        "resolve-unity-shaders",
        |o| Ok(Box::new(resolve_shaders::ResolveUnityShaders::from_options(o)?)),
        &[("tier", "0"), ("keywords", "none"), ("colour-space", "linear")],
    );
    reg.register_requiring(
        "lower-unity-ambient",
        |o| Ok(Box::new(lower_ambient::LowerUnityAmbient::from_options(o)?)),
        &[("probe", "omit")],
    );
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
