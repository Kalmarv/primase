use super::BaseMipBilinearWrap;
use super::resolve::{self, Unbound, resolve, textures_of};
use super::sampler::address;
use super::state::{AlphaState, PipelineState, PixelState, SlotTexture, TARGETS, VertexState};
use super::vertex::VertexStage;
use crate::{Result, invalid, materials::LayeredMaterial};
use primase_core::Entity;
use primase_core::parallel::Threads;
use primase_core::shader::{
    AddressMode, ConstantRole, FrameParameter, ObjectParameter, StageBinding, TextureSource, ViewParameter,
};
use primase_image::sampling::Address;
use primase_shader::Interface;
use primase_shader_exec::ProgramShader;
use primase_shader_exec::{
    Grid, Input, Invocation, Lane, Layout, PixelShader, Raw, evaluate::Constants, evaluate_raw_with,
};
use std::{collections::BTreeMap, sync::Arc};

/// Frozen per-vertex input for one baked layer-weight group. Choosing groups
/// and replacing the second UV set with the first ([`SECOND_UV_APPROXIMATION`]) are separate lowering policies.
#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    pub weights_snow: [f32; 4],
    pub tint_color: [f32; 4],
    pub detail: f32,
}
impl Default for Inputs {
    fn default() -> Self {
        Self {
            weights_snow: [0.; 4],
            tint_color: [1.; 4],
            detail: 1.,
        }
    }
}
/// The programs a layered material's bake runs, as resolved against: the pixel program lifted (the interface its
/// binding names), its container's bytes, its pixel stage binding and, where the material has one, the vertex program
/// (`core.shader`) with its binding.
#[derive(Debug)]
pub struct Programs<'a> {
    pub shader: &'a ProgramShader,
    pub bytecode: &'a [u8],
    pub stage: &'a StageBinding,
    pub vertex: Option<(&'a primase_core::shader::Shader, &'a StageBinding)>,
}

