//! `prepare-reskate-blended-surfaces{cutoff=C,materials=all|<name>;<name>...,uniform=cutout|frost[,opacity=F,grey=G]}`:
//! blended surfaces drawn as skate draws world geometry, before `raise-skate`. Studio compiles an alpha-blended
//! material to the stock decal/blend shader pair, which skate does not draw as standalone world geometry (a blended
//! tree canopy vanishes); an alpha-tested one compiles to the cutout programs and draws. So a blended PBR material that
//! is neither a decal (drawn over the surface beneath it, which skate's decal path draws: `core.draw_order`'s decal
//! layer) nor water (`core.material_usage` water) is drawn as a mask at the stated cutoff: soft edges become hard.
//!
//! A material's alpha is uniform when it is its base colour factor's alpha, times its base colour texture's when that
//! is the same at every texel the material's primitives sample: their triangles rasterised in the texture's first
//! level through the slot's coordinate set, transform and wrap (an atlas transparent in its gutters is judged on its
//! islands only); over the whole first level when no loaded primitive's coordinates can say. Each material is reported
//! with how it was judged and over what. A uniform alpha at or above the cutoff draws whole: the material is drawn
//! opaque (exactly what the mask draws, without an alpha texture). A uniform alpha below the cutoff would vanish
//! whole as a mask, though it still collides (glass). What it becomes is the required `uniform` choice:
//! - `cutout`: drawn as nothing (a mask on its base colour factor below the cutoff, its texture dropped, so no texture
//!   alpha draws it in its place);
//! - `frost` (with `opacity=F` in (0, 1] and `grey=G` in [0, 1], both required): drawn opaque as frosted glass, its
//!   colour its base colour composited over a light grey at an opacity a = max(alpha, F), in linear light:
//!   colour = a * base + (1 - a) * (G, G, G), where base is the base colour factor's colour times the base colour
//!   texture's mean colour over the texels its primitives sample (sRGB decoded); the texture is dropped.
//!
//! An alpha that varies over the texels sampled takes the mask. A material whose base colour texture is not loaded is
//! taken as varying, counted. Each material changed is reported by name as an
//! approximation.
//!
//! An explicit workaround for the compiled rendering path observed, chosen by the program: `materials=all` takes every
//! eligible blended material, or a list of names takes those only (a named material present but not eligible, or not
//! blended, is refused). No choice has a default. Not the same predicate as `prepare-reskate-uv-repeats{blended=}`,
//! which adapts only triangles past Studio's repeat limit, after raising.

use std::collections::{BTreeSet, HashMap};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    DrawLayer, DrawOrder, Entity, MaterialUsage, MaterialVariants, Mesh, Name, Semantic, Stream, Texture,
    Topology, World,
};
use primase_texcodec::{Decoder, TexDecoder};

/// Which blended materials become cutouts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlendedSelection {
    /// Every eligible blended material (not a decal, not water).
    All,
    /// The materials of these names only; each must be an eligible blended material where present.
    Named(BTreeSet<String>),
}

/// What a blended material of uniform alpha below the cutoff becomes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UniformAlpha {
    /// The mask at the cutoff (it vanishes).
    Cutout,
    /// Opaque frosted glass: its base colour over grey `grey` (linear) at opacity max(alpha, `opacity`).
    Frost { opacity: f32, grey: f32 },
}

/// See the module documentation. Made by [`PrepareReskateBlendedSurfaces::new`]; from options by the same call.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateBlendedSurfaces {
    cutoff: f32,
    selection: BlendedSelection,
    uniform: UniformAlpha,
}

impl PrepareReskateBlendedSurfaces {
    /// Blended surfaces of `selection` drawn as cutouts at `cutoff` (finite, above 0 and at most 1), those of uniform
    /// alpha below it as `uniform` says (frost: opacity in (0, 1], grey in [0, 1]); a named selection names at least
    /// one material.
    pub fn new(
        cutoff: f32,
        selection: BlendedSelection,
        uniform: UniformAlpha,
    ) -> Result<PrepareReskateBlendedSurfaces, String> {
        if !(cutoff.is_finite() && cutoff > 0.0 && cutoff <= 1.0) {
            return Err(format!("cutoff {cutoff} is not in (0, 1]"));
        }
        if let BlendedSelection::Named(names) = &selection
            && (names.is_empty() || names.iter().any(String::is_empty))
        {
            return Err("materials names no material (all, or names separated by ;)".into());
        }
        if let UniformAlpha::Frost { opacity, grey } = uniform {
            if !(opacity.is_finite() && opacity > 0.0 && opacity <= 1.0) {
                return Err(format!("frost opacity {opacity} is not in (0, 1]"));
            }
            if !(0.0..=1.0).contains(&grey) {
                return Err(format!("frost grey {grey} is not in [0, 1]"));
            }
        }
        Ok(PrepareReskateBlendedSurfaces {
            cutoff,
            selection,
            uniform,
        })
    }

