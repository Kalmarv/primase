use super::BaseMipBilinearWrap;
use crate::{Result, invalid, materials::LayeredMaterial, mtd::Value};
use primase_dxbc::{Program, Reflection, Sample, Type};
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
#[derive(Clone, Debug)]
enum Source {
    Image(Arc<BaseMipBilinearWrap>),
    Constant([f32; 4]),
}
impl Source {
    fn sample(&self, uv: [f32; 2]) -> Result<[f32; 4]> {
        match self {
            Self::Image(i) => i.sample(uv),
            Self::Constant(c) => Ok(*c),
        }
    }
}
#[derive(Debug)]
pub struct Prepared {
    program: Program,
    reflection: Reflection,
    buffers: Vec<(u32, Vec<[u32; 4]>)>,
    sources: BTreeMap<u32, Source>,
    alpha: Option<(Source, [f32; 2])>,
    pub policies: BTreeMap<String, u64>,
    pub decoded_bytes: usize,
}
#[derive(Debug)]
pub struct Bake {
    pub size: u32,
    pub albedo: Vec<u8>,
    /// Source-frame normal: X along cross(N,T)*w, Y along T. Consumer conversion
    /// is deliberately deferred, exactly as for source normal textures.
    pub normal: Vec<u8>,
    pub instructions: u64,
    pub samples: u64,
    pub policies: BTreeMap<String, u64>,
}
fn dxbc(e: primase_dxbc::Error) -> crate::Error {
    invalid("layer DXBC", e.word * 4, e.detail)
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
impl Prepared {
    /// Frozen-frame preview binding: caller supplies decoded views by texture
    /// name and sRGB flag. Missing slots use explicitly counted preview defaults;
    /// no missing resource is silently supplied by the generic interpreter.
    pub fn new(
        bytes: &[u8],
        material: &LayeredMaterial,
        mut image: impl FnMut(&str, bool) -> Result<Option<Arc<BaseMipBilinearWrap>>>,
    ) -> Result<Self> {
        let program = Program::parse(bytes).map_err(dxbc)?;
        let reflection = primase_dxbc::reflection(bytes).map_err(dxbc)?;
        let leaf = material
            .definition
            .shader
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        let shader = leaf.rsplit_once('.').map_or(leaf, |x| x.0);
        if !matches!(
            shader,
            "M[Multiple]" | "M[MultipleGround]" | "M[Multiple]_Emissive"
        ) {
            return Err(invalid("layer binding", 0, "unsupported layered shader family"));
        }
        let prefix = format!("{}_snp_", shader.replace(['[', ']'], "_"));
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
        let mut buffers: BTreeMap<u32, Vec<[u32; 4]>> = program
            .constant_buffers()
            .iter()
            .map(|&(slot, size)| (slot, vec![[0; 4]; size as usize]))
            .collect();
        let mut written = BTreeMap::<(u32, usize), bool>::new();
        let mut set = |name: &str, word: usize, values: &[f32]| {
            if let Some(&(slot, at)) = offsets.get(name)
                && let Some(buffer) = buffers.get_mut(&slot)
            {
                for (i, &v) in values.iter().enumerate() {
                    let target = at + word + i;
                    if let Some(vector) = buffer.get_mut(target / 4) {
                        vector[target % 4] = v.to_bits();
                        written.insert((slot, target), true);
                    }
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
        let mut sources = BTreeMap::new();
        let mut cache = BTreeMap::<(String, bool), Option<Arc<BaseMipBilinearWrap>>>::new();
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
                source = cache[&key].clone().map(Source::Image);
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
                source = Some(Source::Constant(match role {
                    "AlbedoMap" => [0.5, 0.5, 0.5, 1.],
                    "NormalMap" => [0.5, 0.5, 1., 1.],
                    _ => [0.; 4],
                }));
            }
            sources.insert(resource.slot, source.expect("explicit default"));
        }
        for &slot in program.textures() {
            if !sources.contains_key(&slot) {
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
        let total: usize = buffers.values().map(|v| v.len() * 4).sum();
        policies.insert(
            "buffer words left at frozen zero (including padding and unused fields)".into(),
            (total - written.len()) as u64,
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
                    Source::Image(t),
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
            program,
            reflection,
            buffers: buffers.into_iter().collect(),
            sources,
            alpha,
            policies,
            decoded_bytes,
        })
    }
    pub fn bake(&self, size: u32, period: [f32; 2], params: Inputs) -> Result<Bake> {
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
        let mut inputs = vec![[0; 4]; self.program.input_count()];
        let mut uv_register = None;
        for s in &self.reflection.inputs {
            if s.used_mask == 0 {
                continue;
            }
            if s.register as usize >= inputs.len() {
                return Err(invalid(
                    "layer binding",
                    0,
                    "signature register exceeds declarations",
                ));
            }
            let name = s.name.to_ascii_uppercase();
            let values = match (name.as_str(), s.index) {
                ("POSITION", 1) => [0., 0., 0., 1.],
                ("NORMAL", 0) => [0., 1., 0., 0.],
                ("TANGENT", 0) => [1., 0., 0., 1.],
                ("TEXCOORD", 0) => {
                    uv_register = Some(s.register as usize);
                    [0.; 4]
                }
                ("TEXCOORD", 5) => params.weights_snow,
                ("TEXCOORD", 7) => params.tint_color,
                ("TEXCOORD", 12) => [params.detail, 0., 0., 0.],
                ("SV_ISFRONTFACE", 0) => {
                    inputs[s.register as usize] = [u32::MAX, 0, 0, 0];
                    continue;
                }
                ("SV_POSITION", 0) => [0.; 4],
                _ => {
                    return Err(invalid(
                        "layer binding",
                        0,
                        format!("unclassified input {}{}", s.name, s.index),
                    ));
                }
            };
            if s.register as usize >= inputs.len() {
                return Err(invalid(
                    "layer binding",
                    0,
                    "signature register exceeds declarations",
                ));
            }
            inputs[s.register as usize] = values.map(f32::to_bits);
        }
        let uv_register = uv_register.ok_or_else(|| invalid("layer binding", 0, "missing TEXCOORD0"))?;
        let refs: Vec<_> = self.buffers.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let mut albedo = Vec::with_capacity(size as usize * size as usize * 4);
        let mut normal = Vec::with_capacity(albedo.capacity());
        let (mut instructions, mut samples) = (0, 0);
        let byte = |f: f32| -> u8 { (f.clamp(0., 1.) * 255.).round_ties_even() as u8 };
        for y in 0..size {
            for x in 0..size {
                let uv = [
                    (x as f32 + 0.5) / size as f32 * period[0],
                    (y as f32 + 0.5) / size as f32 * period[1],
                ];
                inputs[uv_register] = [uv[0], uv[1], uv[0], uv[1]].map(f32::to_bits);
                let result = self
                    .program
                    .execute(&inputs, &refs, &mut |s: Sample| {
                        self.sources
                            .get(&s.texture)
                            .ok_or_else(|| format!("unbound texture {}", s.texture))?
                            .sample(s.uv)
                            .map_err(|e| e.to_string())
                    })
                    .map_err(dxbc)?;
                if result.output_masks.first().is_none_or(|m| m & 7 != 7)
                    || result.output_masks.get(2).is_none_or(|m| m & 7 != 7)
                {
                    return Err(invalid("layer bake", 0, "missing G-buffer normal or albedo"));
                }
                let color = result.outputs[2].map(f32::from_bits);
                let n = result.outputs[0].map(f32::from_bits);
                if !color[..3].iter().chain(&n[..3]).all(|v| v.is_finite()) {
                    return Err(invalid("layer bake", 0, "nonfinite shader output"));
                }
                for &c in &color[..3] {
                    albedo.push(
                        (primase_image::linear_to_srgb((c as f64).clamp(0., 1.)) * 255.).round_ties_even()
                            as u8,
                    );
                }
                albedo.push(if let Some((source, scale)) = &self.alpha {
                    byte(source.sample([uv[0] * scale[0], uv[1] * scale[1]])?[3])
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
                instructions += result.instructions as u64;
                samples += result.samples as u64;
            }
        }
        let mut policies = self.policies.clone();
        policies.insert(
            "constant layer weights, tint and detail".into(),
            size as u64 * size as u64,
        );
        Ok(Bake {
            size,
            albedo,
            normal,
            instructions,
            samples,
            policies,
        })
    }
}
