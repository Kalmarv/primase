//! Unity players' data files read into the world.
//!
//! The files are read by `unity_formats` (SerializedFile, type trees, TPK, streamed resources, a scene's hierarchy,
//! materials, shader state, terrains, the pipeline's settings). The reader modules here (`read/`, re-exported):
//! - [`level`]: a scene into the world, Unity's frame converted; [`components`] the `unity.*` dialect. Its report
//!   serialises ([`level::LevelReport::to_json`]).
//! - [`convert`]: a scene as a conversion's reader ([`convert::UnityReader`]: the level read, a part its roots' closure).
//! - [`asset`]: one mesh, texture or material read into a world as a preview.
//! - [`mesh`]: meshes (vertex streams, compressed meshes, index buffers, submeshes).
//! - [`texture`]: textures (formats, mip chains, rows flipped top first, normal map conventions).
//! - [`program`]: a shader's compiled programs (variants, constant-buffer layouts, bind slots, code).
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
#[cfg(test)]
mod fixture;
pub mod humanoid;
pub mod passes;
mod read;
pub mod state_pose;

pub use passes::{
    draw_keywords, drawn, lower, lower_ambient, lower_animator, lower_collision, lower_lights, lower_terrain,
    resolve_shaders,
};
pub use read::{asset, clip, convert, level, mesh, program, texture};

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

pub(crate) fn invalid(msg: impl Into<String>) -> unity_formats::Error {
    unity_formats::Error::Invalid(msg.into())
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
