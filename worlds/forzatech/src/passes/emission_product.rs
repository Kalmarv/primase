//! `bake-forzatech-emission-product`: the emission of an `EmissiveAltUV` material, its emission map times its base
//! colour map, once `rebase-texcoord-set` has drawn both through one coordinate set.
//!
//! The shaders of `environment` and `environment_signage` emit `base colour * EmissiveTexture * tint`, the emission map
//! read at `TEXCOORD4` when `EmissiveAltUV` is on and the base colour map at the material's own set. The lowering keeps
//! the emission map on `TEXCOORD4` and marks the material ([`crate::components::EmissionByBaseColour`]); the rebasing
//! pass draws every map of the material in the space of `TEXCOORD4`, or folds an emission map that is one colour into
//! the emissive factor; this pass then makes the product, for each marked material, and removes the mark:
//!
//! - both maps drawn through the same set with the same transform: one emission map, the product of the two texel by
//!   texel (sRGB through linear light), at the base colour map's size, the emission map sampled at the same normalised
//!   position;
//! - no emission map left (folded into the factor): the base colour map is the emission map, the factor is the colour;
//! - no base colour map: the emission map as it is;
//! - the maps still on different sets (the rebasing pass left the primitive as it was, counted there): the emission is
//!   left out, counted by material and by triangle, as a material is sampled through one coordinate set;
//! - a marked material no primitive draws (the rebased primitives draw copies of it, a variant of a prototype may
//!   still draw it): its emission cleared, counted apart, no triangle losing anything.
//!
//! Run after `rebase-texcoord-set` and before `bake-forzatech-swatch-tint`.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{Component, Entity, MaterialVariants, Mesh, Name, Texture, World};

use crate::components::EmissionByBaseColour;
use crate::passes::materials::{decode, painted};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BakeForzaEmissionProduct;

/// `bake-forzatech-emission-product`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const BAKE_FORZATECH_EMISSION_PRODUCT: PassSpec = PassSpec {
    name: "bake-forzatech-emission-product",
    options: &[],
    make: |_| Ok(Box::new(BakeForzaEmissionProduct)),
};

/// What a material that cannot draw its emission loses, by triangle.
const LEFT_OUT: &str = "emission texture on TEXCOORD4 (EmissiveAltUV) left out: the material's maps could not be drawn through one coordinate set";

/// The emission of material `m` removed, counted by material and by the triangles it shades.
fn leave_out(w: &mut World, log: &mut PassLog, m: Entity, why: &str, triangles: u64) {
    if let Some(pbr) = w.get_mut::<PbrMaterial>(m) {
        pbr.emissive_texture = None;
        pbr.emissive = [0.0; 3];
        pbr.emissive_strength = 1.0;
    }
    log.approximate_on(m, why, 1);
    log.approximate(&format!("{why}: triangles"), triangles);
}

impl Pass for BakeForzaEmissionProduct {
    fn name(&self) -> &'static str {
        "bake-forzatech-emission-product"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[EmissionByBaseColour::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let marked: Vec<Entity> = w.iter::<EmissionByBaseColour>().map(|(e, _)| e).collect();
        if marked.is_empty() {
            return Ok(());
        }
        // the triangles each material draws, as a primitive's own or as a placement's variant of it
        let mut triangles: BTreeMap<Entity, u64> = BTreeMap::new();
        for (mesh_entity, mesh) in w.iter::<Mesh>() {
            let variants = w.get::<MaterialVariants>(mesh_entity);
            for (i, p) in mesh.primitives().iter().enumerate() {
                let corners = p.indices.as_ref().map_or_else(|| p.vertex_count(), Vec::len);
                let own = p.material.into_iter();
                let swapped = variants
                    .and_then(|v| v.0.get(i))
                    .into_iter()
                    .flat_map(|choices| choices.values().copied());
                for m in own.chain(swapped) {
                    *triangles.entry(m).or_default() += corners as u64 / 3;
                }
            }
        }
        let mut products: BTreeMap<(Entity, Entity), Entity> = BTreeMap::new();
        for m in marked {
            w.remove::<EmissionByBaseColour>(m);
            let tris = triangles.get(&m).copied().unwrap_or(0);
            let Some(pbr) = w.get::<PbrMaterial>(m).cloned() else {
                continue;
            };
            if tris == 0 {
                if let Some(pbr) = w.get_mut::<PbrMaterial>(m) {
                    pbr.emissive_texture = None;
                    pbr.emissive = [0.0; 3];
                    pbr.emissive_strength = 1.0;
                }
                log.count(
                    "EmissiveAltUV materials drawn by no primitive: emission cleared",
                    1,
                );
                continue;
            }
            let Some(base) = pbr.base_color_texture else {
                log.count(
                    "EmissiveAltUV emission with no base colour map: the emission map alone",
                    1,
                );
                continue;
            };
            let Some(emission) = pbr.emissive_texture else {
                // folded into the factor by the rebasing pass: the colour is the factor, the pattern the base colour map's
                if let Some(pbr) = w.get_mut::<PbrMaterial>(m) {
                    pbr.emissive_texture = Some(base);
                }
                log.count(
                    "EmissiveAltUV emission folded into a factor: the base colour map is the emission map",
                    1,
                );
                continue;
            };
            if emission.texcoord != base.texcoord || emission.transform != base.transform {
                leave_out(w, log, m, LEFT_OUT, tris);
                continue;
            }
            let product = match products.get(&(base.texture, emission.texture)) {
                Some(&t) => t,
                None => {
                    let made = match (w.get::<Texture>(base.texture), w.get::<Texture>(emission.texture)) {
                        (Some(b), Some(e)) if !b.levels.is_empty() => match decode(e)? {
                            Some(image) => Some(painted(b, None, Some((e.color_space, &image)))?),
                            None => None,
                        },
                        _ => None,
                    };
                    let Some(texture) = made else {
                        leave_out(
                            w,
                            log,
                            m,
                            "emission texture on TEXCOORD4 (EmissiveAltUV) left out: a map has no pixels",
                            tris,
                        );
                        continue;
                    };
                    let name = w
                        .get::<Name>(base.texture)
                        .map(|n| n.0.clone())
                        .unwrap_or_default();
                    let t = w.spawn();
                    w.insert(t, Name(format!("{name} (emission by base colour)")));
                    w.insert(t, texture);
                    products.insert((base.texture, emission.texture), t);
                    log.count(
                        "emission maps made as the emission map times the base colour map",
                        1,
                    );
                    t
                }
            };
            if let Some(pbr) = w.get_mut::<PbrMaterial>(m) {
                pbr.emissive_texture = Some(TextureRef {
                    texture: product,
                    texcoord: base.texcoord,
                    wrap: base.wrap,
                    transform: base.transform,
                    filters: emission.filters,
                });
            }
            log.count("EmissiveAltUV emission maps multiplied by the base colour map", 1);
        }
        Ok(())
    }
}
