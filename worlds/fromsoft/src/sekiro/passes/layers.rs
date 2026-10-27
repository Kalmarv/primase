//! Area-weighted static layer grouping over an area whose geometry `lower-fromsoft-geometry` lowered: each layered
//! part keeps its raw per-vertex shader inputs (`fromsoft.layer_inputs`); grouping and baking are the bake pass's.
use crate::layer_bake::grouping::{self, Features};
use crate::sekiro::source::{LayerInputs, Payloads, PrototypeSource, TexturePayloads};
use crate::{Result, invalid, materials::Material};
use iridia_exec::PixelShader;
use iridia_exec::ProgramShader;
use iridia_interface::Stage;
use primase_core::{Entity, Instance, Mesh, Semantic, Stream, World};
use primase_core::{shader::Shader as ShaderProgram, shader::ShaderBinding, shader::StageBinding};
use primase_meshops::vector::Vec3;
use std::{collections::BTreeMap, sync::Arc};

/// Prepared bake inventory over the texture files it borrows; each worker may prepare one material at a time, keeping
/// decoded pixel memory bounded. A group's program, constants and textures are its material's `core.shader_binding`
/// (`resolve-fromsoft-shaders`), read from the world when the group is prepared.
#[derive(Debug)]
pub struct AreaBake<'a> {
    pub groups: Vec<MaterialGroups>,
    payloads: &'a Payloads,
}
/// Nothing, for a world without the resource.
static EMPTY: Payloads = BTreeMap::new();
impl<'a> AreaBake<'a> {
    /// Group the layered parts of a world's placed prototypes by bake inputs, over the world's own source data.
    pub fn from_world(world: &'a World) -> Result<Self> {
        let payloads = world.resource::<TexturePayloads>().map_or(&EMPTY, |p| &p.0);
        Self::with_sources(world, payloads)
    }
    /// The same over texture files held apart from `world`.
    pub fn with_sources(world: &World, payloads: &'a Payloads) -> Result<Self> {
        let mut groups = groups(world)?;
        for group in &mut groups {
            let material = group.material.layered.as_ref().expect("layered group");
            let (shader, ..) = Self::bound(world, group)?;
            let repeat = crate::layer_bake::sampled_period(PixelShader::interface(&shader), material)?;
            group.period = repeat.period;
            group.period_exact = repeat.exact;
        }
        Ok(Self { groups, payloads })
    }
    /// A group's program (parsed, and its container's bytes), its pixel stage binding and, where its material has one, its
    /// vertex program and binding, from its material entity.
    #[allow(clippy::type_complexity)]
    fn bound<'w>(
        world: &'w World,
        group: &MaterialGroups,
    ) -> Result<(
        ProgramShader,
        &'w [u8],
        StageBinding,
        Option<(&'w ShaderProgram, StageBinding)>,
    )> {
        let binding = world.get::<ShaderBinding>(group.entity).ok_or_else(|| {
            invalid(
                "area bake",
                0,
                "a layered material without core.shader_binding (resolve-fromsoft-shaders runs first)",
            )
        })?;
        let stage = binding
            .stage(Stage::Pixel)
            .ok_or_else(|| invalid("area bake", 0, "a layered material without a pixel stage binding"))?;
        let program = world.get::<ShaderProgram>(stage.shader).ok_or_else(|| {
            invalid(
                "area bake",
                0,
                "a pixel stage whose shader entity has no core.shader",
            )
        })?;
        let shader = crate::layer_bake::executor(&program.bytecode)?;
        let vertex = binding
            .stage(Stage::Vertex)
            .map(|vertex| {
                world
                    .get::<ShaderProgram>(vertex.shader)
                    .map(|program| (program, vertex.clone()))
                    .ok_or_else(|| {
                        invalid(
                            "area bake",
                            0,
                            "a vertex stage whose shader entity has no core.shader",
                        )
                    })
            })
            .transpose()?;
        Ok((shader, &program.bytecode, stage.clone(), vertex))
    }
    /// A group's program bound to its materials' inputs, sampled at `sample_time` seconds.
    pub fn prepare(
        &self,
        world: &World,
        group: usize,
        sample_time: f32,
    ) -> Result<crate::layer_bake::Prepared> {
        use primase_core::Texture;
        use primase_texcodec::TexDecoder;
        let g = self
            .groups
            .get(group)
            .ok_or_else(|| invalid("area bake", group, "group index"))?;
        let material = g
            .material
            .layered
            .as_ref()
            .ok_or_else(|| invalid("area bake", group, "group index"))?;
        let (shader, bytecode, stage, vertex) = Self::bound(world, g)?;
        let view = |t: &Texture,
                    srgb: bool,
                    address: [primase_image::sampling::Address; 2]|
         -> Result<Arc<iridia_exec::SampledTexture>> {
            let level = t
                .levels
                .first()
                .ok_or_else(|| invalid("area bake pixels", 0, "a texture without a base mip"))?;
            let pixels = primase_texcodec::Decoder::default()
                .decode_rgba8(t.format, t.width, t.height, level)
                .map_err(|e| invalid("area bake pixels", 0, e))?;
            Ok(Arc::new(crate::layer_bake::base_texture(
                t.width, t.height, &pixels, srgb, address,
            )?))
        };
        crate::layer_bake::Prepared::from_binding(
            crate::layer_bake::Programs {
                shader: &shader,
                bytecode,
                stage: &stage,
                vertex: vertex.as_ref().map(|(program, binding)| (*program, binding)),
            },
            material,
            &crate::layer_bake::FrozenFrame::static_bake(sample_time),
            |entity: Entity, srgb, address| {
                let t = world.get::<Texture>(entity).ok_or_else(|| {
                    invalid("area bake", group, "a bound texture entity without core.texture")
                })?;
                view(t, srgb, address)
            },
            |name, srgb, address| {
                let Some(bytes) = self.payloads.get(name) else {
                    return Ok(None);
                };
                let texture = primase_dds::read(bytes).map_err(|e| invalid("area bake DDS", 0, e))?;
                view(&texture, srgb, address).map(Some)
            },
        )
    }
}

