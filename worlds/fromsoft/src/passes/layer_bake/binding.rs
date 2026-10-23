use super::BaseMipBilinearWrap;
use super::resolve::{self, Unbound, resolve, textures_of};
use super::sampler::address;
use crate::{Result, invalid, materials::LayeredMaterial};
use primase_core::Entity;
use primase_core::parallel::Threads;
use primase_core::shader::{
    AddressMode, ConstantRole, FrameParameter, ObjectParameter, StageBinding, TextureSource, ViewParameter,
};
use primase_dxbc::Shader;
use primase_image::sampling::Address;
use primase_shader::{
    Constants, Grid, Input, Interface, Invocation, Lane, Layout, Texture, evaluate_raw_with,
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
/// Evaluated by the toolkit ([`evaluate_raw`], one pixel at a time): the program, its constants and a texture per
/// sampled slot, frozen; the main albedo's alpha, sampled beside the program at its own scale.
#[derive(Debug)]
pub struct Prepared {
    shader: Shader,
    constants: Constants,
    textures: BTreeMap<u32, Texture>,
    alpha: Option<(Arc<BaseMipBilinearWrap>, [f32; 2])>,
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
fn dxbc(e: primase_dxbc::Error) -> crate::Error {
    invalid("layer DXBC", e.word * 4, e.detail)
}
fn evaluation(e: primase_shader::Error) -> crate::Error {
    invalid("layer bake", e.at, e.detail)
}

/// What a static bake freezes of the engine's per-view, per-frame and per-object values: the roles a binding states
/// ([`ConstantRole`]) and never values. These are the bake's choices, reported as policies when a binding reads them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenFrame {
    /// Seconds of global time (`Frame(Time)`), which scrolls some layers' texture coordinates.
    pub time: f32,
    /// `Frame(Exposure)`: material radiance is baked at unit exposure, not a runtime camera's pre-exposure.
    pub exposure: f32,
    /// `Frame(AlbedoTint)`: white.
    pub albedo_tint: [f32; 4],
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
}
impl FrozenFrame {
    /// The static bake's frame, its global time `time` seconds.
    pub fn static_bake(time: f32) -> Self {
        let mut inverse_view_projection = [0.; 16];
        inverse_view_projection[15] = 1.;
        Self {
            time,
            exposure: 1.,
            albedo_tint: [1.; 4],
            colour_slot: 0.,
            camera_position: [0., 1000., 0., 1.],
            inverse_view_projection,
            world: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.],
            instance_colour: [1.; 4],
        }
    }
}

