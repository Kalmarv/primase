//! `prepare-reskate-textures`: the textures a skate. material draws, made as the map writer encodes them.
//!
//! skate.'s materials draw their base texture as it is; a `skate.material` `tint` (sRGB bytes, from raising a base
//! colour factor or from `lower-tint-reskate`'s per-vertex colours) is multiplied into a copy of the base texture's
//! pixels, in linear light (a linear texture's bytes are scaled as they are), 8 bits per channel: one copy per (base
//! texture, tint), named as its base (the writer names files by name and pixels), its source format and size the
//! base's. The material then draws the copy and has no tint; the base texture stays, for any material drawing it
//! untinted (one no placed primitive's material draws any more keeps its pixels but is no longer a `skate.texture` of
//! the map). Run it once, after the passes that set tints and any `cap-textures`: a tinted copy is made at the size the
//! base has then, and a material without a tint is left alone, so a second run changes nothing.
//!
//! A tint on a material with no base texture tints nothing (skate. draws its base colour): left out, reported. A base
//! texture whose stored levels cannot be decoded keeps its tint, counted (the writer reports it unwritten).
//!
//! Normal maps (green flipped up, Z stored) and Studio masks are encoded by the writer: exact re-encodings, not
//! preparation. A size cap is `cap-textures`, never this pass.

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::{ColorSpace, Entity, Name, TexFormat, TexRole, Texture, World};

use crate::components::{FileRef, Other, SkateMaterial, SkateTexture};
use crate::texture::TexSource;

/// See the module documentation.
#[derive(Debug, Default)]
pub struct PrepareReskateTextures;

/// A tint (sRGB bytes) multiplied into RGBA8 pixels: in linear light, or for a `linear` texture into its bytes as
/// they are; alpha untouched.
pub fn tint_pixels(rgba: &mut [u8], tint: [u8; 3], linear: bool) {
    let to_lin = primase_image::srgb8_to_linear_f32;
    let [tr, tg, tb] = tint.map(to_lin);
    let byte = |v: f32| ((v * 255.0 + 0.5) as i32).clamp(0, 255) as u8;
    let mut lut = [[0u8; 3]; 256];
    for (i, e) in lut.iter_mut().enumerate() {
        *e = if linear {
            let c = i as f32 / 255.0;
            [byte(c * tr), byte(c * tg), byte(c * tb)]
        } else {
            let l = to_lin(i as u8);
            [tr, tg, tb].map(|t| primase_image::linear_to_srgb8_f32(l * t))
        };
    }
    for px in rgba.as_chunks_mut::<4>().0 {
        px[0] = lut[px[0] as usize][0];
        px[1] = lut[px[1] as usize][1];
        px[2] = lut[px[2] as usize][2];
    }
}

/// A tinted copy: its pixels, its dialect entry and its name.
type TintedCopy = (Texture, SkateTexture, String);

/// A tinted copy of `base`: its first level decoded to RGBA8 (as the writer decodes it) with `tint` multiplied in;
/// `None` when the level does not decode.
fn tinted(w: &World, base: Entity, tint: [u8; 3]) -> Result<Option<TintedCopy>, String> {
    let (Some(tex), Some(st)) = (w.get::<Texture>(base), w.get::<SkateTexture>(base)) else {
        return Ok(None);
    };
    let name = w
        .get::<Name>(base)
        .map_or_else(|| st.key.clone(), |n| n.0.clone());
    let src = TexSource::from_texture(&name, tex, st.source_format.as_deref(), st.source_size)
        .map_err(|e| e.to_string())?;
    let Some(mut rgba) = src
        .decode(0, tex.width, tex.height)
        .map_err(|e| e.to_string())?
        .filter(|px| px.len() >= tex.width as usize * tex.height as usize * 4)
    else {
        return Ok(None);
    };
    let linear = st.linear == Some(true);
    tint_pixels(&mut rgba, tint, linear);
    let key = format!("{}_tint_{:02x}{:02x}{:02x}", st.key, tint[0], tint[1], tint[2]);
    let texture = Texture {
        width: tex.width,
        height: tex.height,
        format: TexFormat::Rgba8Unorm,
        levels: vec![rgba],
        color_space: if linear {
            ColorSpace::Linear
        } else {
            ColorSpace::Srgb
        },
        role: TexRole::Color,
        normal: None,
    };
    let skate = SkateTexture {
        file: FileRef {
            path: format!("textures/{key}.png"),
            sha256: None,
        },
        key,
        stem: st.stem.clone(),
        // the copy is its base as the map records it: the base's format and size
        source_format: Some(src.format_name.clone()),
        source_size: Some([src.source_width as i32, src.source_height as i32]),
        linear: st.linear,
        green: None,
        z: None,
        other: Other::new(),
    };
    Ok(Some((texture, skate, name)))
}

