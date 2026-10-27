//! Execute the resolved pixel stage and decode only the roles its binding declares, through primase's one reading of a
//! program's outputs as a surface (`iridia_interface::surface`).
use crate::layer_bake::{self, FrozenFrame};
use iridia_exec::{
    Grid, Input, Invocation, Lane, PixelShader, Raw, SampledTexture, SamplerUse, Unstated, evaluate_raw_with,
};
use iridia_interface::{NormalSpace, OutputBinding, OutputRole};
use iridia_interface::{surface::Base, surface::Place, surface::Surface};
use primase_core::parallel::Threads;
use primase_core::pbr::{PbrMaterial, UvTransform, Wrap};
use primase_core::{ColorSpace, Texture, World};
use primase_core::{shader::Shader, shader::StageBinding, shader::TextureSource};
use primase_image::sampling::{Address, Levels, Transfer};
use primase_texcodec::TexDecoder;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug)]
pub(super) struct Baked {
    pub size: u32,
    pub requested_size: u32,
    pub source_size: [u32; 2],
    pub albedo: Vec<u8>,
    pub normal: Vec<u8>,
    pub orm: Vec<u8>,
    pub specular: Option<Vec<u8>>,
    pub emission: Option<Vec<[f32; 3]>>,
    pub policies: BTreeMap<String, u64>,
}
fn image(t: &Texture, sampler: SamplerUse) -> Result<Arc<SampledTexture>, String> {
    let level = t.levels.first().ok_or("bound texture has no mip")?;
    let rgba = primase_texcodec::Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, level)
        .map_err(|e| e.to_string())?;
    let transfer = if t.color_space == ColorSpace::Srgb {
        Transfer::SrgbColour
    } else {
        Transfer::Stored
    };
    Ok(Arc::new(SampledTexture::new(
        Levels::one(t.width, t.height, rgba).map_err(|e| e.to_string())?,
        transfer,
        sampler,
        layer_bake::FILTERING,
    )?))
}
fn byte(x: f32) -> u8 {
    (255. * x.clamp(0., 1.)).round_ties_even() as u8
}
fn colour(x: f32) -> u8 {
    (255. * primase_image::linear_to_srgb(f64::from(x.clamp(0., 1.)))).round_ties_even() as u8
}

/// Output register comes from the reflected varying, not from the semantic's index or a game's target number.
fn target(shader: &dyn PixelShader, output: &OutputBinding) -> Result<(u32, u8), String> {
    let varying = shader
        .interface()
        .outputs
        .iter()
        .find(|v| v.label == output.label)
        .ok_or("bound output missing from program")?;
    let d3d = varying.provenance.d3d.ok_or("output has no register")?;
    Ok((d3d.register, output.lanes))
}
/// The four values of the output a term is placed in at `pixel`: the term's lanes written and finite, else an error.
/// The other lanes are as the evaluation left them; a term reads only its own.
fn values(raw: &Raw, place: Place, pixel: usize) -> Result<[f32; 4], String> {
    let (output, mask) = (place.output, place.lanes);
    let (words, written) = raw.pixels[output][pixel];
    if written & mask != mask {
        return Err(format!("output {output} lacks lanes {mask:x} at texel {pixel}"));
    }
    let values = words.map(f32::from_bits);
    if (0..4).any(|c| mask & (1 << c) != 0 && !values[c].is_finite()) {
        return Err("nonfinite G-buffer output".into());
    }
    Ok(values)
}
/// A unit world or view normal in the baked texture's bytes.
fn normal(n: [f32; 3]) -> [u8; 4] {
    // Frozen N=(0,1,0), T=(1,0,0), cross(N,T)=(0,0,-1). Restore the source normal convention.
    [
        byte(-n[2] * 0.5 + 0.5),
        byte(n[0] * 0.5 + 0.5),
        byte(n[1] * 0.5 + 0.5),
        255,
    ]
}

