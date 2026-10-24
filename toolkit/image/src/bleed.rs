//! `bleed-alpha-colors`: the colour of every texel of a masked material's base-colour texture whose alpha is under the
//! material's cutoff is replaced by the colour of the nearest texel that is not, so that the filtering and mips of a
//! texture drawn without its alpha test (after `clip-alpha-cutouts` cut the geometry) do not pull the hidden colour
//! in at the clipped edge. A source's hidden colour is often a white or black matte; the old route measured it showing
//! as pale outlines along the edges of cards.
//!
//! **Which textures.** The base-colour textures of materials with alpha mode `Mask`. A texel is hidden when the
//! material's base-colour alpha times the texel's alpha is under the cutoff, the test `clip-alpha-cutouts` makes. Run
//! this pass first: it reads the cutoff from the masked material, which `clip-alpha-cutouts` then makes opaque.
//! A texture every use of which is the base colour of masked materials with one cutoff and one base-colour alpha is
//! changed in place; any other (also used by another slot, by a material that is not masked, or at another cutoff)
//! gets a copy for each cutoff and alpha it is masked with, named `<name>~bleed<cutoff>` (with `x<alpha>` when the
//! alpha is not 1), the masked materials repointed to it, and the original is left for its other uses. A texture in a
//! format that is not exact in 8-bit RGBA is refused (`lower-textures-rgba8` first).
//!
//! **Method.** The nearest not-hidden texel by Euclidean distance between texel centres, from an exact distance
//! transform ([`nearest_visible`]; ties go to the one found first, deterministically), on the first level; the
//! texture is not read across its edges. Alpha and every not-hidden texel are untouched. A texture with more levels
//! has them rebuilt from the changed first level by the integer box filter ([`crate::mip_chain_rgba8`], on the stored
//! values), because the stored ones were made from the hidden colour. A texture with nothing hidden is left as it
//! is; one hidden everywhere has no colour to take and is left, counted.
//!
//! Reported: textures changed (and copied), the hidden texels and how many of them changed colour, the textures
//! whose mips were rebuilt (an approximation, with levels decoded to RGBA8 when the format was another).

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{Component, Entity, Name, TexFormat, Texture, TextureFile, World};

use crate::cutout_field::effective;
use crate::{Samples, mip_chain_rgba8, rgba8_exact, texture_level_rgba8};

/// The pass; see the module documentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct BleedAlphaColors;

/// For every texel of a `width` by `height` grid, the index of the nearest texel `visible` marks, by the Euclidean
/// distance between texel centres (`u32::MAX` everywhere when none is marked). The distance transform is exact (two
/// passes: nearest in the column, then the lower envelope of the columns' parabolas along the row); a tie goes to the
/// nearer column in the envelope's order and, within a column, to the texel above.
pub fn nearest_visible(visible: &[bool], width: usize, height: usize) -> Vec<u32> {
    let mut nearest = vec![u32::MAX; width * height];
    if !visible.iter().any(|&v| v) {
        return nearest;
    }
    // per column and row: the row of the nearest marked texel in that column, and its squared distance
    let mut site_row = vec![u32::MAX; width * height];
    let mut dist2 = vec![f64::INFINITY; width * height];
    for x in 0..width {
        let mut above: Option<usize> = None;
        let mut up = vec![None; height];
        for y in 0..height {
            if visible[y * width + x] {
                above = Some(y);
            }
            up[y] = above;
        }
        let mut below: Option<usize> = None;
        for y in (0..height).rev() {
            if visible[y * width + x] {
                below = Some(y);
            }
            let best = match (up[y], below) {
                (Some(a), Some(b)) => Some(if y - a <= b - y { a } else { b }),
                (a, b) => a.or(b),
            };
            if let Some(s) = best {
                site_row[y * width + x] = s as u32;
                let d = s.abs_diff(y) as f64;
                dist2[y * width + x] = d * d;
            }
        }
    }
    // along each row: the lower envelope of the parabolas q -> (q - x)^2 + dist2(x)
    let mut v = vec![0usize; width];
    let mut z = vec![0.0f64; width + 1];
    for y in 0..height {
        let f = |x: usize| dist2[y * width + x];
        let mut k = usize::MAX;
        for q in 0..width {
            if f(q).is_infinite() {
                continue;
            }
            if k == usize::MAX {
                k = 0;
                v[0] = q;
                z[0] = f64::NEG_INFINITY;
                z[1] = f64::INFINITY;
                continue;
            }
            let mut s = envelope_cut(&f, q, v[k]);
            while s <= z[k] {
                k -= 1;
                s = envelope_cut(&f, q, v[k]);
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
        }
        if k == usize::MAX {
            continue;
        }
        let mut j = 0;
        for x in 0..width {
            while z[j + 1] < x as f64 {
                j += 1;
            }
            let column = v[j];
            nearest[y * width + x] = site_row[y * width + column] * width as u32 + column as u32;
        }
    }
    nearest
}

/// A texture to change: its masked users.
struct Group {
    texture: Entity,
    cutoff: f32,
    factor: f32,
    materials: Vec<Entity>,
}

fn name_of(w: &World, e: Entity) -> String {
    w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone())
}

