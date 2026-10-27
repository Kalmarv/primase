//! Shader programs and materials' bindings to them, as components: the vocabulary itself (interfaces, resources,
//! roles, render state) is `iridia_interface`'s.

mod binding;
mod facts;
mod program;
mod select_variant;

pub use binding::{ShaderBinding, StageBinding, TextureBinding, TextureSource};
pub use facts::{MaterialPrograms, ProgramFacts};
pub use program::Shader;
pub use select_variant::{SELECT_VARIANT, SelectVariant};
