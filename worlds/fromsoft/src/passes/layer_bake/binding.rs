use super::BaseMipBilinearWrap;
use crate::{Result, invalid, materials::LayeredMaterial, mtd::Value};
use primase_dxbc::{Shader, Type};
use primase_shader::{Constants, Grid, Input, Invocation, Lane, Layout, PreparedProgram, Texture};
use std::{collections::BTreeMap, sync::Arc};

/// Frozen per-vertex input for one baked layer-weight group. Choosing groups
/// and replacing the second UV set with the first are separate lowering policies.
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
/// Evaluated by the toolkit ([`PreparedProgram`], one pixel at a time): the program, its constants and a texture per
/// sampled slot, frozen; the main albedo's alpha, sampled beside the program at its own scale.
#[derive(Debug)]
pub struct Prepared {
    program: PreparedProgram<Shader>,
    alpha: Option<(Arc<BaseMipBilinearWrap>, [f32; 2])>,
    pub policies: BTreeMap<String, u64>,
    pub decoded_bytes: usize,
    /// Static repeat policy calculated from resources used by this shader.
    pub period: [f32; 2],
    pub period_exact: bool,
    largest_colour_dimension: u32,
}
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
fn fields(
    name: &str,
    offset: u32,
    ty: &Type,
    out: &mut BTreeMap<String, (u32, usize)>,
    slot: u32,
) -> Result<()> {
    if ty.members.is_empty() {
        if !offset.is_multiple_of(4) {
            return Err(invalid("layer binding", offset as usize, "unaligned field"));
        }
        if out.insert(name.to_owned(), (slot, offset as usize / 4)).is_some() {
            return Err(invalid(
                "layer binding",
                offset as usize,
                format!("ambiguous reflected field {name}"),
            ));
        }
    }
    for m in &ty.members {
        fields(
            &m.name,
            offset
                .checked_add(m.offset)
                .ok_or_else(|| invalid("layer binding", 0, "member overflow"))?,
            &m.ty,
            out,
            slot,
        )?;
    }
    Ok(())
}
fn slot_name(kind: &str) -> Option<(u32, &str)> {
    let rest = kind.rsplit_once("Texture2D_")?.1;
    let (slot, role) = rest.split_once('_')?;
    Some((slot.parse().ok()?, role.split('_').next()?))
}
fn declared_role(material: &LayeredMaterial, slot: u32) -> Result<&str> {
    let mut role = None;
    for texture in &material.definition.textures {
        if let Some((index, found)) = slot_name(&texture.kind)
            && index == slot
        {
            if role.is_some_and(|r| r != found) {
                return Err(invalid("layer binding", 0, "conflicting declared texture roles"));
            }
            role = Some(found);
        }
    }
    Ok(role.unwrap_or("?"))
}
/// The frozen inputs must be finite: they become shader constants.
fn check_params(params: Inputs) -> Result<()> {
    if !params
        .weights_snow
        .iter()
        .chain(&params.tint_color)
        .chain(std::iter::once(&params.detail))
        .all(|v| v.is_finite())
    {
        return Err(invalid("layer bake", 0, "nonfinite frozen vertex input"));
    }
    Ok(())
}
impl Prepared {
    /// Frozen-frame preview binding: caller supplies decoded views by texture
    /// name and sRGB flag. Missing slots use explicitly counted preview defaults;
    /// no missing resource is silently supplied by the generic interpreter.
    /// A program that reads the global time (`FC_GlobalTime`, which scrolls
    /// some layers' texture coordinates) is sampled at `sample_time` seconds,
    /// counted; the time of day stays at frozen zero, counted where reflected.
    pub fn new(
        bytes: &[u8],
        material: &LayeredMaterial,
        sample_time: f32,
        mut image: impl FnMut(&str, bool) -> Result<Option<Arc<BaseMipBilinearWrap>>>,
    ) -> Result<Self> {
        let shader = Shader::parse(bytes).map_err(dxbc)?;
        let (program, reflection) = (&shader.program, &shader.reflection);
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
        let prefix = format!("{}_snp_", family.replace(['[', ']'], "_"));
        let mut policies = BTreeMap::from([
            (
                "frozen identity world, white instance colour and overhead camera".into(),
                1,
            ),
            (
                "base-mip bilinear wrap overrides runtime sampler state".into(),
                program.samplers().len() as u64,
            ),
            ("second UV set replaced by first for static bake".into(), 1),
        ]);
        let mut offsets = BTreeMap::new();
        for buffer in &reflection.buffers {
            let binding = reflection
                .resources
                .iter()
                .find(|r| r.kind == 0 && r.name == buffer.name)
                .ok_or_else(|| invalid("layer binding", 0, "buffer has no resource binding"))?;
            for v in &buffer.variables {
                fields(&v.name, v.offset, &v.ty, &mut offsets, binding.slot)?;
            }
        }
        // the buffers the program reads, by slot: their words
        let words: BTreeMap<u32, usize> = program
            .constant_buffers()
            .iter()
            .map(|&(slot, rows)| (slot, rows as usize * 4))
            .collect();
        let mut constants = Constants::new(&Layout {
            buffers: words.iter().map(|(&slot, &n)| (slot, n as u32)).collect(),
            fields: BTreeMap::new(),
        });
        // a field's words from `word` on, those inside a buffer the program reads (the program reads no others)
        let mut set = |name: &str, word: usize, values: &[f32]| {
            if let Some(&(slot, at)) = offsets.get(name)
                && let Some(&n) = words.get(&slot)
            {
                let start = at + word;
                let bits: Vec<u32> = values
                    .iter()
                    .take(n.saturating_sub(start))
                    .map(|v| v.to_bits())
                    .collect();
                if !bits.is_empty() {
                    constants
                        .set_words(slot, start as u32, &bits)
                        .expect("words inside a buffer the program reads");
                }
            }
        };
        let mut slots = BTreeMap::new();
        // Numeric-slot ties prefer the MTD declaration, retaining other bindings
        // in the source LayeredMaterial for later policies.
        for (kind, binding) in &material.bindings {
            if !binding.scale.iter().all(|v| v.is_finite()) {
                return Err(invalid("layer binding", 0, "nonfinite texture scale"));
            }
            if let Some((slot, role)) = slot_name(kind) {
                let declared = material.definition.textures.iter().any(|t| t.kind == *kind);
                if !slots.contains_key(&slot) || declared {
                    slots.insert(slot, (role, binding));
                }
            }
        }
        let mut textures = BTreeMap::new();
        let sampled_scales: Vec<_> = reflection
            .resources
            .iter()
            .filter(|r| r.kind == 2)
            .filter_map(|r| r.name.rsplit_once("Texture2D_")?.1.parse::<u32>().ok())
            .filter_map(|slot| slots.get(&slot).map(|(_, b)| b.scale))
            .collect();
        let main_scale = slots
            .get(&8)
            .map(|(_, b)| b.scale)
            .or_else(|| sampled_scales.first().copied())
            .unwrap_or([1.; 2]);
        let (period, period_exact) = super::period::common_period(&sampled_scales, main_scale)?;
        let mut cache = BTreeMap::<(String, bool), Option<Arc<BaseMipBilinearWrap>>>::new();
        let mut largest_colour_dimension = 0;
        for resource in reflection.resources.iter().filter(|r| r.kind == 2) {
            if resource.count != 1 || resource.dimension != 4 {
                return Err(invalid(
                    "layer binding",
                    0,
                    "only individual Texture2D resources are supported",
                ));
            }
            let slot = resource
                .name
                .rsplit_once("Texture2D_")
                .and_then(|(_, s)| s.parse::<u32>().ok())
                .ok_or_else(|| {
                    invalid(
                        "layer binding",
                        0,
                        format!("unrecognized texture binding {}", resource.name),
                    )
                })?;
            // A declared slot retains its semantic role even without a path.
            // In particular an unbound normal uses the named flat-normal
            // preview default, not the unknown-resource zero default.
            let fallback_role = declared_role(material, slot)?;
            let (role, binding) = slots
                .get(&slot)
                .map_or((fallback_role, None), |(role, b)| (*role, Some(*b)));
            let mut source = None;
            if let Some(b) = binding {
                let key = (b.texture.clone(), role == "AlbedoMap");
                if !cache.contains_key(&key) {
                    cache.insert(key.clone(), image(&key.0, key.1)?);
                }
                source = cache[&key]
                    .clone()
                    .map(|t| Texture::Image(t as Arc<dyn primase_shader::Image>));
                if role == "AlbedoMap"
                    && let Some(t) = &cache[&key]
                {
                    largest_colour_dimension =
                        largest_colour_dimension.max(*t.dimensions().iter().max().expect("two axes") as u32);
                }
                set(
                    "FC_TextureTilingScale",
                    resource.slot as usize * 4,
                    &[b.scale[0], b.scale[1], 0., 0.],
                );
            } else {
                set(
                    "FC_TextureTilingScale",
                    resource.slot as usize * 4,
                    &[1., 1., 0., 0.],
                );
            }
            if source.is_none() {
                *policies
                    .entry(format!("preview default for missing {role} binding"))
                    .or_default() += 1;
                source = Some(Texture::Constant(match role {
                    "AlbedoMap" => [0.5, 0.5, 0.5, 1.],
                    "NormalMap" => [0.5, 0.5, 1., 1.],
                    _ => [0.; 4],
                }));
            }
            textures.insert(resource.slot, source.expect("explicit default"));
        }
        for &slot in program.textures() {
            if !textures.contains_key(&slot) {
                return Err(invalid(
                    "layer binding",
                    0,
                    format!("t{slot} has no named resource"),
                ));
            }
        }
        set("FC_DifMapMulCol", 0, &[1.; 4]);
        set("mWorld", 0, &[1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.]);
        set("aColor", 0, &[1.; 4]);
        set("FC_ColorSlotNo", 0, &[0.]);
        set(
            "FC_MatrixInvViewProj",
            0,
            &[0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 1.],
        );
        set("SC_RenderCameraPos", 0, &[0., 1000., 0., 1.]);
        if !sample_time.is_finite() {
            return Err(invalid("layer binding", 0, "nonfinite sample time"));
        }
        set("FC_GlobalTime", 0, &[sample_time]);
        if offsets.contains_key("FC_GlobalTime") {
            policies.insert("global time at the program's sample time".into(), 1);
        }
        if offsets.contains_key("FC_TimeOfDay") {
            policies.insert("time of day left at frozen zero".into(), 1);
        }
        // Material radiance is baked at unit exposure, not a runtime camera's
        // pre-exposure. Otherwise zero-initialized constants suppress emission.
        set("SC_PreExposure", 0, &[1.]);
        if offsets.contains_key("SC_PreExposure") {
            policies.insert("unit pre-exposure for static material bake".into(), 1);
        }
        for name in offsets
            .keys()
            .filter(|n| n.starts_with(&prefix) && !n.ends_with("_PADDING"))
        {
            let Some(param) = material.definition.parameters.iter().find(|p| p.name == *name) else {
                *policies
                    .entry("missing SAT parameter left at frozen zero".into())
                    .or_default() += 1;
                continue;
            };
            let mut values = match &param.value {
                Value::Bool(b) => vec![u8::from(*b) as f32],
                Value::Int(v) => v.iter().map(|&i| i as f32).collect(),
                Value::Float(v) => v.clone(),
            };
            if name.contains("_color_") {
                let intensity = material
                    .definition
                    .parameters
                    .iter()
                    .find(|p| p.name == format!("{name}-Intensity"))
                    .and_then(|p| match &p.value {
                        Value::Float(v) => v.first().copied(),
                        _ => None,
                    })
                    .unwrap_or(1.);
                for v in values.iter_mut().take(3) {
                    *v *= intensity;
                }
                if values.len() == 3 {
                    values.push(1.);
                }
            }
            set(name, 0, &values);
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
        if let Some((_, (_, binding))) = main {
            let key = (binding.texture.clone(), true);
            if !cache.contains_key(&key) {
                cache.insert(key.clone(), image(&key.0, true)?);
            }
            if let Some(t) = cache[&key].clone() {
                alpha = Some((
                    t,
                    if binding.scale.iter().all(|&v| v > 0.) {
                        binding.scale
                    } else {
                        [1.; 2]
                    },
                ));
            }
        }
        if alpha.is_none() {
            policies.insert("opaque alpha because main albedo is missing".into(), 1);
        }
        let decoded_bytes = cache.values().flatten().map(|t| t.decoded_bytes()).sum();
        Ok(Self {
            program: PreparedProgram::new(shader, constants, textures),
            alpha,
            policies,
            decoded_bytes,
            period,
            period_exact,
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
    /// The parsed source program: its instructions and reflection, read-only.
    pub fn shader(&self) -> &Shader {
        self.program.shader()
    }
    /// The constants the program is bound with, written rows included.
    pub fn constants(&self) -> &Constants {
        self.program.constants()
    }
    /// Every texture slot the program samples, as bound (an image or one constant value).
    pub fn textures(&self) -> &BTreeMap<u32, Texture> {
        self.program.textures()
    }
    /// The shared shader contract, prepared without evaluating or baking it. The policies of this binding recipe
    /// remain in `policies`; its alpha sampler and interpretation of outputs remain FromSoft data.
    pub fn program(&self) -> &PreparedProgram<Shader> {
        &self.program
    }
    /// The main albedo's alpha sampler and its scale, when one was bound.
    pub fn alpha(&self) -> Option<(&BaseMipBilinearWrap, [f32; 2])> {
        self.alpha.as_ref().map(|(image, scale)| (image.as_ref(), *scale))
    }
    /// Every used input by its semantic: the grid's UV in TEXCOORD0 for a bake, or `uv` for one invocation.
    fn bind_inputs(&self, params: Inputs, uv: Option<[f32; 2]>) -> Result<BTreeMap<(String, u32), Input>> {
        let mut inputs = BTreeMap::new();
        for s in self
            .shader()
            .reflection
            .inputs
            .iter()
            .filter(|s| s.used_mask != 0)
        {
            let input = match (s.name.to_ascii_uppercase().as_str(), s.index) {
                ("POSITION", 1) => Input::Constant([0., 0., 0., 1.]),
                ("NORMAL", 0) => Input::Constant([0., 1., 0., 0.]),
                ("TANGENT", 0) => Input::Constant([1., 0., 0., 1.]),
                ("TEXCOORD", 0) => match uv {
                    Some([u, v]) => Input::Constant([u, v, u, v]),
                    None => Input::Lanes([Lane::U, Lane::V, Lane::U, Lane::V]),
                },
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
                        format!("unclassified input {}{}", s.name, s.index),
                    ));
                }
            };
            inputs.insert((s.name.clone(), s.index), input);
        }
        if !inputs
            .keys()
            .any(|(name, index)| name.eq_ignore_ascii_case("TEXCOORD") && *index == 0)
        {
            return Err(invalid("layer binding", 0, "missing TEXCOORD0"));
        }
        Ok(inputs)
    }
    /// Run one pixel invocation at `uv` with the frozen inputs `params`: the five output registers (0 G-buffer normal,
    /// 1 gloss, 2 albedo, 3 reflectance, 4 emission) as the program wrote them, each register's bits and written
    /// lanes. The program's constants and textures are this `Prepared`'s. The invocation has no screen derivatives; an
    /// implicit-LOD sample uses the bound sampler's base mip.
    pub fn evaluate_pixel(&self, params: Inputs, uv: [f32; 2]) -> Result<[([u32; 4], u8); 5]> {
        check_params(params)?;
        if !uv.iter().all(|v| v.is_finite()) {
            return Err(invalid("layer binding", 0, "nonfinite pixel uv"));
        }
        let inputs = self.bind_inputs(params, Some(uv))?;
        let raw = self
            .program
            .as_ref()
            .evaluate_raw(
                &inputs,
                Grid {
                    size: 1,
                    period: [1.; 2],
                },
                &[(0, 7), (1, 1), (2, 7), (3, 7), (4, 7)],
                Invocation::Scalar,
            )
            .map_err(evaluation)?;
        let mut out = [([0u32; 4], 0u8); 5];
        if raw.discarded.iter().any(|&v| v) {
            return Err(invalid(
                "layer binding",
                0,
                "pixel discarded; this output-only oracle cannot represent coverage",
            ));
        }
        for (slot, pixels) in out.iter_mut().zip(raw.pixels) {
            *slot = pixels
                .into_iter()
                .next()
                .ok_or_else(|| invalid("layer binding", 0, "no pixel in a one-pixel evaluation"))?;
        }
        Ok(out)
    }
    pub fn bake(&self, size: u32, period: [f32; 2], params: Inputs) -> Result<Bake> {
        Ok(self.bake_channels(size, period, params, false)?.image)
    }
    pub(crate) fn bake_with_reflectance(
        &self,
        size: u32,
        period: [f32; 2],
        params: Inputs,
    ) -> Result<BakedChannels> {
        self.bake_channels(size, period, params, true)
    }
    fn bake_channels(
        &self,
        size: u32,
        period: [f32; 2],
        params: Inputs,
        capture_reflectance: bool,
    ) -> Result<BakedChannels> {
        check_params(params)?;
        if size == 0 || size > 4096 || !period.iter().all(|v| v.is_finite() && *v > 0.) {
            return Err(invalid("layer bake", 0, "invalid size or period"));
        }
        let inputs = self.bind_inputs(params, None)?;
        // the G-buffer normal, the second output's x, the albedo, the reflectance and the emission
        let raw = self
            .program
            .as_ref()
            .evaluate_raw(
                &inputs,
                Grid { size, period },
                &[(0, 7), (1, 1), (2, 7), (3, 7), (4, 7)],
                Invocation::Scalar,
            )
            .map_err(evaluation)?;
        let [normals, seconds, colours, reflected, emitted] = &raw.pixels[..] else {
            unreachable!("five targets requested");
        };
        if raw.discarded.iter().any(|&v| v) {
            return Err(invalid(
                "layer bake",
                0,
                "program discards pixels; this bake does not preserve coverage",
            ));
        }
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
        instruction(98, &operand(1, 0));
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
            program: PreparedProgram::new(
                Shader {
                    program: primase_dxbc::Program::from_words(&words).unwrap(),
                    reflection: primase_dxbc::Reflection {
                        buffers: vec![],
                        resources: vec![],
                        outputs: vec![],
                        inputs: vec![primase_dxbc::Semantic {
                            name: "TEXCOORD".into(),
                            index: 0,
                            system_value: 0,
                            component_type: 3,
                            register: 0,
                            mask: 15,
                            used_mask: 15,
                        }],
                    },
                },
                Constants::new(&Layout::default()),
                BTreeMap::new(),
            ),
            alpha: None,
            policies: BTreeMap::new(),
            decoded_bytes: 0,
            period: [1.; 2],
            period_exact: true,
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
                .bake_channels(2, [1.; 2], Inputs::default(), false)
                .unwrap();
            assert!(public.reflectance.is_none());
            let internal = prepared
                .bake_with_reflectance(2, [1.; 2], Inputs::default())
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

    #[test]
    fn prepared_exposes_bound_program_and_evaluates_one_pixel() {
        let prepared = prepared(false);
        assert_eq!(prepared.shader().reflection.inputs[0].name, "TEXCOORD");
        assert!(prepared.textures().is_empty());
        assert!(prepared.alpha().is_none());
        assert!(prepared.constants().words(0).is_none());

        let [normal, gloss, albedo, reflectance, emission] =
            prepared.evaluate_pixel(Inputs::default(), [0.25, 0.75]).unwrap();
        assert_eq!(normal.1, 7);
        assert_eq!(albedo.1, 7);
        assert_eq!(normal.0.map(f32::from_bits), [0.5, 0.75, 0.5, 0.0]);
        assert_eq!(albedo.0.map(f32::from_bits), [0.5, 0.75, 0.5, 0.0]);
        assert_eq!((gloss.1, reflectance.1, emission.1), (0, 0, 0));
    }

    #[test]
    fn constants_expose_written_rows_and_reflected_fields() {
        let mut constants = Constants::new(&Layout {
            buffers: vec![(0, 8)],
            fields: BTreeMap::from([(
                "x".to_owned(),
                primase_shader::Field {
                    buffer: 0,
                    offset: 1,
                    words: 2,
                },
            )]),
        });
        constants.set_words(0, 1, &[7, 8]).unwrap();
        assert_eq!(constants.words(0).unwrap(), &[[0, 7, 8, 0], [0; 4]]);
        assert!(constants.words(1).is_none());
        assert_eq!(constants.field("x").unwrap().offset, 1);
        assert!(constants.field("missing").is_none());
    }
}
