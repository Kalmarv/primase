//! Execute the resolved pixel stage and decode only the roles its binding declares.
use crate::layer_bake::{self, FrozenFrame};
use primase_core::parallel::Threads;
use primase_core::pbr::{PbrMaterial, Wrap};
use primase_core::shader::{
    NormalEncoding, NormalSpace, OutputBinding, OutputRole, Shader, StageBinding, TextureSource,
};
use primase_core::{ColorSpace, Texture, World};
use primase_image::sampling::{Address, Levels, Transfer};
use primase_shader_exec::{
    Grid, Input, Invocation, Lane, Layout, PixelShader, Raw, SampledTexture, SamplerUse, Unstated,
    evaluate::Constants, evaluate_raw_with,
};
use primase_texcodec::TexDecoder;
use std::{collections::BTreeMap, sync::Arc};

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
fn lanes(raw: &Raw, output: usize, pixel: usize, mask: u8) -> Result<Vec<f32>, String> {
    let (words, written) = raw.pixels[output][pixel];
    if written & mask != mask {
        return Err(format!("output {output} lacks lanes {mask:x} at texel {pixel}"));
    }
    let values: Vec<_> = (0..4)
        .filter(|c| mask & (1 << c) != 0)
        .map(|c| f32::from_bits(words[c]))
        .collect();
    if values.iter().any(|x| !x.is_finite()) {
        return Err("nonfinite G-buffer output".into());
    }
    Ok(values)
}
fn normal(encoded: &[f32], encoding: NormalEncoding, space: NormalSpace) -> Result<[u8; 4], String> {
    if encoded.len() != 3 || !matches!(space, NormalSpace::World | NormalSpace::View) {
        return Err("normal is not a three-lane world/view normal".into());
    }
    let bias = match encoding {
        NormalEncoding::Xyz => 0.,
        NormalEncoding::XyzUnorm => 0.5,
        NormalEncoding::XyzBiased { bias_bits } => f32::from_bits(bias_bits),
        _ => return Err("unsupported normal encoding".into()),
    };
    let scale = if encoding == NormalEncoding::Xyz { 1. } else { 2. };
    let n: [f32; 3] = std::array::from_fn(|i| (encoded[i] - bias) * scale);
    let length = n.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length <= 1e-8 {
        return Err("zero G-buffer normal".into());
    }
    // Frozen N=(0,1,0), T=(1,0,0), cross(N,T)=(0,0,-1). Restore the source normal convention.
    Ok([
        byte(-n[2] / length * 0.5 + 0.5),
        byte(n[0] / length * 0.5 + 0.5),
        byte(n[1] / length * 0.5 + 0.5),
        255,
    ])
}

