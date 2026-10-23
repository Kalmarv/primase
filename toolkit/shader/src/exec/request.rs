//! What an evaluation asks of its caller, and the trait that answers it.
//!
//! A request names its resources by `R`: [`ResourceId`] where the program's own interface is the vocabulary
//! ([`Backend`]), the D3D bind slot where [`crate::Sampler`] answers for a bytecode that counts slots.

use crate::ResourceId;

/// A texture's shape as a request states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dim {
    D1,
    D2,
    D3,
    Cube,
}

/// How a request picks its level of detail. Gradients are three components wide: a 1D or 2D texture leaves the rest 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageLod {
    /// The implicit level, whose footprint a scalar evaluation leaves to the sampler.
    Implicit,
    /// The implicit level plus a bias the program states.
    Bias(f32),
    /// A level the program states.
    Level(f32),
    /// The coordinate's screen derivatives (d/dx, then d/dy): those the program states, or a quad's own for an
    /// implicit-level sample.
    Gradients([[f32; 3]; 2]),
    /// A quad's own derivatives plus the bias the program states.
    GradientsBias([[f32; 3]; 2], f32),
}

/// A sample of any texture: 1D, 2D, 3D, cube, arrayed, with an offset, a gather or a comparison.
#[derive(Clone, Copy, Debug)]
pub struct ImageSample<R> {
    pub texture: R,
    pub sampler: R,
    pub dim: Dim,
    /// The coordinate's components (1D: x; 2D: x, y; 3D and cube: x, y, z); the rest 0.
    pub coord: [f32; 3],
    /// An array's layer, as the program gives it (already rounded and clamped to 0 or more).
    pub layer: Option<f32>,
    pub lod: ImageLod,
    /// The constant texel offset (0 where the program states none).
    pub offset: [i32; 3],
    /// A gather: the component (0 to 3) whose four texels are returned.
    pub gather: Option<u8>,
    /// A comparison sample's reference value.
    pub depth_ref: Option<f32>,
    /// The texture is a depth texture: the first lane of the result is its (compared) depth.
    pub depth: bool,
}

/// A texel load (`ld`, `ld_ms`, a typed unordered-access load).
#[derive(Clone, Copy, Debug)]
pub struct ImageLoad<R> {
    pub texture: R,
    pub dim: Dim,
    /// A view of an unordered-access resource rather than a shader resource.
    pub storage: bool,
    pub texel: [i32; 3],
    pub layer: Option<i32>,
    pub sample: Option<i32>,
    pub mip: Option<i32>,
}

/// Which property of a texture a query reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryKind {
    /// The width, height and depth at `level` (as many as the dimension has).
    Size,
    Levels,
    Layers,
    Samples,
}

/// A `resinfo`-style query.
#[derive(Clone, Copy, Debug)]
pub struct ImageQuery<R> {
    pub texture: R,
    pub dim: Dim,
    pub kind: QueryKind,
    pub level: Option<u32>,
}

/// A write to an unordered-access texture.
#[derive(Clone, Copy, Debug)]
pub struct ImageStore<R> {
    pub view: R,
    pub dim: Dim,
    pub texel: [u32; 3],
    pub layer: Option<u32>,
    pub value: [u32; 4],
    /// How many of the value's lanes the program writes.
    pub lanes: u8,
}

/// Everything an evaluation asks of its caller: texture sampling, loads, queries, and the effects of unordered-access
/// writes. A request the caller cannot answer is an error that stops the evaluation. Reads of buffers need no call:
/// the buffers are bound before the run.
pub trait Backend {
    fn sample(&mut self, request: ImageSample<ResourceId>) -> Result<[f32; 4], String>;
    /// The texel's four lanes as its view returns them: f32 bits for a float view, the integers otherwise.
    fn load(&mut self, request: ImageLoad<ResourceId>) -> Result<[u32; 4], String> {
        Err(format!("resource {}: no loads given", request.texture.0))
    }
    fn query(&mut self, request: ImageQuery<ResourceId>) -> Result<[u32; 4], String> {
        Err(format!("resource {}: no queries given", request.texture.0))
    }
    fn store_image(&mut self, request: ImageStore<ResourceId>) -> Result<(), String> {
        Err(format!("resource {}: no stores given", request.view.0))
    }
    /// Words written to a buffer: the lanes `mask` marks of `value`, to the consecutive 32-bit words from word `word`
    /// of the buffer. The effect is the caller's: it reaches none of this evaluation's reads.
    fn store_buffer(&mut self, view: ResourceId, word: u32, value: [u32; 4], mask: u8) -> Result<(), String> {
        let _ = (word, value, mask);
        Err(format!("resource {}: no stores given", view.0))
    }
    /// `count` (1 to 4) consecutive 32-bit words from word `word` of a buffer the caller did not bind
    /// ([`super::Resources::buffers`]); lanes past `count` are 0.
    fn read_buffer(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String> {
        let _ = (word, count);
        Err(format!("resource {}: no buffer given", view.0))
    }
    /// How many words a buffer the caller did not bind has.
    fn buffer_words(&mut self, view: ResourceId) -> Result<u32, String> {
        Err(format!("resource {}: no buffer given", view.0))
    }
    /// An atomic add to word `word` of a buffer; its result the word as it was before.
    fn atomic_add(&mut self, view: ResourceId, word: u32, value: u32) -> Result<u32, String> {
        let _ = (word, value);
        Err(format!("resource {}: no atomics given", view.0))
    }
}
