//! Executing a [`Program`] on the CPU: the one executor for a shader whatever bytecode it was lifted from.
//!
//! [`Executable::new`] checks a program and compiles it once into a flat form (typed operations on four-lane word
//! registers, pointers folded to word offsets, dead operations dropped). [`Executable::bind`] flattens the buffers a
//! material reads once for all its pixels, and a [`Runner`] executes the program for one invocation or a 2 x 2 quad in
//! lockstep, asking its [`Backend`] for everything outside the program: texture samples, loads and queries, and the
//! effects of writes to unordered-access views. [`ProgramShader`] puts a program behind [`crate::PixelShader`], in
//! the register vocabulary of the D3D bytecode it came from.
//!
//! What a run does follows the program's [`crate::D3dModes`]: a program that says its body spells out D3D's integer
//! division, conversions and `min`/`max` runs as written, and one that expects denormals flushed to zero has every
//! 32-bit float operation flush its operands and result (sign kept). Anything the program does that the module cannot
//! make valid (an index past the end of a local array, an integer division by zero in the module) is an error of the
//! run, never a silent value.

mod compile;
mod flat;
pub mod geometry;
mod mask;
pub mod mesh;
mod pipeline;
mod pixel;
mod rayquery;
mod request;
mod run;
mod stage;
pub mod tessellation;
pub mod tessellator;

pub use flat::Executable;
pub use mask::{Mask, Wide};
pub use pipeline::{DispatchStats, Pipeline, PipelineResources, ShaderTable};
pub use pixel::ProgramShader;
pub use rayquery::{Built, Geometry, Instance, InstanceFlags, Scene, flags as ray_flags};
pub use request::{
    Backend, CallShader, Dim, Element, ImageAtomic, ImageAtomicOp, ImageLoad, ImageLod, ImageQuery,
    ImageSample, ImageStore, QueryKind, ReportHit, TraceRay,
};
pub use run::{Bound, Decision, Dispatched, Output, Resources, Runner, STEP_LIMIT, Scratch};
pub use stage::Constants;

use crate::{Error, Program};

/// An error of checking or compiling a program, in the program's own terms.
pub(crate) fn error(detail: impl Into<String>) -> Error {
    Error {
        bytecode: "program",
        at: 0,
        detail: detail.into(),
    }
}

impl Executable {
    /// Check `program` (its module and its interface) and compile it. A construct the executor does not run (a
    /// 64-bit value, an atomic other than add, an image atomic) is refused here, by name.
    pub fn new(program: &Program) -> Result<Self, Error> {
        if crate::stage_functions(&program.interface)
            .map_err(error)?
            .is_some()
        {
            return Err(error(if program.stage.is_ray_tracing() {
                format!(
                    "a {:?} program does the ray tracing pipeline's work through operations only the pipeline has \
                     ({}): run it with Executable::function inside a pipeline (exec::Pipeline)",
                    program.stage,
                    crate::rt::OPERATIONS.join(", ")
                )
            } else {
                format!(
                    "a {:?} program is functions, not an entry point: run one with Executable::function",
                    program.stage
                )
            }));
        }
        let info = program.validate().map_err(|problems| {
            error(
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        compile::compile(program, &info, None).map_err(error)
    }

    /// Compile one function of a program made of functions (a hull program's `main` and `patch_constant`, a domain
    /// program's or a position-less vertex program's `main`; see [`crate::stage_functions`]) to run for one
    /// invocation. The registers it takes are its arguments in order, an array argument (the control points) as one
    /// register per element; the members of its result are the members of [`crate::FunctionSig::result`].
    pub fn function(program: &Program, name: &str) -> Result<Self, Error> {
        let info = program.validate().map_err(|problems| {
            error(
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
        compile::compile(program, &info, Some(name)).map_err(error)
    }

    /// Every function of a program made of functions ([`crate::stage_functions`]: a hull, domain, geometry or ray tracing
    /// program, a vertex program that feeds a tessellator), each compiled by [`Executable::function`] under its name;
    /// `None` for a program that is an entry point, which [`Executable::new`] takes.
    pub fn functions(program: &Program) -> Result<Option<Vec<(&'static str, Self)>>, Error> {
        let Some(signatures) = crate::stage_functions(&program.interface).map_err(error)? else {
            return Ok(None);
        };
        signatures
            .iter()
            .map(|sig| Self::function(program, sig.name).map(|executable| (sig.name, executable)))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// The words of pointer argument `k` (a ray tracing program's payload, attributes or parameter).
    pub(super) fn pointer_words(&self, k: usize) -> Option<u32> {
        self.pointer_args.get(k).map(|a| a.2)
    }

    /// The textures and the samplers the program's compiled code samples, loads from or asks the size of (an operation
    /// nothing reads is not in it): the resources a caller must bind for a run that may reach any path.
    pub fn sampled(
        &self,
    ) -> (
        std::collections::BTreeSet<crate::ResourceId>,
        std::collections::BTreeSet<crate::ResourceId>,
    ) {
        let (mut textures, mut samplers) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for (_, op) in &self.ops {
            match op {
                flat::Op::Sample { tex, smp, .. } => {
                    textures.insert(tex.id);
                    samplers.insert(smp.id);
                }
                flat::Op::Texel {
                    tex, storage: false, ..
                }
                | flat::Op::Query { tex, .. } => {
                    textures.insert(tex.id);
                }
                _ => {}
            }
        }
        (textures, samplers)
    }

    /// Whether the program needs the other lanes of its 2 x 2 quad: it takes a screen derivative, or samples at an
    /// implicit level of detail (`sample`, `sample_b`), whose footprint is the quad's. A program that does not runs
    /// the same one invocation at a time as in a quad.
    pub fn needs_quad(&self) -> bool {
        self.ops.iter().any(|(_, op)| {
            matches!(
                op,
                flat::Op::Deriv { .. }
                    | flat::Op::Sample {
                        level: flat::LevelOp::Auto | flat::LevelOp::Bias(_),
                        ..
                    }
            )
        })
    }
}
