//! Materials of a map: what a scene says a material is ([`MaterialSpec`]), and the library that turns specs into map
//! materials named after their content (the reference writer's MatLib).

use std::collections::{HashMap, HashSet};

use crate::Result;
use crate::layered::VertexBlend;
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
    /// Source blend; requires geometry lowering before native map writing.
    Layered = 9,
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
    /// Studio's per-vertex blend (`vertex_blend`; what it draws): its parts carry a weight per vertex.
    pub vertex_blend: Option<VertexBlend>,
}

/// What a front end says a material is.
#[derive(Clone, Debug)]
pub struct MaterialSpec<'a> {
    pub class: VClass,
    pub base: Option<&'a TexRef>,
    pub flat: u32,
    pub flat_alpha: u8,
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
    pub vertex_blend: Option<VertexBlend>,
    /// Studio's base colour factor (linear RGBA), as a budget left the material (`skate.base_factor`).
    pub base_factor: Option<[f32; 4]>,
}

impl<'a> MaterialSpec<'a> {
    /// A class's defaults for a base texture with (`tex_alpha`) or without visible alpha.
    pub fn derived(class: VClass, tex_alpha: bool) -> MaterialSpec<'a> {
        let mut m = MaterialSpec {
            class,
            base: None,
            flat: 0x808080,
            flat_alpha: 255,
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
            vertex_blend: None,
            base_factor: None,
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
        VClass::Layered => "layered",
    }
}

impl MatLib {
    pub fn by_id(&self, id: usize) -> &Material {
        &self.list[id]
    }

    /// Writes the spec's base texture (or flat colour) and returns its material id.
    pub fn resolve(&mut self, tex: &mut TexLib, s: &MaterialSpec) -> Result<usize> {
        let t = match s.base {
            Some(b) => tex.register(b, s.alpha != 1)?,
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
        if let Some(f) = s.base_factor {
            extra += &format!(
                "|f{:08x}{:08x}{:08x}{:08x}",
                f[0].to_bits(),
                f[1].to_bits(),
                f[2].to_bits(),
                f[3].to_bits()
            );
        }
        // a chain overlay's rank is part of its identity: two overlays alike in everything else (two black layers)
        // stay two materials, each drawn at its own place in the chain
        match s.vertex_blend.map(VertexBlend::chain_rank) {
            Some(None) => extra += "|vb",
            Some(Some(rank)) => extra += &format!("|vb{rank}"),
            None => {}
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
        // unique within the map (map.json names materials, and prepare-reskate-overlays checks pieces by name): a
        // name taken gets the material's id, then a count, until it is new
        if !self.names.insert(name.clone()) {
            self.collisions.push(name.clone());
            let base = name.clone();
            name = format!("{base}_{id}");
            let mut n = 1;
            while !self.names.insert(name.clone()) {
                name = format!("{base}_{id}_{n}");
                n += 1;
            }
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
            base_factor: s.base_factor,
            vertex_blend: s.vertex_blend,
        });
        self.map.insert(key, id);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A material's name is new in its map even when its name and the id-suffixed one are both taken: the count goes
    /// on until it is.
    #[test]
    fn material_names_are_unique_however_they_collide() {
        let tex = TexLib::new(std::path::Path::new(""));
        let spec = MaterialSpec::derived(VClass::Decal, false);
        let mut first = MatLib::default();
        let id = first.get(&tex, &spec, -1);
        let name = first.by_id(id).name.clone();
        // a map where both names it would take are another material's already
        let mut lib = MatLib::default();
        lib.names.insert(name.clone());
        lib.names.insert(format!("{name}_0"));
        let id = lib.get(&tex, &spec, -1);
        assert_eq!(lib.by_id(id).name, format!("{name}_0_1"));
        assert_eq!(lib.collisions, [name]);
    }
}