    pub fn cutoff(&self) -> f32 {
        self.cutoff
    }

    pub fn selection(&self) -> &BlendedSelection {
        &self.selection
    }

    pub fn uniform(&self) -> UniformAlpha {
        self.uniform
    }

    /// From its options: `cutoff`, `materials` (`all`, or names separated by `;`) and `uniform` (`cutout`, or `frost`
    /// with `opacity` and `grey`), all required.
    pub fn from_options(o: &Options) -> Result<PrepareReskateBlendedSurfaces, String> {
        if let Some(k) = o.keys().find(|k| {
            !matches!(
                k.as_str(),
                "cutoff" | "materials" | "uniform" | "opacity" | "grey"
            )
        }) {
            return Err(format!("unknown option {k}"));
        }
        let number = |k: &str| -> Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse::<f32>().map_err(|_| format!("{k}={v} is not a number")))
                .transpose()
        };
        let cutoff = number("cutoff")?.ok_or("cutoff is required: the alpha a cutout surface draws above")?;
        let selection = match o.get("materials").map(String::as_str) {
            Some("all") => BlendedSelection::All,
            Some(names) => BlendedSelection::Named(names.split(';').map(str::to_owned).collect()),
            None => return Err("materials is required: all, or names separated by ;".into()),
        };
        let (opacity, grey) = (number("opacity")?, number("grey")?);
        let uniform = match (o.get("uniform").map(String::as_str), opacity, grey) {
            (Some("cutout"), None, None) => UniformAlpha::Cutout,
            (Some("cutout"), _, _) => {
                return Err("opacity and grey are frost's: uniform=cutout takes neither".into());
            }
            (Some("frost"), Some(opacity), Some(grey)) => UniformAlpha::Frost { opacity, grey },
            (Some("frost"), _, _) => {
                return Err(
                    "uniform=frost requires opacity (its least opacity) and grey (linear, 0..1)".into(),
                );
            }
            (Some(v), _, _) => return Err(format!("uniform={v}: cutout or frost")),
            (None, _, _) => {
                return Err(
                    "uniform is required: cutout or frost (a blended surface of uniform alpha below the cutoff)".into(),
                );
            }
        };
        PrepareReskateBlendedSurfaces::new(cutoff, selection, uniform)
    }
}

/// [`PrepareReskateBlendedSurfaces::from_options`] as a pass factory.
pub fn prepare_reskate_blended_surfaces(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateBlendedSurfaces::from_options(o)?))
}

/// Why a blended material is not drawn as a cutout: drawn over a surface, or water; `None`: eligible.
fn kept_blended(w: &World, e: Entity) -> Option<&'static str> {
    if DrawOrder::of(w, e).layer == DrawLayer::Decal {
        Some("a decal")
    } else if w.get::<MaterialUsage>(e).copied() == Some(MaterialUsage::Water) {
        Some("water")
    } else {
        None
    }
}

/// An sRGB byte in linear light (the IEC 61966-2-1 curve).
fn linear(b: u8) -> f32 {
    let c = f32::from(b) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The primitives drawing each material: (mesh entity, primitive index), its own material or one of its variants.
type Uses = HashMap<Entity, Vec<(Entity, usize)>>;

fn uses(w: &World) -> Uses {
    let mut uses: Uses = HashMap::new();
    for (e, mesh) in w.iter::<Mesh>() {
        let variants = w.get::<MaterialVariants>(e);
        for (i, p) in mesh.primitives().iter().enumerate() {
            let mut drawn: BTreeSet<Entity> = p.material.into_iter().collect();
            if let Some(v) = variants.and_then(|v| v.0.get(i)) {
                drawn.extend(v.values().copied());
            }
            for m in drawn {
                uses.entry(m).or_default().push((e, i));
            }
        }
    }
    uses
}

/// Texel `i` of a `n`-texel axis under `wrap`.
fn wrapped(i: i64, n: u32, wrap: Wrap) -> usize {
    let n = i64::from(n.max(1));
    (match wrap {
        Wrap::Repeat => i.rem_euclid(n),
        Wrap::Clamp => i.clamp(0, n - 1),
        Wrap::Mirror => {
            let m = i.rem_euclid(2 * n);
            if m >= n { 2 * n - 1 - m } else { m }
        }
    }) as usize
}

/// The texels of a `tw` x `th` texture that triangle `t` (in texel units) covers: those whose centre it holds, and
/// each vertex's own, wrapped as `wrap` says. A triangle spanning the texture both ways covers all of it.
fn mark(covered: &mut [bool], (tw, th): (u32, u32), wrap: [Wrap; 2], t: [[f32; 2]; 3]) {
    let mut set =
        |x: i64, y: i64| covered[wrapped(y, th, wrap[1]) * tw as usize + wrapped(x, tw, wrap[0])] = true;
    for q in t {
        set(q[0].floor() as i64, q[1].floor() as i64);
    }
    let lo = [0, 1].map(|c| t.iter().map(|q| q[c]).fold(f32::INFINITY, f32::min).floor() as i64);
    let hi = [0, 1].map(|c| t.iter().map(|q| q[c]).fold(f32::NEG_INFINITY, f32::max).ceil() as i64);
    let span = [hi[0] - lo[0], hi[1] - lo[1]];
    if (span[0] >= i64::from(tw) && span[1] >= i64::from(th))
        || span[0].saturating_mul(span[1]) > 4 * i64::from(tw) * i64::from(th)
    {
        covered.iter_mut().for_each(|c| *c = true);
        return;
    }
    let [a, b, c] = t;
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
    if area.abs() < 1e-12 {
        return;
    }
    let edge = |p: [f32; 2], q: [f32; 2], x: f32, y: f32| {
        ((q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])) / area
    };
    for y in lo[1]..hi[1] {
        for x in lo[0]..hi[0] {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            if edge(a, b, fx, fy) >= 0.0 && edge(b, c, fx, fy) >= 0.0 && edge(c, a, fx, fy) >= 0.0 {
                set(x, y);
            }
        }
    }
}