/// Evaluated by the toolkit ([`evaluate_raw_with`], one pixel at a time) on the program executor: the lifted program,
/// its constants and a texture per sampled slot, frozen; the main albedo's alpha, sampled beside the program at its
/// own scale.
#[derive(Debug)]
pub struct Prepared {
    /// The pixel program's container as stored, which `shader` is lifted from.
    bytecode: Arc<[u8]>,
    shader: ProgramShader,
    constants: Constants,
    textures: BTreeMap<u32, SlotTexture>,
    alpha: Option<(Arc<BaseMipBilinearWrap>, [f32; 2])>,
    /// The material's vertex program, which gives the pixel program what it reads of a vertex; `None` where the
    /// material has none, and a fixed table does.
    vertex: Option<VertexStage>,
    pub policies: BTreeMap<String, u64>,
    pub decoded_bytes: usize,
    /// Static repeat policy calculated from resources used by this shader.
    pub period: [f32; 2],
    pub period_exact: bool,
    /// Whether the program reads the second texture coordinate set (`TEXCOORD0.zw`), which the static bake replaces by
    /// the first ([`SECOND_UV_APPROXIMATION`]).
    pub reads_second_uv: bool,
    largest_colour_dimension: u32,
}
/// The one approximation of how the static bake feeds a layered program's texture coordinates, on the programs that
/// read `TEXCOORD0.zw` (11 of the 22 layered pixel programs of the corpus): the grid's `(u, v)` fills `zw` as it fills
/// `xy`, so the second set is the first. The stored second set (`uv1`, the pair the layered vertex program adds
/// `cbTexScroll` to and hands on in `zw`) is not made from the first: a census of Sekiro's layered meshes finds it
/// differs from uv0 in 236 of 1,420 meshes of m10_00 (183 of them on a program that reads `zw`, in 64 of 384
/// prototypes) and 31 of 1,790 of m11_01's scene region (14 on a reading program, 9 prototypes), by up to 31 units, and
/// not by any scale and offset of uv0. None of those 197 meshes has a unique, non-overlapping uv0 unwrap (m10_00: 182
/// tile uv0 outside 0 to 1 and one overlaps inside it; m11_01: 13 and 1), so there is no texture-space rasterisation
/// of the surface a baked texture could stand for: a texel has several surface points with several uv1. The bake
/// reports it by material with the meshes and vertices affected; the faithful path is a renderer that runs the game's
/// shaders, or a destination that keeps uv1 and the layered material.
pub const SECOND_UV_APPROXIMATION: &str = "second UV set replaced by first for static bake";
#[derive(Debug)]
pub struct Bake {
    pub size: u32,
    pub albedo: Vec<u8>,
    /// Source-frame normal: X along cross(N,T)*w, Y along T. Consumer conversion
    /// is deliberately deferred, exactly as for source normal textures.
    pub normal: Vec<u8>,
    /// The shader's second output, x channel: linear and unquantized. When
    /// written, contains size * size values in row-major order, like albedo.
    /// None when the shader does not write this channel. Kept separately from
    /// normal Z; interpretation as gloss and roughness conversion belong to PBR lowering.
    pub gloss: Option<Vec<f32>>,
    /// Linear, unquantized RGB from shader output register 4. When written,
    /// contains size * size pixels in row-major order, like albedo. None when
    /// the shader does not write RGB. Values may exceed one; encoding and
    /// emissive strength conversion belong to PBR lowering.
    pub emissive: Option<Vec<[f32; 3]>>,
    pub instructions: u64,
    pub samples: u64,
    pub policies: BTreeMap<String, u64>,
}
/// Internal transport only: output3 remains encoded bits plus executed RGB mask.
/// Public bake callers retain the original result shape and allocate no capture.
#[derive(Debug)]
pub(crate) struct BakedChannels {
    pub image: Bake,
    pub reflectance: Option<Vec<OutputRgb>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputRgb {
    pub bits: [u32; 3],
    pub mask: u8,
}
fn lifting(e: primase_dxbc::Error) -> crate::Error {
    invalid("layer lift", e.at * 4, e.detail)
}
fn evaluation(e: primase_shader_exec::Error) -> crate::Error {
    invalid("layer bake", e.at, e.detail)
}
/// A DXBC pixel program's container lifted and ready to run.
pub(crate) fn executor(bytecode: &[u8]) -> Result<ProgramShader> {
    ProgramShader::new(primase_dxbc::lift(bytecode).map_err(lifting)?).map_err(evaluation)
}

/// A program's container of `format` (a game's: [`crate::Game::shader_format`]) lifted and ready to run: DXBC
/// ([`executor`]) or DXIL.
pub(crate) fn program_executor(
    format: primase_core::shader::ShaderFormat,
    bytecode: &[u8],
) -> Result<ProgramShader> {
    use primase_core::shader::ShaderFormat;
    match format {
        ShaderFormat::Dxbc => executor(bytecode),
        ShaderFormat::Dxil => {
            ProgramShader::new(primase_dxil::lift(bytecode).map_err(|e| invalid("program lift", 0, e))?)
                .map_err(evaluation)
        }
        other => Err(invalid(
            "program lift",
            0,
            format!("a FromSoftware program of format {other:?}"),
        )),
    }
}

const IDENTITY: [f32; 16] = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

/// An input's four lanes.
fn lanes_of_input(input: &Input) -> [Lane; 4] {
    match input {
        Input::Lanes(lanes) => *lanes,
        Input::Constant(c) => c.map(|v| Lane::Bits(v.to_bits())),
        Input::GridUv => [Lane::U, Lane::V, Lane::Bits(0), Lane::Bits(0)],
    }
}

/// The policy a consumer counts when it freezes an instance's own record (environment data, matrix base, clip plane) to
/// zero. By a data-flow reading of the 102 layered pixel programs (writes of a lane that depend on the word, control flow
/// not followed), the environment data reaches only the alpha lane of the reflectance target in 68 and, in 34 more, also
/// those of the normal and gloss targets: no lane a bake reads. The matrix base is the first matrix of the palette (the
/// identity in a bake) and the clip plane, zero, clips nothing.
pub const INSTANCE_DATA_POLICY: &str = "instance record frozen to its zeros: environment data, matrix base and clip plane (the environment data reaches no baked lane)";

/// `FC_AlphaRef.x` of a program that does not test alpha: below any alpha (the engine's own value for a plain program).
const ALPHA_TEST_OFF: f32 = -100.;

/// The policy a consumer counts when it freezes the dither and camera-fade fields to zero (both fades off: every fragment kept).
/// The five fields (`FC_isDitherAlpha`, `FC_isEnableCameraDitherFade`, `FC_DitherEnable`, `FC_CameraFadeEnd`,
/// `FC_CameraFadeInvRange`) are read by the 102 `_[A]` programs only, and a fade discards only inside its flag's test (the camera
/// fade under `FC_isEnableCameraDitherFade`, the dither under `FC_DitherEnable > 0` and `FC_isDitherAlpha != 0`). In a capture of
/// the running game (a castle interior, 11,252 draws of `_Gbuf_[A]` programs, the buffers read back from the D3D11 context) the
/// camera-fade flag and `FC_isDitherAlpha` were zero in every draw, `FC_DitherEnable` zero in 11,153 and one in 99 (with
/// `FC_isDitherAlpha` zero in those), so the engine's value for a draw that does not fade is zero; the end and the inverse range were
/// never set in 10,244 of the draws (NaN or infinity) and other numbers in the rest, read only under the flag.
pub const FADE_POLICY: &str = "dither and camera fades off in the bake (the flags and their parameters frozen to zero): the engine sets them per draw";

/// The policy a bake counts for an alpha-tested material (its `_Gbuf_[A]` program discards against `FC_AlphaRef`): the test
/// is off, every texel is kept, and the cutout is the albedo's alpha, sampled beside the program.
pub const ALPHA_TEST_POLICY: &str =
    "alpha test off in the bake (FC_AlphaRef.x frozen to -100): the cutout is the albedo's alpha";

/// The policy a consumer counts when it runs a skinning program: the pose is the bind pose, every joint's matrix the
/// identity, so a vertex at weight one on any joint is where it is stored. What the layered pixel program is given
/// (coordinates, layer weights, tint) does not depend on the pose.
pub const BIND_POSE_POLICY: &str =
    "skinning palette frozen to the identity (bind pose); the pixel inputs do not depend on the pose";

/// The words a role's constant is frozen to by `frame`, counting the policy the freezing is (`policies`); `None` for a
/// role the bake gives no value (the time of day, and an unknown one, counted by its reason): the words stay zero. A
/// material's own words are the role's own. A role the bake has no value for is an error.
pub(super) fn frozen_words(
    role: &ConstantRole,
    frame: &FrozenFrame,
    policies: &mut BTreeMap<String, u64>,
) -> Result<Option<Vec<u32>>> {
    let bits = |values: &[f32]| -> Vec<u32> { values.iter().map(|v| v.to_bits()).collect() };
    Ok(Some(match role {
        ConstantRole::Material(words) => words.clone(),
        ConstantRole::View(ViewParameter::CameraPosition) => bits(&frame.camera_position),
        ConstantRole::View(ViewParameter::InverseViewProjection) => bits(&frame.inverse_view_projection),
        ConstantRole::View(
            ViewParameter::View
            | ViewParameter::Projection
            | ViewParameter::ViewProjection
            | ViewParameter::InverseView
            | ViewParameter::InverseProjection,
        ) => bits(&frame.view_matrix),
        ConstantRole::View(ViewParameter::ViewportSize) => bits(&frame.viewport_size),
        ConstantRole::Object(ObjectParameter::World) => bits(&frame.world),
        ConstantRole::Object(ObjectParameter::EnvironmentData) => {
            policies.insert(INSTANCE_DATA_POLICY.into(), 1);
            vec![frame.environment_data]
        }
        ConstantRole::Object(ObjectParameter::MatrixBase) => {
            policies.insert(INSTANCE_DATA_POLICY.into(), 1);
            vec![frame.matrix_base, 0, 0, 0]
        }
        ConstantRole::Object(ObjectParameter::ClipPlane) => {
            policies.insert(INSTANCE_DATA_POLICY.into(), 1);
            bits(&frame.clip_plane)
        }
        ConstantRole::Object(ObjectParameter::BonePalette) => {
            policies.insert(BIND_POSE_POLICY.into(), 1);
            // every joint's matrix the identity: the palette's words, as many as any buffer holds
            (0..64).flat_map(|_| bits(&frame.world)).collect()
        }
        ConstantRole::Object(ObjectParameter::InstanceColour) => bits(&frame.instance_colour),
        ConstantRole::Object(
            ObjectParameter::CameraFadeEnabled
            | ObjectParameter::CameraFadeEnd
            | ObjectParameter::CameraFadeInverseRange
            | ObjectParameter::DitherAlphaEnabled
            | ObjectParameter::DitherEnabled,
        ) => {
            policies.insert(FADE_POLICY.into(), 1);
            vec![0]
        }
        ConstantRole::Object(ObjectParameter::TextureScroll(set)) => {
            policies.insert(TEXTURE_SCROLL_POLICY.into(), 1);
            let scroll = frame
                .texture_scroll
                .get(usize::from(*set))
                .ok_or_else(|| invalid("layer binding", 0, "a texture scroll of a fifth set"))?;
            bits(scroll)
        }
        ConstantRole::Frame(FrameParameter::ColourSlot) => bits(&[frame.colour_slot]),
        ConstantRole::Frame(FrameParameter::Time) => {
            policies.insert("global time at the program's sample time".into(), 1);
            bits(&[frame.time])
        }
        ConstantRole::Frame(FrameParameter::Exposure) => {
            policies.insert("unit pre-exposure for static material bake".into(), 1);
            bits(&[frame.exposure])
        }
        ConstantRole::Frame(FrameParameter::TimeOfDay) => {
            policies.insert("time of day left at frozen zero".into(), 1);
            return Ok(None);
        }
        ConstantRole::Unknown { why } => {
            *policies.entry(why.clone()).or_default() += 1;
            return Ok(None);
        }
        other => {
            return Err(invalid(
                "layer binding",
                0,
                format!("no frozen value for {other:?}"),
            ));
        }
    }))
}

/// The policy a consumer counts when it freezes a model's texture scroll ([`ObjectParameter::TextureScroll`]) to the
/// identity (scale one, offset zero, in each of the four sets): a capture of the running game (Sekiro 1.06, a castle
/// interior, a hook on its D3D11 contexts) found the vertex program's `cbTexScroll` to be that, in all 40,744 layered
/// draws (33 vertex programs, 61 pixel programs) over 88 frames; no scrolling model (water, flags) was in view, so the
/// value of an animated model is not observed.
///
/// Where a scroll is stored, if anywhere: the vertex programs read the buffer as `uv = stored_uv / 2048 * scroll.xy +
/// scroll.zw` for set `n` (layered programs sets 0 and 1, the GXFlver ones set 0), and a FLVER material can carry GX items
/// for it: `GX20`..`GX34` ("Texture Scroll": per texture type, Albedo1/2, Reflectance1/2, Shininess1/2, Normal1/2,
/// DetailNormal1/2, Displacement1/2, Emissive1/2, BlendMask, a U and a V speed and one unknown float; names from Smithbox's
/// `GX_Item_Descriptors.json`) and `GX07` (water: the normal and diffuse scroll speeds among its fields). A scan of every
/// named binder and FLVER of the install (14,459 FLVERs, 43,435 materials) found them zero everywhere except twelve items on
/// four map pieces, all GXFlver-family shaders: `m25_00_00_00_906000` (and its `_S` level of detail) materials `03` and `04`
/// (`M[A]_al`, GX20 V -0.03 and -0.05), `m17_00_00_00_106000` (and `_S`) `01 - Default` (`M[A]_al_NoLight`, GX20 U and V
/// 0.005), `m20_00_00_00_920000` `川` (`M[ARSN]`, GX20 [0, -0.08] and GX26 [0, -0.04]) and `m25_00_00_00_300010` `川` and `滝`
/// (`M[ARSN]_Water_3normaltet`, GX07). No layered material carries a scroll item (layered materials carry GX00 and GX04
/// only), so for everything a layer bake reads the identity is the right value. That the engine writes offset = speed x time
/// into the buffer from these items is an inference from their names and from GXFlver materials being their only carriers;
/// it is not confirmed: no capture has seen one of the four pieces drawn (it needs the game at m17_00, m20_00 or m25_00).
/// The SAT shaders that visibly scroll (`M[Flow]_Water`, `Character_AMSN_Scroll*`) do it inside the program from their MTD
/// `float2` parameters and `FC_GlobalTime`, which is a material source already bound.
pub const TEXTURE_SCROLL_POLICY: &str = "texture scroll frozen to the identity: 40,744 layered draws in a Sekiro capture all identity; animated models unobserved";

/// What a static bake freezes of the engine's per-view, per-frame and per-object values: the roles a binding states
/// ([`ConstantRole`]) and never values. These are the bake's choices, reported as policies when a binding reads them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenFrame {
    /// Seconds of global time (`Frame(Time)`), which scrolls some layers' texture coordinates.
    pub time: f32,
    /// `Frame(Exposure)`: material radiance is baked at unit exposure, not a runtime camera's pre-exposure.
    pub exposure: f32,
    /// `Frame(ColourSlot)`: the first.
    pub colour_slot: f32,
    /// `View(CameraPosition)`: overhead.
    pub camera_position: [f32; 4],
    /// `View(InverseViewProjection)`: zero but for the last element.
    pub inverse_view_projection: [f32; 16],
    /// `Object(World)`: identity, as three rows of four.
    pub world: [f32; 12],
    /// `Object(InstanceColour)`: white.
    pub instance_colour: [f32; 4],
    /// `Object(TextureScroll(n))`: the identity, scale one and offset zero ([`TEXTURE_SCROLL_POLICY`]).
    pub texture_scroll: [[f32; 4]; 4],
    /// `Object(EnvironmentData)`: zero.
    pub environment_data: u32,
    /// `Object(MatrixBase)`: zero, the first matrix of the palette.
    pub matrix_base: u32,
    /// `Object(ClipPlane)`: zero, which clips nothing.
    pub clip_plane: [f32; 4],
    /// `View(View)`, `Projection`, `ViewProjection`, `InverseView` and `InverseProjection`: the identity, so a vertex
    /// program's clip position is its camera-relative position.
    pub view_matrix: [f32; 16],
    /// `View(ViewportSize)`: one by one pixel, and its reciprocals.
    pub viewport_size: [f32; 4],
}
impl FrozenFrame {
    /// The static bake's frame, its global time `time` seconds.
    pub fn static_bake(time: f32) -> Self {
        let mut inverse_view_projection = [0.; 16];
        inverse_view_projection[15] = 1.;
        Self {
            time,
            exposure: 1.,
            colour_slot: 0.,
            camera_position: [0., 1000., 0., 1.],
            inverse_view_projection,
            world: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
            instance_colour: [1.; 4],
            texture_scroll: [[1., 1., 0., 0.]; 4],
            environment_data: 0,
            matrix_base: 0,
            clip_plane: [0.; 4],
            view_matrix: IDENTITY,
            viewport_size: [1.; 4],
        }
    }
}