/// The resolved values both the cache key and the evaluation consume. Entity identities never enter the key.
pub(super) struct Prepared<'a> {
    pub pixel: &'a Shader,
    pub vertex: &'a Shader,
    pub shader: iridia_exec::ProgramShader,
    pub outputs: Vec<&'a OutputBinding>,
    /// The outputs read as a surface: every place is a position in `outputs`, and it has the albedo, the normal and the
    /// roughness the bake needs, and the metallic or the reflectance.
    pub surface: Surface,
    pub policies: BTreeMap<String, u64>,
    pub constants: iridia_exec::evaluate::Constants,
    pub textures: BTreeMap<u32, BoundTexture>,
    pub inputs: BTreeMap<(String, u32), Input>,
    pub targets: Vec<(u32, u8)>,
    pub size: u32,
    pub requested_size: u32,
    pub source_size: [u32; 2],
    pub alpha: Option<(Arc<SampledTexture>, Option<UvTransform>)>,
    pub base_alpha: f32,
    pub sample_time: f32,
}

pub(super) enum BoundTexture {
    Image(Arc<SampledTexture>),
    Constant([f32; 4]),
}
impl BoundTexture {
    fn view(&self) -> iridia_exec::View {
        match self {
            Self::Image(image) => iridia_exec::View::Image(image.clone()),
            Self::Constant(value) => iridia_exec::View::Constant(*value),
        }
    }
}

