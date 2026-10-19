//! `uv-frame-normals`: normal maps re-expressed in the tangent frame derived from texture coordinates, for a target
//! that derives its own frame from them instead of reading supplied tangents.
use crate::{Error, Result};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Component, Entity, Mesh, Name, NormalConvention, Semantic, Stream, TexFormat, TexRole, Texture,
    TextureFile, World,
};
use primase_meshops::frames::{AxisMap, axis_map, uv_axes};
use primase_meshops::vector::Vec3;
use std::collections::BTreeMap;

/// A triangle's frame agrees with a map of the texture's when their matrices differ by at most this.
pub const EXACT: f32 = 1e-3;
/// Beyond this a triangle's frame is no quarter-turn or mirror of the derived one (sheared, or turned between).
pub const UNRELATED: f32 = 0.5;

/// Which way the derived bitangent points along the texture's v axis: the target's choice, as its renderer derives
/// it (`+v`: towards increasing v; `-v`: towards decreasing v).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bitangent {
    PlusV,
    MinusV,
}

/// `uv-frame-normals{bitangent=+v|-v}` (the option is required: it is the target's convention): every normal map a
/// `pbr.material` binds, in glTF's convention against its meshes' supplied tangents (`normalize-normals` first),
/// re-expressed against the frame derived from the coordinates it is sampled with: tangent along increasing u,
/// bitangent `cross(N, T) * w` along the chosen v direction, both in the surface. The `TANGENT` streams of every
/// primitive whose map is re-based become that frame, so the pair stays consistent for any writer; a material none of
/// whose triangles is near any map keeps its map and its supplied tangents.
///
/// Each triangle's supplied frame is a quarter turn or mirror of the derived one ([`AxisMap`]), or neither (sheared or
/// turned between). Each material binding a normal map takes the map most of its triangles' area takes; the map is
/// applied to the texture's red and green bytes exactly (a swap, `255 - v`), every level. A texture whose materials
/// take different maps keeps the map of the largest area and gets a re-expressed copy (`<name>~uv<n>`) for each other
/// map, its materials rebound to it. The area of triangles that take another map than their material's, or none, is
/// counted as an approximation in square metres (rounded); triangles are measured in their mesh's own space, as a
/// target deriving frames from coordinates measures them. The identity map leaves a texture as it is. The frames
/// come from the meshes; the pass knows no game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UvFrameNormals {
    pub bitangent: Bitangent,
}

impl UvFrameNormals {
    pub fn from_options(o: &Options) -> std::result::Result<Self, String> {
        let mut bitangent = None;
        for (k, v) in o {
            match (k.as_str(), v.as_str()) {
                ("bitangent", "+v") => bitangent = Some(Bitangent::PlusV),
                ("bitangent", "-v") => bitangent = Some(Bitangent::MinusV),
                ("bitangent", v) => return Err(format!("bitangent={v} is not +v or -v")),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(UvFrameNormals {
            bitangent: bitangent.ok_or("bitangent=+v|-v is required: it is the target's frame")?,
        })
    }
}

/// Per material binding a normal map: the area each map takes, and the area of triangles near none.
#[derive(Default)]
struct Votes {
    maps: BTreeMap<AxisMap, (f64, f64)>,
    unrelated: f64,
}

fn stream3(p: &primase_core::Primitive, s: Semantic) -> Option<&Vec<[f32; 3]>> {
    match p.streams.get(&s) {
        Some(Stream::F32x3(v)) => Some(v),
        _ => None,
    }
}

impl Pass for UvFrameNormals {
    fn name(&self) -> &'static str {
        "uv-frame-normals"
    }
    fn options(&self) -> Options {
        Options::from([(
            "bitangent".to_owned(),
            match self.bitangent {
                Bitangent::PlusV => "+v",
                Bitangent::MinusV => "-v",
            }
            .to_owned(),
        )])
    }
    fn requires(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME, Mesh::NAME, PbrMaterial::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[TextureFile::NAME]
    }
    fn approximates(&self) -> &'static str {
        "triangles whose supplied frame is not the normal map's map of the derived one"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        run(world, self.bitangent, log).map_err(|e| e.to_string())
    }
}

