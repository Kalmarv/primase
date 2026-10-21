//! `pack-texture-atlas{size=S}` (required): every mesh drawn with several `pbr.material`s drawn with one, whose textures
//! are atlases of its materials' textures, for a destination that draws a mesh with one set of textures.
//!
//! **Which meshes.** A mesh whose primitives draw with two or more distinct materials. Its materials (the distinct
//! set) get one new material, shared by every mesh drawn with exactly that set; each of those primitives is drawn with
//! it. A primitive with no material is left as it is. A mesh drawing one material is not touched.
//!
//! **Layout.** Each material gets a rectangle, the size of its largest texture (1 x 1 for a material with none),
//! with a gutter of [`GUTTER`] texels around it that repeats the rectangle's edge texels, so filtering at the edge
//! does not read a neighbour. Rectangles are shelf-packed in rows of at most `S`: sorted by height (then width, then
//! material) descending, each placed left to right on the current shelf, a new shelf under it when the row is full.
//! When they do not fit `S` x `S`, the material with the largest rectangle (the earlier on a tie) is halved in both
//! dimensions (never below 1) and the layout redone, until they fit; a layout that cannot fit with every rectangle at
//! 1 x 1 is refused. The atlas is the layout's extent rounded up to a power of two, at most `S`. All slots share this
//! layout: every atlas is the same size, each material's texture in a slot is resampled to its rectangle (box filter
//! over whole source texels when shrinking, texel replication when enlarging; a normal map is filtered as normals
//! are, by `resize_normals`).
//!
//! **Coordinates.** Each primitive's `TexCoord(0)` (the only set atlased; a slot sampling another is refused) maps
//! into its material's rectangle: `(x + u * w, y + v * h)` over the atlas size. A material with no texture has its
//! vertices all at the middle of its 1 x 1 rectangle (the stream is made when the primitive has none). Coordinates
//! outside 0-1 have no atlas form (tiling): refused naming the material, unless every texture of the material is
//! clamped on that axis, when they are clamped to 0-1 (an approximation, counted). A slot transform is refused
//! (`bake-texture-transforms` first).
//!
//! **Materials.** One atlas a slot (base colour, metallic-roughness, normal, occlusion, emissive), made when some
//! material has a texture in it or the materials' factors for it differ. A material lacking the texture in a slot
//! another has gets that slot's neutral value: white (255, 255, 255, 255), and for a normal map the flat normal
//! (128, 128, 255, 255). A slot's factors (base colour; metallic and roughness; occlusion strength; normal scale;
//! emissive) that differ between the materials are multiplied into its atlas pixels in the stored space of the slot
//! (sRGB colours through linear light), 8 bits a channel, and the new material's factor is 1 (an approximation,
//! counted); factors the same everywhere stay factors. What a single material cannot hold is refused naming it: alpha
//! modes (and cutoffs), double sidedness, shading, emissive strength and filters that differ, a slot's textures that
//! differ in colour space, role or normal convention, and any specular extension (anything but the same one with no
//! textures). Textures must be loaded as pixels in a format RGBA8 holds exactly (`load-textures`,
//! `lower-textures-rgba8` otherwise). Slot atlases with identical pixels and metadata share one texture. The atlases
//! have their base level only, sampled with clamped wrap and each slot's own filters.
//!
//! The new material carries only `pbr.material` (no dialect data of the materials it replaces, which stay as they
//! are, unused by these meshes). A changed mesh is no longer its file's: the file link goes.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, Filters, PbrMaterial, TextureRef, TextureSlot, Wrap};
use primase_core::{
    ColorSpace, Component, Entity, MaterialVariants, Mesh, MeshFile, Name, NormalConvention, Semantic,
    Stream, TexFormat, TexRole, Texture, TextureFile, World,
};

use crate::{Image, Samples, linear_to_srgb8, resize_normals, rgba8_exact, scale_normal, srgb_to_linear};

/// Texels repeating each rectangle's edge on every side.
pub const GUTTER: u32 = 2;

/// The slots atlased, in glTF's order.
const SLOTS: [TextureSlot; 5] = [
    TextureSlot::BaseColor,
    TextureSlot::MetallicRoughness,
    TextureSlot::Normal,
    TextureSlot::Occlusion,
    TextureSlot::Emissive,
];

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackTextureAtlas {
    size: u32,
}