/// The texels of a `size` texture that the primitives `drawing` sample through slot `r` (its coordinate set, transform
/// and wrap); `None` when none can say (no loaded triangle list with those coordinates draws the material).
fn footprint(w: &World, drawing: &[(Entity, usize)], r: &TextureRef, size: (u32, u32)) -> Option<Vec<bool>> {
    let (tw, th) = size;
    let mut covered = vec![false; tw as usize * th as usize];
    let transform = r.transform.unwrap_or(UvTransform::IDENTITY);
    let mut any = false;
    for &(mesh, i) in drawing {
        let p = w.get::<Mesh>(mesh)?.primitives().get(i)?;
        if p.topology != Topology::Triangles {
            return None;
        }
        let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(r.texcoord)) else {
            return None;
        };
        let texel = |k: u32| -> Option<[f32; 2]> {
            let q = transform.apply(*uv.get(k as usize)?);
            Some([q[0] * tw as f32, q[1] * th as f32])
        };
        let n = uv.len() as u32;
        let triangles: Vec<[u32; 3]> = match &p.indices {
            Some(idx) => idx.as_chunks::<3>().0.to_vec(),
            None => (0..n / 3).map(|k| [3 * k, 3 * k + 1, 3 * k + 2]).collect(),
        };
        for [a, b, c] in triangles {
            mark(&mut covered, size, r.wrap, [texel(a)?, texel(b)?, texel(c)?]);
            any = true;
        }
    }
    any.then_some(covered)
}

/// How an eligible material's alpha was judged: uniform (the alpha and the base colour, the factor's times the texels'
/// mean, linear) or varying (`None`), and over what.
struct Judged {
    uniform: Option<(f32, [f32; 3])>,
    over: &'static str,
}

/// Material `m`'s alpha (drawn by the primitives `drawing`): its base colour factor's alpha, times its base colour
/// texture's when that is the same at every texel its primitives sample (the whole first level when none can say);
/// `Err` when its base colour texture is not loaded. `decoded` keeps each texture's first level for the run.
fn judge(
    w: &World,
    m: &PbrMaterial,
    drawing: &[(Entity, usize)],
    decoded: &mut HashMap<Entity, (u32, u32, Vec<u8>)>,
) -> Result<Judged, String> {
    let factor = [m.base_color[0], m.base_color[1], m.base_color[2]];
    let Some(r) = m.base_color_texture else {
        return Ok(Judged {
            uniform: Some((m.base_color[3], factor)),
            over: "its base colour factor",
        });
    };
    let (tw, th, rgba) = match decoded.entry(r.texture) {
        std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
        std::collections::hash_map::Entry::Vacant(v) => {
            let t = w
                .get::<Texture>(r.texture)
                .ok_or("its base colour texture is not loaded")?;
            let level = t.levels.first().ok_or("its base colour texture has no level")?;
            let rgba = Decoder::default()
                .decode_rgba8(t.format, t.width, t.height, level)
                .map_err(|e| e.to_string())?;
            v.insert((t.width, t.height, rgba))
        }
    };
    let texels = rgba.as_chunks::<4>().0;
    let (sampled, over): (Vec<&[u8; 4]>, _) = match footprint(w, drawing, &r, (*tw, *th)) {
        Some(covered) => (
            texels
                .iter()
                .zip(covered)
                .filter(|(_, c)| *c)
                .map(|(p, _)| p)
                .collect(),
            "the texels its primitives sample",
        ),
        None => (
            texels.iter().collect(),
            "its whole base colour texture (no primitive's coordinates to sample it by)",
        ),
    };
    let Some(first) = sampled.first() else {
        return Ok(Judged { uniform: None, over });
    };
    if sampled.iter().any(|p| p[3] != first[3]) {
        return Ok(Judged { uniform: None, over });
    }
    let n = sampled.len() as f64;
    let mean: [f32; 3] =
        std::array::from_fn(|c| (sampled.iter().map(|p| f64::from(linear(p[c]))).sum::<f64>() / n) as f32);
    Ok(Judged {
        uniform: Some((
            m.base_color[3] * f32::from(first[3]) / 255.0,
            std::array::from_fn(|c| factor[c] * mean[c]),
        )),
        over,
    })
}

