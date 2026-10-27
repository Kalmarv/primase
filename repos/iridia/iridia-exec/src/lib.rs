//! Executing a [`iridia::Program`] on the CPU: the one executor for a shader whatever bytecode it was lifted from, and the
//! pixel-shader evaluation interface built on it ([`PixelShader`], [`Sampler`]).
//!
//! [`Executable::new`] checks a program and compiles it once into a flat form (typed operations on four-lane word
//! registers, pointers folded to word offsets, dead operations dropped). [`Executable::bind`] flattens the buffers a
//! material reads once for all its pixels, and a [`Runner`] executes the program for one invocation or a 2 x 2 quad in
//! lockstep, asking its [`Backend`] for everything outside the program: texture samples, loads and queries, and the
//! effects of writes to unordered-access views. [`ProgramShader`] puts a program behind [`PixelShader`], in
//! the register vocabulary of the D3D bytecode it came from.
//!
//! What a run does follows the program's [`iridia::D3dModes`]: a program that says its body spells out D3D's integer
//! division, conversions and `min`/`max` runs as written, and one that expects denormals flushed to zero has every
//! 32-bit float operation flush its operands and result (sign kept). Anything the program does that the module cannot
//! make valid (an index past the end of a local array, an integer division by zero in the module) is an error of the
//! run, never a silent value.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod draw;
pub mod evaluate;
mod periodic;
pub use evaluate::{
    Evaluation, Field, Grid, Input, Invocation, Lane, Layout, Raw, evaluate, evaluate_raw, evaluate_raw_with,
    evaluate_with,
};
pub use periodic::{evaluate_periodic, evaluate_periodic_with};
pub mod texture;
pub mod views;
pub use texture::{Filtering, SampledTexture, SamplerUse, Unstated};
pub use views::{Absent, Image, View, Views};

use iridia::Interface;

mod compile;
mod flat;
pub mod geometry;
mod mask;
pub mod mesh;
mod pipeline;
mod pixel;
pub mod probe;
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

