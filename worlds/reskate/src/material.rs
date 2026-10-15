//! Materials of a map: what a scene says a material is ([`MaterialSpec`]), and the library that turns specs into map
//! materials named after their content (the reference writer's MatLib).

use std::collections::{HashMap, HashSet};

use crate::Result;
use crate::texture::{Hash64, TexLib, TexRef, safe_label};

/// A surface class as the writer sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VClass {
    Opaque = 0,
    Cutout = 1,
    Decal = 2,
    Glass = 3,
    Water = 4,
    Terrain = 5,
    Leaves = 6,
    Wood = 7,
    Alpha = 8,
}

/// A map material. `alpha` and `domain` use Studio's numbers: 1 opaque, 2 mask, 3 blend; 0 surface, 1 decal,
/// 2 foliage.
#[derive(Clone, Debug)]
pub struct Material {
    pub id: usize,
    pub name: String,
    pub class: VClass,
    pub tex: i32,
    pub normal: i32,
    pub mask: i32,
    pub alpha: i32,
    pub cutoff: f32,
    pub domain: i32,
    pub two_sided: bool,
    pub emissive: bool,
    pub tris: u64,
    pub srgb: bool,
    pub roughness: Option<f64>,
    pub metallic: Option<f64>,
    pub wrap: Option<String>,
    pub ocean: Option<bool>,
    pub cast_shadows: Option<bool>,
    /// base_color_factor (linear RGBA), set by the texture budget.
    pub base_factor: Option<[f32; 4]>,
}

/// What a front end says a material is.
#[derive(Clone, Debug)]
pub struct MaterialSpec<'a> {
    pub class: VClass,
    pub base: Option<&'a TexRef>,
    pub flat: u32,
    pub flat_alpha: u8,
    pub tint: u32,
    pub label: Option<String>,
    pub emissive: bool,
    pub alpha: i32,
    pub cutoff: f32,
    pub two_sided: bool,
    pub domain: i32,
    pub normal: i32,
    pub mask: i32,
    pub srgb: bool,
    pub roughness: Option<f64>,
    pub metallic: Option<f64>,
    pub wrap: Option<String>,
    pub ocean: Option<bool>,
    pub cast_shadows: Option<bool>,
}

impl<'a> MaterialSpec<'a> {
    /// A class's defaults for a base texture with (`tex_alpha`) or without visible alpha.
    pub fn derived(class: VClass, tex_alpha: bool) -> MaterialSpec<'a> {
        let mut m = MaterialSpec {
            class,
            base: None,
            flat: 0x808080,
            flat_alpha: 255,
            tint: 0xFFFFFF,
            label: None,
            emissive: false,
            alpha: 1,
            cutoff: 0.5,
            two_sided: false,
            domain: 0,
            normal: -1,
            mask: -1,
            srgb: true,
            roughness: None,
            metallic: None,
            wrap: None,
            ocean: None,
            cast_shadows: None,
        };
        match class {
            VClass::Cutout => {
                m.alpha = if tex_alpha { 2 } else { 1 };
                m.two_sided = true;
            }
            VClass::Leaves => {
                m.alpha = if tex_alpha { 2 } else { 1 };
                m.cutoff = 0.33;
                m.domain = 2;
                m.two_sided = true;
            }
            VClass::Decal => {
                m.alpha = if tex_alpha { 3 } else { 1 };
                m.domain = 1;
            }
            VClass::Glass | VClass::Alpha | VClass::Water => m.alpha = if tex_alpha { 3 } else { 1 },
            _ => {}
        }
        m
    }
}

/// The map's materials.
#[derive(Debug, Default)]
pub struct MatLib {
    map: HashMap<String, usize>,
    names: HashSet<String>,
    pub list: Vec<Material>,
    /// Material names that collided and got a run-dependent suffix (the reference warns).
    pub collisions: Vec<String>,
}

