//! Area-weighted static layer grouping over an area whose geometry `lower-fromsoft-geometry` lowered: each layered
//! part keeps its raw per-vertex shader inputs (`fromsoft.layer_inputs`); grouping and baking are the bake pass's.
use super::source::{LayerInputs, PrototypeSource, ShaderPrograms, TexturePayloads};
use crate::layer_bake::grouping::{self, Features};
use crate::{Result, invalid, materials::Material};
use primase_core::{Instance, Mesh, Semantic, Stream, World};
use primase_meshops::vector::Vec3;
use std::{collections::BTreeMap, sync::Arc};

/// Prepared bake inventory over the source programs and texture files it borrows; each worker may prepare one
/// material at a time, keeping decoded pixel memory bounded.
#[derive(Debug)]
pub struct AreaBake<'a> {
    pub groups: Vec<MaterialGroups>,
    shaders: &'a BTreeMap<String, Vec<u8>>,
    payloads: &'a BTreeMap<String, Vec<u8>>,
}
/// Nothing, for a world without the resource.
static EMPTY: BTreeMap<String, Vec<u8>> = BTreeMap::new();
impl<'a> AreaBake<'a> {
    /// Group the layered parts of a world's placed prototypes by bake inputs, over the world's own source data.
    pub fn from_world(world: &'a World) -> Result<Self> {
        let shaders = world.resource::<ShaderPrograms>().map_or(&EMPTY, |s| &s.0);
        let payloads = world.resource::<TexturePayloads>().map_or(&EMPTY, |p| &p.0);
        Self::with_sources(world, shaders, payloads)
    }
    /// The same over source data held apart from `world`.
    pub fn with_sources(
        world: &World,
        shaders: &'a BTreeMap<String, Vec<u8>>,
        payloads: &'a BTreeMap<String, Vec<u8>>,
    ) -> Result<Self> {
        let mut groups = groups(world)?;
        for group in &mut groups {
            let material = group.material.layered.as_ref().expect("layered group");
            let prepared =
                crate::layer_bake::Prepared::new(program(shaders, material)?, material, 0., |_, _| Ok(None))?;
            group.period = prepared.period;
            group.period_exact = prepared.period_exact;
        }
        Ok(Self {
            groups,
            shaders,
            payloads,
        })
    }
    /// A group's program bound to its materials' inputs, sampled at `sample_time` seconds.
    pub fn prepare(&self, group: usize, sample_time: f32) -> Result<crate::layer_bake::Prepared> {
        use primase_texcodec::TexDecoder;
        let material = self
            .groups
            .get(group)
            .and_then(|g| g.material.layered.as_ref())
            .ok_or_else(|| invalid("area bake", group, "group index"))?;
        crate::layer_bake::Prepared::new(
            program(self.shaders, material)?,
            material,
            sample_time,
            |name, srgb| {
                let Some(bytes) = self.payloads.get(name) else {
                    return Ok(None);
                };
                let texture = primase_dds::read(bytes).map_err(|e| invalid("area bake DDS", 0, e))?;
                let pixels = primase_texcodec::Decoder::default()
                    .decode_rgba8(texture.format, texture.width, texture.height, &texture.levels[0])
                    .map_err(|e| invalid("area bake pixels", 0, e))?;
                Ok(Some(Arc::new(crate::layer_bake::BaseMipBilinearWrap::new(
                    texture.width,
                    texture.height,
                    &pixels,
                    srgb,
                )?)))
            },
        )
    }
}

#[derive(Debug, serde::Serialize)]
pub struct MaterialGroups {
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

fn program<'a>(
    programs: &'a BTreeMap<String, Vec<u8>>,
    material: &crate::materials::LayeredMaterial,
) -> Result<&'a [u8]> {
    let name = material.program_name()?;
    programs
        .get(&name)
        .map(Vec::as_slice)
        .ok_or_else(|| invalid("layer shader", 0, format!("missing {name}")))
}

// This is the static bake-input equivalence relation, not source-material
// identity. Raw GX is retained on per-source output materials; the current
// interpreter consumes the definition/bindings and does not interpret GX.
pub(super) fn bake_identity(material: &Material) -> Result<String> {
    let mut projected = material.clone();
    projected.gx.clear();
    primase_core::json::to_value(&projected)
        .map(|v| v.to_string())
        .map_err(|e| invalid("layer groups", 0, e))
}

/// The eight bake inputs per vertex from the raw Byte4C members: layer weights, snow, tint RGB and detail.
/// MultipleGround reads its second weight from member 2's y, Multiple from its z. Missing members read zero.
fn vertex_inputs(primitive: &primase_core::Primitive, ground: bool) -> Result<Vec<Features>> {
    let channels = [
        (1, 0),
        (2, if ground { 1 } else { 2 }),
        (4, 0),
        (5, 1),
        (3, 0),
        (3, 1),
        (3, 2),
        (6, 2),
    ];
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
    let mut prototypes: Vec<_> = world
        .iter::<PrototypeSource>()
        .filter(|(e, _)| uses.contains_key(e) && world.has::<LayerInputs>(*e))
        .map(|(e, s)| (s.id.clone(), e))
        .collect();
    prototypes.sort();
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
        for (id, e, part) in parts {
            let primitive = &world.get::<Mesh>(*e).expect("listed prototype").primitives()[*part];
            let material = primitive
                .material
                .and_then(|m| world.get::<Material>(m))
                .expect("layered part");
            first.get_or_insert_with(|| material.clone());
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
