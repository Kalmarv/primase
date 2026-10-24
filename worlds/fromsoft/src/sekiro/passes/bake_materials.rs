//! Static texture-space material bake from resolved game programs. The binding owns the G-buffer layout.
mod pixels;

use super::settings::{PixelCap, Seconds};
use crate::materials::Material;
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, Specular, TextureRef};
use primase_core::shader::{ShaderBinding, Stage};
use primase_core::{ColorSpace, Component, Mesh, Name, TexFormat, TexRole, Texture, World};
use std::collections::BTreeSet;

/// `bake-fromsoft-materials{max-size=N,sample-time=S}`: replace lowered surface channels with the resolved pixel
/// program's outputs over UV0. Run after resolve and material lowering, before normal conversion. Geometry must
/// keep its source UV tiling. The source material and shader binding remain available to a game-program renderer.
/// Unsupported programs retain their lowered material, with a refusal reason. Layer-weight materials belong to
/// `bake-fromsoft-layers`, which groups their vertex weights; this pass does not replace that result with one weight.
/// The square bake requests the largest bound source texture edge, limited by `max-size`; each allocation and cap
/// is reported. This follows native texture density, not the density of repeated detail layers.
/// The static frame uses a flat tangent frame, white vertex colour, coincident UV sets and no per-light gloss term.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BakeFromSoftMaterials {
    max_size: PixelCap,
    sample_time: Seconds,
}
impl BakeFromSoftMaterials {
    pub fn new(max_size: PixelCap, sample_time: Seconds) -> Self {
        Self {
            max_size,
            sample_time,
        }
    }
}
pub const BAKE_FROMSOFT_MATERIALS: PassSpec = PassSpec {
    name: "bake-fromsoft-materials",
    options: &[
        OptionSpec::required("max-size", "512", "pixels, 1 to 4096: largest baked edge"),
        OptionSpec::required("sample-time", "0", "seconds: static material frame"),
    ],
    make: |g| {
        Ok(Box::new(BakeFromSoftMaterials::new(
            PixelCap::parse("max-size", g.required("max-size"))?,
            Seconds::parse("sample-time", g.required("sample-time"))?,
        )))
    },
};

impl Pass for BakeFromSoftMaterials {
    fn name(&self) -> &'static str {
        "bake-fromsoft-materials"
    }
    fn options(&self) -> Options {
        Options::from([
            ("max-size".into(), self.max_size.to_string()),
            ("sample-time".into(), self.sample_time.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME, ShaderBinding::NAME, PbrMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "static flat tangent frame and white vertex colour; secondary UV sets replaced by UV0; base-mip bilinear sampling; per-light gloss correction omitted; UNORM8 output quantization"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let drawn: BTreeSet<_> = world
            .iter::<Mesh>()
            .flat_map(|(_, m)| m.primitives().iter().filter_map(|p| p.material))
            .collect();
        for entity in drawn {
            let Some(source) = world.get::<Material>(entity) else {
                continue;
            };
            if source.layered.is_some() {
                log.count("layer-weight materials left to the grouped layer bake", 1);
                continue;
            }
            let Some(mut material) = world.get::<PbrMaterial>(entity).cloned() else {
                continue;
            };
            let Some(binding) = world.get::<ShaderBinding>(entity) else {
                log.count("materials without a resolved pixel program", 1);
                continue;
            };
            let Some(stage) = binding.stage(Stage::Pixel) else {
                log.count("materials without a resolved pixel program", 1);
                continue;
            };
            if world
                .get::<super::UvTiling>(entity)
                .is_some_and(|s| s.0 != [1.; 2])
            {
                return Err("bake-fromsoft-materials requires geometry uv-tiling=keep".into());
            }
            let baked = match pixels::bake(
                world,
                stage,
                binding.stage(Stage::Vertex),
                &material,
                self.max_size.get(),
                self.sample_time.get(),
            ) {
                Ok(b) => b,
                Err(e) => {
                    log.approximate(
                        &format!("program bake refused; lowered material retained: {e}"),
                        1,
                    );
                    continue;
                }
            };
            let name = format!("baked_{}", source.identity_digest().map_err(|e| e.to_string())?);
            let material_name = world.get::<Name>(entity).map_or(name.as_str(), |n| n.0.as_str());
            let allocation = format!(
                "program bake {material_name} ({name}): source maxima {}x{}, requested {}x{}, allocated {}x{}",
                baked.source_size[0],
                baked.source_size[1],
                baked.requested_size,
                baked.requested_size,
                baked.size,
                baked.size,
            );
            log.count(&allocation, 1);
            if baked.size < baked.requested_size {
                log.approximate(&format!("native texture density capped: {allocation}"), 1);
            }
            if baked.source_size == [0; 2] {
                log.approximate("no bound image dimensions; program bake uses max-size", 1);
            }
            log.approximate(
                "program bake follows native texture dimensions; tiled detail can lose density",
                1,
            );
            for (k, n) in baked.policies {
                log.approximate(&k, n);
            }
            let add = |world: &mut World, suffix: &str, bytes, role, color_space, normal| {
                let e = world.spawn();
                world.insert(e, Name(format!("{name}_{suffix}")));
                world.insert(
                    e,
                    Texture {
                        width: baked.size,
                        height: baked.size,
                        format: TexFormat::Rgba8Unorm,
                        levels: vec![bytes],
                        role,
                        color_space,
                        normal,
                    },
                );
                TextureRef::new(e)
            };
            material.base_color_texture = Some(add(
                world,
                "albedo",
                baked.albedo,
                TexRole::Color,
                ColorSpace::Srgb,
                None,
            ));
            material.base_color = [1.; 4];
            material.normal_texture = Some(add(
                world,
                "normal",
                baked.normal,
                TexRole::Normal,
                ColorSpace::Linear,
                Some(super::normals::SOURCE),
            ));
            material.normal_scale = 1.;
            material.metallic_roughness_texture = Some(add(
                world,
                "orm",
                baked.orm,
                TexRole::Mask,
                ColorSpace::Linear,
                None,
            ));
            material.roughness = 1.;
            material.metallic = if baked.specular.is_some() { 0. } else { 1. };
            material.specular = baked.specular.map(|rgb| Specular {
                color: [25.; 3],
                color_texture: Some(add(world, "f0", rgb, TexRole::Color, ColorSpace::Srgb, None)),
                ..Specular::default()
            });
            if let Some(rgb) = baked.emission {
                let (texture, strength, zeroed) =
                    super::pbr::emissive_from_linear(baked.size, baked.size, &rgb)
                        .map_err(|e| e.to_string())?;
                let e = world.spawn();
                world.insert(e, Name(format!("{name}_emission")));
                world.insert(e, texture);
                material.emissive_texture = Some(TextureRef::new(e));
                material.emissive = [1.; 3];
                material.emissive_strength = strength;
                log.count("nonzero baked emission texels quantized black", zeroed);
            }
            world.insert(entity, material);
            log.count("materials baked from their G-buffer program", 1);
            log.count("program bake texels", u64::from(baked.size).pow(2));
        }
        Ok(())
    }
}