fn kind_name(c: VClass) -> &'static str {
    match c {
        VClass::Cutout => "cut",
        VClass::Leaves => "leaf",
        VClass::Decal => "decal",
        VClass::Glass => "glass",
        VClass::Alpha => "alpha",
        VClass::Water => "water",
        VClass::Terrain => "ter",
        VClass::Wood => "wood",
        VClass::Opaque => "m",
    }
}

impl MatLib {
    pub fn by_id(&self, id: usize) -> &Material {
        &self.list[id]
    }

    /// Writes the spec's base texture (or flat colour) and returns its material id.
    pub fn resolve(&mut self, tex: &mut TexLib, s: &MaterialSpec) -> Result<usize> {
        let t = match s.base {
            Some(b) => tex.register(b, s.alpha != 1, s.tint)?,
            None => tex.register_flat(s.flat, s.flat_alpha),
        };
        Ok(self.get(tex, s, t))
    }

    /// The material of a spec whose base texture is output texture `tex_id`.
    pub fn get(&mut self, tex: &TexLib, s: &MaterialSpec, tex_id: i32) -> usize {
        let cls = s.class;
        let safe = safe_label(s.label.as_deref().unwrap_or(""));
        let d = MaterialSpec::derived(cls, s.alpha != 1);
        let mut extra = String::new();
        if s.cutoff != d.cutoff {
            extra += &format!("|c{:08x}", s.cutoff.to_bits());
        }
        if s.two_sided != d.two_sided {
            extra += &format!("|s{}", i32::from(s.two_sided));
        }
        if s.domain != d.domain {
            extra += &format!("|d{}", s.domain);
        }
        if !s.srgb {
            extra += "|lin";
        }
        if let Some(r) = s.roughness {
            extra += &format!("|r{:016x}", r.to_bits());
        }
        if let Some(m) = s.metallic {
            extra += &format!("|m{:016x}", m.to_bits());
        }
        if let Some(w) = &s.wrap {
            extra += &format!("|w{w}");
        }
        if let Some(o) = s.ocean {
            extra += &format!("|o{}", i32::from(o));
        }
        if let Some(c) = s.cast_shadows {
            extra += &format!("|h{}", i32::from(c));
        }
        let key = format!(
            "{}|{}|{}|{}|{}|{}|{}{}",
            cls as i32,
            tex_id,
            s.alpha,
            i32::from(s.emissive),
            safe,
            s.normal,
            s.mask,
            extra
        );
        if let Some(&known) = self.map.get(&key) {
            return known;
        }
        let id = self.list.len();
        let mut h = Hash64::new();
        h.int(cls as i32)
            .word(tex.content_of(tex_id))
            .int(s.alpha)
            .int(i32::from(s.emissive))
            .string(&safe);
        if s.normal >= 0 || s.mask >= 0 {
            h.word(tex.content_of(s.normal)).word(tex.content_of(s.mask));
        }
        if !extra.is_empty() {
            h.string(&extra);
        }
        let id40 = h.value() & 0xFF_FFFF_FFFF;
        let mut name = format!("{}_{}_{:010x}", kind_name(cls), safe, id40);
        if !self.names.insert(name.clone()) {
            self.collisions.push(name.clone());
            name = format!("{name}_{id}");
            self.names.insert(name.clone());
        }
        self.list.push(Material {
            id,
            name,
            class: cls,
            tex: tex_id,
            normal: s.normal,
            mask: s.mask,
            alpha: s.alpha,
            cutoff: s.cutoff,
            domain: s.domain,
            two_sided: s.two_sided,
            emissive: s.emissive,
            tris: 0,
            srgb: s.srgb,
            roughness: s.roughness,
            metallic: s.metallic,
            wrap: s.wrap.clone(),
            ocean: s.ocean,
            cast_shadows: s.cast_shadows,
            base_factor: None,
        });
        self.map.insert(key, id);
        id
    }
}
