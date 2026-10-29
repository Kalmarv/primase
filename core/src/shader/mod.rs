//! Shader programs and materials' bindings to them, as components, and what each thing a program reads and writes
//! stands for: the roles of its inputs, constants, textures, buffers and outputs ([`roles`]), how an output stores a
//! value ([`encoding`]), and the one reading of a pixel program's outputs as a lit surface ([`surface`]). The program's
//! own vocabulary (its interface, resources and render state) is `iridia_interface`'s.

mod binding;
pub mod encoding;
mod facts;
mod program;
pub mod roles;
mod select_variant;
pub mod surface;

pub(crate) use binding::BindingPlace;
pub use binding::{SamplerBinding, ShaderBinding, StageBinding, TextureBinding, TextureSource};
pub use encoding::{LaneAffine, NormalEncoding, NormalSpace, ScalarEncoding};
pub use facts::{MaterialPrograms, ProgramFacts, ProgramResourceFacts};
pub use program::Shader;
pub use roles::{
    Attribute, BufferBinding, BufferSource, BufferTable, ConstantBinding, ConstantRole, FrameParameter,
    FrameTexture, InputBinding, InputRole, KeyPart, LaneStream, ObjectMatrix, ObjectParameter, Occluded,
    OutputBinding, OutputRole, OutputUses, ScreenTriangleValue, SkinPrefix, VariantKey, ViewParameter,
    bind_output_uses, state_lines,
};
pub use select_variant::{SELECT_VARIANT, SelectVariant};
pub use surface::Surface;
