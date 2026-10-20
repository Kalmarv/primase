//! `lower-stuntboost-materials{baked=base-colour|emission,transparent=blend|opaque}`: every `stuntboost.material` gets
//! a `pbr.material` on its own entity.
//!
//! A bake material (`bake_mat_<N>`, on the `bake_<N>_object` meshes) samples its bake group's atlas
//! (`Bake<N>_Denoised`), the final colour with the level's lighting baked in. Lowered as `base-colour`, the atlas is
//! the base colour (factors as stored): the destination lights it again, so it reads darker and shadows double where
//! the destination's lights differ from the bake's. As `emission`, it is the emission (strength 1) over a black base:
//! the surface shows the baked light as baked, whatever the destination's lighting (its specular aside). The bake's
//! direct-light atlas (`KHR_materials_specular`'s colour texture, `Bake<N>_DIRECT_Denoised`) is not a specular colour
//! and is not lowered (counted; it stays in the dialect).
//!
//! Other materials keep their glTF meaning: base colour, metal, roughness, emission, alpha mode, double-sidedness and
//! texture coordinates as stored. A material drawn by a transparent node (a name containing `Transparent`,
//! [`crate::components::StuntboostDraw::Transparent`]) is, as `transparent=blend`, alpha blended with its alpha
//! multiplied by the node's `Transparent(alpha)`; as `opaque`, as stored. Both options are the program's, stated (no
//! default). Wrap is glTF's repeat; the samplers' filters stay in the dialect.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{Component, Entity, Instance, Mesh, World};

use crate::components::{StuntboostDraw, StuntboostMaterial, StuntboostNode, StuntboostTextureRef};

/// How a bake material's atlas lowers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Baked {
    BaseColour,
    Emission,
}

/// What a transparent node's materials become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transparent {
    Blend,
    Opaque,
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerStuntboostMaterials {
    pub baked: Baked,
    pub transparent: Transparent,
}

impl LowerStuntboostMaterials {
    /// From its options, both required: `baked` (`base-colour` or `emission`), `transparent` (`blend` or `opaque`).
    pub fn from_options(o: &Options) -> Result<LowerStuntboostMaterials, String> {
        if let Some(k) = o.keys().find(|k| !["baked", "transparent"].contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| {
            o.get(k).map(String::as_str).ok_or_else(|| {
                format!("{k} is required: baked=base-colour|emission, transparent=blend|opaque")
            })
        };
        Ok(LowerStuntboostMaterials {
            baked: match get("baked")? {
                "base-colour" => Baked::BaseColour,
                "emission" => Baked::Emission,
                v => return Err(format!("baked={v}: base-colour or emission")),
            },
            transparent: match get("transparent")? {
                "blend" => Transparent::Blend,
                "opaque" => Transparent::Opaque,
                v => return Err(format!("transparent={v}: blend or opaque")),
            },
        })
    }
}

fn texture(t: &Option<StuntboostTextureRef>) -> Option<TextureRef> {
    let t = t.as_ref()?;
    let mut r = TextureRef::new(t.texture);
    r.texcoord = u8::try_from(t.texcoord).unwrap_or(0);
    Some(r)
}

impl Pass for LowerStuntboostMaterials {
    fn name(&self) -> &'static str {
        "lower-stuntboost-materials"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "baked".to_owned(),
                match self.baked {
                    Baked::BaseColour => "base-colour",
                    Baked::Emission => "emission",
                }
                .to_owned(),
            ),
            (
                "transparent".to_owned(),
                match self.transparent {
                    Transparent::Blend => "blend",
                    Transparent::Opaque => "opaque",
                }
                .to_owned(),
            ),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the alpha a transparent node draws each material with (the lowest when nodes differ; 1 when none states one)
        let mut alpha_of: BTreeMap<Entity, f32> = BTreeMap::new();
        if self.transparent == Transparent::Blend {
            for (e, n) in w.iter::<StuntboostNode>() {
                let StuntboostDraw::Transparent { alpha } = n.name.draw() else {
                    continue;
                };
                let Some(mesh) = w.get::<Instance>(e).and_then(|i| w.get::<Mesh>(i.prototype)) else {
                    continue;
                };
                for m in mesh.primitives().iter().filter_map(|p| p.material) {
                    let a = alpha_of.entry(m).or_insert(1.0);
                    *a = a.min(alpha.unwrap_or(1.0));
                }
            }
        }
        let materials: Vec<(Entity, StuntboostMaterial)> = w
            .iter::<StuntboostMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        for (e, m) in materials {
            let mut pbr = PbrMaterial {
                base_color: m.base_color,
                base_color_texture: texture(&m.base_color_texture),
                metallic: m.metallic,
                roughness: m.roughness,
                emissive: m.emissive,
                double_sided: m.double_sided,
                alpha_mode: match (m.alpha_mode.as_str(), m.alpha_cutoff) {
                    ("MASK", c) => AlphaMode::Mask {
                        cutoff: c.unwrap_or(0.5),
                    },
                    ("BLEND", _) => AlphaMode::Blend,
                    _ => AlphaMode::Opaque,
                },
                ..PbrMaterial::default()
            };
            if m.name.starts_with("bake_mat_") {
                log.count("bake materials", 1);
                if self.baked == Baked::Emission {
                    pbr.emissive = [1.0, 1.0, 1.0];
                    pbr.emissive_texture = pbr.base_color_texture.take();
                    pbr.emissive_strength = 1.0;
                    pbr.base_color = [0.0, 0.0, 0.0, m.base_color[3]];
                }
            }
            if m.specular_color_texture.is_some() {
                log.count(
                    "baked direct-light atlases (KHR_materials_specular colour) not lowered",
                    1,
                );
            }
            if let Some(&alpha) = alpha_of.get(&e) {
                pbr.alpha_mode = AlphaMode::Blend;
                pbr.base_color[3] *= alpha;
                log.count("materials of transparent nodes blended", 1);
            }
            w.insert(e, pbr);
        }
        Ok(())
    }
}