use iridia::Program;

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
        if iridia::stage_functions(&program.interface)
            .map_err(error)?
            .is_some()
        {
            return Err(error(if program.stage.is_ray_tracing() {
                format!(
                    "a {:?} program does the ray tracing pipeline's work through operations only the pipeline has \
                     ({}): run it with Executable::function inside a pipeline (Pipeline)",
                    program.stage,
                    iridia::rt::OPERATIONS.join(", ")
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
    /// program's or a position-less vertex program's `main`; see [`iridia::stage_functions`]) to run for one
    /// invocation. The registers it takes are its arguments in order, an array argument (the control points) as one
    /// register per element; the members of its result are the members of [`iridia::FunctionSig::result`].
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

    /// Every function of a program made of functions ([`iridia::stage_functions`]: a hull, domain, geometry or ray tracing
    /// program, a vertex program that feeds a tessellator), each compiled by [`Executable::function`] under its name;
    /// `None` for a program that is an entry point, which [`Executable::new`] takes.
    pub fn functions(program: &Program) -> Result<Option<Vec<(&'static str, Self)>>, Error> {
        let Some(signatures) = iridia::stage_functions(&program.interface).map_err(error)? else {
            return Ok(None);
        };
        signatures
            .iter()
            .map(|sig| Self::function(program, sig.name).map(|executable| (sig.name, executable)))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// The words of pointer argument `k` (a ray tracing program's payload, attributes or parameter).
    pub(crate) fn pointer_words(&self, k: usize) -> Option<u32> {
        self.pointer_args.get(k).map(|a| a.2)
    }

    /// The textures and the samplers the program's compiled code samples, loads from or asks the size of (an operation
    /// nothing reads is not in it): the resources a caller must bind for a run that may reach any path.
    pub fn sampled(
        &self,
    ) -> (
        std::collections::BTreeSet<iridia::ResourceId>,
        std::collections::BTreeSet<iridia::ResourceId>,
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

/// How a sample request picks its level of detail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lod {
    /// The implicit level (`sample`), whose footprint a scalar evaluation leaves to the sampler.
    Implicit,
    /// The implicit level plus a bias the program states (`sample_b`, as given: the sampler clamps it to D3D's
    /// -16..15.99).
    Bias(f32),
    /// A level the program states (`sample_l`).
    Level(f32),
    /// The UV's screen derivatives: those the program states (`sample_d`), or a quad's own for an implicit-LOD
    /// sample (d/dx, then d/dy).
    Gradients([[f32; 2]; 2]),
    /// A quad's own UV derivatives plus the bias the program states (`sample_b` in a quad).
    GradientsBias([[f32; 2]; 2], f32),
}

impl From<ImageLod> for Lod {
    /// A 2D sample's level of detail as the image request states it (its gradients' u and v).
    fn from(lod: ImageLod) -> Self {
        match lod {
            ImageLod::Implicit => Lod::Implicit,
            ImageLod::Bias(bias) => Lod::Bias(bias),
            ImageLod::Level(level) => Lod::Level(level),
            ImageLod::Gradients(g) => Lod::Gradients([[g[0][0], g[0][1]], [g[1][0], g[1][1]]]),
            ImageLod::GradientsBias(g, bias) => {
                Lod::GradientsBias([[g[0][0], g[0][1]], [g[1][0], g[1][1]]], bias)
            }
        }
    }
}

impl From<Lod> for ImageLod {
    /// A 2D sample's level of detail as an image request states it (its gradients' third lane 0).
    fn from(lod: Lod) -> Self {
        let lift = |g: [[f32; 2]; 2]| [[g[0][0], g[0][1], 0.], [g[1][0], g[1][1], 0.]];
        match lod {
            Lod::Implicit => ImageLod::Implicit,
            Lod::Bias(bias) => ImageLod::Bias(bias),
            Lod::Level(level) => ImageLod::Level(level),
            Lod::Gradients(g) => ImageLod::Gradients(lift(g)),
            Lod::GradientsBias(g, bias) => ImageLod::GradientsBias(lift(g), bias),
        }
    }
}

/// A sample request of a 2D texture (or a 2D array's layer).
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Texture bind slot (`t<n>`).
    pub texture: u32,
    /// Sampler bind slot (`s<n>`).
    pub sampler: u32,
    pub uv: [f32; 2],
    /// A 2D array's layer coordinate, as the program gives it (the sampler rounds it); `None` for a 2D texture.
    pub layer: Option<f32>,
    pub lod: Lod,
}

/// A load request (`ld`): one texel by its integer address, without a sampler.
#[derive(Clone, Copy, Debug)]
pub struct Load {
    /// Texture bind slot (`t<n>`).
    pub texture: u32,
    /// x, y, and a 2D array's layer or a 3D texture's z (0 for a 2D texture); a buffer's element at x (y and z 0).
    pub texel: [i32; 3],
    /// The mip level (0 for a buffer).
    pub mip: i32,
}

/// A store request (`store_uav_typed`, `store_structured`).
#[derive(Clone, Copy, Debug)]
pub struct Store {
    /// Unordered-access view bind slot (`u<n>`).
    pub view: u32,
    /// A typed view's x, y, and z or a 2D array's layer (0 where the view has fewer); a structured view's element and
    /// byte offset (then 0).
    pub address: [u32; 3],
    /// The lanes as the program writes them (bits), only those `mask` marks.
    pub value: [u32; 4],
    pub mask: u8,
}

/// An atomic add (`imm_atomic_iadd`) to the 32-bit word of an unordered-access view at `address`: a structured view's
/// element and byte offset in its first two lanes, a typed view's x, y (and z) otherwise (0 where the view has fewer).
#[derive(Clone, Copy, Debug)]
pub struct AtomicAdd {
    /// Unordered-access view bind slot (`u<n>`).
    pub view: u32,
    pub address: [u32; 3],
    pub value: u32,
}

/// Answers sample, load, store and structured-buffer requests; an error stops the evaluation. A closure answers samples
/// only.
pub trait Sampler {
    fn sample(&mut self, request: Sample) -> std::result::Result<[f32; 4], String>;
    /// The texel's four lanes as its view returns them: f32 bits for a float view, the integers for a uint or sint
    /// one.
    fn load(&mut self, request: Load) -> std::result::Result<[u32; 4], String> {
        Err(format!("t{}: no loads given", request.texture))
    }
    /// A store to an unordered-access view ([`Store`]): the lanes `mask` marks of `value`, at the address the program
    /// gives (a structured view's lanes the consecutive words from the byte offset). Its effect is the caller's: it
    /// reaches none of this evaluation's outputs.
    fn store(&mut self, request: Store) -> std::result::Result<(), String> {
        Err(format!("u{}: no stores given", request.view))
    }
    /// An atomic add ([`AtomicAdd`]); its result the word as it was before. The add itself is the caller's effect.
    fn atomic_add(&mut self, request: AtomicAdd) -> std::result::Result<u32, String> {
        Err(format!("u{}: no atomics given", request.view))
    }
    /// The 32-bit word at byte `offset` of element `element` of the structured buffer at `t<slot>`.
    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> std::result::Result<u32, String> {
        let _ = (element, offset);
        Err(format!("t{slot}: no structured buffer given"))
    }

    /// How many elements the buffer view at bind slot `slot` has, where the sampler knows. A program that reads past the
    /// end gets zeros (D3D's rule) when this is stated; when it is not, no read is taken for out of range.
    fn buffer_elements(&mut self, slot: u32) -> Option<u32> {
        let _ = slot;
        None
    }

    /// A sample of any texture: 1D, 3D or cube, with a texel offset, a gather or a comparison. A plain 2D sample
    /// (or a 2D array's layer) goes to [`Sampler::sample`]; anything else is refused unless the sampler answers it.
    fn sample_image(&mut self, request: ImageSample<u32>) -> std::result::Result<[f32; 4], String> {
        if request.dim != Dim::D2
            || request.gather.is_some()
            || request.depth_ref.is_some()
            || request.depth
            || request.offset != [0; 3]
        {
            return Err(format!(
                "t{}: this sampler answers plain 2D samples only",
                request.texture
            ));
        }
        self.sample(Sample {
            texture: request.texture,
            sampler: request.sampler,
            uv: [request.coord[0], request.coord[1]],
            layer: request.layer,
            lod: Lod::from(request.lod),
        })
    }
    /// A texel load of any texture. A 2D or 3D one (or a 2D array's layer) goes to [`Sampler::load`].
    fn load_image(&mut self, request: ImageLoad<u32>) -> std::result::Result<[u32; 4], String> {
        if !matches!(request.dim, Dim::D2 | Dim::D3) || request.storage || request.sample.is_some() {
            return Err(format!(
                "t{}: this sampler answers 2D, array and 3D loads of shader resources only",
                request.texture
            ));
        }
        self.load(Load {
            texture: request.texture,
            texel: [
                request.texel[0],
                request.texel[1],
                request.layer.unwrap_or(request.texel[2]),
            ],
            mip: request.mip.unwrap_or(0),
        })
    }
    /// A texture's size, level count, layer count or sample count.
    fn query_image(&mut self, request: ImageQuery<u32>) -> std::result::Result<[u32; 4], String> {
        Err(format!("t{}: no queries given", request.texture))
    }
    /// An atomic operation on a texel of a single-channel 32-bit unordered-access texture; its result is the word as it
    /// was before.
    fn atomic_image(&mut self, request: ImageAtomic<u32>) -> std::result::Result<u32, String> {
        Err(format!("u{}: no image atomics given", request.view))
    }
    /// A write to an unordered-access texture; the plain form is [`Sampler::store`].
    fn store_image(&mut self, request: ImageStore<u32>) -> std::result::Result<(), String> {
        self.store(Store {
            view: request.view,
            address: [
                request.texel[0],
                request.texel[1],
                request.layer.unwrap_or(request.texel[2]),
            ],
            value: request.value,
            mask: ((1u16 << request.lanes) - 1) as u8,
        })
    }
}
impl<F: FnMut(Sample) -> std::result::Result<[f32; 4], String>> Sampler for F {
    fn sample(&mut self, request: Sample) -> std::result::Result<[f32; 4], String> {
        self(request)
    }
}

/// What one evaluation wrote.
#[derive(Clone, Debug, Default)]
pub struct Execution {
    /// One entry per output register.
    pub outputs: Vec<[u32; 4]>,
    /// Only lanes marked in this mask were written by the shader.
    pub output_masks: Vec<u8>,
    pub instructions: usize,
    pub samples: usize,
    /// Whether the program discarded the pixel (`discard`): its outputs are then those written before.
    pub discarded: bool,
}

/// An evaluation or binding error, located in the implementation's own terms (a DXBC word, a DXIL value).
#[derive(Debug, thiserror::Error)]
#[error("{bytecode} at {at}: {detail}")]
pub struct Error {
    pub bytecode: &'static str,
    pub at: usize,
    pub detail: String,
}

/// A checked pixel shader that can be evaluated.
pub trait PixelShader: std::fmt::Debug + Sync {
    /// The program's varyings and resources, in primase's vocabulary.
    fn interface(&self) -> &Interface;
    /// Input registers an evaluation needs.
    fn input_count(&self) -> usize;
    /// Constant buffers the code reads: bind slot and its size in rows of four lanes.
    fn constant_buffers(&self) -> &[(u32, u32)];
    /// Texture bind slots the code samples.
    fn textures(&self) -> &[u32];
    /// Sampler bind slots the code uses.
    fn samplers(&self) -> &[u32];
    /// Evaluate once. Missing inputs or buffers are errors; no partial output is returned (the sampler may already
    /// have been called).
    fn execute(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<Execution, Error>;
    /// Evaluate once with the inputs' screen derivatives: per input register, its d/dx then d/dy lanes (f32 bits),
    /// for programs that read derivatives. A bytecode that reads none evaluates as [`PixelShader::execute`].
    fn execute_with_derivatives(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<Execution, Error> {
        let _ = derivatives;
        self.execute(inputs, buffers, sampler)
    }
    /// Evaluate invocations that share their inputs' screen derivatives (`derivatives`, as
    /// [`PixelShader::execute_with_derivatives`] takes them), each as that would alone: `each` is given every
    /// invocation's index and execution (the same outputs, instructions and samples), in order. On an error, the first
    /// failing invocation's index and its error, `each` given those before it. The sampler may be asked in another
    /// order than one invocation after another. The default evaluates them one after another.
    fn execute_lanes(
        &self,
        inputs: &[&[[u32; 4]]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
        each: &mut dyn FnMut(usize, &Execution),
    ) -> std::result::Result<(), (usize, Error)> {
        for (i, one) in inputs.iter().enumerate() {
            let e = self
                .execute_with_derivatives(one, derivatives, buffers, sampler)
                .map_err(|e| (i, e))?;
            each(i, &e);
        }
        Ok(())
    }
    /// Evaluate a 2 x 2 quad together (lanes 0, 1 the top row, 2, 3 the bottom), its derivatives and implicit
    /// levels of detail the quad's own. A bytecode without quad evaluation refuses.
    fn execute_quad(
        &self,
        inputs: [&[[u32; 4]]; 4],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> std::result::Result<[Execution; 4], Error> {
        let _ = (inputs, buffers, sampler);
        Err(Error {
            bytecode: "this bytecode",
            at: 0,
            detail: "no quad evaluation".to_owned(),
        })
    }
}
