//! A shader program as primase understands it: a [`naga::Module`] (the typed body, structured control flow, the
//! resources as globals) together with what the module cannot say: the interface in primase's vocabulary (labels, system
//! values, interpolation), the pipeline conventions the program was written for, which D3D behaviours its body spells
//! out, and where it came from.
//!
//! A program is derived, never stored: `core.shader` keeps the bytecode as it was, and a lifter (`primase-dxbc`'s
//! `lift`) builds this on demand. The rules for differences between APIs are the shader design's:
//!
//! - **Conventions are normalized.** D3D register packing is unpacked into one varying per label, constant buffers
//!   become explicit layouts, registers become bindings; the source's own addresses stay in provenance.
//! - **Behaviour is preserved explicitly.** Where D3D's result differs from the IR's or other APIs' (integer division
//!   by zero, float to integer conversion of NaN and overflow, `min`/`max` with a NaN, saturation of a NaN), the lifter
//!   writes D3D's result out as plain operations in the body, and [`D3dModes`] says it did. Nothing silently takes
//!   one API's answer.
//! - **Unknowns are refused by name** by the lifter, and counted by whoever runs it over a corpus.
//!
//! # How the module carries the interface
//!
//! The entry point takes one argument per input varying, in [`Interface::inputs`] order, named `<NAME><index>` after
//! the label (`TEXCOORD5`), and returns a structure `Out` with one member per output varying in [`Interface::outputs`]
//! order, named the same way (no result at all when there are no outputs). A varying with a [`SystemValue`] has the
//! matching built-in binding; every other varying has a `@location` numbered 0, 1, ... in interface order (system
//! values do not use up locations), and a pixel input carries the interpolation the interface states. A pixel
//! stage's `Target(n)` output is `@location(n)`. A vertex stage's `ClipDistance` outputs are one member, the
//! built-in array of all their lanes ([`entry_io`] is the one statement of all this).
//!
//! Resources are module globals with a binding, matched to [`Interface::resources`] through [`Program::bindings`]. A
//! constant buffer is a uniform `array<vec4<u32>, N>` of D3D's registers' lanes as raw words (a program reads them as
//! the type its instruction implies, with a bitcast), so a buffer holds the bytes its layout describes whatever the
//! packing rules were.

mod io;
mod text;
mod validate;

pub use io::{
    EntryIo, FnItem, FunctionSig, IoItem, IoType, ItemList, entry_io, mesh_vertex_io, stage_functions,
};
pub(crate) use validate::module_info;
pub use validate::{Problem, validate};

use std::collections::BTreeSet;

use naga::{Handle, Module};

use crate::{Interface, ResourceId, ShaderFormat, Stage};

/// The version of the program representation: bumped when what a program means changes (a convention, a mode, how
/// the module carries the interface). Each lifter's tests hash what it makes of its fixtures under this and its own version
/// (`primase_shader::gate`), so a change that moves them without a bump of one of the two fails.
pub const VERSION: u32 = 2;

/// Which way clip space's depth runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipDepth {
    /// 0 at the near plane, 1 at the far plane (D3D, Vulkan, Metal, WebGPU).
    ZeroToOne,
    /// -1 at the near plane, 1 at the far plane (OpenGL).
    MinusOneToOne,
}

/// Which way a vertical axis runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    TopLeft,
    BottomLeft,
}

/// The pipeline rules a program was written for, declared where it meets the pipeline (the vertex stage's clip
/// position, the pixel stage's fragment position and texture coordinates) and applied once at that boundary by whoever
/// runs it on another API's rules; the body itself is never rewritten for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conventions {
    pub clip_depth: ClipDepth,
    /// Where window space (and so the fragment position) puts its origin.
    pub fragment_origin: Origin,
    /// Where a texture's first row is: the top for D3D, the bottom for OpenGL.
    pub texture_origin: Origin,
}
impl Conventions {
    /// Direct3D: depth 0 to 1, window and texture origin at the top left.
    pub const D3D: Self = Self {
        clip_depth: ClipDepth::ZeroToOne,
        fragment_origin: Origin::TopLeft,
        texture_origin: Origin::TopLeft,
    };
}

/// Which of D3D's behaviours the body spells out. The IR's own results for these differ between the APIs that lower to
/// it (WGSL defines integer division by zero as the dividend, SPIR-V leaves it undefined), so a D3D lifter writes
/// D3D's answer as plain operations and says it did: a consumer that finds a flag unset knows the body takes the
/// IR's answer there, and a writer never has to guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct D3dModes {
    /// Unsigned and signed division and remainder by zero give all ones (division) and the dividend (remainder) for
    /// `udiv`; `idiv` is not an instruction of Shader Model 4 and 5.
    pub divide_by_zero: bool,
    /// Float to integer conversion truncates toward zero, saturates to the integer range and turns NaN into 0.
    pub float_to_int: bool,
    /// `min` and `max` with one NaN operand return the other operand.
    pub min_max_nan: bool,
    /// Saturation maps NaN to 0.
    pub saturate_nan: bool,
    /// 32-bit float denormals are flushed to zero (sign kept), as D3D11 states. The body does not spell this out: an
    /// implementation may or may not flush, and the flag records what the source expects.
    pub flush_denormals: bool,
}
impl D3dModes {
    /// All of D3D's behaviours spelled out (the flush being the source's expectation).
    pub const ALL: Self = Self {
        divide_by_zero: true,
        float_to_int: true,
        min_max_nan: true,
        saturate_nan: true,
        flush_denormals: true,
    };
}