impl PackTextureAtlas {
    /// Atlases of at most `size` x `size` texels (at least `4 * GUTTER + 1`, at most 16384).
    pub fn new(size: u32) -> Result<PackTextureAtlas, String> {
        let least = 4 * GUTTER + 1;
        if !(least..=16384).contains(&size) {
            return Err(format!("size={size}: an atlas is {least} to 16384 texels a side"));
        }
        Ok(PackTextureAtlas { size })
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// From its options: `size`, required.
    pub fn from_options(o: &Options) -> Result<PackTextureAtlas, String> {
        let mut size = None;
        for (k, v) in o {
            match k.as_str() {
                "size" => {
                    size = Some(
                        v.parse::<u32>()
                            .map_err(|_| format!("size={v} is not a number of texels"))?,
                    )
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        PackTextureAtlas::new(size.ok_or("size is required (texels a side of the largest atlas)")?)
    }
}

/// A rectangle of texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Rectangles of `sizes` shelf-packed in `limit` x `limit`, and the atlas' size; `None` when they do not fit.
fn pack(sizes: &[(u32, u32)], limit: u32) -> Option<(Vec<Rect>, (u32, u32))> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(sizes[i].1), std::cmp::Reverse(sizes[i].0), i));
    let mut rects = vec![
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0
        };
        sizes.len()
    ];
    let (mut x, mut y, mut shelf, mut used_w) = (0u32, 0u32, 0u32, 0u32);
    for i in order {
        let (w, h) = (sizes[i].0 + 2 * GUTTER, sizes[i].1 + 2 * GUTTER);
        if w > limit {
            return None;
        }
        if x + w > limit {
            y += shelf;
            (x, shelf) = (0, 0);
        }
        if y + h > limit {
            return None;
        }
        rects[i] = Rect {
            x: x + GUTTER,
            y: y + GUTTER,
            w: sizes[i].0,
            h: sizes[i].1,
        };
        x += w;
        shelf = shelf.max(h);
        used_w = used_w.max(x);
    }
    let round = |n: u32| n.next_power_of_two().min(limit);
    Some((rects, (round(used_w), round(y + shelf))))
}

/// A layout: the rectangles, the atlas' size, and the sizes the rectangles have (the natural ones, halved to fit).
type Layout = (Vec<Rect>, (u32, u32), Vec<(u32, u32)>);

/// The rectangles for materials of `natural` sizes: halved, largest first, until they fit. Returns the sizes used.
fn layout(natural: &[(u32, u32)], limit: u32) -> Result<Layout, String> {
    let mut sizes = natural.to_vec();
    loop {
        if let Some((rects, atlas)) = pack(&sizes, limit) {
            return Ok((rects, atlas, sizes));
        }
        let largest = (0..sizes.len())
            .filter(|&i| sizes[i].0 > 1 || sizes[i].1 > 1)
            .max_by_key(|&i| {
                (
                    u64::from(sizes[i].0) * u64::from(sizes[i].1),
                    std::cmp::Reverse(i),
                )
            });
        let Some(i) = largest else {
            return Err(format!(
                "{} materials do not fit {limit} x {limit} texels even at 1 x 1 each, with a gutter of {GUTTER}",
                sizes.len()
            ));
        };
        sizes[i] = ((sizes[i].0 / 2).max(1), (sizes[i].1 / 2).max(1));
    }
}