impl Pass for PrepareReskateBlendedSurfaces {
    fn name(&self) -> &'static str {
        "prepare-reskate-blended-surfaces"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            ("cutoff".to_owned(), self.cutoff.to_string()),
            (
                "materials".to_owned(),
                match &self.selection {
                    BlendedSelection::All => "all".to_owned(),
                    BlendedSelection::Named(n) => n.iter().cloned().collect::<Vec<_>>().join(";"),
                },
            ),
        ]);
        match self.uniform {
            UniformAlpha::Cutout => {
                o.insert("uniform".to_owned(), "cutout".to_owned());
            }
            UniformAlpha::Frost { opacity, grey } => {
                o.insert("uniform".to_owned(), "frost".to_owned());
                o.insert("opacity".to_owned(), opacity.to_string());
                o.insert("grey".to_owned(), grey.to_string());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "blended surfaces drawn as alpha-tested cutouts (soft edges hard), or those of uniform alpha below the cutoff \
         as opaque frosted glass when chosen"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let name_of =
            |w: &World, e: Entity| w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone());
        let materials: Vec<Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        let uses = uses(w);
        let mut decoded = HashMap::new();
        for e in materials {
            let name = name_of(w, e);
            let named = match &self.selection {
                BlendedSelection::All => None,
                BlendedSelection::Named(n) => Some(n.contains(&name)),
            };
            if named == Some(false) {
                continue;
            }
            let blended = w
                .get::<PbrMaterial>(e)
                .is_some_and(|m| m.alpha_mode == AlphaMode::Blend);
            let kept = kept_blended(w, e);
            match (blended, kept, named) {
                // a material the program named must be one this pass changes
                (false, _, Some(true)) => {
                    return Err(format!(
                        "prepare-reskate-blended-surfaces: material {name} is named but is not blended"
                    ));
                }
                (true, Some(why), Some(true)) => {
                    return Err(format!(
                        "prepare-reskate-blended-surfaces: material {name} is named but is {why}, which stays blended"
                    ));
                }
                (false, _, _) => {}
                (true, Some(why), _) => {
                    log.count(&format!("blended materials kept blended: {why}"), 1);
                }
                (true, None, _) => {
                    let drawing = uses.get(&e).map_or(&[][..], Vec::as_slice);
                    self.lower(w, e, &name, drawing, &mut decoded, log);
                }
            }
        }
        Ok(())
    }
}

