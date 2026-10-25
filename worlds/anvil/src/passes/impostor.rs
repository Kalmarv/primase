//! Static side-card form of an authored impostor. The caller explicitly chooses a cross; the game selects one
//! azimuth card per instance. Top-view selection, runtime fade, self shadow and translucency are not invented.
use std::collections::BTreeMap;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{ColorSpace, Component, Entity, Mesh, Name, Primitive, Semantic, Stream,
    TexRole, Texture, Topology, VertexColor, World, NormalConvention, NormalGreen, NormalZ,
    NormalX, NormalBasis};
use primase_image::{TextureMetadata, map_rgba8_levels};
use crate::components::{AnvilImpostor, AnvilRenderStatus};

#[derive(Clone, Debug)]
pub struct LowerAnvilImpostor;

pub const LOWER_ANVIL_IMPOSTOR: PassSpec = PassSpec {
    name: "lower-anvil-impostor",
    options: &[OptionSpec::required("form", "side-cross", "side-cross: all authored side cards, a static preview rather than camera selection")],
    make: |g: &Given| {
        if g.required("form") != "side-cross" { return Err("form must be side-cross".into()); }
        Ok(Box::new(LowerAnvilImpostor))
    },
};

fn metadata(role: TexRole, color_space: ColorSpace) -> TextureMetadata {
    TextureMetadata { role, color_space, normal: None }
}

fn decoded(world: &World, entity: Entity) -> Result<Texture, String> {
    let texture = world.get::<Texture>(entity).ok_or("Impostor atlas has no core.texture")?;
    map_rgba8_levels(texture, 0..texture.levels.len(), metadata(TexRole::Mask, ColorSpace::Linear),
        |texel| Ok(texel.rgba)).map_err(|e| e.to_string())
}

fn sample(texture: &Texture, level: usize, u: f32, v: f32) -> [u8; 4] {
    let level = level.min(texture.levels.len().saturating_sub(1));
    let width = (texture.width >> level).max(1);
    let height = (texture.height >> level).max(1);
    let x = ((u.clamp(0.0, 1.0) * width as f32) as u32).min(width - 1);
    let y = ((v.clamp(0.0, 1.0) * height as f32) as u32).min(height - 1);
    let at = (y as usize * width as usize + x as usize) * 4;
    texture.levels[level][at..at + 4].try_into().expect("validated RGBA8")
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a.into_iter().zip(b).map(|(a, b)| a * b).sum() }
fn normalized(value: [f32; 3]) -> [f32; 3] {
    let length = dot(value, value).sqrt();
    if length > 1e-8 { value.map(|v| v / length) } else { [0.0, 0.0, 1.0] }
}