/// `image` resampled to `w` x `h`: every output texel the mean (rounded half up) of the source texels in its box,
/// the box edges at `x * source / w` in integers, so a larger output repeats texels; normals by `resize_normals`.
fn resample(image: &Image, w: u32, h: u32, normal: bool) -> Result<Image, String> {
    if (image.width, image.height) == (w, h) {
        return Ok(image.clone());
    }
    if normal {
        return resize_normals(image, w, h)
            .map(|n| n.image)
            .map_err(|e| e.to_string());
    }
    let Samples::U8(src) = &image.samples else {
        return Err("only 8-bit pixels are resampled".to_owned());
    };
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        let ya = u64::from(y) * u64::from(image.height) / u64::from(h);
        let yb = ((u64::from(y) + 1) * u64::from(image.height) / u64::from(h)).max(ya + 1);
        for x in 0..w {
            let xa = u64::from(x) * u64::from(image.width) / u64::from(w);
            let xb = ((u64::from(x) + 1) * u64::from(image.width) / u64::from(w)).max(xa + 1);
            let mut sum = [0u64; 4];
            for sy in ya..yb {
                for sx in xa..xb {
                    let at = (sy as usize * image.width as usize + sx as usize) * 4;
                    for c in 0..4 {
                        sum[c] += u64::from(src[at + c]);
                    }
                }
            }
            let n = (xb - xa) * (yb - ya);
            out.extend(sum.map(|s| ((s + n / 2) / n) as u8));
        }
    }
    Ok(Image {
        width: w,
        height: h,
        samples: Samples::U8(out),
    })
}

/// What a slot's atlas is, apart from its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Meta {
    color_space: ColorSpace,
    role: TexRole,
    normal: Option<NormalConvention>,
}