/// The program's common repeat over the UV scales of the texture slots it samples that the material binds, and
/// whether it is exact ([`super::period::common_period`]).
pub(crate) fn sampled_period(interface: &Interface, material: &LayeredMaterial) -> Result<([f32; 2], bool)> {
    let slots = resolve::slots(&material.view())?;
    let sampled_scales: Vec<_> = textures_of(interface)
        .filter_map(|r| resolve::texture_slot(r).ok())
        .filter_map(|slot| slots.get(&slot).map(|(_, b)| b.scale))
        .collect();
    let main_scale = slots
        .get(&8)
        .map(|(_, b)| b.scale)
        .or_else(|| sampled_scales.first().copied())
        .unwrap_or([1.; 2]);
    super::period::common_period(&sampled_scales, main_scale)
}

impl Prepared {
    /// Frozen-frame preview binding of a layered material straight from its source: resolves the program's binding
    /// ([`resolve`], unbound slots given their counted preview colours) and binds it ([`Prepared::from_binding`]).
    /// The caller supplies decoded views by texture name and sRGB flag. A program that reads the global time
    /// (`FC_GlobalTime`, which scrolls some layers' texture coordinates) is sampled at `sample_time` seconds.
    pub fn new(
        bytes: &[u8],
        material: &LayeredMaterial,
        sample_time: f32,
        mut image: impl FnMut(&str, bool) -> Result<Option<Arc<BaseMipBilinearWrap>>>,
    ) -> Result<Self> {
        let shader = executor(bytes)?;
        // the textures stand as entities of a scratch world, each with the view the caller decoded
        let mut scratch = primase_core::World::default();
        let program = scratch.spawn();
        let mut views: BTreeMap<Entity, Arc<BaseMipBilinearWrap>> = BTreeMap::new();
        let mut known: BTreeMap<(String, bool), Option<Entity>> = BTreeMap::new();
        // the layer bake's materials are Sekiro's layered ones
        let resolution = resolve(
            &shader,
            program,
            &material.view(),
            crate::Game::Sekiro,
            None,
            Unbound::Preview,
            &mut |stem, srgb| {
                let key = (stem.to_owned(), srgb);
                if let Some(found) = known.get(&key) {
                    return Ok(*found);
                }
                let found = image(stem, srgb)?.map(|view| {
                    let e = scratch.spawn();
                    views.insert(e, view);
                    e
                });
                known.insert(key, found);
                Ok(found)
            },
        )?;
        Self::from_binding(
            Programs {
                shader: &shader,
                bytecode: bytes,
                stage: &resolution.stage,
                vertex: None,
            },
            material,
            &FrozenFrame::static_bake(sample_time),
            |texture, _, _| {
                views
                    .get(&texture)
                    .cloned()
                    .ok_or_else(|| invalid("layer binding", 0, "a texture the caller did not supply"))
            },
            |name, srgb, _| image(name, srgb),
        )
    }