impl Pass for BleedAlphaColors {
    fn name(&self) -> &'static str {
        "bleed-alpha-colors"
    }
    // no `requires`: a world without masked materials has nothing to change
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME, PbrMaterial::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[TextureFile::NAME]
    }
    /// The file links it removes are a side effect: the changed textures no longer match their files.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "mip levels rebuilt from the changed first level by a box filter on the stored values; levels decoded to RGBA8"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the masked users of each texture's base colour, by cutoff and alpha (compared by bits)
        let mut groups: BTreeMap<(Entity, u32, u32), Group> = BTreeMap::new();
        for (m, mat) in w.iter::<PbrMaterial>() {
            let AlphaMode::Mask { cutoff } = mat.alpha_mode else {
                continue;
            };
            let Some(r) = mat.base_color_texture else {
                continue;
            };
            if !cutoff.is_finite() || !mat.base_color[3].is_finite() {
                return Err(format!(
                    "material {}: the cutoff or base-colour alpha is not finite",
                    name_of(w, m)
                ));
            }
            groups
                .entry((r.texture, cutoff.to_bits(), mat.base_color[3].to_bits()))
                .or_insert_with(|| Group {
                    texture: r.texture,
                    cutoff,
                    factor: mat.base_color[3],
                    materials: Vec::new(),
                })
                .materials
                .push(m);
        }
        // every use of a texture, to tell whether the masked base-colour uses are all of them
        let mut uses: BTreeMap<Entity, usize> = BTreeMap::new();
        for (_, mat) in w.iter::<PbrMaterial>() {
            for r in mat.textures() {
                *uses.entry(r.texture).or_default() += 1;
            }
        }
        let mut masked_uses: BTreeMap<Entity, usize> = BTreeMap::new();
        let mut keys_of: BTreeMap<Entity, usize> = BTreeMap::new();
        for g in groups.values() {
            *masked_uses.entry(g.texture).or_default() += g.materials.len();
            *keys_of.entry(g.texture).or_default() += 1;
        }
        // plan: the changed texture of each group, and where it goes; nothing changes until all are made
        let mut made: Vec<(Entity, Option<String>, Texture, Vec<Entity>)> = Vec::new();
        for g in groups.values() {
            let what = format!("texture {}", name_of(w, g.texture));
            let tex = w
                .get::<Texture>(g.texture)
                .ok_or_else(|| format!("{what} has no pixels (load it first)"))?;
            if !rgba8_exact(tex.format) {
                return Err(format!(
                    "{what}: {:?} is not exact in 8-bit RGBA (lower-textures-rgba8 first)",
                    tex.format
                ));
            }
            let level = texture_level_rgba8(tex, 0).map_err(|e| format!("{what}: {e}"))?;
            let (width, height) = (level.width as usize, level.height as usize);
            let Samples::U8(mut px) = level.samples else {
                return Err(format!("{what}: decoded to something other than 8-bit"));
            };
            let visible: Vec<bool> = px
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| effective(g.factor, f32::from(p[3])) >= g.cutoff)
                .collect();
            let hidden = visible.iter().filter(|&&v| !v).count();
            if hidden == 0 {
                log.count("textures with no texel under the cutoff, left as they are", 1);
                continue;
            }
            if hidden == visible.len() {
                log.count("textures under the cutoff everywhere, left as they are", 1);
                continue;
            }
            let nearest = nearest_visible(&visible, width, height);
            let mut changed = 0u64;
            for i in 0..visible.len() {
                if visible[i] {
                    continue;
                }
                let s = nearest[i] as usize;
                if px[i * 4..i * 4 + 3] != px[s * 4..s * 4 + 3] {
                    changed += 1;
                }
                px.copy_within(s * 4..s * 4 + 3, i * 4);
            }
            let mut levels = vec![px.clone()];
            if tex.levels.len() > 1 {
                let chain =
                    mip_chain_rgba8(width as u32, height as u32, &px).map_err(|e| format!("{what}: {e}"))?;
                levels = chain
                    .into_iter()
                    .take(tex.levels.len())
                    .map(|(_, _, p)| p)
                    .collect();
                if levels.len() != tex.levels.len() {
                    return Err(format!("{what}: its mip chain is not the full one"));
                }
                log.approximate("textures with their mip levels rebuilt", 1);
            }
            if tex.format != TexFormat::Rgba8Unorm {
                log.approximate("texture levels decoded to RGBA8", levels.len() as u64);
            }
            log.count("hidden texels", hidden as u64);
            log.count("hidden texels given another colour", changed);
            let mut out = tex.clone();
            out.format = TexFormat::Rgba8Unorm;
            out.levels = levels;
            let in_place = keys_of[&g.texture] == 1
                && masked_uses[&g.texture] == uses.get(&g.texture).copied().unwrap_or(0);
            let copy_name = (!in_place).then(|| {
                let base = name_of(w, g.texture);
                if g.factor == 1.0 {
                    format!("{base}~bleed{}", g.cutoff)
                } else {
                    format!("{base}~bleed{}x{}", g.cutoff, g.factor)
                }
            });
            made.push((g.texture, copy_name, out, g.materials.clone()));
        }
        for (texture, copy_name, out, materials) in made {
            let target = match copy_name {
                None => {
                    log.count("textures changed in place", 1);
                    texture
                }
                Some(name) => {
                    let e = w.spawn();
                    w.insert(e, Name(name));
                    log.count("textures copied for their masked uses", 1);
                    for m in materials {
                        if let Some(r) = w
                            .get_mut::<PbrMaterial>(m)
                            .expect("listed")
                            .base_color_texture
                            .as_mut()
                        {
                            r.texture = e;
                        }
                    }
                    e
                }
            };
            w.insert(target, out);
            w.remove::<TextureFile>(target);
        }
        Ok(())
    }
}

/// `bleed-alpha-colors`' declaration: no options.
pub const BLEED_ALPHA_COLORS: PassSpec = PassSpec {
    name: "bleed-alpha-colors",
    options: &[],
    make: |_| Ok(Box::new(BleedAlphaColors)),
};

/// Where the parabola of column `q` rises above that of column `p` (`p < q`): the lower envelope's breakpoint.
fn envelope_cut(f: &impl Fn(usize) -> f64, q: usize, p: usize) -> f64 {
    ((f(q) + (q * q) as f64) - (f(p) + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64)
}