fn default_meta(slot: TextureSlot) -> Meta {
    match slot {
        TextureSlot::BaseColor | TextureSlot::Emissive => Meta {
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
        TextureSlot::Normal => Meta {
            color_space: ColorSpace::Linear,
            role: TexRole::Normal,
            normal: Some(NormalConvention::GLTF),
        },
        _ => Meta {
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        },
    }
}

/// The value of a texel with no texture, in the stored space of the slot.
fn neutral(slot: TextureSlot) -> [u8; 4] {
    match slot {
        TextureSlot::Normal => [128, 128, 255, 255],
        _ => [255; 4],
    }
}

fn slot_ref(m: &PbrMaterial, slot: TextureSlot) -> Option<TextureRef> {
    match slot {
        TextureSlot::BaseColor => m.base_color_texture,
        TextureSlot::MetallicRoughness => m.metallic_roughness_texture,
        TextureSlot::Normal => m.normal_texture,
        TextureSlot::Occlusion => m.occlusion_texture,
        TextureSlot::Emissive => m.emissive_texture,
        TextureSlot::Specular | TextureSlot::SpecularColor => None,
    }
}

/// Whether `a` and `b` state the same factor for `slot`.
fn same_factor(a: &PbrMaterial, b: &PbrMaterial, slot: TextureSlot) -> bool {
    match slot {
        TextureSlot::BaseColor => a.base_color == b.base_color,
        TextureSlot::MetallicRoughness => (a.metallic, a.roughness) == (b.metallic, b.roughness),
        TextureSlot::Normal => a.normal_scale == b.normal_scale,
        TextureSlot::Occlusion => a.occlusion_strength == b.occlusion_strength,
        TextureSlot::Emissive => a.emissive == b.emissive,
        TextureSlot::Specular | TextureSlot::SpecularColor => true,
    }
}

/// `px` (stored in `meta`'s space) with `m`'s factor for `slot` multiplied in.
fn bake(px: [u8; 4], m: &PbrMaterial, slot: TextureSlot, meta: Meta) -> [u8; 4] {
    let srgb = meta.color_space == ColorSpace::Srgb;
    let byte = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let colour = |c: u8, f: f32| {
        if srgb {
            linear_to_srgb8(srgb_to_linear(f64::from(c) / 255.0) * f64::from(f))
        } else {
            byte(f64::from(c) / 255.0 * f64::from(f))
        }
    };
    match slot {
        TextureSlot::BaseColor => [
            colour(px[0], m.base_color[0]),
            colour(px[1], m.base_color[1]),
            colour(px[2], m.base_color[2]),
            byte(f64::from(px[3]) / 255.0 * f64::from(m.base_color[3])),
        ],
        TextureSlot::Emissive => [
            colour(px[0], m.emissive[0]),
            colour(px[1], m.emissive[1]),
            colour(px[2], m.emissive[2]),
            px[3],
        ],
        TextureSlot::MetallicRoughness => [
            px[0],
            byte(f64::from(px[1]) / 255.0 * f64::from(m.roughness)),
            byte(f64::from(px[2]) / 255.0 * f64::from(m.metallic)),
            px[3],
        ],
        TextureSlot::Occlusion => {
            let o = 1.0 + f64::from(m.occlusion_strength) * (f64::from(px[0]) / 255.0 - 1.0);
            [byte(o), px[1], px[2], px[3]]
        }
        TextureSlot::Normal => {
            let mut px = px;
            scale_normal(&mut px, m.normal_scale);
            px
        }
        TextureSlot::Specular | TextureSlot::SpecularColor => px,
    }
}

/// The new coordinates of primitive `.0`.
type Coordinates = (usize, Vec<[f32; 2]>);

/// The new material of one set of materials and the atlas it came from.
struct Atlas {
    /// The new material, its texture slots not yet pointing at the textures.
    material: PbrMaterial,
    textures: Vec<(TextureSlot, Texture)>,
    filters: BTreeMap<TextureSlot, Filters>,
    rects: Vec<Rect>,
    size: (u32, u32),
    /// Which materials have no texture at all: their rectangle is one texel.
    flat: Vec<bool>,
    /// Per material, on each axis, whether every texture of it clamps there.
    clamped: Vec<[bool; 2]>,
}

/// A name for a material in a message.
fn label(w: &World, e: Entity) -> String {
    w.get::<Name>(e)
        .map_or_else(|| format!("{e:?}"), |n| format!("{e:?} ({})", n.0))
}

impl PackTextureAtlas {
    /// The atlas of `set` (distinct materials, ascending), built from the world's pixels.
    fn build(&self, w: &World, set: &[Entity], log: &mut PassLog) -> Result<Atlas, String> {
        let mats: Vec<PbrMaterial> = set
            .iter()
            .map(|&m| {
                w.get::<PbrMaterial>(m)
                    .cloned()
                    .ok_or_else(|| format!("material {} has no pbr.material", label(w, m)))
            })
            .collect::<Result<_, _>>()?;
        let first = &mats[0];
        // what one material cannot hold
        for (m, &e) in mats.iter().zip(set).skip(1) {
            let differs = |what: &str| {
                format!(
                    "material {} and {} differ in {what}",
                    label(w, set[0]),
                    label(w, e)
                )
            };
            if m.alpha_mode != first.alpha_mode {
                return Err(differs(&format!(
                    "alpha mode ({:?} against {:?})",
                    first.alpha_mode, m.alpha_mode
                )));
            }
            if m.double_sided != first.double_sided {
                return Err(differs("double sidedness"));
            }
            if m.shading != first.shading {
                return Err(differs("shading"));
            }
            if m.emissive_strength != first.emissive_strength {
                return Err(differs(
                    "emissive strength (it cannot be multiplied into 8-bit texels)",
                ));
            }
            if m.specular != first.specular {
                return Err(differs("the specular extension"));
            }
        }
        for (m, &e) in mats.iter().zip(set) {
            let textured = m
                .specular
                .is_some_and(|s| s.texture.is_some() || s.color_texture.is_some());
            if textured {
                return Err(format!(
                    "material {} has specular textures, which are not atlased",
                    label(w, e)
                ));
            }
            if let AlphaMode::Mask { cutoff } = m.alpha_mode
                && !cutoff.is_finite()
            {
                return Err(format!(
                    "material {} has a cutoff that is not finite",
                    label(w, e)
                ));
            }
        }
        // the textures: loaded pixels, coordinate set 0, no transform, one meta and filters a slot
        let mut images: BTreeMap<Entity, Image> = BTreeMap::new();
        let mut meta: BTreeMap<TextureSlot, Meta> = BTreeMap::new();
        let mut filters: BTreeMap<TextureSlot, Filters> = BTreeMap::new();
        let mut natural: Vec<(u32, u32)> = vec![(1, 1); set.len()];
        let mut flat = vec![true; set.len()];
        let mut clamped = vec![[true; 2]; set.len()];
        for (k, (m, &e)) in mats.iter().zip(set).enumerate() {
            for slot in SLOTS {
                let Some(r) = slot_ref(m, slot) else { continue };
                let at = format!("material {}'s {} texture", label(w, e), slot.name());
                if r.texcoord != 0 {
                    return Err(format!(
                        "{at} samples coordinate set {}: only set 0 is atlased",
                        r.texcoord
                    ));
                }
                if r.transform
                    .is_some_and(|t| t != primase_core::pbr::UvTransform::IDENTITY)
                {
                    return Err(format!("{at} has a transform: run bake-texture-transforms first"));
                }
                let Some(t) = w.get::<Texture>(r.texture) else {
                    return Err(if w.has::<TextureFile>(r.texture) {
                        format!("{at} is not loaded: run load-textures first")
                    } else {
                        format!("{at} ({:?}) is not a texture", r.texture)
                    });
                };
                if !rgba8_exact(t.format) {
                    return Err(format!(
                        "{at} is {:?}, which 8-bit RGBA does not hold exactly: run lower-textures-rgba8 first",
                        t.format
                    ));
                }
                let this = Meta {
                    color_space: t.color_space,
                    role: t.role,
                    normal: t.normal,
                };
                if *meta.entry(slot).or_insert(this) != this {
                    return Err(format!(
                        "{at} differs in colour space, role or normal convention from another material's in the \
                         slot"
                    ));
                }
                if *filters.entry(slot).or_insert(r.filters) != r.filters {
                    return Err(format!(
                        "{at} is sampled with other filters than another material's in the slot"
                    ));
                }
                if let std::collections::btree_map::Entry::Vacant(v) = images.entry(r.texture) {
                    v.insert(crate::texture_level_rgba8(t, 0).map_err(|e| format!("{at}: {e}"))?);
                }
                flat[k] = false;
                natural[k] = (natural[k].0.max(t.width), natural[k].1.max(t.height));
                for (c, wrap) in clamped[k].iter_mut().zip(r.wrap) {
                    *c &= wrap == Wrap::Clamp;
                }
            }
        }
        // a texture-less material takes a texel; one with textures has natural[k] from them
        for (n, &f) in natural.iter_mut().zip(&flat) {
            if f {
                *n = (1, 1);
            }
        }
        let (rects, size, used) = layout(&natural, self.size)?;
        for (k, (&n, &u)) in natural.iter().zip(&used).enumerate() {
            if n != u {
                log.approximate("materials whose textures are scaled down to fit their atlas", 1);
                log.count(
                    &format!(
                        "material {} scaled from {} x {} to {} x {} texels in its atlas",
                        label(w, set[k]),
                        n.0,
                        n.1,
                        u.0,
                        u.1
                    ),
                    1,
                );
            }
        }
        // which slots are atlased, and which factors are baked
        let mut material = first.clone();
        material.base_color_texture = None;
        material.metallic_roughness_texture = None;
        material.normal_texture = None;
        material.occlusion_texture = None;
        material.emissive_texture = None;
        let mut textures: Vec<(TextureSlot, Texture)> = Vec::new();
        for slot in SLOTS {
            let present = mats.iter().any(|m| slot_ref(m, slot).is_some());
            let baked = mats.iter().any(|m| !same_factor(&mats[0], m, slot));
            if !present && !baked {
                continue;
            }
            let meta = meta.get(&slot).copied().unwrap_or_else(|| default_meta(slot));
            let (aw, ah) = (size.0 as usize, size.1 as usize);
            let mut canvas: Vec<u8> = neutral(slot).repeat(aw * ah);
            for (k, m) in mats.iter().enumerate() {
                let r = rects[k];
                let region = match slot_ref(m, slot) {
                    Some(tr) => {
                        let img = &images[&tr.texture];
                        if (img.width, img.height) != (r.w, r.h) && (img.width < r.w || img.height < r.h) {
                            log.approximate("textures enlarged to their material's rectangle", 1);
                        }
                        resample(img, r.w, r.h, slot == TextureSlot::Normal)?
                    }
                    None => {
                        if present {
                            log.count("rectangles filled with the slot's neutral value", 1);
                        }
                        Image {
                            width: r.w,
                            height: r.h,
                            samples: Samples::U8(neutral(slot).repeat(r.w as usize * r.h as usize)),
                        }
                    }
                };
                let Samples::U8(src) = &region.samples else {
                    return Err("only 8-bit pixels are packed".to_owned());
                };
                let (rw, rh) = (r.w as i64, r.h as i64);
                let g = i64::from(GUTTER);
                for dy in -g..rh + g {
                    for dx in -g..rw + g {
                        let (sx, sy) = (dx.clamp(0, rw - 1) as usize, dy.clamp(0, rh - 1) as usize);
                        let at = (sy * r.w as usize + sx) * 4;
                        let mut px: [u8; 4] = src[at..at + 4].try_into().expect("four channels");
                        if baked {
                            px = bake(px, m, slot, meta);
                        }
                        let to = ((i64::from(r.y) + dy) as usize * aw + (i64::from(r.x) + dx) as usize) * 4;
                        canvas[to..to + 4].copy_from_slice(&px);
                    }
                }
            }
            if baked {
                log.approximate(
                    "slots whose differing factors are multiplied into the atlas' 8-bit pixels",
                    1,
                );
            }
            let texture = Texture {
                width: size.0,
                height: size.1,
                format: TexFormat::Rgba8Unorm,
                levels: vec![canvas],
                color_space: meta.color_space,
                role: meta.role,
                normal: meta.normal,
            };
            // the new material: the common factor, or 1 where it is in the pixels
            match slot {
                TextureSlot::BaseColor if baked => material.base_color = [1.0; 4],
                TextureSlot::MetallicRoughness if baked => {
                    (material.metallic, material.roughness) = (1.0, 1.0)
                }
                TextureSlot::Normal if baked => material.normal_scale = 1.0,
                TextureSlot::Occlusion if baked => material.occlusion_strength = 1.0,
                TextureSlot::Emissive if baked => material.emissive = [1.0; 3],
                _ => {}
            }
            textures.push((slot, texture));
        }
        Ok(Atlas {
            material,
            textures,
            filters,
            rects,
            size,
            flat,
            clamped,
        })
    }

    /// The new `TexCoord(0)` of every primitive of mesh `e` drawn with one of `set`, checked against the atlas.
    fn coordinates(
        &self,
        w: &World,
        e: Entity,
        set: &[Entity],
        atlas: &Atlas,
        log: &mut PassLog,
    ) -> Result<Vec<Coordinates>, String> {
        let (aw, ah) = (f64::from(atlas.size.0), f64::from(atlas.size.1));
        let mut edits = Vec::new();
        for (i, p) in w.get::<Mesh>(e).expect("listed").primitives().iter().enumerate() {
            let Some(k) = p.material.and_then(|m| set.iter().position(|&s| s == m)) else {
                continue;
            };
            let r = atlas.rects[k];
            let n = p.vertex_count();
            if atlas.flat[k] {
                let middle = [
                    ((f64::from(r.x) + f64::from(r.w) / 2.0) / aw) as f32,
                    ((f64::from(r.y) + f64::from(r.h) / 2.0) / ah) as f32,
                ];
                edits.push((i, vec![middle; n]));
                continue;
            }
            let at = format!("{e:?} primitive {i} (material {})", label(w, set[k]));
            let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(0)) else {
                return Err(format!("{at} has no float2 TexCoord(0) to atlas"));
            };
            let mut mapped = Vec::with_capacity(uv.len());
            for (v, c) in uv.iter().enumerate() {
                let mut c = *c;
                for axis in 0..2 {
                    if !c[axis].is_finite() {
                        return Err(format!("{at}: vertex {v}'s coordinate is not finite"));
                    }
                    if !(0.0..=1.0).contains(&c[axis]) {
                        if !atlas.clamped[k][axis] {
                            return Err(format!(
                                "{at}: vertex {v}'s coordinate {c:?} is outside 0-1, which tiles the texture and \
                                 has no atlas form (only a texture clamped on the axis may be atlased)"
                            ));
                        }
                        c[axis] = c[axis].clamp(0.0, 1.0);
                        log.approximate(
                            "vertex coordinates outside 0-1, clamped as their texture clamps",
                            1,
                        );
                    }
                }
                mapped.push([
                    ((f64::from(r.x) + f64::from(c[0]) * f64::from(r.w)) / aw) as f32,
                    ((f64::from(r.y) + f64::from(c[1]) * f64::from(r.h)) / ah) as f32,
                ]);
            }
            edits.push((i, mapped));
        }
        Ok(edits)
    }
}