#[derive(Debug, serde::Serialize)]
pub struct MaterialGroups {
    /// The first material entity of the group, whose `core.shader_binding` the group's bake uses (the group's
    /// materials differ at most in raw GX, which no binding reads).
    pub entity: Entity,
    pub material: Material,
    pub centers: Vec<Features>,
    pub parts: Vec<PartGroups>,
    pub period: [f32; 2],
    pub period_exact: bool,
}
#[derive(Debug, serde::Serialize)]
pub struct PartGroups {
    pub prototype: String,
    pub part: usize,
    pub assignments: Vec<usize>,
}

// This is the static bake-input equivalence relation, not source-material
// identity. Raw GX is retained on per-source output materials; the current
// interpreter consumes the definition/bindings and does not interpret GX.
pub(super) fn bake_identity(material: &Material) -> Result<String> {
    let mut projected = material.with_effective_addressing();
    projected.gx.clear();
    primase_core::json::to_value(&projected)
        .map(|v| v.to_string())
        .map_err(|e| invalid("layer groups", 0, e))
}

/// The eight bake inputs per vertex from the raw Byte4C members: layer weights, snow, tint RGB and detail.
/// MultipleGround reads its second weight from member 2's y, Multiple from its z. Missing members read zero.
fn vertex_inputs(primitive: &primase_core::Primitive, ground: bool) -> Result<Vec<Features>> {
    let channels = crate::layer_bake::resolve::layer_channels(ground);
    let count = primitive.positions().map(|p| p.len()).unwrap_or(0);
    let mut features = vec![[0.; 8]; count];
    for (k, (member, channel)) in channels.into_iter().enumerate() {
        match primitive.streams.get(&Semantic::Color(member)) {
            None => {}
            Some(Stream::U8x4(values)) if values.len() == count => {
                for (f, v) in features.iter_mut().zip(values) {
                    f[k] = f32::from(v[channel]) / 255.;
                }
            }
            Some(_) => {
                return Err(invalid(
                    "layer inputs",
                    usize::from(member),
                    "not a per-vertex UNORM8x4 stream",
                ));
            }
        }
    }
    Ok(features)
}

