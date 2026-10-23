//! What one bake evaluates, as data ([`PipelineState`]): the programs as stored, their constant buffers as words, the
//! textures as the texels they sample, what the vertex program is run on and what the pixel program is given. A bake
//! builds its state first and then evaluates the state and nothing else ([`super::Prepared::pipeline_state`]), so the
//! state a caller takes is the one the bake ran: another executor (a rasterizer running the same programs) can be
//! given exactly it.
//!
//! The values in it are the bake's policy (the frozen frame, the vertex feeds, the layer weights; counted in
//! [`super::Prepared::policies`]); what another executor can check against it is the execution: the vertex program's
//! outputs at the corners, the pixel program over the grid, the interpolation and the sampling.
use std::collections::BTreeMap;
use std::sync::Arc;

use primase_shader::{Constants, Input, Lane, Texture};

use super::BaseMipBilinearWrap;

/// The pixel program's outputs a bake reads, as (output register, component mask): the G-buffer normal, the second
/// output's x, the albedo, the reflectance and the emission.
pub const TARGETS: [(u32, u8); 5] = [(0, 7), (1, 1), (2, 7), (3, 7), (4, 7)];

/// A texture slot's content.
#[derive(Clone, Debug)]
pub enum SlotTexture {
    /// A texture's base mip, sampled bilinearly with its addressing ([`BaseMipBilinearWrap`]).
    Image(Arc<BaseMipBilinearWrap>),
    /// One value everywhere: an unbound slot's preview colour, or the engine's dither texture.
    Constant([f32; 4]),
}

/// The pixel stage of a bake.
#[derive(Clone, Debug)]
pub struct PixelState {
    /// The program's container as stored.
    pub bytecode: Arc<[u8]>,
    /// Every constant buffer the program reads, by slot ([`Constants::buffers`]).
    pub constants: Constants,
    /// Every texture slot the program samples, by `t` register.
    pub textures: BTreeMap<u32, SlotTexture>,
    /// What each used input is given, by (semantic, index): each lane its bits, or the grid's U or V
    /// ([`primase_shader::Grid`]: `(x + 0.5) / size * period[0]` along x, likewise V along y).
    pub inputs: BTreeMap<(String, u32), [Lane; 4]>,
}

/// The vertex stage of a bake whose material has a vertex program: the program run once at each corner of a texture
/// cell ([`super::vertex`]), where the pixel program's inputs are read from what it writes there.
#[derive(Clone, Debug)]
pub struct VertexState {
    /// The program's container as stored.
    pub bytecode: Arc<[u8]>,
    /// Every constant buffer the program reads, by slot, as rows of four words.
    pub constants: Vec<(u32, Vec<[u32; 4]>)>,
    /// The input registers of each run, at the cell's corners `(u, v)` = (0, 0), (1, 0), (0, 1) and (1, 1) in this
    /// order. Every structured buffer the program reads gives zero (one instance, index zero).
    pub corners: [Vec<[u32; 4]>; 4],
    /// The output registers each run wrote, in the same order.
    pub outputs: [Vec<[u32; 4]>; 4],
}

/// The albedo's alpha, which the bake samples beside the program (the program's own alpha is not the cutout): the main
/// albedo at `scale` times the grid's coordinate.
#[derive(Clone, Debug)]
pub struct AlphaState {
    pub image: Arc<BaseMipBilinearWrap>,
    pub scale: [f32; 2],
}

/// Everything one bake evaluates.
#[derive(Clone, Debug)]
pub struct PipelineState {
    /// The grid: `size` x `size` pixels over `period` texture units along u and v.
    pub size: u32,
    pub period: [f32; 2],
    pub pixel: PixelState,
    /// `None` where the material has no vertex program: the pixel inputs are then the fixed table's.
    pub vertex: Option<VertexState>,
    /// `None` where the material has no main albedo: the alpha is opaque.
    pub alpha: Option<AlphaState>,
    /// The pixel inputs where the vertex program's differ from the fixed table's, by input and the two values: counted
    /// beside the bake's policies.
    pub differences: BTreeMap<String, u64>,
}

impl PixelState {
    /// The textures as the executor samples them.
    pub(super) fn evaluated_textures(&self) -> BTreeMap<u32, Texture> {
        self.textures
            .iter()
            .map(|(&slot, t)| {
                (
                    slot,
                    match t {
                        SlotTexture::Image(image) => {
                            Texture::Image(image.clone() as Arc<dyn primase_shader::Image>)
                        }
                        SlotTexture::Constant(value) => Texture::Constant(*value),
                    },
                )
            })
            .collect()
    }

    /// The inputs as the executor takes them.
    pub(super) fn evaluated_inputs(&self) -> BTreeMap<(String, u32), Input> {
        self.inputs
            .iter()
            .map(|(key, lanes)| (key.clone(), Input::Lanes(*lanes)))
            .collect()
    }
}