impl Pass for PackTextureAtlas {
    fn name(&self) -> &'static str {
        "pack-texture-atlas"
    }
    fn options(&self) -> Options {
        Options::from([("size".to_owned(), self.size.to_string())])
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    /// The atlas textures and materials, their names.
    fn provides(&self) -> &[&'static str] {
        &[Texture::NAME, PbrMaterial::NAME, Name::NAME]
    }
    /// A changed mesh's file link.
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    fn approximates(&self) -> &'static str {
        "textures scaled to fit their atlas, differing factors multiplied into 8-bit pixels, clamp-wrapped \
         coordinates outside 0-1 clamped"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the meshes drawn with several materials, by the set of materials
        let mut groups: BTreeMap<Vec<Entity>, Vec<Entity>> = BTreeMap::new();
        for (e, mesh) in w.iter::<Mesh>() {
            let set: BTreeSet<Entity> = mesh.primitives().iter().filter_map(|p| p.material).collect();
            if set.len() > 1 {
                groups.entry(set.into_iter().collect()).or_default().push(e);
            }
        }
        for meshes in groups.values() {
            if let Some(e) = meshes.iter().find(|&&e| {
                w.get::<MaterialVariants>(e)
                    .is_some_and(|v| v.0.iter().any(|m| !m.is_empty()))
            }) {
                return Err(format!(
                    "{e:?} has material variants, which one atlas material cannot draw: run \
                     specialize-material-variants first"
                ));
            }
        }
        for (set, meshes) in groups {
            let atlas = self.build(w, &set, log)?;
            let mut edits = Vec::new();
            for &e in &meshes {
                edits.push((e, self.coordinates(w, e, &set, &atlas, log)?));
            }
            // everything checked: the textures, the material, then the meshes
            let mut material = atlas.material.clone();
            let mut made: Vec<(Entity, &Texture)> = Vec::new();
            for (slot, texture) in &atlas.textures {
                let same = |t: &Texture| {
                    (&t.levels, t.color_space, t.role, t.normal)
                        == (&texture.levels, texture.color_space, texture.role, texture.normal)
                };
                let shared = made.iter().find(|(_, t)| same(t)).map(|(t, _)| *t);
                let entity = shared.unwrap_or_else(|| {
                    let t = w.spawn();
                    w.insert(t, texture.clone());
                    w.insert(
                        t,
                        Name(format!("{} atlas of {} materials", slot.name(), set.len())),
                    );
                    made.push((t, texture));
                    log.count("atlas textures made", 1);
                    t
                });
                let r = TextureRef {
                    texture: entity,
                    texcoord: 0,
                    wrap: [Wrap::Clamp; 2],
                    transform: None,
                    filters: atlas.filters.get(slot).copied().unwrap_or_default(),
                };
                match slot {
                    TextureSlot::BaseColor => material.base_color_texture = Some(r),
                    TextureSlot::MetallicRoughness => material.metallic_roughness_texture = Some(r),
                    TextureSlot::Normal => material.normal_texture = Some(r),
                    TextureSlot::Occlusion => material.occlusion_texture = Some(r),
                    TextureSlot::Emissive => material.emissive_texture = Some(r),
                    TextureSlot::Specular | TextureSlot::SpecularColor => {}
                }
            }
            let drawn_with = w.spawn();
            w.insert(drawn_with, material);
            w.insert(drawn_with, Name(format!("atlas of {} materials", set.len())));
            for (e, coordinates) in edits {
                let mesh = w.get_mut::<Mesh>(e).expect("listed");
                for (i, uv) in coordinates {
                    let mut p = mesh.primitive_mut(i).expect("a listed primitive");
                    p.streams_mut().insert(Semantic::TexCoord(0), Stream::F32x2(uv));
                    *p.material_mut() = Some(drawn_with);
                }
                w.remove::<MeshFile>(e);
                log.count("meshes drawn with an atlas material", 1);
            }
            log.count("atlas materials made", 1);
            log.count("materials packed", set.len() as u64);
            log.count(
                &format!(
                    "atlas of {} x {} texels packing {} materials",
                    atlas.size.0,
                    atlas.size.1,
                    set.len()
                ),
                1,
            );
        }
        Ok(())
    }
}