/// Group every layered part of the world's placed prototypes by bake inputs, in prototype id order.
pub fn groups(world: &World) -> Result<Vec<MaterialGroups>> {
    let mut uses = BTreeMap::<primase_core::Entity, usize>::new();
    for (_, i) in world.iter::<Instance>() {
        *uses.entry(i.prototype).or_default() += 1;
    }
    // in source id order, which reaches the output (the clustering's feature order, so the centres and levels): an
    // id names one prototype, so entity order never decides it
    let mut prototypes: Vec<_> = world
        .iter::<PrototypeSource>()
        .filter(|(e, _)| uses.contains_key(e) && world.has::<LayerInputs>(*e))
        .map(|(e, s)| (s.id.clone(), e))
        .collect();
    prototypes.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(w) = prototypes.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(invalid(
            "layer groups",
            0,
            format!("two placed prototypes with source id {}", w[0].0),
        ));
    }
    let mut by_material = BTreeMap::<String, Vec<(String, primase_core::Entity, usize)>>::new();
    for (id, e) in &prototypes {
        let mesh = world
            .get::<Mesh>(*e)
            .ok_or_else(|| invalid("layer groups", 0, "prototype without mesh"))?;
        for (part, primitive) in mesh.primitives().iter().enumerate() {
            let Some(material) = primitive.material.and_then(|m| world.get::<Material>(m)) else {
                continue;
            };
            if material.layered.is_some() {
                by_material
                    .entry(bake_identity(material)?)
                    .or_default()
                    .push((id.clone(), *e, part));
            }
        }
    }
    let mut output = Vec::new();
    for parts in by_material.values() {
        let mut features = Vec::new();
        let mut weights = Vec::new();
        let mut spans = Vec::new();
        let mut first = None;
        let mut first_entity = None;
        for (id, e, part) in parts {
            let primitive = &world.get::<Mesh>(*e).expect("listed prototype").primitives()[*part];
            let material_entity = primitive.material.expect("layered part");
            let material = world.get::<Material>(material_entity).expect("layered part");
            first.get_or_insert_with(|| material.clone());
            first_entity.get_or_insert(material_entity);
            let layer = vertex_inputs(primitive, material.shader.starts_with("M[MultipleGround]"))?;
            let positions = primitive
                .positions()
                .ok_or_else(|| invalid("layer groups", 0, "positions"))?;
            let Some(Stream::F32x3(normals)) = primitive.streams.get(&Semantic::Normal) else {
                return Err(invalid("layer groups", 0, "layered part without normals"));
            };
            let indices = primitive
                .indices
                .as_ref()
                .ok_or_else(|| invalid("layer groups", 0, "indexed geometry"))?;
            let start = features.len();
            for tri in indices.as_chunks::<3>().0 {
                let [a, b, c] = tri.map(|i| i as usize);
                let mut f: Features = std::array::from_fn(|k| (layer[a][k] + layer[b][k] + layer[c][k]) / 3.);
                let normal = Vec3::from(normals[a]) + Vec3::from(normals[b]) + Vec3::from(normals[c]);
                let up = if normal.length_squared() > 1e-12 {
                    normal.normalize().y.max(0.)
                } else {
                    0.
                };
                f[3] *= up;
                let cross = (Vec3::from(positions[b]) - Vec3::from(positions[a]))
                    .cross(Vec3::from(positions[c]) - Vec3::from(positions[a]));
                weights.push(f64::from(cross.length()) * 0.5 * uses[e] as f64);
                features.push(f);
            }
            spans.push((id.clone(), *part, start..features.len()));
        }
        let groups = grouping::cluster(&features, &weights)?;
        output.push(MaterialGroups {
            entity: first_entity.expect("nonempty group"),
            material: first.expect("nonempty group"),
            centers: groups.centers,
            period: [1.; 2],
            period_exact: false,
            parts: spans
                .into_iter()
                .map(|(id, part, span)| PartGroups {
                    prototype: id,
                    part,
                    assignments: groups.triangles[span].to_vec(),
                })
                .collect(),
        });
    }
    Ok(output)
}

/// Counts for the static base-mip bilinear-wrap bake, at per-material sizes
/// capped at 512 pixels. Channel presence counts use bake-input groups,
/// excluding both weight variants and raw GX-only source distinctions.
#[derive(Default, Debug)]
pub struct BakeReport {
    /// Unique full source identities, including raw GX.
    pub source_materials: usize,
    /// Static bake-input groups, excluding only raw GX.
    pub materials: usize,
    pub variants: usize,
    pub nonunit_uv_variants: usize,
    pub gloss_materials: usize,
    pub missing_gloss_materials: usize,
    pub emissive_materials: usize,
    pub missing_emissive_materials: usize,
    pub policies: primase_core::pass::PassLog,
}