pub(super) fn prepare<'a>(
    world: &'a World,
    stage: &'a StageBinding,
    vertex: Option<&StageBinding>,
    previous: &PbrMaterial,
    cap: u32,
    time: f32,
) -> Result<Prepared<'a>, String> {
    let core = world
        .get::<Shader>(stage.shader)
        .ok_or("bound program is absent")?;
    let shader = layer_bake::program_executor(core.format, &core.bytecode).map_err(|e| e.to_string())?;
    let outputs: Vec<_> = stage
        .outputs
        .iter()
        .filter(|o| {
            matches!(
                o.role,
                OutputRole::Albedo
                    | OutputRole::Normal { .. }
                    | OutputRole::Metallic
                    | OutputRole::Gloss { .. }
                    | OutputRole::Roughness
                    | OutputRole::Reflectance { .. }
                    | OutputRole::Emission { .. }
            )
        })
        .collect();
    let surface = Surface::read(outputs.iter().map(|o| (o.lanes, &o.role)));
    if let Some(why) = surface.refused.first() {
        return Err(format!("output role whose lanes do not fit it: {why}"));
    }
    match surface.base {
        Some(Base::Albedo(place)) if place.width() == 3 => {}
        Some(Base::Albedo(_)) => return Err("albedo role is not RGB".into()),
        _ => return Err(super::colour::refusal(shader.program(), stage).to_owned()),
    }
    let normal = surface.normal.ok_or("no normal output role")?;
    if !matches!(normal.space, NormalSpace::World | NormalSpace::View) {
        return Err("normal is not a world/view normal".into());
    }
    if surface.roughness.is_none() {
        return Err("no gloss/roughness output role".into());
    }
    if surface.metallic.is_none() && surface.reflectance.is_none() {
        return Err("neither metallic nor reflectance output role".into());
    }
    let mut policies = BTreeMap::new();
    let frame = FrozenFrame::static_bake(time);
    let constants = layer_bake::program_constants(&shader, &core.interface, stage, &frame, &mut policies)
        .map_err(|e| e.to_string())?;
    let mut textures = BTreeMap::new();
    let mut source_size = [0; 2];
    for t in &stage.textures {
        let resource = core
            .interface
            .resource(t.resource)
            .ok_or("texture resource absent")?;
        let slot = layer_bake::resolve::register(resource).map_err(|e| e.to_string())?;
        let value = match &t.source {
            TextureSource::Texture { texture, sampler } => {
                let texture = world
                    .get::<Texture>(*texture)
                    .ok_or("bound texture has no pixels")?;
                source_size[0] = source_size[0].max(texture.width);
                source_size[1] = source_size[1].max(texture.height);
                let use_sampler = SamplerUse::of(sampler, Unstated::Assume(Address::Wrap))?;
                if sampler.wrap[..2].contains(&None) {
                    *policies
                        .entry("unstated sampler addressing taken as wrap".into())
                        .or_default() += 1;
                }
                if sampler.mip_lod_bias.is_some_and(|x| x != 0.) {
                    *policies
                        .entry("sampler LOD bias omitted by base-mip bake".into())
                        .or_default() += 1;
                }
                BoundTexture::Image(image(texture, use_sampler)?)
            }
            TextureSource::Default { value, why } => {
                *policies.entry(why.clone()).or_default() += 1;
                BoundTexture::Constant(*value)
            }
            TextureSource::Frame(frame) => {
                return Err(format!("frame texture {frame:?} cannot be baked as a material"));
            }
            TextureSource::Missing => return Err(format!("sampled texture t{slot} is missing")),
        };
        textures.insert(slot, value);
    }
    let largest = source_size[0].max(source_size[1]);
    let requested_size = if largest == 0 { cap } else { largest };
    let size = cap.min(requested_size);
    let vertex = vertex.ok_or("no resolved vertex program to derive pixel inputs")?;
    let vertex_shader = world
        .get::<Shader>(vertex.shader)
        .ok_or("bound vertex program is absent")?;
    let vertex = layer_bake::VertexStage::new(vertex_shader, vertex, shader.program(), false, &frame)
        .map_err(|e| e.to_string())?;
    let mut inputs = vertex
        .pixel_inputs(shader.interface(), &layer_bake::Inputs::default())
        .map_err(|e| e.to_string())?;
    policies.extend(vertex.policies);
    for v in shader.interface().inputs.iter().filter(|v| v.used) {
        let key = (v.label.name.clone(), v.label.index);
        if inputs.contains_key(&key) {
            continue;
        }
        let input = match (v.label.name.to_ascii_uppercase().as_str(), v.label.index) {
            ("SV_POSITION", _) => Input::Constant([0.5, 0.5, 0.5, 1.]),
            ("SV_ISFRONTFACE", _) => {
                Input::Lanes([Lane::Bits(u32::MAX), Lane::Bits(0), Lane::Bits(0), Lane::Bits(0)])
            }
            _ => {
                return Err(format!(
                    "no static bake input for {}{}",
                    v.label.name, v.label.index
                ));
            }
        };
        inputs.insert(key, input);
    }
    policies.insert(
        "static flat tangent frame, white vertex colour and coincident UV sets".into(),
        1,
    );
    policies.insert(
        "base-mip bilinear sampling; per-light gloss correction omitted".into(),
        1,
    );
    let targets = outputs
        .iter()
        .map(|o| target(&shader, o))
        .collect::<Result<Vec<_>, _>>()?;
    let alpha = previous
        .base_color_texture
        .map(|r| {
            if r.texcoord != 0 {
                return Err("alpha uses a secondary UV set".to_owned());
            }
            let t = world.get::<Texture>(r.texture).ok_or("alpha texture is absent")?;
            let address = r.wrap.map(|w| match w {
                Wrap::Repeat => Address::Wrap,
                Wrap::Mirror => Address::Mirror,
                Wrap::Clamp => Address::Clamp,
            });
            Ok((image(t, SamplerUse::addressed(address))?, r.transform))
        })
        .transpose()?;
    Ok(Prepared {
        pixel: core,
        vertex: vertex_shader,
        shader,
        outputs,
        surface,
        policies,
        constants,
        textures,
        inputs,
        targets,
        size,
        requested_size,
        source_size,
        alpha,
        base_alpha: previous.base_color[3],
        sample_time: time,
    })
}