pub(super) fn bake(
    world: &World,
    stage: &StageBinding,
    vertex: Option<&StageBinding>,
    previous: &PbrMaterial,
    cap: u32,
    time: f32,
) -> Result<Baked, String> {
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
    let index = |predicate: fn(&OutputRole) -> bool| outputs.iter().position(|o| predicate(&o.role));
    let albedo = index(|r| matches!(r, OutputRole::Albedo))
        .ok_or_else(|| super::colour::refusal(shader.program(), stage))?;
    let normal_index = index(|r| matches!(r, OutputRole::Normal { .. })).ok_or("no normal output role")?;
    let rough = index(|r| matches!(r, OutputRole::Gloss { .. } | OutputRole::Roughness))
        .ok_or("no gloss/roughness output role")?;
    let metal = index(|r| matches!(r, OutputRole::Metallic));
    let specular = index(|r| matches!(r, OutputRole::Reflectance { .. }));
    if metal.is_none() && specular.is_none() {
        return Err("neither metallic nor reflectance output role".into());
    }
    let emission = index(|r| matches!(r, OutputRole::Emission { .. }));
    let mut policies = BTreeMap::new();
    let layout = Layout::of(&shader).map_err(|e| e.to_string())?;
    let mut constants = Constants::new(&layout);
    let frame = FrozenFrame::static_bake(time);
    for c in &stage.constants {
        let resource = core
            .interface
            .resource(c.resource)
            .ok_or("constant resource absent")?;
        let slot = layer_bake::resolve::register(resource).map_err(|e| e.to_string())?;
        let Some(mut words) = layer_bake::frozen_words(&c.role, c.lines.as_ref(), &frame, &mut policies)
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        if c.name.as_deref() == Some("FC_AlphaRef") && !words.is_empty() {
            words[0] = (-100f32).to_bits();
            policies.insert(layer_bake::ALPHA_TEST_POLICY.into(), 1);
        }
        let Some(&(_, rows)) = shader.constant_buffers().iter().find(|(s, _)| *s == slot) else {
            continue;
        };
        let start = c.offset / 4;
        words.truncate((c.size / 4).min((rows * 4).saturating_sub(start)) as usize);
        if !words.is_empty() {
            constants
                .set_words(slot, start, &words)
                .map_err(|e| e.to_string())?;
        }
    }
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
                primase_shader_exec::View::Image(image(texture, use_sampler)?)
            }
            TextureSource::Default { value, why } => {
                *policies.entry(why.clone()).or_default() += 1;
                primase_shader_exec::View::Constant(*value)
            }
            TextureSource::Frame(frame) => {
                return Err(format!("frame texture {frame:?} cannot be baked as a material"));
            }
            TextureSource::Missing => return Err(format!("sampled texture t{slot} is missing")),
        };
        textures.insert(slot, value);
    }
    let textures = primase_shader_exec::Views::refusing(textures);
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
    let mut result = Baked {
        size,
        requested_size,
        source_size,
        albedo: Vec::new(),
        normal: Vec::new(),
        orm: Vec::new(),
        specular: specular.map(|_| Vec::new()),
        emission: emission.map(|_| Vec::new()),
        policies,
    };
    if specular.is_some() && metal.is_none() {
        result.policies.insert(
            "decoded reflectance represented by PBR specular workflow; diffuse energy model approximated"
                .into(),
            1,
        );
    }
    for p in 0..size as usize * size as usize {
        let value = |i: usize| lanes(&raw, i, p, outputs[i].lanes);
        let a = value(albedo)?;
        if a.len() != 3 {
            return Err("albedo role is not RGB".into());
        }
        result.albedo.extend(a.iter().copied().map(colour));
        let uv =
            [(p % size as usize) as f32 + 0.5, (p / size as usize) as f32 + 0.5].map(|x| x / size as f32);
        let opacity = if let Some((t, transform)) = &alpha {
            layer_bake::sample(t, transform.map_or(uv, |t| t.apply(uv))).map_err(|e| e.to_string())?[3]
        } else {
            1.
        };
        result.albedo.push(byte(opacity * previous.base_color[3]));
        let OutputRole::Normal { encoding, space } = outputs[normal_index].role else {
            unreachable!()
        };
        result
            .normal
            .extend(normal(&value(normal_index)?, encoding, space)?);
        let r = value(rough)?;
        if r.len() != 1 {
            return Err("roughness/gloss role is not scalar".into());
        }
        let r = match outputs[rough].role {
            OutputRole::Gloss { encoding } => 1. - encoding.decode(r[0]),
            _ => r[0],
        };
        let m = if let Some(i) = metal {
            let m = value(i)?;
            if m.len() != 1 {
                return Err("metallic role is not scalar".into());
            }
            m[0]
        } else {
            0.
        };
        if !r.is_finite() || !m.is_finite() {
            return Err("nonfinite decoded surface channel".into());
        }
        result.orm.extend([255, byte(r), byte(m), 255]);
        if let Some(i) = specular {
            let OutputRole::Reflectance { encoding } = outputs[i].role else {
                unreachable!()
            };
            let f0 = value(i)?;
            if f0.len() != 3 {
                return Err("reflectance role is not RGB".into());
            }
            let out = result.specular.as_mut().expect("allocated reflectance");
            for v in f0 {
                let v = encoding.decode(v);
                if !v.is_finite() {
                    return Err("nonfinite reflectance".into());
                }
                out.push(colour(v));
            }
            out.push(255);
        }
        if let Some(i) = emission {
            let OutputRole::Emission { encoding } = outputs[i].role else {
                unreachable!()
            };
            let e = value(i)?;
            if e.len() != 3 {
                return Err("emission role is not RGB".into());
            }
            result.emission.as_mut().expect("allocated emission").push([
                encoding.decode(e[0]),
                encoding.decode(e[1]),
                encoding.decode(e[2]),
            ]);
        }
    }
    result
        .policies
        .insert("baked surface channels quantized to UNORM8".into(), 1);
    Ok(result)
}