/// `t` with `map` applied to its red and green bytes on every level, as RGBA8.
fn re_express(t: &Texture, map: AxisMap) -> Result<Texture> {
    let mut levels = Vec::with_capacity(t.levels.len());
    for (i, level) in t.levels.iter().enumerate() {
        let (w, h) = crate::texture::level_dimensions(t, i)?;
        let mut rgba = if t.format == TexFormat::Rgba8Unorm {
            level.clone()
        } else {
            use primase_texcodec::TexDecoder;
            primase_texcodec::Decoder::default().decode_rgba8(t.format, w, h, level)?
        };
        for p in rgba.as_chunks_mut::<4>().0 {
            [p[0], p[1]] = map.apply_bytes([p[0], p[1]]);
        }
        levels.push(rgba);
    }
    let mut changed = t.clone();
    changed.format = TexFormat::Rgba8Unorm;
    changed.levels = levels;
    Ok(changed)
}

fn run(world: &mut World, bitangent: Bitangent, log: &mut PassLog) -> Result<()> {
    let side = match bitangent {
        Bitangent::PlusV => 1f32,
        Bitangent::MinusV => -1.,
    };
    // the normal map each material binds, with its coordinates
    let mut bound = BTreeMap::<Entity, primase_core::pbr::TextureRef>::new();
    for (e, m) in world.iter::<PbrMaterial>() {
        if let Some(r) = m.normal_texture {
            let t = world
                .get::<Texture>(r.texture)
                .ok_or_else(|| Error::Invalid(format!("normal map {:?} is not loaded", r.texture)))?;
            if t.role != TexRole::Normal || t.normal != Some(NormalConvention::GLTF) {
                return Err(Error::Invalid(format!(
                    "normal map {:?} is not in glTF's convention: run normalize-normals first",
                    r.texture
                )));
            }
            bound.insert(e, r);
        }
    }
    let mut votes = BTreeMap::<Entity, Votes>::new();
    // per mesh entity, per primitive: the derived tangents
    let mut tangents = Vec::<(Entity, usize, Entity, Vec<[f32; 4]>)>::new();
    for (e, mesh) in world.iter::<Mesh>() {
        for (index, p) in mesh.primitives().iter().enumerate() {
            let Some(r) = p.material.and_then(|m| bound.get(&m)) else {
                continue;
            };
            let (Some(positions), Some(normals), Some(Stream::F32x4(supplied)), Some(Stream::F32x2(uv))) = (
                stream3(p, Semantic::Position),
                stream3(p, Semantic::Normal),
                p.streams.get(&Semantic::Tangent),
                p.streams.get(&Semantic::TexCoord(r.texcoord)),
            ) else {
                log.count("primitives without a tangent frame left as they are", 1);
                continue;
            };
            let transform = r.transform.unwrap_or(primase_core::pbr::UvTransform::IDENTITY);
            let uv: Vec<[f32; 2]> = uv.iter().map(|&c| transform.apply(c)).collect();
            let indices: Vec<u32> = p
                .indices
                .clone()
                .unwrap_or_else(|| (0..positions.len() as u32).collect());
            let vote = votes.entry(p.material.expect("bound material")).or_default();
            let mut sum_u = vec![Vec3::ZERO; positions.len()];
            let mut sum_v = vec![Vec3::ZERO; positions.len()];
            for t in indices.as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| i as usize);
                let n = Vec3::from(normals[a]) + Vec3::from(normals[b]) + Vec3::from(normals[c]);
                let area = f64::from(
                    (Vec3::from(positions[b]) - Vec3::from(positions[a]))
                        .cross(Vec3::from(positions[c]) - Vec3::from(positions[a]))
                        .length(),
                ) * 0.5;
                let Some((u, v)) = uv_axes(
                    [positions[a], positions[b], positions[c]],
                    [uv[a], uv[b], uv[c]],
                    n.into(),
                ) else {
                    vote.unrelated += area;
                    continue;
                };
                let v = <[f32; 3]>::from(Vec3::from(v) * side);
                // the supplied frame of the triangle: its corners' tangents and bitangents summed
                let mut st = Vec3::ZERO;
                let mut sb = Vec3::ZERO;
                for i in [a, b, c] {
                    let tt = Vec3::new(supplied[i][0], supplied[i][1], supplied[i][2]);
                    st += tt;
                    sb += Vec3::from(normals[i]).cross(tt) * supplied[i][3];
                }
                let flat = |x: Vec3| {
                    let nn = n.normalize_or(Vec3::ZERO);
                    <[f32; 3]>::from((x - nn * x.dot(nn)).normalize_or(Vec3::ZERO))
                };
                let (map, error) = axis_map([flat(st), flat(sb)], [u, v]);
                if error > UNRELATED {
                    vote.unrelated += area;
                } else {
                    let entry = vote.maps.entry(map).or_default();
                    entry.0 += area;
                    if error > EXACT {
                        entry.1 += area;
                    }
                }
                for i in [a, b, c] {
                    sum_u[i] += Vec3::from(u);
                    sum_v[i] += Vec3::from(v);
                }
            }
            let derived = (0..positions.len())
                .map(|i| {
                    let n = Vec3::from(normals[i]).normalize_or(Vec3::ZERO);
                    let t = (sum_u[i] - n * sum_u[i].dot(n)).normalize_or(Vec3::ZERO);
                    if t == Vec3::ZERO {
                        // unused or degenerate everywhere: the supplied tangent stays
                        return supplied[i];
                    }
                    let w = if n.cross(t).dot(sum_v[i]) < 0. { -1. } else { 1. };
                    [t.x, t.y, t.z, w]
                })
                .collect();
            tangents.push((e, index, p.material.expect("bound material"), derived));
        }
    }
    // each material's map, and the maps each texture needs
    let mut needs = BTreeMap::<Entity, BTreeMap<AxisMap, (f64, Vec<Entity>)>>::new();
    // materials whose texture is read in the derived frame afterwards
    let mut rebased = std::collections::BTreeSet::new();
    for (material, vote) in &votes {
        let total: f64 = vote.maps.values().map(|(a, _)| a).sum::<f64>() + vote.unrelated;
        let Some((&map, &(area, inexact))) = vote
            .maps
            .iter()
            .max_by(|a, b| a.1.0.total_cmp(&b.1.0).then(b.0.cmp(a.0)))
        else {
            log.count(
                "normal-mapped materials with no triangle in a frame: left as they are",
                1,
            );
            if total > 0. {
                log.approximate(
                    "triangle area (square metres) read with its material's frame map but not in it",
                    total.round() as u64,
                );
            }
            continue;
        };
        let other = total - area + inexact;
        if other > 0. {
            log.approximate(
                "triangle area (square metres) read with its material's frame map but not in it",
                other.round() as u64,
            );
        }
        rebased.insert(*material);
        let texture = bound[material].texture;
        let need = needs.entry(texture).or_default().entry(map).or_default();
        need.0 += total;
        need.1.push(*material);
    }
    // re-expressed textures, checked and made before anything changes in the world
    let mut changes = Vec::new();
    let mut copies = Vec::new();
    for (texture, maps) in &needs {
        let mut maps: Vec<_> = maps.iter().collect();
        maps.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.0.cmp(b.0)));
        let t = world.get::<Texture>(*texture).expect("bound above");
        for (k, (map, (_, materials))) in maps.into_iter().enumerate() {
            if k == 0 && *map == AxisMap::IDENTITY {
                log.count("normal maps already in the derived frame", 1);
                continue;
            }
            let changed = if *map == AxisMap::IDENTITY {
                t.clone()
            } else {
                re_express(t, *map)?
            };
            log.count(&format!("normal maps re-expressed ({:?})", *map), 1);
            if k == 0 {
                changes.push((*texture, changed));
            } else {
                copies.push((*texture, k, changed, materials.clone()));
            }
        }
    }
    for (e, t) in changes {
        world.insert(e, t);
        world.remove::<TextureFile>(e);
    }
    for (source, k, t, materials) in copies {
        let name = world
            .get::<Name>(source)
            .map_or_else(|| format!("{source:?}"), |n| n.0.clone());
        let copy = world.spawn();
        world.insert(copy, Name(format!("{name}~uv{k}")));
        world.insert(copy, t);
        for m in materials {
            let pbr = world.get_mut::<PbrMaterial>(m).expect("bound material");
            if let Some(r) = pbr.normal_texture.as_mut() {
                r.texture = copy;
            }
        }
        log.count("normal-map copies for materials in another frame", 1);
    }
    for (e, index, material, derived) in tangents {
        if !rebased.contains(&material) {
            // its map stays in the supplied frame, and so do the tangents it is read with
            continue;
        }
        let mesh = world.get_mut::<Mesh>(e).expect("listed mesh");
        mesh.primitives_mut()[index]
            .streams
            .insert(Semantic::Tangent, Stream::F32x4(derived));
        log.count("primitives given the derived tangent frame", 1);
    }
    Ok(())
}