pub(super) fn bake(prepared: Prepared<'_>, world: &World) -> Result<Baked, String> {
    let Prepared {
        pixel: _,
        vertex: _,
        shader,
        outputs: _,
        surface,
        policies,
        constants,
        textures,
        inputs,
        targets,
        size,
        requested_size,
        source_size,
        alpha,
        base_alpha,
        sample_time: _,
    } = prepared;
    let textures = iridia_exec::Views::refusing(textures.iter().map(|(&slot, t)| (slot, t.view())).collect());
    let raw = evaluate_raw_with(
        Threads::of(world),
        &shader,
        &constants,
        &textures,
        &inputs,
        Grid {
            size,
            period: [1.; 2],
        },
        &targets,
        Invocation::Quads,
    )
    .map_err(|e| e.to_string())?;
    if raw.discarded.iter().any(|x| *x) {
        return Err("program still discards texels with alpha test disabled".into());
    }
    let surfaces = Decode {
        surface: &surface,
        alpha: alpha.as_ref(),
        base_alpha,
        size,
    }
    .surfaces(&raw, Threads::of(world))?;
    let mut result = Baked {
        size,
        requested_size,
        source_size,
        albedo: surfaces.albedo,
        normal: surfaces.normal,
        orm: surfaces.orm,
        specular: surfaces.specular,
        emission: surfaces.emission,
        policies,
    };
    if surface.reflectance.is_some() && surface.metallic.is_none() {
        result.policies.insert(
            "decoded reflectance represented by PBR specular workflow; diffuse energy model approximated"
                .into(),
            1,
        );
    }
    result
        .policies
        .insert("baked surface channels quantized to UNORM8".into(), 1);
    Ok(result)
}

/// The surface channels a bake decodes from its evaluation, texel by texel in row-major order.
#[derive(Debug, Default, PartialEq)]
struct Surfaces {
    albedo: Vec<u8>,
    normal: Vec<u8>,
    orm: Vec<u8>,
    specular: Option<Vec<u8>>,
    emission: Option<Vec<[f32; 3]>>,
}

/// How a bake's evaluated targets decode into its surface channels: the surface its outputs are read as (each place a
/// position among the evaluated targets, in the order the evaluation keeps them, with the terms [`prepare`] requires),
/// the texture whose alpha (times `base_alpha`) is the albedo's, and the grid's side.
struct Decode<'a> {
    surface: &'a Surface,
    alpha: Option<&'a (Arc<SampledTexture>, Option<UvTransform>)>,
    base_alpha: f32,
    size: u32,
}

