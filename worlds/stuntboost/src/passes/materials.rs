//! `lower-stuntboost-materials{baked=base-colour|emission|unlit,transparent=blend|opaque}`: every
//! `stuntboost.material` gets a `pbr.material` on its own entity, its textures sampled with the filters and wrap their
//! samplers state.
//!
//! A bake material (`bake_mat_<N>`, on the `bake_<N>_object` meshes) samples its bake group's atlas
//! (`Bake<N>_Denoised`), the final colour with the level's lighting baked in. Lowered as `base-colour`, the atlas is
//! the base colour (factors as stored): the destination lights it again, so it reads darker and shadows double where
//! the destination's lights differ from the bake's. As `emission`, it is the emission (strength 1) over a black base:
//! the surface shows the baked light as baked, whatever the destination's lighting (its specular aside). As
//! `unlit`, it is the base colour of an unlit material (`Shading::Unlit`): drawn as baked, no light added. That is how
//! the game draws a baked level (one with a `SetBakedLighting` node): unlit,
//! `colour = atlas * (1 + k * ShadowColor) * lerp(AmbientLightInShadow, 1, 1 - k)`, where `k` is the board's dynamic
//! shadow gated by the direct-light mask; with no dynamic shadow (`k = 0`) exactly the atlas. Choosing `unlit` for a
//! level without that node (the game lights it) is counted. The bake's
//! direct-light mask ([`crate::components::StuntboostDirectLight`]) gates only the game's dynamic shadows and is not
//! lowered (counted; it stays in the dialect).
//!
//! Other materials keep their glTF meaning: base colour, metal, roughness, emission, alpha mode, double-sidedness and
//! texture coordinates as stored. A transparent node (a name containing `Transparent`,
//! [`crate::components::StuntboostDraw::Transparent`]) draws, as `transparent=blend`, with blended copies of its
//! materials, their alpha multiplied by the node's `Transparent(alpha)`: one copy per material and alpha, given to the
//! node as a `core.material_variants` variant of its prototype, so the opaque nodes sharing a material (in STUNTBOOST
//! the glass and curtains draw with the level's bake material) keep it unchanged. As `opaque`, transparent nodes draw
//! as stored. Both options are the program's, stated (no
//! default). Wrap is glTF's repeat; the samplers' filters stay in the dialect.

use std::collections::BTreeMap;

use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, TextureRef};
use primase_core::{Component, Entity, Instance, MaterialVariants, Mesh, Name, World};

use crate::components::{
    StuntboostDirectLight, StuntboostDraw, StuntboostMaterial, StuntboostNode, StuntboostTextureRef,
};

/// How a bake material's atlas lowers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Baked {
    BaseColour,
    Emission,
    Unlit,
}

impl Choice for Baked {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("base-colour", Baked::BaseColour),
        ("emission", Baked::Emission),
        ("unlit", Baked::Unlit),
    ];
}

/// What a transparent node's materials become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transparent {
    Blend,
    Opaque,
}

impl Choice for Transparent {
    const VALUES: &'static [(&'static str, Self)] =
        &[("blend", Transparent::Blend), ("opaque", Transparent::Opaque)];
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerStuntboostMaterials {
    pub baked: Baked,
    pub transparent: Transparent,
}

/// `lower-stuntboost-materials`' declaration ([`primase_core::pass::PassRegistry::add`]): both options are the
/// program's, without defaults.
pub const LOWER_STUNTBOOST_MATERIALS: PassSpec = PassSpec {
    name: "lower-stuntboost-materials",
    options: &[
        OptionSpec::choice::<Baked>(
            "baked",
            "what a bake material's atlas lowers as: base colour, emission, or unlit",
        ),
        OptionSpec::choice::<Transparent>(
            "transparent",
            "a transparent node's materials blended at their alpha, or opaque",
        ),
    ],
    make: |g| {
        Ok(Box::new(LowerStuntboostMaterials {
            baked: g.choice("baked"),
            transparent: g.choice("transparent"),
        }))
    },
};

fn texture(t: &Option<StuntboostTextureRef>) -> Option<TextureRef> {
    let t = t.as_ref()?;
    Some(TextureRef {
        texcoord: t.texcoord,
        wrap: t.wrap,
        filters: t.filters,
        ..TextureRef::new(t.texture)
    })
}