    /// The bake of a layered material from its pixel stage's binding (`core.shader_binding`): the constants as their
    /// roles say (the material's own words as stored, the engine's values from `frame`, counted when a policy
    /// applies, an unknown one left at zero, counted by its reason), each texture slot as bound (`texture` decodes a
    /// texture entity, `true` for the sRGB colour layers; a default is its constant), and the outputs by their roles.
    /// The material source gives what a shader does not read: the main albedo's alpha (`stem` decodes a texture file
    /// by name), the shader family the bake supports and the sampled layers' UV scales. `shader` is the program the
    /// binding was resolved against (its resources are what the binding names) and `bytecode` its container, which
    /// the bake lifts and executes.
    pub fn from_binding(
        programs: Programs<'_>,
        material: &LayeredMaterial,
        frame: &FrozenFrame,
        mut texture: impl FnMut(Entity, bool, [Address; 2]) -> Result<Arc<BaseMipBilinearWrap>>,
        mut stem: impl FnMut(&str, bool, [Address; 2]) -> Result<Option<Arc<BaseMipBilinearWrap>>>,
    ) -> Result<Self> {
        let Programs {
            shader,
            bytecode,
            stage,
            vertex,
        } = programs;
        let interface = PixelShader::interface(shader);
        let leaf = material
            .definition
            .shader
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        let family = leaf.rsplit_once('.').map_or(leaf, |x| x.0);
        if !matches!(
            family,
            "M[Multiple]" | "M[MultipleGround]" | "M[Multiple]_Emissive"
        ) {
            return Err(invalid("layer binding", 0, "unsupported layered shader family"));
        }
        if !frame.time.is_finite() {
            return Err(invalid("layer binding", 0, "nonfinite sample time"));
        }
        let mut policies = BTreeMap::from([
            (
                "frozen identity world, white instance colour and overhead camera".into(),
                1,
            ),
            (
                "base-mip bilinear wrap overrides runtime sampler state".into(),
                PixelShader::samplers(shader).len() as u64,
            ),
        ]);
        let reads_second_uv = interface
            .inputs
            .iter()
            .find(|v| v.label.name.eq_ignore_ascii_case("TEXCOORD") && v.label.index == 0)
            .and_then(|v| v.provenance.d3d)
            .is_some_and(|d| d.read_write_mask & 0b1100 != 0);
        if reads_second_uv {
            policies.insert(SECOND_UV_APPROXIMATION.into(), 1);
        }
        let vertex = vertex
            .map(|(core, vertex_binding)| {
                VertexStage::new(core, vertex_binding, family == "M[MultipleGround]", frame)
            })
            .transpose()?;
        match &vertex {
            Some(stage) => policies.extend(stage.policies.iter().map(|(k, v)| (k.clone(), *v))),
            None => {
                policies.insert(
                    "pixel inputs from a fixed table: the material has no vertex program".into(),
                    1,
                );
            }
        }
        let lifted = executor(bytecode)?;
        let layout = Layout::of(&lifted).map_err(evaluation)?;
        // the buffers the program reads, by slot: their words
        let words: BTreeMap<u32, usize> = lifted
            .constant_buffers()
            .iter()
            .map(|&(slot, rows)| (slot, rows as usize * 4))
            .collect();
        let mut constants = Constants::new(&layout);
        for c in &stage.constants {
            let slot =
                resolve::register(interface.resource(c.resource).ok_or_else(|| {
                    invalid("layer binding", 0, "a constant of a resource the shader lacks")
                })?)?;
            let Some(mut values) = frozen_words(&c.role, frame, &mut policies)? else {
                continue;
            };
            // the alpha test is off in a bake: a discarded texel has no G-buffer, and the cutout is the albedo's own alpha,
            // sampled beside the program
            if c.name.as_deref() == Some("FC_AlphaRef")
                && matches!(c.role, ConstantRole::Material(_))
                && values.first().is_some_and(|w| f32::from_bits(*w) > 0.)
            {
                values[0] = ALPHA_TEST_OFF.to_bits();
                policies.insert(ALPHA_TEST_POLICY.into(), 1);
            }
            // the words inside a buffer the program reads (it reads no others)
            let Some(&n) = words.get(&slot) else {
                continue;
            };
            let start = c.offset as usize / 4;
            values.truncate((c.size as usize / 4).min(n.saturating_sub(start)));
            if !values.is_empty() {
                constants
                    .set_words(slot, start as u32, &values)
                    .expect("words inside a buffer the program reads");
            }
        }
        let slots = resolve::slots(&material.view())?;
        let (period, period_exact) = sampled_period(interface, material)?;
        let mut textures = BTreeMap::new();
        let mut cache = BTreeMap::<(Entity, bool, [Option<AddressMode>; 2]), Arc<BaseMipBilinearWrap>>::new();
        let mut largest_colour_dimension = 0;
        for bound in &stage.textures {
            let resource = interface
                .resource(bound.resource)
                .ok_or_else(|| invalid("layer binding", 0, "a texture the shader lacks"))?;
            let register = resolve::register(resource)?;
            // an engine texture (no material slot in its name) is a constant, as the binding states it Default
            if resolve::texture_slot(resource).is_err()
                && !resolve::named_by_material(&material.view(), &resource.provenance.name)
            {
                match &bound.source {
                    TextureSource::Default { value, why } => {
                        *policies.entry(why.clone()).or_default() += 1;
                        textures.insert(register, SlotTexture::Constant(*value));
                        continue;
                    }
                    TextureSource::Frame(frame) => {
                        // the layered programs read no frame image; a static bake has none to give a program that does
                        *policies
                            .entry(format!("{frame:?} of the frame left at zero in the bake"))
                            .or_default() += 1;
                        textures.insert(register, SlotTexture::Constant([0.; 4]));
                        continue;
                    }
                    _ => {
                        return Err(invalid(
                            "layer binding",
                            0,
                            "an engine texture is bound to a texture",
                        ));
                    }
                }
            }
            let (role, _) = resolve::role_and_binding(&material.view(), &slots, resource)?;
            let source = match &bound.source {
                TextureSource::Texture {
                    texture: entity,
                    sampler,
                } => {
                    let stated = [sampler.wrap[0], sampler.wrap[1]];
                    if stated.contains(&None) {
                        *policies
                            .entry("texture slot with no stored addressing sampled with wrap".into())
                            .or_default() += 1;
                    }
                    let key = (*entity, role == "AlbedoMap", stated);
                    let image = match cache.entry(key) {
                        std::collections::btree_map::Entry::Occupied(found) => found.into_mut(),
                        std::collections::btree_map::Entry::Vacant(slot) => {
                            slot.insert(texture(key.0, key.1, [address(stated[0])?, address(stated[1])?])?)
                        }
                    };
                    if key.1 {
                        largest_colour_dimension = largest_colour_dimension
                            .max(*image.dimensions().iter().max().expect("two axes") as u32);
                    }
                    SlotTexture::Image(image.clone())
                }
                TextureSource::Default { value, why } => {
                    *policies.entry(why.clone()).or_default() += 1;
                    SlotTexture::Constant(*value)
                }
                TextureSource::Frame(_) => {
                    return Err(invalid(
                        "layer binding",
                        0,
                        "a material texture slot is bound to a frame image",
                    ));
                }
                TextureSource::Missing => continue,
            };
            textures.insert(register, source);
        }
        for &slot in lifted.textures() {
            if !textures.contains_key(&slot) {
                return Err(invalid(
                    "layer binding",
                    0,
                    format!("t{slot} has no texture bound"),
                ));
            }
        }
        policies.insert(
            "buffer words left at frozen zero (including padding and unused fields)".into(),
            constants.unset_words(),
        );
        let main = slots
            .iter()
            .filter(|(_, (role, _))| *role == "AlbedoMap")
            .min_by_key(|(slot, _)| (**slot != 8, **slot));
        let mut alpha = None;
        let mut alpha_bytes = 0;
        if let Some((_, (_, binding))) = main
            && let Some(t) = stem(
                &binding.texture,
                true,
                [address(binding.address[0])?, address(binding.address[1])?],
            )?
        {
            alpha_bytes = t.decoded_bytes();
            alpha = Some((
                t,
                if binding.scale.iter().all(|&v| v > 0.) {
                    binding.scale
                } else {
                    [1.; 2]
                },
            ));
        }
        if alpha.is_none() {
            policies.insert("opaque alpha because main albedo is missing".into(), 1);
        }
        let decoded_bytes = cache.values().map(|t| t.decoded_bytes()).sum::<usize>() + alpha_bytes;
        Ok(Self {
            bytecode: bytecode.into(),
            shader: lifted,
            constants,
            textures,
            alpha,
            vertex,
            policies,
            decoded_bytes,
            period,
            period_exact,
            reads_second_uv,
            largest_colour_dimension,
        })
    }
    /// Largest sampled colour layer, bounded by the caller's cap; the cap for a
    /// material without a colour image.
    pub fn bake_size(&self, cap: u32) -> Result<u32> {
        if cap == 0 || cap > 4096 {
            return Err(invalid("layer bake", 0, "invalid size cap"));
        }
        Ok(if self.largest_colour_dimension == 0 {
            cap
        } else {
            self.largest_colour_dimension.min(cap)
        })
    }
    pub fn bake(&self, size: u32, period: [f32; 2], params: Inputs) -> Result<Bake> {
        Ok(self
            .bake_channels(Threads::ONE, size, period, params, false)?
            .image)
    }
    /// The bake with its reflectance, the shader evaluated on `threads` threads (a row of the grid each at a time):
    /// the same channels on any count.
    pub(crate) fn bake_with_reflectance(
        &self,
        threads: Threads,
        size: u32,
        period: [f32; 2],
        params: Inputs,
    ) -> Result<BakedChannels> {
        self.bake_channels(threads, size, period, params, true)
    }
    /// What a bake of `size` x `size` pixels over `period`, the level's vertex inputs `params`, evaluates
    /// ([`PipelineState`]): the bake builds it and then evaluates it alone ([`Prepared::bake`]).
    pub fn pipeline_state(&self, size: u32, period: [f32; 2], params: Inputs) -> Result<PipelineState> {
        if !params
            .weights_snow
            .iter()
            .chain(&params.tint_color)
            .chain(std::iter::once(&params.detail))
            .all(|v| v.is_finite())
        {
            return Err(invalid("layer bake", 0, "nonfinite frozen vertex input"));
        }
        if size == 0 || size > 4096 || !period.iter().all(|v| v.is_finite() && *v > 0.) {
            return Err(invalid("layer bake", 0, "invalid size or period"));
        }
        // every used input by its semantic: the grid's UV in TEXCOORD0, the frozen values elsewhere
        let mut inputs = BTreeMap::new();
        for s in self.shader.interface().inputs.iter().filter(|s| s.used) {
            let input = match (s.label.name.to_ascii_uppercase().as_str(), s.label.index) {
                ("POSITION", 1) => Input::Constant([0., 0., 0., 1.]),
                ("NORMAL", 0) => Input::Constant([0., 1., 0., 0.]),
                ("TANGENT", 0) => Input::Constant([1., 0., 0., 1.]),
                // zw is the second set, replaced by the first: SECOND_UV_APPROXIMATION
                ("TEXCOORD", 0) => Input::Lanes([Lane::U, Lane::V, Lane::U, Lane::V]),
                ("TEXCOORD", 5) => Input::Constant(params.weights_snow),
                ("TEXCOORD", 7) => Input::Constant(params.tint_color),
                ("TEXCOORD", 12) => Input::Constant([params.detail, 0., 0., 0.]),
                ("SV_ISFRONTFACE", 0) => {
                    Input::Lanes([Lane::Bits(u32::MAX), Lane::Bits(0), Lane::Bits(0), Lane::Bits(0)])
                }
                ("SV_POSITION", 0) => Input::Constant([0.; 4]),
                _ => {
                    return Err(invalid(
                        "layer binding",
                        0,
                        format!("unclassified input {}{}", s.label.name, s.label.index),
                    ));
                }
            };
            inputs.insert((s.label.name.clone(), s.label.index), input);
        }
        // what the material's own vertex program writes replaces the table's value, and where they differ is counted by input
        let mut differences = BTreeMap::new();
        let mut vertex_state = None;
        if let Some(vertex) = &self.vertex {
            let corners = vertex.corners(&params);
            let outputs = vertex.outputs(&corners)?;
            let derived_inputs = vertex.pixel_inputs_from(self.shader.interface(), &outputs)?;
            vertex_state = Some(VertexState {
                bytecode: vertex.bytecode().clone(),
                constants: vertex.constants().to_vec(),
                corners,
                outputs,
            });
            for (key, derived) in derived_inputs {
                let width = self
                    .shader
                    .interface()
                    .inputs
                    .iter()
                    .find(|i| (i.label.name.as_str(), i.label.index) == (key.0.as_str(), key.1))
                    .map_or(4, |i| i.value.width as usize);
                if let Some(table) = inputs.get(&key) {
                    let (a, b) = (lanes_of_input(&derived), lanes_of_input(table));
                    if a[..width] != b[..width] {
                        let name = |l: &[Lane; 4]| {
                            l[..width]
                                .iter()
                                .map(|l| match l {
                                    Lane::U => "U".to_owned(),
                                    Lane::V => "V".to_owned(),
                                    Lane::Bits(b) => format!("{}", f32::from_bits(*b)),
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        *differences
                            .entry(format!(
                                "pixel input {}{} from the vertex program is [{}], the fixed table's was [{}]",
                                key.0,
                                key.1,
                                name(&a),
                                name(&b)
                            ))
                            .or_insert(0u64) += 1;
                    }
                }
                inputs.insert(key, derived);
            }
        }
        if !inputs
            .keys()
            .any(|(name, index)| name.eq_ignore_ascii_case("TEXCOORD") && *index == 0)
        {
            return Err(invalid("layer binding", 0, "missing TEXCOORD0"));
        }
        Ok(PipelineState {
            size,
            period,
            pixel: PixelState {
                bytecode: self.bytecode.clone(),
                constants: self.constants.clone(),
                textures: self.textures.clone(),
                inputs: inputs
                    .into_iter()
                    .map(|(key, input)| (key, lanes_of_input(&input)))
                    .collect(),
            },
            vertex: vertex_state,
            alpha: self.alpha.as_ref().map(|(image, scale)| AlphaState {
                image: image.clone(),
                scale: *scale,
            }),
            differences,
        })
    }
    fn bake_channels(
        &self,
        threads: Threads,
        size: u32,
        period: [f32; 2],
        params: Inputs,
        capture_reflectance: bool,
    ) -> Result<BakedChannels> {
        let state = self.pipeline_state(size, period, params)?;
        let (size, period) = (state.size, state.period);
        let raw = evaluate_raw_with(
            threads,
            &self.shader,
            &state.pixel.constants,
            &state.pixel.evaluated_textures(),
            &state.pixel.evaluated_inputs(),
            Grid { size, period },
            &TARGETS,
            Invocation::Scalar,
        )
        .map_err(evaluation)?;
        channels(&state, &raw, self.policies.clone(), capture_reflectance)
    }
}

/// A bake's channels from its state and what the pixel program wrote over the state's grid (`raw`, the targets
/// [`TARGETS`] in order): the albedo in sRGB bytes with the main albedo's alpha beside it, the G-buffer normal unbiased
/// into bytes, the second output's x and the emission as written, and the reflectance where asked; `base` the policies
/// the bake already counts.
fn channels(
    state: &PipelineState,
    raw: &Raw,
    base: BTreeMap<String, u64>,
    capture_reflectance: bool,
) -> Result<BakedChannels> {
    let (size, period) = (state.size, state.period);
    if raw.pixels.len() != TARGETS.len()
        || raw
            .pixels
            .iter()
            .any(|t| t.len() != size as usize * size as usize)
    {
        return Err(invalid(
            "layer bake",
            0,
            "outputs that are not the state's targets over its grid",
        ));
    }
    let [normals, seconds, colours, reflected, emitted] = &raw.pixels[..] else {
        unreachable!("five targets requested");
    };
    let mut albedo = Vec::with_capacity(size as usize * size as usize * 4);
    let mut normal = Vec::with_capacity(albedo.capacity());
    let mut gloss = None;
    let mut writes_gloss = None;
    let mut emissive = None;
    let mut writes_emissive = None;
    let mut reflectance = capture_reflectance.then(|| Vec::with_capacity(size as usize * size as usize));
    let byte = |f: f32| -> u8 { (f.clamp(0., 1.) * 255.).round_ties_even() as u8 };
    for pixel in 0..size as usize * size as usize {
        let (x, y) = (pixel % size as usize, pixel / size as usize);
        let uv = [
            (x as f32 + 0.5) / size as f32 * period[0],
            (y as f32 + 0.5) / size as f32 * period[1],
        ];
        let (n, n_mask) = normals[pixel];
        let (color, color_mask) = colours[pixel];
        if n_mask != 7 || color_mask != 7 {
            return Err(invalid("layer bake", 0, "missing G-buffer normal or albedo"));
        }
        if let Some(pixels) = &mut reflectance {
            let (bits, mask) = reflected[pixel];
            pixels.push(OutputRgb {
                bits: [bits[0], bits[1], bits[2]],
                mask,
            });
        }
        let (second, second_mask) = seconds[pixel];
        let writes = second_mask & 1 != 0;
        if writes_gloss.is_some_and(|previous| previous != writes) {
            return Err(invalid("layer bake", 0, "inconsistent second-output x coverage"));
        }
        writes_gloss = Some(writes);
        if writes {
            let value = f32::from_bits(second[0]);
            if !value.is_finite() {
                return Err(invalid("layer bake", 0, "nonfinite second-output x"));
            }
            gloss
                .get_or_insert_with(|| Vec::with_capacity(size as usize * size as usize))
                .push(value);
        }
        let (emission, rgb_mask) = emitted[pixel];
        if rgb_mask != 0 && rgb_mask != 7 {
            return Err(invalid("layer bake", 0, "partial emission RGB output"));
        }
        let writes = rgb_mask == 7;
        if writes_emissive.is_some_and(|previous| previous != writes) {
            return Err(invalid("layer bake", 0, "inconsistent emission RGB coverage"));
        }
        writes_emissive = Some(writes);
        if writes {
            let rgb = std::array::from_fn(|c| f32::from_bits(emission[c]));
            if !rgb.iter().all(|v| v.is_finite()) {
                return Err(invalid("layer bake", 0, "nonfinite emission RGB"));
            }
            emissive
                .get_or_insert_with(|| Vec::with_capacity(size as usize * size as usize))
                .push(rgb);
        }
        let color = color.map(f32::from_bits);
        let n = n.map(f32::from_bits);
        if !color[..3].iter().chain(&n[..3]).all(|v| v.is_finite()) {
            return Err(invalid("layer bake", 0, "nonfinite shader output"));
        }
        for &c in &color[..3] {
            albedo.push(
                (primase_image::linear_to_srgb((c as f64).clamp(0., 1.)) * 255.).round_ties_even() as u8,
            );
        }
        albedo.push(if let Some(AlphaState { image, scale }) = &state.alpha {
            byte(image.sample([uv[0] * scale[0], uv[1] * scale[1]])?[3])
        } else {
            255
        });
        // Invert the observed G-buffer bias from the binary operand,
        // not its rounded decimal rendering in a disassembly.
        let bias = f32::from_bits(0x3f002008);
        let n =
            primase_meshops::vector::Vec3::from([(n[0] - bias) * 2., (n[1] - bias) * 2., (n[2] - bias) * 2.])
                .normalize_or([0., 1., 0.].into());
        normal.extend([
            byte(-n.z * 0.5 + 0.5),
            byte(n.x * 0.5 + 0.5),
            byte(n.y * 0.5 + 0.5),
            255,
        ]);
    }
    let (instructions, samples) = (raw.instructions, raw.samples);
    let mut policies = base;
    policies.extend(state.differences.clone());
    // A per-material decision for this bake; area censuses deduplicate
    // weight variants by their source material before summing this count.
    policies.insert(
        if gloss.is_some() {
            "material writes second-output x"
        } else {
            "material omits second-output x"
        }
        .into(),
        1,
    );
    policies.insert(
        if emissive.is_some() {
            "material writes emission RGB"
        } else {
            "material omits emission RGB"
        }
        .into(),
        1,
    );
    policies.insert(
        "constant layer weights, tint and detail".into(),
        size as u64 * size as u64,
    );
    Ok(BakedChannels {
        image: Bake {
            size,
            albedo,
            normal,
            gloss,
            emissive,
            instructions,
            samples,
            policies,
        },
        reflectance,
    })
}

impl PipelineState {
    /// The pixel program run over the state's grid (lifted from the state's container): what [`PipelineState::bake`]
    /// reads.
    pub fn evaluate(&self, threads: Threads) -> Result<Raw> {
        let shader = executor(&self.pixel.bytecode)?;
        evaluate_raw_with(
            threads,
            &shader,
            &self.pixel.constants,
            &self.pixel.evaluated_textures(),
            &self.pixel.evaluated_inputs(),
            Grid {
                size: self.size,
                period: self.period,
            },
            &TARGETS,
            Invocation::Scalar,
        )
        .map_err(evaluation)
    }

    /// The bake of what a pixel program wrote over this state's grid (`raw`: [`PipelineState::evaluate`], or another
    /// executor's outputs in its shape), by the bake's own rules: what [`Prepared::bake`] makes of its state.
    pub fn bake(&self, raw: &Raw) -> Result<Bake> {
        Ok(channels(self, raw, BTreeMap::new(), false)?.image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dither and camera-fade fields freeze to zero (both fades off), counted once as [`FADE_POLICY`].
    #[test]
    fn the_fade_fields_freeze_to_zero_and_are_counted() {
        let mut policies = BTreeMap::new();
        for parameter in [
            ObjectParameter::CameraFadeEnabled,
            ObjectParameter::CameraFadeEnd,
            ObjectParameter::CameraFadeInverseRange,
            ObjectParameter::DitherAlphaEnabled,
            ObjectParameter::DitherEnabled,
        ] {
            let words = frozen_words(
                &ConstantRole::Object(parameter),
                &FrozenFrame::static_bake(0.),
                &mut policies,
            )
            .unwrap();
            assert_eq!(words, Some(vec![0]), "{parameter:?}");
        }
        assert_eq!(policies, BTreeMap::from([(FADE_POLICY.to_owned(), 1)]));
    }

    fn prepared(writes: bool) -> Prepared {
        prepared_output(writes, None)
    }
    fn prepared_output(writes: bool, output: Option<(u8, [u32; 4])>) -> Prepared {
        let mut words = vec![0x50, 0];
        let operand = |kind: u32, index| [2 | (15 << 4) | (kind << 12) | (1 << 20), index];
        let mut instruction = |opcode: u32, args: &[u32]| {
            words.push(opcode | (((args.len() + 1) as u32) << 24));
            words.extend_from_slice(args);
        };
        // dcl_input_ps linear v0.xyzw: a pixel input is declared with an interpolation mode
        instruction(98 | (2 << 11), &operand(1, 0));
        for index in [0, 2] {
            instruction(101, &operand(2, index));
        }
        if output.is_some() {
            instruction(101, &operand(2, 3));
        }
        if writes {
            instruction(101, &operand(2, 1));
            instruction(101, &operand(2, 4));
        }
        for index in [0, 2] {
            let mut args = operand(2, index).to_vec();
            args.extend([
                2 | (4 << 12),
                0.5f32.to_bits(),
                0.75f32.to_bits(),
                0.5f32.to_bits(),
                0,
            ]);
            instruction(54, &args);
        }
        if writes {
            for index in [1, 4] {
                let mut args = operand(2, index).to_vec();
                args.extend([2 | (1 << 2) | (0xe4 << 4) | (1 << 12) | (1 << 20), 0]);
                instruction(54, &args);
            }
        }
        if let Some((mask, bits)) = output {
            let mut destination = operand(2, 3);
            destination[0] = (destination[0] & !(15 << 4)) | ((mask as u32) << 4);
            let mut args = destination.to_vec();
            args.push(2 | (4 << 12));
            args.extend(bits);
            instruction(54, &args);
        }
        instruction(62, &[]);
        words[1] = words.len() as u32;
        // the signatures state what the program writes: o0 and o2 whole, o1 and o4 when `writes`, o3 as `output` says
        let mut targets = vec![(0, 15), (2, 15)];
        if writes {
            targets.extend([(1, 15), (4, 15)]);
        }
        targets.extend(output.map(|(mask, _)| (3, mask)));
        let bytes = container(
            &words,
            &signature(&[("TEXCOORD", 0, 0, 0, 15, 15)]),
            &signature(
                &targets
                    .iter()
                    .map(|&(register, mask)| ("SV_Target", register, 64, register, mask, 0))
                    .collect::<Vec<_>>(),
            ),
        );
        Prepared {
            bytecode: bytes.as_slice().into(),
            shader: ProgramShader::new(primase_dxbc::lift(&bytes).unwrap()).unwrap(),
            constants: Constants::new(&Layout::default()),
            textures: BTreeMap::new(),
            alpha: None,
            vertex: None,
            policies: BTreeMap::new(),
            decoded_bytes: 0,
            period: [1.; 2],
            period_exact: true,
            reads_second_uv: false,
            largest_colour_dimension: 0,
        }
    }
    /// A signature chunk: its elements as (name, index, system value, register, mask, read-write mask), floats.
    fn signature(elements: &[(&str, u32, u32, u32, u8, u8)]) -> Vec<u8> {
        let mut records = Vec::new();
        let mut names = Vec::new();
        let names_at = 8 + 24 * elements.len() as u32;
        for &(name, index, system, register, mask, read_write) in elements {
            for word in [names_at + names.len() as u32, index, system, 3, register] {
                records.extend(word.to_le_bytes());
            }
            records.extend([mask, read_write, 0, 0]);
            names.extend(name.bytes().chain([0]));
        }
        let mut out = Vec::new();
        for word in [elements.len() as u32, 8] {
            out.extend(word.to_le_bytes());
        }
        out.extend(records);
        out.extend(names);
        out.resize(out.len().next_multiple_of(4), 0);
        out
    }
    /// A DXBC container around a ps_5_0 program's `words` and its two signatures.
    fn container(words: &[u32], input: &[u8], output: &[u8]) -> Vec<u8> {
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let chunks: [(&[u8; 4], &[u8]); 3] = [(b"ISGN", input), (b"OSGN", output), (b"SHEX", &code)];
        let mut at = 32 + 4 * chunks.len();
        let mut offsets = Vec::new();
        for (_, data) in &chunks {
            offsets.push(at as u32);
            at += 8 + data.len();
        }
        let mut out = b"DXBC".to_vec();
        out.extend([0u8; 16]);
        for word in [1, at as u32, chunks.len() as u32] {
            out.extend(word.to_le_bytes());
        }
        for offset in offsets {
            out.extend(offset.to_le_bytes());
        }
        for (tag, data) in chunks {
            out.extend(tag);
            out.extend((data.len() as u32).to_le_bytes());
            out.extend(data);
        }
        out
    }

    #[test]
    fn optional_capture_preserves_bits_masks_and_public_bake() {
        for output in [
            None,
            Some((7, [0x3e31_5cac; 4])),
            Some((3, [0x3e31_5cac; 4])),
            Some((7, [f32::NAN.to_bits(); 4])),
            Some((7, [f32::INFINITY.to_bits(); 4])),
        ] {
            let prepared = prepared_output(true, output);
            let public = prepared
                .bake_channels(Threads::ONE, 2, [1.; 2], Inputs::default(), false)
                .unwrap();
            assert!(public.reflectance.is_none());
            let internal = prepared
                .bake_with_reflectance(Threads::ONE, 2, [1.; 2], Inputs::default())
                .unwrap();
            assert_eq!(public.image.albedo, internal.image.albedo);
            assert_eq!(public.image.normal, internal.image.normal);
            assert_eq!(public.image.gloss, internal.image.gloss);
            assert_eq!(public.image.emissive, internal.image.emissive);
            assert_eq!(public.image.policies, internal.image.policies);
            assert_eq!(public.image.instructions, internal.image.instructions);
            assert_eq!(public.image.samples, internal.image.samples);
            let pixels = internal.reflectance.unwrap();
            assert_eq!(pixels.len(), 4);
            for pixel in pixels {
                assert_eq!(pixel.mask, output.map_or(0, |v| v.0));
                for c in 0..3 {
                    if pixel.mask & (1 << c) != 0 {
                        assert_eq!(pixel.bits[c], output.unwrap().1[c]);
                    }
                }
            }
        }
    }

    /// On several threads the bake is the one-thread bake: every channel, the reflectance, the counts.
    #[test]
    fn on_any_thread_count_the_bake_is_the_serial_one() {
        for emissive in [true, false] {
            let prepared = prepared(emissive);
            let bake = |n: usize| {
                prepared
                    .bake_with_reflectance(Threads::new(n).unwrap(), 7, [4., 2.], Inputs::default())
                    .unwrap()
            };
            let one = bake(1);
            for n in [2, 5] {
                let many = bake(n);
                assert_eq!(many.image.albedo, one.image.albedo, "{n} threads");
                assert_eq!(many.image.normal, one.image.normal, "{n} threads");
                assert_eq!(
                    many.image
                        .gloss
                        .as_ref()
                        .map(|g| g.iter().map(|v| v.to_bits()).collect::<Vec<_>>()),
                    one.image
                        .gloss
                        .as_ref()
                        .map(|g| g.iter().map(|v| v.to_bits()).collect::<Vec<_>>()),
                    "{n} threads"
                );
                assert_eq!(
                    many.image
                        .emissive
                        .as_ref()
                        .map(|e| e.iter().map(|v| v.map(f32::to_bits)).collect::<Vec<_>>()),
                    one.image
                        .emissive
                        .as_ref()
                        .map(|e| e.iter().map(|v| v.map(f32::to_bits)).collect::<Vec<_>>()),
                    "{n} threads"
                );
                assert_eq!(many.image.policies, one.image.policies, "{n} threads");
                assert_eq!(many.image.instructions, one.image.instructions, "{n} threads");
                assert_eq!(many.image.samples, one.image.samples, "{n} threads");
                assert_eq!(many.reflectance, one.reflectance, "{n} threads");
            }
        }
    }

    /// The pipeline state is what the bake evaluates: its pixel stage, run alone over its grid, gives the bake's colours.
    #[test]
    fn the_bake_is_its_pipeline_state_evaluated() {
        let prepared = prepared(true);
        let state = prepared.pipeline_state(5, [4., 2.], Inputs::default()).unwrap();
        assert_eq!((state.size, state.period), (5, [4., 2.]));
        assert_eq!(&*state.pixel.bytecode, &*prepared.bytecode);
        assert!(state.vertex.is_none() && state.alpha.is_none() && state.differences.is_empty());
        let texcoord = state
            .pixel
            .inputs
            .iter()
            .find(|((name, index), _)| name.eq_ignore_ascii_case("TEXCOORD") && *index == 0)
            .map(|(_, lanes)| *lanes)
            .unwrap();
        assert_eq!(texcoord, [Lane::U, Lane::V, Lane::U, Lane::V]);
        let raw = evaluate_raw_with(
            Threads::ONE,
            &ProgramShader::new(primase_dxbc::lift(&state.pixel.bytecode).unwrap()).unwrap(),
            &state.pixel.constants,
            &state.pixel.evaluated_textures(),
            &state.pixel.evaluated_inputs(),
            Grid {
                size: state.size,
                period: state.period,
            },
            &TARGETS,
            Invocation::Scalar,
        )
        .unwrap();
        let bake = prepared.bake(5, [4., 2.], Inputs::default()).unwrap();
        for (pixel, (colour, _)) in raw.pixels[2].iter().enumerate() {
            for (c, &bits) in colour[..3].iter().enumerate() {
                let linear = f64::from(f32::from_bits(bits)).clamp(0., 1.);
                let byte = (primase_image::linear_to_srgb(linear) * 255.).round_ties_even() as u8;
                assert_eq!(bake.albedo[pixel * 4 + c], byte, "pixel {pixel} lane {c}");
            }
        }
    }

    /// A state survives its encoding: decoded and encoded again it is the same description and files, images,
    /// constants and the alpha included, and the decoded pixel stage evaluates to the same bits.
    #[test]
    fn a_pipeline_state_round_trips_through_its_encoding() {
        let mut state = prepared(true)
            .pipeline_state(3, [1.5, 2.], Inputs::default())
            .unwrap();
        let texels: Vec<u8> = (0..2 * 3 * 4).map(|i| (i * 37 % 251) as u8).collect();
        let image = Arc::new(
            BaseMipBilinearWrap::addressed(2, 3, &texels, true, [Address::Clamp, Address::MirrorOnce])
                .unwrap(),
        );
        state.pixel.textures.insert(4, SlotTexture::Image(image.clone()));
        state
            .pixel
            .textures
            .insert(6, SlotTexture::Constant([0.25, -1., 0.5, 1.]));
        state.alpha = Some(AlphaState {
            image,
            scale: [2., 0.5],
        });
        let encoded = state.encode();
        assert_eq!(encoded.files["t4.rgba"], texels);
        let decoded = PipelineState::decode(&encoded).unwrap();
        let again = decoded.encode();
        assert_eq!(again.description, encoded.description);
        assert_eq!(again.files, encoded.files);
        let run = |s: &PipelineState| {
            evaluate_raw_with(
                Threads::ONE,
                &ProgramShader::new(primase_dxbc::lift(&s.pixel.bytecode).unwrap()).unwrap(),
                &s.pixel.constants,
                &s.pixel.evaluated_textures(),
                &s.pixel.evaluated_inputs(),
                Grid {
                    size: s.size,
                    period: s.period,
                },
                &TARGETS,
                Invocation::Scalar,
            )
            .unwrap()
            .pixels
        };
        assert_eq!(run(&decoded), run(&state));
    }

    #[test]
    fn emission_preserves_hdr_rgb_and_row_major_layout() {
        let present = prepared(true).bake(3, [4., 2.], Inputs::default()).unwrap();
        let absent = prepared(false).bake(3, [4., 2.], Inputs::default()).unwrap();
        let values = present.emissive.as_ref().unwrap();
        assert_eq!(values.len(), (present.size * present.size) as usize);
        for (i, &value) in values.iter().enumerate() {
            let u = ((i % 3) as f32 + 0.5) / 3. * 4.;
            let v = ((i / 3) as f32 + 0.5) / 3. * 2.;
            assert_eq!(value.map(f32::to_bits), [u, v, u].map(f32::to_bits));
        }
        assert!(values.iter().flatten().any(|&v| v > 1.));
        assert!(absent.emissive.is_none());
        assert_eq!(present.albedo, absent.albedo);
        assert_eq!(present.normal, absent.normal);
        assert_eq!(present.policies["material writes emission RGB"], 1);
        assert_eq!(absent.policies["material omits emission RGB"], 1);
    }

    #[test]
    fn gloss_preserves_shader_channel_and_row_major_layout() {
        let present = prepared(true).bake(3, [1.; 2], Inputs::default()).unwrap();
        let absent = prepared(false).bake(3, [1.; 2], Inputs::default()).unwrap();
        let values = present.gloss.as_ref().unwrap();
        assert_eq!(values.len(), (present.size * present.size) as usize);
        for (i, &value) in values.iter().enumerate() {
            assert_eq!(value.to_bits(), (((i % 3) as f32 + 0.5) / 3.).to_bits());
        }
        assert!(absent.gloss.is_none());
        assert_eq!(present.albedo, absent.albedo);
        assert_eq!(present.normal, absent.normal);
        assert_eq!(present.policies["material writes second-output x"], 1);
        assert_eq!(absent.policies["material omits second-output x"], 1);
    }
}
