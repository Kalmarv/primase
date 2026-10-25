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
//! - [`lower_animator`]: Animator controllers lowered to `core.controller` and `core.controller_binding`.
//! - [`resolve_shaders`]: a material's programs and what they read, as `core.shader` and `core.shader_binding`.
//! - [`register`]: the passes by name.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod animator;
pub mod bind;
pub mod blend_tree;
pub mod clipdata;
pub mod components;
pub mod humanoid;
pub mod passes;
mod read;
pub mod shader;
pub mod state_pose;

pub use passes::{
    draw_keywords, drawn, lower, lower_ambient, lower_animator, lower_collision, lower_lights,
    lower_terrain, resolve_shaders,
};
pub use read::serialized::{ObjectInfo, SerializedFile};
pub use read::tpk::TypeTreePackage;
pub use read::typetree::TypeTree;
pub use read::value::Value;
pub use read::version::UnityVersion;
pub use read::{
    asset, clip, convert, level, listing, material, mesh, pipeline, player, program, resource, scene,
    serialized, terrain, texture, tpk, typetree, value, version,
};

/// Registers the Unity passes by their declarations: `unity-drawn`, `lower-unity-pbr`, `lower-unity-terrain`,
/// `lower-unity-collision`, `lower-unity-lights`, `lower-unity-animator`, `resolve-unity-shaders` and
/// `lower-unity-ambient`. All but `resolve-unity-shaders`, `lower-unity-lights` and `lower-unity-ambient` take no
/// option: each lowers by Unity's own rules (its module says which), not by a choice a program makes.
/// `resolve-unity-shaders` requires the variant and colour space; `lower-unity-lights` the calibration of URP's unitless
/// intensity (`candela-per-unit`, no default); `lower-unity-ambient` that same calibration and the program's choice for
/// an ambient whose probe is not read (`probe`, no default).
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    for spec in [
        &drawn::UNITY_DRAWN,
        &lower::LOWER_UNITY_PBR,
        &lower_terrain::LOWER_UNITY_TERRAIN,
        &lower_collision::LOWER_UNITY_COLLISION,
        &lower_lights::LOWER_UNITY_LIGHTS,
        &lower_animator::LOWER_UNITY_ANIMATOR,
        &resolve_shaders::RESOLVE_UNITY_SHADERS,
        &lower_ambient::LOWER_UNITY_AMBIENT,
    ] {
        reg.add(spec);
    }
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

#[cfg(test)]
mod tests {
    use primase_core::pass::{Options, PassRegistry};

    /// The crate registers exactly these passes by their declarations: those with required options are made only with
    /// them, and every one says what it takes.
    #[test]
    fn the_unity_passes_register_by_name() {
        let mut reg = PassRegistry::new();
        super::register(&mut reg);
        let names = [
            "lower-unity-ambient",
            "lower-unity-animator",
            "lower-unity-collision",
            "lower-unity-lights",
            "lower-unity-pbr",
            "lower-unity-terrain",
            "resolve-unity-shaders",
            "unity-drawn",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        let required = [
            "lower-unity-ambient",
            "lower-unity-lights",
            "resolve-unity-shaders",
        ];
        for name in names {
            assert_eq!(reg.requires_options(name), required.contains(&name), "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            assert_eq!(
                reg.make(name, &Options::new()).is_err(),
                required.contains(&name),
                "{name}"
            );
        }
    }
}
