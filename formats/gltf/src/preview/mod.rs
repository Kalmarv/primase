//! The preview export: standard GLB files plus a documented manifest, so a website can offer the choices a source
//! leaves open (outfits, accessories, skin tones) without the GLBs resolving them.
//!
//! - [`PreviewParts`] is the writer: [`crate::GlbParts`] (one GLB per part, with the state a preview leaves open
//!   legal) and, once every part is written, [`FILE_NAME`] beside them.
//! - The [`Manifest`] lists what the files hold and how it depends on the open choices: choices with their options,
//!   assets with their members and the nodes that draw them, the conditions that make each present, the per-primitive
//!   conditions, material variants, skin and socket bindings, and what the preview approximates ([`Fidelity`]). It
//!   needs no glTF extension: the GLBs are ordinary, and the manifest points into them by node and mesh index. The
//!   schema is `docs/PREVIEW-MANIFEST.md`; [`Manifest::from_slice`] reads it back and checks it.
//! - A [`Recipe`] is the site's answer: the chosen option of some choices, by id. [`Recipe::pipeline`] is the passes
//!   that apply it to a world (`select-outfit`), the same ones a program runs by hand, so preview and export share one
//!   recipe.
//!
//! Writers encode: this one resolves nothing. A world with conditions, per-primitive conditions and overrides on open
//! axes (what `select-outfit` leaves when it is given only some choices) is written whole; the files draw every
//! alternative and the manifest says which depends on what.

mod describe;
pub mod manifest;
pub mod recipe;
mod sink;

pub use manifest::{FILE_NAME, Fidelity, Manifest, VERSION};
pub use recipe::Recipe;
pub use sink::PreviewParts;
