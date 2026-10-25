//! Static texture-space material bake from resolved game programs. The binding owns the G-buffer layout.
mod cache;
mod colour;
mod pixels;

use super::settings::{PixelCap, Seconds};
use crate::materials::Material;
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, Specular, TextureRef};
use primase_core::shader::{ShaderBinding, Stage};
use primase_core::{ColorSpace, Component, Mesh, Name, TexFormat, TexRole, Texture, World};
use std::collections::BTreeSet;

/// `bake-fromsoft-materials{max-size=N,sample-time=S,cache=off|on}`: replace lowered surface channels with the resolved pixel
/// program's outputs over UV0. Run after resolve and material lowering, before normal conversion. Geometry must
/// keep its source UV tiling. The source material and shader binding remain available to a game-program renderer.
/// Unsupported programs retain their lowered material, with a refusal reason. Layer-weight materials belong to
/// `bake-fromsoft-layers`, which groups their vertex weights; this pass does not replace that result with one weight.
/// The square bake requests the largest bound source texture edge, limited by `max-size`. Allocation and cap
/// counts aggregate by source/requested/allocated dimensions, without material names. The source width and height
/// are independent maxima across bound images and may come from different images. This follows native texture
/// density, not the density of repeated detail layers.
/// Caching is off by default: the pass chooses no storage and performs no filesystem work. `cache=on` uses
/// the caller's optional `core.pass_memo` service; without that service the pass still computes every result.
/// The program owns the store and its error statistics, size limit and trimming policy. Hits replay the exact
/// material planes and report; hits/misses count enabled lookups, including refused misses.
/// The static frame uses a flat tangent frame, white vertex colour, coincident UV sets and no per-light gloss term.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BakeFromSoftMaterials {
    max_size: PixelCap,
    sample_time: Seconds,
    cache: bool,
}
impl BakeFromSoftMaterials {
    pub fn new(max_size: PixelCap, sample_time: Seconds) -> Self {
        Self {
            max_size,
            sample_time,
            cache: false,
        }
    }
    /// Use the caller's memo service when enabled; the constructor leaves caching off.
    pub fn with_cache(mut self, enabled: bool) -> Self {
        self.cache = enabled;
        self
    }
}
/// Whether `bake-fromsoft-materials` uses the caller's memo store (`cache=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cache {
    /// No storage is used and no filesystem work is done.
    Off,
    /// Each material's bake is looked up in, and stored to, the caller's `core.pass_memo` service.
    On,
}

impl primase_core::pass::Choice for Cache {
    const VALUES: &'static [(&'static str, Self)] = &[("off", Cache::Off), ("on", Cache::On)];
}

pub const BAKE_FROMSOFT_MATERIALS: PassSpec = PassSpec {
    name: "bake-fromsoft-materials",
    options: &[
        OptionSpec::required("max-size", "512", "pixels, 1 to 4096: largest baked edge"),
        OptionSpec::required("sample-time", "0", "seconds: static material frame"),
        OptionSpec::optional_choice::<Cache>(
            "cache",
            "use the caller's memo store for each material's bake",
            "off: no storage is used",
        ),
    ],
    make: |g| {
        let pass = BakeFromSoftMaterials::new(
            PixelCap::parse("max-size", g.required("max-size"))?,
            Seconds::parse("sample-time", g.required("sample-time"))?,
        );
        Ok(Box::new(pass.with_cache(g.optional_choice::<Cache>("cache") == Some(Cache::On))))
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
            ("cache".into(), if self.cache { "on" } else { "off" }.into()),
        ])
    }
    /// Neither `core.shader_binding` nor `pbr.material` is required: a material whose program the reader does not hold
    /// stays unbound (`resolve-fromsoft-shaders` counts it), a read none of whose materials has one (a weapon part) has
    /// nothing to bake, and one with no material that is drawn (an area whose models were all left out) has no lowered
    /// material either; `run` counts each material it finds without either and bakes the rest. A read with no
    /// `fromsoft.material` at all skips the step.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME]
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
        let memo = if self.cache {
            world.resource::<primase_core::pass::memo::Memo>().cloned()
        } else {
            None
        };
        log.count("program bake cache hits", 0);
        log.count("program bake cache misses", 0);
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
            let baked = match pixels::prepare(
                world,
                stage,
                binding.stage(Stage::Vertex),
                &material,
                self.max_size.get(),
                self.sample_time.get(),
            )
            .map_err(cache::Error::Refused)
            .and_then(|prepared| cache::bake(prepared, world, memo.as_ref(), log))
            {
                Ok(b) => b,
                Err(cache::Error::Failed(e)) => return Err(e),
                Err(cache::Error::Refused(e)) => {
                    log.approximate(
                        &format!("program bake refused; lowered material retained: {e}"),
                        1,
                    );
                    continue;
                }
            };
            let name = format!("baked_{}", source.identity_digest().map_err(|e| e.to_string())?);
            baked.report(log);
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
            if let Some((rgba, strength)) = baked.emission {
                material.emissive_texture = Some(add(
                    world,
                    "emission",
                    rgba,
                    TexRole::Color,
                    ColorSpace::Srgb,
                    None,
                ));
                material.emissive = [1.; 3];
                material.emissive_strength = strength;
            }
            world.insert(entity, material);
        }
        Ok(())
    }
}
