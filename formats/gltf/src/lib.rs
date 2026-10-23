//! Our own glTF 2.0 / GLB writer from the world, and the passes that prepare a world for it.
//!
//! - [`write()`] / [`write_file`]: a [`primase_core::World`] to one GLB (prototypes as shared meshes, placements as
//!   nodes, `pbr.material`s, embedded PNGs, dialect components in extras); see [`write`](mod@write) for the rules.
//! - [`prepare`]: `prepare-gltf-materials` and `fill-missing-texcoords`.
//! - [`read`](mod@read): a glTF document into a world of one asset (skeletons, skins, morphs, placements, clips).
//! - [`preview`]: the preview export, GLBs plus a documented manifest of the choices they leave open, and the recipe a
//!   site hands back.
//! - [`parts`]: the writer in a conversion, its [`target`] and [`GlbParts`] (one GLB per part: one per object).
//!
//! The GLB container, the strict reader and the vertex encoding are `primase-glb`'s.
//!
//! Frames: the world frame is glTF's (metres, right-handed, +Y up, -Z forward), so nothing is converted.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod lower;
pub mod parts;
pub mod prepare;
pub mod preview;
pub mod read;
mod rig;
pub mod textures;
pub mod write;

use std::path::PathBuf;

use primase_core::Entity;

pub use parts::{GlbParts, preview_target, target};
pub use preview::{Manifest, PreviewParts, Recipe};
pub use read::{ReadReport, read_glb, read_with_images};
pub use textures::{GltfMaterialTextures, GltfSlot, ImageResolver, SlotTexture};
pub use write::{
    Drawn, GroupBy, ImageMemo, Layout, Piece, RigNodes, Selected, Selection, WriteOptions, WriteReport,
    write, write_document, write_file,
};

/// Register this crate's passes by their declarations.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    for spec in [
        &prepare::PREPARE_GLTF_MATERIALS,
        &prepare::FILL_MISSING_TEXCOORDS,
        &lower::LOWER_GLTF_MATERIALS,
    ] {
        reg.add(spec);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// The GLB could not be assembled (over glTF's 4 GiB limit), or the document not serialized.
    #[error(transparent)]
    Glb(#[from] primase_glb::Error),
    /// A number JSON cannot hold.
    #[error("{0}")]
    Number(#[from] primase_core::json::JsonError),
    /// A world resource whose JSON view loses a value.
    #[error("{item}: {detail}")]
    Invalid { item: String, detail: String },
    /// The world holds something the writer cannot express (a malformed mesh, a placement without a transform).
    #[error("entity {entity:?}: {detail}")]
    World { entity: Entity, detail: String },
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use primase_core::pass::{Options, PassRegistry};

    /// The crate registers exactly these passes by their declarations: the one with required options is made only with
    /// them, and every one says what it takes.
    #[test]
    fn the_gltf_passes_register_by_name() {
        let mut reg = PassRegistry::new();
        super::register(&mut reg);
        let names = [
            "fill-missing-texcoords",
            "lower-gltf-materials",
            "prepare-gltf-materials",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        for name in names {
            let required = name == "prepare-gltf-materials";
            assert_eq!(reg.requires_options(name), required, "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            assert_eq!(reg.make(name, &Options::new()).is_err(), required, "{name}");
        }
    }
}