impl Pass for LowerStuntboostMaterials {
    fn name(&self) -> &'static str {
        "lower-stuntboost-materials"
    }
    fn options(&self) -> Options {
        Options::from([
            ("baked".to_owned(), self.baked.text().to_owned()),
            ("transparent".to_owned(), self.transparent.text().to_owned()),
        ])
    }
    /// The materials' `pbr.material`; with `transparent=blend`, the blended copies (each with `core.name`) and the
    /// prototypes' `core.material_variants`.
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, MaterialVariants::NAME, Name::NAME]
    }
    /// A transparent node's placement takes its variant; a prototype's variants gain the blended ones.
    fn modifies(&self) -> &[&'static str] {
        &[Instance::NAME, MaterialVariants::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let baked_level = w
            .iter::<StuntboostNode>()
            .any(|(_, n)| n.name.class_name() == "SetBakedLighting");
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
                shading: m.shading,
                ..PbrMaterial::default()
            };
            if m.name.starts_with("bake_mat_") {
                log.count("bake materials", 1);
                match self.baked {
                    Baked::BaseColour => {}
                    Baked::Emission => {
                        pbr.emissive = [1.0, 1.0, 1.0];
                        pbr.emissive_texture = pbr.base_color_texture.take();
                        pbr.emissive_strength = 1.0;
                        pbr.base_color = [0.0, 0.0, 0.0, m.base_color[3]];
                    }
                    Baked::Unlit => {
                        pbr.shading = Shading::Unlit;
                        if !baked_level {
                            log.count(
                                "bake materials drawn unlit in a level the game lights (no SetBakedLighting)",
                                1,
                            );
                        }
                    }
                }
            }
            if w.has::<StuntboostDirectLight>(e) {
                log.count("direct-light masks kept in the dialect, not lowered", 1);
            }
            w.insert(e, pbr);
        }
        if self.transparent == Transparent::Blend {
            blend_transparent_nodes(w, log);
        }
        Ok(())
    }
}

/// Each transparent node draws with blended copies of its materials: per (material, alpha) one copy (the material's
/// `pbr.material`, blended, its base colour alpha multiplied by the node's), and per (prototype, alpha) one variant of
/// the prototype naming the copies, which the node's placement takes.
fn blend_transparent_nodes(w: &mut World, log: &mut PassLog) {
    let nodes: Vec<(Entity, Entity, f32)> = w
        .iter::<StuntboostNode>()
        .filter_map(|(e, n)| {
            let StuntboostDraw::Transparent { alpha } = n.name.draw() else {
                return None;
            };
            Some((e, w.get::<Instance>(e)?.prototype, alpha.unwrap_or(1.0)))
        })
        .collect();
    // copies by (material, alpha bits); variants by (prototype, alpha bits)
    let mut copies: BTreeMap<(Entity, u32), Entity> = BTreeMap::new();
    let mut variants: BTreeMap<(Entity, u32), u32> = BTreeMap::new();
    for (e, prototype, alpha) in nodes {
        let key = (prototype, alpha.to_bits());
        let variant = match variants.get(&key) {
            Some(&v) => v,
            None => {
                let Some(materials) = w
                    .get::<Mesh>(prototype)
                    .map(|m| m.primitives().iter().map(|p| p.material).collect::<Vec<_>>())
                else {
                    continue;
                };
                let mut table = w.get::<MaterialVariants>(prototype).cloned().unwrap_or_default();
                table.0.resize(materials.len(), BTreeMap::new());
                // the next variant number this prototype has not used
                let v = table.0.iter().flat_map(|m| m.keys()).max().map_or(1, |v| v + 1);
                for (i, m) in materials.iter().enumerate() {
                    let Some(m) = *m else { continue };
                    let copy = match copies.get(&(m, alpha.to_bits())) {
                        Some(&c) => c,
                        None => {
                            let Some(mut pbr) = w.get::<PbrMaterial>(m).cloned() else {
                                continue;
                            };
                            pbr.alpha_mode = AlphaMode::Blend;
                            pbr.base_color[3] *= alpha;
                            let name = w.get::<Name>(m).map_or("", |n| n.0.as_str()).to_owned();
                            let c = w.spawn();
                            w.insert(c, pbr);
                            w.insert(c, Name(format!("{name} (transparent {alpha})")));
                            copies.insert((m, alpha.to_bits()), c);
                            log.count("blended material copies made for transparent nodes", 1);
                            c
                        }
                    };
                    if let Some(slot) = table.0.get_mut(i) {
                        slot.insert(v, copy);
                    }
                }
                w.insert(prototype, table);
                variants.insert(key, v);
                v
            }
        };
        if let Some(i) = w.get_mut::<Instance>(e) {
            i.variant = variant;
        }
        log.count("transparent nodes drawn with blended copies", 1);
    }
}