impl Pass for LowerAnvilImpostor {
    fn name(&self) -> &'static str { "lower-anvil-impostor" }
    fn options(&self) -> Options { Options::from([("form".into(), "side-cross".into())]) }
    fn may_lack_inputs(&self) -> bool { true }
    fn requires(&self) -> &[&'static str] { &[AnvilImpostor::NAME] }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME, PbrMaterial::NAME, Texture::NAME, Name::NAME, VertexColor::NAME, AnvilRenderStatus::NAME]
    }
    fn removes(&self) -> &[&'static str] { &[AnvilImpostor::NAME] }
    fn approximates(&self) -> &'static str {
        "all authored side cards, no camera azimuth selection, LOD dither, self shadow or translucency; atlas samples at matching texel centres"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let entities: Vec<_> = world.iter::<AnvilImpostor>().map(|(e, _)| e).collect();
        // The same resource can be a form of multiple selectors; share derived images/materials across copies.
        let mut made: BTreeMap<(u64, usize), Entity> = BTreeMap::new();
        let mut atlases: BTreeMap<u64, (Entity, Entity)> = BTreeMap::new();
        for entity in entities {
            let Some(source) = world.get::<AnvilImpostor>(entity).cloned() else { continue; };
            let (Some(diffuse), Some(coverage), Some(misc)) =
                (source.textures[0], source.textures[1], source.textures[3]) else {
                let reason = format!("Impostor {:#x}: unresolved Diffuse, WorldNormalAlpha or Misc atlas; references {:?}",
                    source.index_id, source.stored.maps);
                world.insert(entity, AnvilRenderStatus { source: format!("{:#x}", source.index_id),
                    template: None, reason: reason.clone(), unsupported: true });
                log.omit(&reason, 1);
                continue;
            };
            let alpha = decoded(world, coverage)?;
            let miscellaneous = decoded(world, misc)?;
            let (base, roughness) = if let Some(&images) = atlases.get(&source.index_id) { images } else {
                let diffuse_texture = world.get::<Texture>(diffuse).ok_or("Impostor Diffuse atlas missing")?;
                let albedo = map_rgba8_levels(diffuse_texture, 0..diffuse_texture.levels.len(),
                metadata(TexRole::Color, diffuse_texture.color_space), |texel| {
                    let uv = [(texel.x as f32 + 0.5) / texel.width as f32,
                              (texel.y as f32 + 0.5) / texel.height as f32];
                    let mask = sample(&alpha, texel.level, uv[0], uv[1]);
                    Ok([texel.rgba[0], texel.rgba[1], texel.rgba[2], mask[3]])
                }).map_err(|e| e.to_string())?;
            let base = world.spawn();
            world.insert(base, albedo);
            world.insert(base, Name(format!("Impostor {:#x} colour and coverage", source.index_id)));
            let orm = map_rgba8_levels(&miscellaneous, 0..miscellaneous.levels.len(),
                metadata(TexRole::Mask, ColorSpace::Linear), |p| Ok([255, p.rgba[1], 0, 255]))
                .map_err(|e| e.to_string())?;
            let roughness = world.spawn();
            world.insert(roughness, orm);
                atlases.insert(source.index_id, (base, roughness));
                (base, roughness)
            };
            let mut primitives = Vec::new();
            for card_index in source.stored.top..source.stored.top + source.stored.side {
                let card = source.stored.cards[card_index];
                let a = normalized([card[12], card[13], card[14]]);
                let b = normalized([card[15], card[16], card[17]]);
                let c = normalized([card[18], card[19], card[20]]);
                let key = (source.index_id, card_index);
                let material = if let Some(&material) = made.get(&key) { material } else {
                    // The atlas normal is in the asset frame. Re-express it in this card's TBN; the placement then
                    // rotates it just as it rotates the card geometry. Preserve its authored alpha coverage.
                    let mut normal = map_rgba8_levels(&alpha, 0..alpha.levels.len(),
                        metadata(TexRole::Normal, ColorSpace::Linear), |p| {
                            let coverage = f32::from(p.rgba[3]) / 255.0;
                            let unpremultiply = if f32::from_bits(source.stored.table[20]) != 0.0 && coverage > 0.0 {
                                1.0 / coverage
                            } else { 1.0 };
                            let n = normalized([p.rgba[0], p.rgba[1], p.rgba[2]]
                                .map(|v| f32::from(v) / 255.0 * unpremultiply * 2.0 - 1.0));
                            let encode = |value: f32| ((value * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
                            Ok([encode(dot(n, b)), encode(dot(n, c)), encode(dot(n, a)), 255])
                        }).map_err(|e| e.to_string())?;
                    normal.normal = Some(NormalConvention { green: NormalGreen::Up, z: NormalZ::Stored,
                        x: NormalX::Red, basis: NormalBasis::TangentBitangent });
                    let normals = world.spawn();
                    world.insert(normals, normal);
                    let material = world.spawn();
                    world.insert(material, Name(format!("Impostor {:#x} side {card_index}", source.index_id)));
                    world.insert(material, PbrMaterial {
                        base_color_texture: Some(TextureRef::new(base)),
                        normal_texture: Some(TextureRef::new(normals)),
                        metallic_roughness_texture: Some(TextureRef::new(roughness)),
                        metallic: 0.0, roughness: 1.0, double_sided: true,
                        alpha_mode: AlphaMode::Mask { cutoff: f32::from_bits(source.stored.table[14]) },
                        ..Default::default()
                    });
                    world.insert(material, VertexColor::Ignored);
                    made.insert(key, material);
                    material
                };
                let positions: Vec<_> = card[..12].as_chunks::<3>().0.iter()
                    .map(|v| source.frame.point(*v)).collect();
                let normal = source.frame.direction(a);
                let tangent = source.frame.direction(b);
                let cross = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
                let source_sign = if dot(cross, c) < 0.0 { -1.0 } else { 1.0 };
                let sign = if source.frame.flips_winding() { -source_sign } else { source_sign };
                let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
                    .map(|[u, v]| [card[23] + u * card[21], card[24] + v * card[22]]);
                let indices = if source.frame.flips_winding() { vec![0, 2, 1, 2, 3, 1] }
                    else { vec![0, 1, 2, 2, 1, 3] };
                primitives.push(Primitive { topology: Topology::Triangles,
                    streams: BTreeMap::from([
                        (Semantic::Position, Stream::F32x3(positions)),
                        (Semantic::Normal, Stream::F32x3(vec![normal; 4])),
                        (Semantic::Tangent, Stream::F32x4(vec![[tangent[0], tangent[1], tangent[2], sign]; 4])),
                        (Semantic::TexCoord(0), Stream::F32x2(uv.to_vec())),
                    ]), indices: Some(indices), material: Some(material) });
            }
            world.insert(entity, Mesh::new(primitives));
            world.insert(entity, AnvilRenderStatus {
                source: format!("Impostor {:#x}", source.index_id), template: None, unsupported: false,
                reason: "authored side cards with owned diffuse/coverage/normal/roughness atlases; static cross, no camera selection or LOD dither".into(),
            });
            world.remove::<AnvilImpostor>(entity);
            log.count("Impostors lowered with their own atlases", 1);
            log.approximate("Impostor camera selection replaced by all side cards; self shadow and translucency omitted", 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::impostor::StoredImpostor;
    use primase_core::{TexFormat, frame::SourceFrame};

    fn image(world: &mut World, bytes: Vec<u8>, color_space: ColorSpace) -> Entity {
        let entity = world.spawn();
        world.insert(entity, Texture { width: 2, height: 1, format: TexFormat::Rgba8Unorm,
            levels: vec![bytes], color_space, role: TexRole::Other, normal: None });
        entity
    }

    #[test]
    fn owned_coverage_cuts_the_diffuse_and_derived_atlases_are_shared_between_occurrences() {
        let mut world = World::new();
        // Diffuse has no silhouette; the separate world-normal atlas owns coverage.
        let diffuse = image(&mut world, vec![20, 40, 60, 255, 80, 100, 120, 255], ColorSpace::Srgb);
        let alpha = image(&mut world, vec![128, 128, 255, 0, 128, 128, 255, 255], ColorSpace::Linear);
        let misc = image(&mut world, vec![5, 90, 7, 255, 6, 170, 8, 255], ColorSpace::Linear);
        let mut card = [0.0; 27];
        card[..12].copy_from_slice(&[-1.0, 0.0, 0.0, 1.0, 0.0, 0.0,
            -1.0, 0.0, 2.0, 1.0, 0.0, 2.0]);
        card[13] = -1.0; // normal -Y
        card[15] = 1.0; // tangent +X
        card[20] = 1.0; // bitangent +Z
        card[21] = 1.0;
        card[22] = 1.0;
        let mut table = [0; 21];
        table[1] = 1;
        table[14] = 0.5f32.to_bits();
        let source = AnvilImpostor { index_id: 77, frame: SourceFrame::ENU,
            stored: StoredImpostor { head: [0; 71], maps: [None; 4], table,
                top: 0, side: 1, cards: vec![card] }, textures: [Some(diffuse), Some(alpha), None, Some(misc)] };
        let first = world.spawn();
        let second = world.spawn();
        world.insert(first, source.clone());
        world.insert(second, source);
        LowerAnvilImpostor.run(&mut world, &mut PassLog::default()).unwrap();
        let mesh = world.get::<Mesh>(first).unwrap();
        assert_eq!(mesh.primitives()[0].indices.as_ref().unwrap().len(), 6);
        let material = mesh.primitives()[0].material.unwrap();
        assert_eq!(world.get::<Mesh>(second).unwrap().primitives()[0].material, Some(material));
        let pbr = world.get::<PbrMaterial>(material).unwrap();
        assert_eq!(pbr.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
        let base = world.get::<Texture>(pbr.base_color_texture.as_ref().unwrap().texture).unwrap();
        assert_eq!(base.levels[0], [20, 40, 60, 0, 80, 100, 120, 255]);
        assert_eq!(base.color_space, ColorSpace::Srgb);
        let orm = world.get::<Texture>(pbr.metallic_roughness_texture.as_ref().unwrap().texture).unwrap();
        assert_eq!(orm.levels[0], [255, 90, 0, 255, 255, 170, 0, 255]);
        assert!(!world.has::<AnvilImpostor>(first));
    }
}