impl Decode<'_> {
    /// Every texel of `raw`, its rows decoded on `threads` and joined in order: the same bytes on any count, and the
    /// first failing texel's error, as one thread meets it.
    fn surfaces(&self, raw: &Raw, threads: Threads) -> Result<Surfaces, String> {
        let rows: Vec<u32> = (0..self.size).collect();
        let decoded = primase_core::parallel::map(threads, &rows, |_, &y| self.row(raw, y))
            .map_err(|p| format!("decoding row {}: {}", rows[p.item], p.message))?;
        let mut all = Surfaces {
            specular: self.surface.reflectance.map(|_| Vec::new()),
            emission: self.surface.emission.map(|_| Vec::new()),
            ..Surfaces::default()
        };
        for row in decoded {
            let row = row?;
            all.albedo.extend(row.albedo);
            all.normal.extend(row.normal);
            all.orm.extend(row.orm);
            if let (Some(all), Some(row)) = (&mut all.specular, row.specular) {
                all.extend(row);
            }
            if let (Some(all), Some(row)) = (&mut all.emission, row.emission) {
                all.extend(row);
            }
        }
        Ok(all)
    }

    /// Row `y`'s texels.
    fn row(&self, raw: &Raw, y: u32) -> Result<Surfaces, String> {
        let n = self.size as usize;
        let mut out = Surfaces {
            albedo: Vec::with_capacity(4 * n),
            normal: Vec::with_capacity(4 * n),
            orm: Vec::with_capacity(4 * n),
            specular: self.surface.reflectance.map(|_| Vec::with_capacity(4 * n)),
            emission: self.surface.emission.map(|_| Vec::with_capacity(n)),
        };
        for x in 0..n {
            self.texel(raw, y as usize * n + x, &mut out)?;
        }
        Ok(out)
    }

    /// Texel `p`'s channels, appended to `out`.
    fn texel(&self, raw: &Raw, p: usize, out: &mut Surfaces) -> Result<(), String> {
        let size = self.size as usize;
        let s = self.surface;
        let (Some(base), Some(normal_term), Some(roughness)) = (s.base, s.normal, s.roughness) else {
            return Err("the surface lacks an albedo, a normal or a roughness".into());
        };
        let a = base.colour(values(raw, base.place(), p)?);
        out.albedo.extend(a.map(colour));
        let uv = [(p % size) as f32 + 0.5, (p / size) as f32 + 0.5].map(|x| x / size as f32);
        let opacity = if let Some((t, transform)) = self.alpha {
            layer_bake::sample(t, transform.map_or(uv, |t| t.apply(uv))).map_err(|e| e.to_string())?[3]
        } else {
            1.
        };
        out.albedo.push(byte(opacity * self.base_alpha));
        let n = normal_term
            .decode(values(raw, normal_term.place, p)?)
            .ok_or("zero G-buffer normal")?;
        out.normal.extend(normal(n));
        let r = roughness.decode(values(raw, roughness.term().place, p)?);
        let m = match s.metallic {
            Some(t) => t.decode(values(raw, t.place, p)?, 0),
            None => 0.,
        };
        if !r.is_finite() || !m.is_finite() {
            return Err("nonfinite decoded surface channel".into());
        }
        out.orm.extend([255, byte(r), byte(m), 255]);
        if let Some(t) = s.reflectance {
            let f0 = t.colour(values(raw, t.place, p)?);
            let specular = out.specular.as_mut().expect("allocated reflectance");
            for v in f0 {
                if !v.is_finite() {
                    return Err("nonfinite reflectance".into());
                }
                specular.push(colour(v));
            }
            specular.push(255);
        }
        if let Some(e) = s.emission {
            let light = e.decode(values(raw, e.term().place, p)?, a);
            out.emission.as_mut().expect("allocated emission").push(light);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iridia_interface::{Label, NormalEncoding, ScalarEncoding};

    fn target(name: &str, lanes: u8, role: OutputRole) -> OutputBinding {
        OutputBinding {
            label: Label {
                name: name.into(),
                index: 0,
            },
            lanes,
            role,
        }
    }

    /// The targets of a G-buffer program with every role the bake decodes: albedo, a biased normal, gloss, metallic,
    /// reflectance and emission, each read linearly.
    fn targets() -> Vec<OutputBinding> {
        let linear = ScalarEncoding::LINEAR;
        vec![
            target("SV_Target", 0b0111, OutputRole::Albedo),
            target(
                "SV_Target",
                0b0111,
                OutputRole::Normal {
                    encoding: NormalEncoding::XyzUnorm,
                    space: NormalSpace::World,
                },
            ),
            target("SV_Target", 0b0001, OutputRole::Gloss { encoding: linear }),
            target("SV_Target", 0b0001, OutputRole::Metallic),
            target("SV_Target", 0b0111, OutputRole::Reflectance { encoding: linear }),
            target("SV_Target", 0b0111, OutputRole::Emission { encoding: linear }),
        ]
    }

    /// An evaluation of `size` by `size` texels: texel 0 the designed values the test works by hand, every other its
    /// own; `written(p, target)` the lanes written.
    fn raw(size: usize, written: impl Fn(usize, usize) -> u8) -> Raw {
        let value = |p: usize, t: usize| -> [f32; 4] {
            if p == 0 {
                return [
                    [0.5, 0.25, 1., 0.],
                    [0.5, 1., 0.5, 0.],
                    [0.75, 0., 0., 0.],
                    [1., 0., 0., 0.],
                    [0.04, 0.04, 0.04, 0.],
                    [2., 0., 0.5, 0.],
                ][t];
            }
            let x = (p * 7 + t * 3) as f32;
            [(x % 11.) / 11., (x % 5.) / 5. * 0.5 + 0.25, (x % 3.) / 3., 0.]
        };
        Raw {
            pixels: (0..6)
                .map(|t| {
                    (0..size * size)
                        .map(|p| (value(p, t).map(f32::to_bits), written(p, t)))
                        .collect()
                })
                .collect(),
            discarded: vec![false; size * size],
            instructions: 0,
            samples: 0,
        }
    }

    /// The targets read as a surface: every role in its own target, 0 to 5.
    fn surface(outputs: &[&OutputBinding]) -> Surface {
        let surface = Surface::read(outputs.iter().map(|o| (o.lanes, &o.role)));
        assert!(surface.refused.is_empty(), "{:?}", surface.refused);
        surface
    }

    fn decode(surface: &Surface, size: u32) -> Decode<'_> {
        Decode {
            surface,
            alpha: None,
            base_alpha: 0.5,
            size,
        }
    }

    /// The decoded channels are the same bytes on one thread and on four (rows of an odd grid, so the threads' shares
    /// differ), and the designed texel is what the channels' encodings give by hand: albedo (0.5, 0.25, 1) in sRGB
    /// 188, 137, 255 with the base alpha 0.5 as 128; the normal (0.5, 1, 0.5) unbiased (0, 1, 0), in the source's
    /// convention 128, 128, 255; gloss 0.75 as roughness 0.25, 64, metallic 255; the reflectance 0.04 in sRGB 56; the
    /// emission as written.
    #[test]
    fn the_decode_is_the_same_on_any_thread_count() {
        let targets = targets();
        let outputs: Vec<&OutputBinding> = targets.iter().collect();
        let size = 9;
        let raw = raw(size, |_, t| targets[t].lanes);
        let surface = surface(&outputs);
        let one = decode(&surface, size as u32)
            .surfaces(&raw, Threads::ONE)
            .unwrap();
        let four = decode(&surface, size as u32)
            .surfaces(&raw, Threads::new(4).unwrap())
            .unwrap();
        assert_eq!(one, four);
        assert_eq!(one.albedo.len(), 4 * size * size);
        assert_eq!(one.albedo[..4], [188, 137, 255, 128]);
        assert_eq!(one.normal[..4], [128, 128, 255, 255]);
        assert_eq!(one.orm[..4], [255, 64, 255, 255]);
        assert_eq!(one.specular.as_ref().unwrap()[..4], [56, 56, 56, 255]);
        assert_eq!(one.emission.as_ref().unwrap()[0], [2., 0., 0.5]);
    }

    /// Where texels fail, the error is the first one's in row-major order, whatever the threads: a texel of row 2
    /// whose albedo lacks a lane, before one of row 5 whose normal lacks one.
    #[test]
    fn the_first_failing_texel_is_the_error_on_any_thread_count() {
        let targets = targets();
        let outputs: Vec<&OutputBinding> = targets.iter().collect();
        let size = 9;
        let raw = raw(size, |p, t| match (p, t) {
            (20, 0) => 0b0011,
            (47, 1) => 0b0001,
            _ => targets[t].lanes,
        });
        let surface = surface(&outputs);
        for threads in [Threads::ONE, Threads::new(4).unwrap()] {
            assert_eq!(
                decode(&surface, size as u32).surfaces(&raw, threads),
                Err("output 0 lacks lanes 7 at texel 20".into())
            );
        }
    }
}