/// Where a program came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// The encoding of the bytecode it was lifted from.
    pub format: ShaderFormat,
    /// The source's own name for its target (`ps_5_0`).
    pub target: String,
    /// The lifter that built it and its version, so a cache keyed by (bytecode hash, lifter version) knows what is stale.
    pub lifter: String,
    pub lifter_version: u32,
    /// The hash of the bytes the lifter was given (`sha256:` and the digits, [`crate::source_hash`]), which the lifter
    /// computes: a program a lifter made has it, one built by hand may not.
    pub source_hash: Option<String>,
}

/// A shader program: the module, and what the module cannot say.
#[derive(Clone, Debug)]
pub struct Program {
    pub module: Module,
    pub stage: Stage,
    /// The entry point's name; the module has exactly one entry point, of this stage.
    pub entry: String,
    pub interface: Interface,
    /// The module global each resource of the interface is, by id.
    pub bindings: Vec<(ResourceId, Handle<naga::GlobalVariable>)>,
    pub conventions: Conventions,
    pub modes: D3dModes,
    pub provenance: Provenance,
    /// What the lifter approximated or could not state exactly, each once and sorted (`min-precision operands computed
    /// at 32 bits`): the program means what its body says, and a consumer shows these beside it.
    pub notes: Vec<String>,
}

/// What can go wrong building, checking or writing a program.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The module or the program broke a rule ([`Problem`]s, one per line).
    #[error("invalid program: {0}")]
    Invalid(String),
    /// A writer cannot express the program and refuses it rather than approximate.
    #[error("{writer} cannot write this program: {detail}")]
    Unsupported { writer: &'static str, detail: String },
    /// The pipeline a program is written for asks what the program cannot meet ([`crate::emit::Pipeline`]).
    #[error("the pipeline cannot take this program: {0}")]
    Pipeline(String),
}

impl Program {
    /// The global a resource is bound as.
    pub fn global(&self, id: ResourceId) -> Option<Handle<naga::GlobalVariable>> {
        self.bindings.iter().find(|(r, _)| *r == id).map(|(_, g)| *g)
    }

    /// The resources the entry point never names: no expression it computes with refers to their global, once naga's
    /// compaction has dropped the expressions no statement uses (as a writer leaves them out). A writer still declares
    /// them, so a pipeline built from the text binds them, but the program neither reads nor writes them. An error is
    /// the validator's: a module that does not validate is not compacted.
    pub fn unread_resources(&self) -> Result<BTreeSet<ResourceId>, Error> {
        self.validated()?;
        let mut module = self.module.clone();
        naga::compact::compact(&mut module, naga::compact::KeepUnused::No);
        // compaction renumbers the globals it keeps: a resource's global is found again by its name and binding
        let kept = |g: &naga::GlobalVariable| {
            module
                .global_variables
                .iter()
                .any(|(_, k)| k.name == g.name && k.binding == g.binding)
        };
        Ok(self
            .bindings
            .iter()
            .filter(|(_, g)| !kept(&self.module.global_variables[*g]))
            .map(|(id, _)| *id)
            .collect())
    }

    /// The name the entry point's argument or the result structure's member for a varying has: the label's name
    /// followed by its index.
    pub fn varying_name(label: &crate::Label) -> String {
        io::varying_name(label)
    }

    /// Check the program: naga's validation of the module, then ours (the interface against the entry point, the
    /// resources against the globals), every [`Problem`] found.
    pub fn validate(&self) -> Result<naga::valid::ModuleInfo, Vec<Problem>> {
        validate(self)
    }

    /// [`Program::validate`] as one error, its problems joined.
    pub(crate) fn validated(&self) -> Result<naga::valid::ModuleInfo, Error> {
        self.validate().map_err(|problems| {
            Error::Invalid(
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })
    }

    /// The stable text form: a header stating the interface, resources, conventions, modes and provenance, then the
    /// module as WGSL. A program WGSL cannot express (a texture atomic's old value, an exchange on a texture) is not lost to
    /// its text: the module follows as naga's IR dump, under a line saying it is not WGSL and why. The same program always gives
    /// the same text; one that fails validation has none.
    pub fn text(&self) -> Result<String, Error> {
        text::text(self)
    }
}