impl Pass for PrepareReskateTextures {
    fn name(&self) -> &'static str {
        "prepare-reskate-textures"
    }
    // the materials are optional: a map without them has no texture to prepare
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.texture", "skate.texture", "core.name"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["skate.material"]
    }
    /// A base texture no material draws any more is no longer one of the map's: its `skate.texture` goes (its pixels
    /// stay).
    fn removes(&self) -> &[&'static str] {
        &["skate.texture"]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "material tints multiplied into 8-bit copies of their base textures (in linear light)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let tints: Vec<(Entity, Option<Entity>, [u8; 3])> = w
            .iter::<SkateMaterial>()
            .filter_map(|(e, m)| m.tint.map(|t| (e, m.base, t)))
            .collect();
        // white multiplies nothing: the material draws its base as it is
        let (white, tints): (Vec<_>, Vec<_>) = tints.into_iter().partition(|t| t.2 == [255; 3]);
        // every copy made before the first change, so a texture that does not decode changes nothing
        let mut copies: BTreeMap<(Entity, [u8; 3]), Option<TintedCopy>> = BTreeMap::new();
        for &(_, base, tint) in &tints {
            if let Some(base) = base
                && !copies.contains_key(&(base, tint))
            {
                copies.insert((base, tint), tinted(w, base, tint)?);
            }
        }
        for (m, _, _) in white {
            w.get_mut::<SkateMaterial>(m).expect("listed").tint = None;
            log.count("white material tints dropped (multiply nothing)", 1);
        }
        let mut made: BTreeMap<(Entity, [u8; 3]), Entity> = BTreeMap::new();
        for (key, copy) in copies {
            if let Some((texture, skate, name)) = copy {
                let e = w.spawn();
                w.insert(e, Name(name));
                w.insert(e, texture);
                w.insert(e, skate);
                made.insert(key, e);
                log.count("tinted texture copies", 1);
            }
        }
        for (m, base, tint) in tints {
            let Some(base) = base else {
                log.omit("material tints without a base texture (nothing to tint)", 1);
                w.get_mut::<SkateMaterial>(m).expect("listed").tint = None;
                continue;
            };
            match made.get(&(base, tint)) {
                Some(&copy) => {
                    let mat = w.get_mut::<SkateMaterial>(m).expect("listed");
                    mat.base = Some(copy);
                    mat.tint = None;
                    log.approximate(
                        "material tints multiplied into 8-bit copies of their base textures (linear light)",
                        1,
                    );
                }
                None => log.count("material tints kept: base texture not decodable", 1),
            }
        }
        // the textures the placed primitives' materials draw (a material no placed primitive draws, as one
        // lower-tint-reskate replaced by tinted copies, draws nothing): a base only its tinted copies draw is no
        // longer the map's
        let mut drawn_materials: std::collections::BTreeSet<Entity> = std::collections::BTreeSet::new();
        for (_, i) in w.iter::<primase_core::Instance>() {
            if let Some(mesh) = w.get::<primase_core::Mesh>(i.prototype) {
                drawn_materials.extend(mesh.primitives().iter().filter_map(|p| p.material));
            }
            if let Some(v) = w.get::<primase_core::MaterialVariants>(i.prototype) {
                drawn_materials.extend(v.0.iter().flat_map(|row| row.values().copied()));
            }
        }
        let drawn: std::collections::BTreeSet<Entity> = drawn_materials
            .iter()
            .filter_map(|&m| w.get::<SkateMaterial>(m))
            .flat_map(|m| [m.base, m.normal, m.mask])
            .flatten()
            .collect();
        for base in made
            .keys()
            .map(|(b, _)| *b)
            .collect::<std::collections::BTreeSet<_>>()
        {
            if !drawn.contains(&base) {
                w.remove::<SkateTexture>(base);
                log.count(
                    "base textures only drawn tinted: no longer the map's (pixels kept)",
                    1,
                );
            }
        }
        Ok(())
    }
}