/// The program's common repeat over the UV scales of the texture slots it samples that the material binds, and
/// whether it is exact ([`super::period::common_period`]).
pub(crate) fn sampled_period(interface: &Interface, material: &LayeredMaterial) -> Result<([f32; 2], bool)> {
    let slots = resolve::slots(material)?;
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
        let shader = Shader::parse(bytes).map_err(dxbc)?;
        // the textures stand as entities of a scratch world, each with the view the caller decoded
        let mut scratch = primase_core::World::default();
        let program = scratch.spawn();
        let mut views: BTreeMap<Entity, Arc<BaseMipBilinearWrap>> = BTreeMap::new();
        let mut known: BTreeMap<(String, bool), Option<Entity>> = BTreeMap::new();
        let resolution = resolve(&shader, program, material, Unbound::Preview, &mut |stem, srgb| {
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
        })?;
        Self::from_binding(
            shader,
            &resolution.stage,
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
    /// by name), the shader family the bake supports and the sampled layers' UV scales.
    pub fn from_binding(
        shader: Shader,
        stage: &StageBinding,
        material: &LayeredMaterial,
        frame: &FrozenFrame,
        mut texture: impl FnMut(Entity, bool, [Address; 2]) -> Result<Arc<BaseMipBilinearWrap>>,
        mut stem: impl FnMut(&str, bool, [Address; 2]) -> Result<Option<Arc<BaseMipBilinearWrap>>>,
    ) -> Result<Self> {
        let (program, interface) = (&shader.program, &shader.interface);
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
                program.samplers().len() as u64,
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
        let layout = Layout::of(&shader).map_err(evaluation)?;
        // the buffers the program reads, by slot: their words
        let words: BTreeMap<u32, usize> = program
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
            let bits = |values: &[f32]| -> Vec<u32> { values.iter().map(|v| v.to_bits()).collect() };
            let mut values = match &c.role {
                ConstantRole::Material(words) => words.clone(),
                ConstantRole::View(ViewParameter::CameraPosition) => bits(&frame.camera_position),
                ConstantRole::View(ViewParameter::InverseViewProjection) => {
                    bits(&frame.inverse_view_projection)
                }
                ConstantRole::Object(ObjectParameter::World) => bits(&frame.world),
                ConstantRole::Object(ObjectParameter::InstanceColour) => bits(&frame.instance_colour),
                ConstantRole::Frame(FrameParameter::AlbedoTint) => bits(&frame.albedo_tint),
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
                    continue;
                }
                ConstantRole::Unknown { why } => {
                    *policies.entry(why.clone()).or_default() += 1;
                    continue;
                }
                other => {
                    return Err(invalid(
                        "layer binding",
                        0,
                        format!("no frozen value for {other:?}"),
                    ));
                }
            };
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
        let slots = resolve::slots(material)?;
        let (period, period_exact) = sampled_period(interface, material)?;
        let mut textures = BTreeMap::new();
        let mut cache = BTreeMap::<(Entity, bool, [Option<AddressMode>; 2]), Arc<BaseMipBilinearWrap>>::new();
        let mut largest_colour_dimension = 0;
        for bound in &stage.textures {
            let resource = interface
                .resource(bound.resource)
                .ok_or_else(|| invalid("layer binding", 0, "a texture the shader lacks"))?;
            let register = resolve::register(resource)?;
            let slot = resolve::texture_slot(resource)?;
            let role = slots
                .get(&slot)
                .map_or_else(|| resolve::declared_role(material, slot), |(role, _)| Ok(*role))?;
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
                    Texture::Image(image.clone() as Arc<dyn primase_shader::Image>)
                }
                TextureSource::Default { value, why } => {
                    *policies.entry(why.clone()).or_default() += 1;
                    Texture::Constant(*value)
                }
                TextureSource::Missing => continue,
            };
            textures.insert(register, source);
        }
        for &slot in program.textures() {
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
            shader,
            constants,
            textures,
            alpha,
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
    fn bake_channels(
        &self,
        threads: Threads,
        size: u32,
        period: [f32; 2],
        params: Inputs,
        capture_reflectance: bool,
    ) -> Result<BakedChannels> {
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
        for s in self.shader.interface.inputs.iter().filter(|s| s.used) {
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
        if !inputs
            .keys()
            .any(|(name, index)| name.eq_ignore_ascii_case("TEXCOORD") && *index == 0)
        {
            return Err(invalid("layer binding", 0, "missing TEXCOORD0"));
        }
        // the G-buffer normal, the second output's x, the albedo, the reflectance and the emission
        let raw = evaluate_raw_with(
            threads,
            &self.shader,
            &self.constants,
            &self.textures,
            &inputs,
            Grid { size, period },
            &[(0, 7), (1, 1), (2, 7), (3, 7), (4, 7)],
            Invocation::Scalar,
        )
        .map_err(evaluation)?;
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
            albedo.push(if let Some((image, scale)) = &self.alpha {
                byte(image.sample([uv[0] * scale[0], uv[1] * scale[1]])?[3])
            } else {
                255
            });
            // Invert the observed G-buffer bias from the binary operand,
            // not its rounded decimal rendering in a disassembly.
            let bias = f32::from_bits(0x3f002008);
            let n = primase_meshops::vector::Vec3::from([
                (n[0] - bias) * 2.,
                (n[1] - bias) * 2.,
                (n[2] - bias) * 2.,
            ])
            .normalize_or([0., 1., 0.].into());
            normal.extend([
                byte(-n.z * 0.5 + 0.5),
                byte(n.x * 0.5 + 0.5),
                byte(n.y * 0.5 + 0.5),
                255,
            ]);
        }
        let (instructions, samples) = (raw.instructions, raw.samples);
        let mut policies = self.policies.clone();
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        Prepared {
            shader: Shader {
                program: primase_dxbc::Program::from_words(&words).unwrap(),
                interface: primase_dxbc::Interface {
                    stage: primase_shader::Stage::Pixel,
                    inputs: vec![primase_dxbc::Varying {
                        label: primase_shader::Label {
                            name: "TEXCOORD".into(),
                            index: 0,
                        },
                        system: None,
                        value: primase_shader::Value {
                            scalar: primase_shader::ScalarType::Float,
                            width: 4,
                        },
                        interp: None,
                        used: true,
                        provenance: primase_shader::VaryingProvenance {
                            d3d: Some(primase_shader::D3dSignature {
                                register: 0,
                                mask: 15,
                                read_write_mask: 15,
                            }),
                        },
                    }],
                    outputs: vec![],
                    resources: vec![],
                },
            },
            constants: Constants::new(&Layout::default()),
            textures: BTreeMap::new(),
            alpha: None,
            policies: BTreeMap::new(),
            decoded_bytes: 0,
            period: [1.; 2],
            period_exact: true,
            reads_second_uv: false,
            largest_colour_dimension: 0,
        }
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