impl PrepareReskateBlendedSurfaces {
    /// Eligible blended material `e`, named `name`, drawn by `drawing`: opaque when its alpha is uniform at or above the
    /// cutoff; below it, frosted or drawn as nothing as `uniform` says; a cutout when its alpha varies.
    fn lower(
        &self,
        w: &mut World,
        e: Entity,
        name: &str,
        drawing: &[(Entity, usize)],
        decoded: &mut HashMap<Entity, (u32, u32, Vec<u8>)>,
        log: &mut PassLog,
    ) {
        let m = w.get::<PbrMaterial>(e).expect("listed");
        let judged = match judge(w, m, drawing, decoded) {
            Ok(j) => {
                let how = if j.uniform.is_some() { "uniform" } else { "varying" };
                log.count(
                    &format!("blended materials judged {how} over {}: {name}", j.over),
                    1,
                );
                j.uniform
            }
            Err(why) => {
                log.count(&format!("blended materials taken as varying alpha: {why}"), 1);
                None
            }
        };
        let m = w.get_mut::<PbrMaterial>(e).expect("listed");
        match (judged, self.uniform) {
            (Some((alpha, _)), _) if alpha >= self.cutoff => {
                m.alpha_mode = AlphaMode::Opaque;
                m.base_color[3] = 1.0;
                log.count(
                    &format!("blended surfaces of uniform alpha at or above the cutoff drawn opaque: {name}"),
                    1,
                );
            }
            (Some((alpha, base)), UniformAlpha::Frost { opacity, grey }) => {
                let a = alpha.max(opacity);
                let colour: [f32; 3] = std::array::from_fn(|c| a * base[c] + (1.0 - a) * grey);
                m.base_color = [colour[0], colour[1], colour[2], 1.0];
                m.base_color_texture = None;
                m.alpha_mode = AlphaMode::Opaque;
                log.approximate(
                    &format!("blended surface of uniform alpha drawn opaque as frosted glass at opacity {a}: {name}"),
                    1,
                );
                log.count("blended surfaces drawn as frosted glass", 1);
            }
            (Some((alpha, _)), UniformAlpha::Cutout) => {
                m.base_color[3] = alpha;
                m.base_color_texture = None;
                m.alpha_mode = AlphaMode::Mask { cutoff: self.cutoff };
                log.approximate(
                    &format!(
                        "blended surface of uniform alpha {alpha} below the cutoff {} drawn as nothing: {name}",
                        self.cutoff
                    ),
                    1,
                );
                log.count("blended surfaces drawn as nothing", 1);
            }
            (None, _) => {
                m.alpha_mode = AlphaMode::Mask { cutoff: self.cutoff };
                log.approximate(
                    &format!("blended surface drawn as a cutout at {}: {name}", self.cutoff),
                    1,
                );
                log.count("blended surfaces drawn as cutouts", 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A material `name` drawn as `alpha`, its base colour texture's alpha varying (soft-edged foliage).
    fn material(w: &mut World, name: &str, alpha: AlphaMode) -> Entity {
        let soft = texture(w, [[255, 255, 255, 10], [255, 255, 255, 250]]);
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            PbrMaterial {
                alpha_mode: alpha,
                base_color_texture: Some(primase_core::pbr::TextureRef::new(soft)),
                ..PbrMaterial::default()
            },
        );
        e
    }

    fn mode(w: &World, e: Entity) -> AlphaMode {
        w.get::<PbrMaterial>(e).unwrap().alpha_mode
    }

    /// Every eligible blended material becomes a cutout at the cutoff, each reported by name; a decal, water and an
    /// opaque material are left as they are.
    #[test]
    fn eligible_blended_surfaces_become_cutouts() {
        let mut w = World::new();
        let leaves = material(&mut w, "leaves", AlphaMode::Blend);
        let glass = material(&mut w, "glass", AlphaMode::Blend);
        let decal = material(&mut w, "decal", AlphaMode::Blend);
        w.insert(decal, DrawOrder::conventional(DrawLayer::Decal, 2500));
        let water = material(&mut w, "water", AlphaMode::Blend);
        w.insert(water, MaterialUsage::Water);
        let stone = material(&mut w, "stone", AlphaMode::Opaque);
        let mut log = PassLog::default();
        PrepareReskateBlendedSurfaces::new(0.5, BlendedSelection::All, UniformAlpha::Cutout)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        for e in [leaves, glass] {
            assert_eq!(mode(&w, e), AlphaMode::Mask { cutoff: 0.5 });
        }
        assert_eq!(mode(&w, decal), AlphaMode::Blend);
        assert_eq!(mode(&w, water), AlphaMode::Blend);
        assert_eq!(mode(&w, stone), AlphaMode::Opaque);
        assert_eq!(
            log.approximations["blended surface drawn as a cutout at 0.5: leaves"],
            1
        );
        assert_eq!(
            log.approximations["blended surface drawn as a cutout at 0.5: glass"],
            1
        );
        assert_eq!(log.counts["blended materials kept blended: a decal"], 1);
        assert_eq!(log.counts["blended materials kept blended: water"], 1);
    }

    /// A named selection changes those materials only (glass is not foliage because both blend), and refuses a named
    /// material that is not blended or stays blended.
    #[test]
    fn a_named_selection_changes_those_only() {
        let mut w = World::new();
        let leaves = material(&mut w, "leaves", AlphaMode::Blend);
        let glass = material(&mut w, "glass", AlphaMode::Blend);
        let mut log = PassLog::default();
        let pass = PrepareReskateBlendedSurfaces::from_options(&Options::from([
            ("cutoff".to_owned(), "0.4".to_owned()),
            ("materials".to_owned(), "leaves".to_owned()),
            ("uniform".to_owned(), "cutout".to_owned()),
        ]))
        .unwrap();
        pass.run(&mut w, &mut log).unwrap();
        assert_eq!(mode(&w, leaves), AlphaMode::Mask { cutoff: 0.4 });
        assert_eq!(mode(&w, glass), AlphaMode::Blend);
        // a named opaque material
        let mut w = World::new();
        material(&mut w, "leaves", AlphaMode::Opaque);
        let e = pass.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(e.contains("leaves is named but is not blended"), "{e}");
        // a named decal
        let mut w = World::new();
        let d = material(&mut w, "leaves", AlphaMode::Blend);
        w.insert(d, DrawOrder::conventional(DrawLayer::Decal, 2500));
        let e = pass.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(e.contains("is a decal, which stays blended"), "{e}");
    }

    /// Every choice is required and printed; a cutoff outside (0, 1] and an empty name list are refused.
    #[test]
    fn the_choices_are_required_and_printed() {
        let o = |pairs: &[(&str, &str)]| -> Options {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert!(
            PrepareReskateBlendedSurfaces::from_options(&o(&[("materials", "all"), ("uniform", "cutout")]))
                .is_err()
        );
        assert!(
            PrepareReskateBlendedSurfaces::from_options(&o(&[("cutoff", "0.5"), ("uniform", "cutout")]))
                .is_err()
        );
        assert!(
            PrepareReskateBlendedSurfaces::from_options(&o(&[("cutoff", "0.5"), ("materials", "all")]))
                .is_err()
        );
        for bad in ["0", "1.5", "NaN"] {
            assert!(
                PrepareReskateBlendedSurfaces::from_options(&o(&[
                    ("cutoff", bad),
                    ("materials", "all"),
                    ("uniform", "cutout")
                ]))
                .is_err(),
                "{bad}"
            );
        }
        assert!(
            PrepareReskateBlendedSurfaces::from_options(&o(&[
                ("cutoff", "0.5"),
                ("materials", ""),
                ("uniform", "cutout")
            ]))
            .is_err()
        );
        let p = PrepareReskateBlendedSurfaces::from_options(&o(&[
            ("cutoff", "0.5"),
            ("materials", "a;b"),
            ("uniform", "cutout"),
        ]))
        .unwrap();
        assert_eq!(
            PrepareReskateBlendedSurfaces::from_options(&p.options()).unwrap(),
            p
        );
        assert_eq!(p.options()["materials"], "a;b");
        // frost takes opacity and grey, both, and cutout neither; each in range
        let frost = |extra: &[(&str, &str)]| {
            let mut kv = vec![("cutoff", "0.5"), ("materials", "all"), ("uniform", "frost")];
            kv.extend_from_slice(extra);
            PrepareReskateBlendedSurfaces::from_options(&o(&kv))
        };
        assert!(frost(&[("opacity", "0.35")]).is_err());
        assert!(frost(&[("grey", "0.8")]).is_err());
        assert!(frost(&[("opacity", "0"), ("grey", "0.8")]).is_err());
        assert!(frost(&[("opacity", "0.35"), ("grey", "1.5")]).is_err());
        assert!(
            PrepareReskateBlendedSurfaces::from_options(&o(&[
                ("cutoff", "0.5"),
                ("materials", "all"),
                ("uniform", "cutout"),
                ("opacity", "0.35")
            ]))
            .is_err()
        );
        let p = frost(&[("opacity", "0.35"), ("grey", "0.8")]).unwrap();
        assert_eq!(
            p.uniform(),
            UniformAlpha::Frost {
                opacity: 0.35,
                grey: 0.8
            }
        );
        assert_eq!(
            PrepareReskateBlendedSurfaces::from_options(&p.options()).unwrap(),
            p
        );
    }

    fn frost() -> PrepareReskateBlendedSurfaces {
        PrepareReskateBlendedSurfaces::new(
            0.5,
            BlendedSelection::All,
            UniformAlpha::Frost {
                opacity: 0.35,
                grey: 0.8,
            },
        )
        .unwrap()
    }

    /// A blended material `name` of base colour `colour`, its base colour texture `texture` if any.
    fn tinted(w: &mut World, name: &str, colour: [f32; 4], texture: Option<Entity>) -> Entity {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            PbrMaterial {
                alpha_mode: AlphaMode::Blend,
                base_color: colour,
                base_color_texture: texture.map(primase_core::pbr::TextureRef::new),
                ..PbrMaterial::default()
            },
        );
        e
    }

    /// A 2 x 1 RGBA8 texture of these two texels.
    fn texture(w: &mut World, texels: [[u8; 4]; 2]) -> Entity {
        let t = w.spawn();
        w.insert(
            t,
            Texture {
                width: 2,
                height: 1,
                format: primase_core::TexFormat::Rgba8Unorm,
                levels: vec![texels.concat()],
                color_space: primase_core::ColorSpace::Srgb,
                role: primase_core::TexRole::Color,
                normal: None,
            },
        );
        t
    }

    /// Frost: glass of uniform alpha below the cutoff (alpha 0x06, no texture) is drawn opaque, its colour its base
    /// colour over the grey at opacity max(alpha, 0.35), reported by name; glass of uniform alpha above the cutoff, and
    /// one whose texture's alpha varies, take the cutout; with uniform=cutout the thin glass is a cutout too.
    #[test]
    fn uniform_glass_below_the_cutoff_is_frosted() {
        let mut w = World::new();
        let clean = tinted(&mut w, "Clean Shiny Glass", [0.2, 0.4, 0.6, 6.0 / 255.0], None);
        let thick = tinted(&mut w, "Glass_MAT", [1.0, 1.0, 1.0, 0.6], None);
        let mottled = texture(&mut w, [[255, 255, 255, 10], [255, 255, 255, 250]]);
        let leaves = tinted(&mut w, "leaves", [1.0; 4], Some(mottled));
        let mut log = PassLog::default();
        frost().run(&mut w, &mut log).unwrap();
        let m = w.get::<PbrMaterial>(clean).unwrap();
        assert_eq!(m.alpha_mode, AlphaMode::Opaque);
        let want = [0.2, 0.4, 0.6].map(|b| 0.35 * b + 0.65 * 0.8);
        for (c, w) in want.iter().enumerate() {
            assert!((m.base_color[c] - w).abs() < 1e-6, "{c}");
        }
        assert_eq!(m.base_color[3], 1.0);
        assert_eq!(
            log.approximations["blended surface of uniform alpha drawn opaque as frosted glass at opacity 0.35: Clean Shiny Glass"],
            1
        );
        // uniform 0.6 at or above the cutoff draws whole: opaque; a varying alpha takes the cutout
        assert_eq!(mode(&w, thick), AlphaMode::Opaque);
        assert_eq!(w.get::<PbrMaterial>(thick).unwrap().base_color[3], 1.0);
        assert_eq!(mode(&w, leaves), AlphaMode::Mask { cutoff: 0.5 });
        assert_eq!(
            log.counts["blended materials judged uniform over its base colour factor: Glass_MAT"],
            1
        );
        // uniform=cutout: the thin glass is drawn as nothing (a mask on its factor, below the cutoff)
        let mut w = World::new();
        let clean = tinted(&mut w, "Clean Shiny Glass", [0.2, 0.4, 0.6, 6.0 / 255.0], None);
        PrepareReskateBlendedSurfaces::new(0.5, BlendedSelection::All, UniformAlpha::Cutout)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert_eq!(mode(&w, clean), AlphaMode::Mask { cutoff: 0.5 });
        assert!(w.get::<PbrMaterial>(clean).unwrap().base_color[3] < 0.5);
    }

    /// An atlas opaque under the material's coordinates and transparent around them (its gutters), the material's
    /// factor alpha 0.2: judged over the texels its primitives sample, the alpha is the factor's, uniform, below the
    /// cutoff: drawn as nothing (uniform=cutout, the atlas dropped so its alpha cannot draw it) or frosted
    /// (uniform=frost); unplaced (no primitive to say), the whole atlas is judged, varying: a cutout on the atlas.
    #[test]
    fn an_atlas_is_judged_over_the_texels_the_primitives_sample() {
        let atlas = |w: &mut World| {
            let t = w.spawn();
            // 4 x 4: opaque in the left half (the island), transparent in the right (the gutter)
            let level: Vec<u8> = (0..16)
                .flat_map(|i| {
                    if i % 4 < 2 {
                        [200, 200, 200, 255]
                    } else {
                        [0, 0, 0, 0]
                    }
                })
                .collect();
            w.insert(
                t,
                Texture {
                    width: 4,
                    height: 4,
                    format: primase_core::TexFormat::Rgba8Unorm,
                    levels: vec![level],
                    color_space: primase_core::ColorSpace::Srgb,
                    role: primase_core::TexRole::Color,
                    normal: None,
                },
            );
            t
        };
        let drawn = |w: &mut World, m: Entity| {
            let p = w.spawn();
            w.insert(
                p,
                Mesh::new(vec![primase_core::Primitive {
                    topology: Topology::Triangles,
                    streams: std::collections::BTreeMap::from([
                        (
                            Semantic::Position,
                            Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
                        ),
                        (
                            Semantic::TexCoord(0),
                            Stream::F32x2(vec![[0.05, 0.05], [0.45, 0.05], [0.05, 0.95]]),
                        ),
                    ]),
                    indices: None,
                    material: Some(m),
                }]),
            );
        };
        let glass = |w: &mut World, placed: bool| {
            let t = atlas(w);
            let m = tinted(w, "glass", [1.0, 1.0, 1.0, 0.2], Some(t));
            if placed {
                drawn(w, m);
            }
            m
        };
        let cutout =
            PrepareReskateBlendedSurfaces::new(0.5, BlendedSelection::All, UniformAlpha::Cutout).unwrap();
        let mut w = World::new();
        let m = glass(&mut w, true);
        let mut log = PassLog::default();
        cutout.run(&mut w, &mut log).unwrap();
        let mat = w.get::<PbrMaterial>(m).unwrap();
        assert_eq!(mat.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
        assert!(mat.base_color_texture.is_none());
        assert!((mat.base_color[3] - 0.2).abs() < 1e-6);
        assert_eq!(
            log.counts["blended materials judged uniform over the texels its primitives sample: glass"],
            1
        );
        assert_eq!(log.counts["blended surfaces drawn as nothing"], 1);
        // frost: opaque at max(0.2, 0.35), over the island's colour
        let mut w = World::new();
        let m = glass(&mut w, true);
        frost().run(&mut w, &mut PassLog::default()).unwrap();
        let mat = w.get::<PbrMaterial>(m).unwrap();
        assert_eq!(mat.alpha_mode, AlphaMode::Opaque);
        let island = linear(200);
        assert!((mat.base_color[0] - (0.35 * island + 0.65 * 0.8)).abs() < 1e-5);
        // unplaced: the whole atlas, varying
        let mut w = World::new();
        let m = glass(&mut w, false);
        let mut log = PassLog::default();
        cutout.run(&mut w, &mut log).unwrap();
        let mat = w.get::<PbrMaterial>(m).unwrap();
        assert_eq!(mat.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
        assert!(mat.base_color_texture.is_some());
        assert_eq!(
            log.counts["blended materials judged varying over its whole base colour texture (no primitive's \
                         coordinates to sample it by): glass"],
            1
        );
    }

    /// A triangle covers the texels whose centre it holds and its vertices' own, wrapped; one spanning the texture
    /// both ways covers it all.
    #[test]
    fn a_footprint_marks_the_texels_a_triangle_covers() {
        let mut c = vec![false; 16];
        mark(
            &mut c,
            (4, 4),
            [Wrap::Repeat; 2],
            [[0.2, 0.2], [1.8, 0.2], [0.2, 3.8]],
        );
        let set: Vec<usize> = (0..16).filter(|&i| c[i]).collect();
        // centres (0.5, 0.5), (1.5, 0.5), (0.5, 1.5), (0.5, 2.5) inside; (0, 3) the third vertex's own
        assert_eq!(set, [0, 1, 4, 8, 12]);
        // past the right edge, repeating: column 0 again
        let mut c = vec![false; 16];
        mark(
            &mut c,
            (4, 4),
            [Wrap::Repeat; 2],
            [[4.2, 0.2], [4.6, 0.2], [4.2, 0.6]],
        );
        assert!(c[0] && c.iter().filter(|x| **x).count() == 1);
        let mut c = vec![false; 16];
        mark(
            &mut c,
            (4, 4),
            [Wrap::Clamp; 2],
            [[-10.0, -10.0], [30.0, -10.0], [-10.0, 30.0]],
        );
        assert!(c.iter().all(|x| *x));
    }

    /// A texture whose alpha is the same at every texel makes the alpha uniform (the factor's times the texture's):
    /// frosted with the texture's mean colour (linear) and the texture dropped; a texture not loaded is taken as
    /// varying, counted.
    #[test]
    fn a_texture_of_uniform_alpha_is_frosted_by_its_mean_colour() {
        let mut w = World::new();
        // sRGB 255 and 0: linear 1 and 0, mean 0.5; alpha 0x7d of 255 times factor 1
        let window = texture(&mut w, [[255, 255, 255, 0x7d], [0, 0, 0, 0x7d]]);
        let clear = tinted(&mut w, "ClearGlass", [1.0; 4], Some(window));
        let unloaded = w.spawn();
        let far = tinted(&mut w, "far glass", [1.0, 1.0, 1.0, 0.1], Some(unloaded));
        let mut log = PassLog::default();
        frost().run(&mut w, &mut log).unwrap();
        let m = w.get::<PbrMaterial>(clear).unwrap();
        assert_eq!(m.alpha_mode, AlphaMode::Opaque);
        assert!(m.base_color_texture.is_none());
        let a = 0x7d as f32 / 255.0;
        let want = a * 0.5 + (1.0 - a) * 0.8;
        assert!(
            (m.base_color[0] - want).abs() < 1e-5,
            "{} vs {want}",
            m.base_color[0]
        );
        assert_eq!(mode(&w, far), AlphaMode::Mask { cutoff: 0.5 });
        assert_eq!(
            log.counts["blended materials taken as varying alpha: its base colour texture is not loaded"],
            1
        );
    }
}
