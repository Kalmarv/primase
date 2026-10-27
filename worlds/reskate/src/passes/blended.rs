//! `prepare-reskate-blended-surfaces{cutoff=C,materials=all|<name>;<name>...,uniform=cutout|frost[,opacity=F,grey=G]}`:
//! blended surfaces drawn as skate draws world geometry, before `raise-skate`. Studio compiles an alpha-blended
//! material to the stock decal/blend shader pair, which skate does not draw as standalone world geometry (a blended
//! tree canopy vanishes); an alpha-tested one compiles to the cutout programs and draws. So a blended PBR material that
//! is neither a decal (drawn over the surface beneath it, which skate's decal path draws: `core.draw_order`'s decal
//! layer) nor water (`core.material_usage` water) is drawn as a mask at the stated cutoff: soft edges become hard. It
//! runs after `prepare-reskate-laid-layers` (it requires that pass's `skate.laid_layers`), so a blended layer laid on a
//! surface is a decal by then and stays blended.
//!
//! A material's alpha is uniform when it is its base colour factor's alpha, times its base colour texture's when that
//! is the same at every texel the material's primitives can sample, at every level they sample (an atlas transparent
//! in its gutters is judged on its islands only): their triangles through the slot's coordinate set, transform and
//! wrap; under nearest filtering (stated both ways) each texel whose square a triangle touches, else each texel a
//! bilinear tap within a triangle weighs (its square grown by half a texel); and every stored mip level the same way,
//! unless the slot states the base level only (which level a pixel samples depends on its screen derivatives, which a
//! long thin triangle can make coarse over many visible pixels, so no level is ruled out by the triangle's size). Over
//! every texel of every level when no loaded primitive's coordinates can say. Each material is reported
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

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    DrawLayer, DrawOrder, Entity, MaterialUsage, MaterialVariants, Mesh, Name, Semantic, Stream, Texture,
    Topology, World,
};
use primase_image::srgb8_to_linear_f32;
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
    /// with `opacity` and `grey`).
    fn from_given(g: &Given) -> Result<PrepareReskateBlendedSurfaces, String> {
        let number = |k: &str| -> Result<Option<f32>, String> {
            g.optional(k)
                .map(|v| v.parse::<f32>().map_err(|_| format!("{k}={v} is not a number")))
                .transpose()
        };
        let cutoff = g.required("cutoff");
        let cutoff = cutoff
            .parse::<f32>()
            .map_err(|_| format!("cutoff={cutoff} is not a number"))?;
        let selection = match g.required("materials") {
            "all" => BlendedSelection::All,
            names => BlendedSelection::Named(names.split(';').map(str::to_owned).collect()),
        };
        let (opacity, grey) = (number("opacity")?, number("grey")?);
        let uniform = match (g.choice::<UniformKind>("uniform"), opacity, grey) {
            (UniformKind::Cutout, None, None) => UniformAlpha::Cutout,
            (UniformKind::Cutout, _, _) => {
                return Err("opacity and grey are frost's: uniform=cutout takes neither".into());
            }
            (UniformKind::Frost, Some(opacity), Some(grey)) => UniformAlpha::Frost { opacity, grey },
            (UniformKind::Frost, _, _) => {
                return Err(
                    "uniform=frost requires opacity (its least opacity) and grey (linear, 0..1)".into(),
                );
            }
        };
        PrepareReskateBlendedSurfaces::new(cutoff, selection, uniform)
    }
}

/// `uniform`'s values: what [`UniformAlpha`] is, without frost's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UniformKind {
    Cutout,
    Frost,
}

impl Choice for UniformKind {
    const VALUES: &'static [(&'static str, Self)] =
        &[("cutout", UniformKind::Cutout), ("frost", UniformKind::Frost)];
}

/// What no value of a frost setting means.
const FROST_ONLY: &str = "required with uniform=frost, not taken with cutout";

/// `prepare-reskate-blended-surfaces`' declaration ([`primase_core::pass::PassRegistry::add`]): which blended surfaces
/// a map draws as cutouts, at what alpha, and what a uniform one below it becomes, are the program's, no default.
pub const PREPARE_RESKATE_BLENDED_SURFACES: PassSpec = PassSpec {
    name: "prepare-reskate-blended-surfaces",
    options: &[
        OptionSpec::required(
            "cutoff",
            "0.5",
            "in (0, 1]: the alpha a cutout surface draws above",
        ),
        OptionSpec::required(
            "materials",
            "all",
            "the blended materials drawn as cutouts: all, or their names separated by ;",
        ),
        OptionSpec::choice::<UniformKind>(
            "uniform",
            "what a blended surface of uniform alpha below the cutoff becomes: a cutout, or opaque frosted glass \
             (frost, with opacity and grey)",
        ),
        OptionSpec::optional(
            "opacity",
            "0.35",
            "in (0, 1]: frosted glass's least opacity",
            FROST_ONLY,
        ),
        OptionSpec::optional(
            "grey",
            "0.8",
            "linear, in [0, 1]: the grey frosted glass is drawn over",
            FROST_ONLY,
        ),
    ],
    make: |g| Ok(Box::new(PrepareReskateBlendedSurfaces::from_given(g)?)),
};

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

/// Whether triangle `t` and the axis-aligned box `lo`..`hi` share any point (touching counts), by separating axes:
/// the box's two and the triangle's three edge normals.
fn overlaps(t: [[f32; 2]; 3], lo: [f32; 2], hi: [f32; 2]) -> bool {
    let corners = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
    // the box's two axes and up to three edge normals
    let mut axes = [[1.0, 0.0], [0.0, 1.0], [0.0; 2], [0.0; 2], [0.0; 2]];
    let mut n = 2;
    for k in 0..3 {
        let (a, b) = (t[k], t[(k + 1) % 3]);
        let normal = [b[1] - a[1], a[0] - b[0]];
        if normal[0] != 0.0 || normal[1] != 0.0 {
            axes[n] = normal;
            n += 1;
        }
    }
    axes[..n].iter().all(|axis| {
        let dot = |p: &[f32; 2]| p[0] * axis[0] + p[1] * axis[1];
        let (t0, t1) = t
            .iter()
            .map(dot)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        let (s0, s1) = corners
            .iter()
            .map(dot)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        t1 >= s0 && s1 >= t0
    })
}

/// A coordinate, in texels, past which a triangle is not worked out: a texture is never this wide, and an `i64` texel
/// index of it (plus the margin) cannot overflow.
const FAR: f32 = 1.0e9;

/// The texels of a `tw` x `th` level that triangle `t` (in that level's texel units) can sample, wrapped as `wrap`
/// says: under nearest filtering each texel whose square the triangle touches; under linear filtering (`reach`) each
/// texel a bilinear tap at a point of the triangle weighs, those whose square grown by half a texel on every side it
/// touches. A triangle spanning the level both ways samples all of it, and so does one that cannot be worked out: a
/// coordinate that is not finite or is past [`FAR`] texels (it cannot say which texels it reaches). Marks them in
/// `covered`, of which `uncovered` are not yet; returns how many it newly marked.
fn mark(
    covered: &mut [bool],
    uncovered: usize,
    (tw, th): (u32, u32),
    wrap: [Wrap; 2],
    t: [[f32; 2]; 3],
    reach: bool,
) -> usize {
    let mut newly = 0;
    let all = |covered: &mut [bool]| {
        covered.iter_mut().for_each(|c| *c = true);
        uncovered
    };
    if t.iter().flatten().any(|v| !v.is_finite() || v.abs() > FAR) {
        return all(covered);
    }
    let grow = if reach { 0.5 } else { 0.0 };
    let lo = [0, 1].map(|c| (t.iter().map(|q| q[c]).fold(f32::INFINITY, f32::min) - grow).floor() as i64 - 1);
    let hi =
        [0, 1].map(|c| (t.iter().map(|q| q[c]).fold(f32::NEG_INFINITY, f32::max) + grow).ceil() as i64 + 1);
    let span = [hi[0] - lo[0], hi[1] - lo[1]];
    if (span[0] >= i64::from(tw) + 2 && span[1] >= i64::from(th) + 2)
        || span[0].saturating_mul(span[1]) > 4 * i64::from(tw) * i64::from(th)
    {
        return all(covered);
    }
    for y in lo[1]..hi[1] {
        for x in lo[0]..hi[0] {
            let (x0, y0) = (x as f32 - grow, y as f32 - grow);
            if overlaps(t, [x0, y0], [x0 + 1.0 + 2.0 * grow, y0 + 1.0 + 2.0 * grow]) {
                let texel = &mut covered[wrapped(y, th, wrap[1]) * tw as usize + wrapped(x, tw, wrap[0])];
                newly += usize::from(!*texel);
                *texel = true;
            }
        }
    }
    newly
}

/// Each stored level's texels (`sizes`, largest first) that the primitives `drawing` can sample through slot `r` (its
/// coordinate set, transform, wrap and filters): `Some(covered)` for each level judged, `None` for a level not sampled
/// (below the base when the slot states no mip levels). `None` overall when no primitive can say (no loaded triangle
/// list with those coordinates draws the material).
fn footprint(
    w: &World,
    drawing: &[(Entity, usize)],
    r: &TextureRef,
    sizes: &[(u32, u32)],
) -> Option<Vec<Option<Vec<bool>>>> {
    use iridia_interface::{Filter, MipFilter};
    let transform = r.transform.unwrap_or(UvTransform::IDENTITY);
    // bilinear reach unless the slot states nearest both ways
    let reach = !(r.filters.mag == Some(Filter::Nearest) && r.filters.min == Some(Filter::Nearest));
    let base_only = r.filters.mip == Some(MipFilter::None);
    let mut covered: Vec<Option<Vec<bool>>> = sizes
        .iter()
        .enumerate()
        .map(|(k, &(lw, lh))| (k == 0 || !base_only).then(|| vec![false; lw as usize * lh as usize]))
        .collect();
    // the texels of each level not yet covered: a level all covered is not worked on again
    let mut uncovered: Vec<usize> = sizes.iter().map(|&(lw, lh)| lw as usize * lh as usize).collect();
    let mut any = false;
    for &(mesh, i) in drawing {
        let p = w.get::<Mesh>(mesh)?.primitives().get(i)?;
        if p.topology != Topology::Triangles {
            return None;
        }
        let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(r.texcoord)) else {
            return None;
        };
        let at = |k: u32| -> Option<[f32; 2]> { Some(transform.apply(*uv.get(k as usize)?)) };
        let n = uv.len() as u32;
        let triangles: Vec<[u32; 3]> = match &p.indices {
            Some(idx) => idx.as_chunks::<3>().0.to_vec(),
            None => (0..n / 3).map(|k| [3 * k, 3 * k + 1, 3 * k + 2]).collect(),
        };
        for [a, b, c] in triangles {
            let t = [at(a)?, at(b)?, at(c)?];
            for (k, level) in covered.iter_mut().enumerate() {
                let Some(level) = level else { continue };
                if uncovered[k] == 0 {
                    continue;
                }
                let (lw, lh) = sizes[k];
                let s = t.map(|q| [q[0] * lw as f32, q[1] * lh as f32]);
                uncovered[k] -= mark(level, uncovered[k], (lw, lh), r.wrap, s, reach);
            }
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

/// A texture's stored levels decoded to RGBA8, largest first, with their sizes.
type Levels = Vec<(u32, u32, Vec<u8>)>;

/// Material `m`'s alpha (drawn by the primitives `drawing`): its base colour factor's alpha, times its base colour
/// texture's when that is the same at every texel its primitives can sample, at every stored level they sample
/// ([`footprint`]; every texel of every level when none can say); `Err` when its base colour texture is not loaded.
/// The colour is the factor's times the mean of the base level's texels sampled. `decoded` keeps each texture's
/// levels for the run.
fn judge(
    w: &World,
    m: &PbrMaterial,
    drawing: &[(Entity, usize)],
    decoded: &mut HashMap<Entity, Levels>,
) -> Result<Judged, String> {
    let factor = [m.base_color[0], m.base_color[1], m.base_color[2]];
    let Some(r) = m.base_color_texture else {
        return Ok(Judged {
            uniform: Some((m.base_color[3], factor)),
            over: "its base colour factor",
        });
    };
    let levels = match decoded.entry(r.texture) {
        std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
        std::collections::hash_map::Entry::Vacant(v) => {
            let t = w
                .get::<Texture>(r.texture)
                .ok_or("its base colour texture is not loaded")?;
            if t.levels.is_empty() {
                return Err("its base colour texture has no level".into());
            }
            let mut out = Vec::new();
            for (k, level) in t.levels.iter().enumerate() {
                let (lw, lh) = ((t.width >> k).max(1), (t.height >> k).max(1));
                let rgba = Decoder::default()
                    .decode_rgba8(t.format, lw, lh, level)
                    .map_err(|e| e.to_string())?;
                out.push((lw, lh, rgba));
            }
            v.insert(out)
        }
    };
    let sizes: Vec<(u32, u32)> = levels.iter().map(|(lw, lh, _)| (*lw, *lh)).collect();
    let (covered, over) = match footprint(w, drawing, &r, &sizes) {
        Some(c) => (c, "the texels its primitives can sample"),
        // every texel of every level the slot can sample: the base only when it states no mip levels
        None => (
            sizes
                .iter()
                .enumerate()
                .map(|(k, &(lw, lh))| {
                    (k == 0 || r.filters.mip != Some(iridia_interface::MipFilter::None))
                        .then(|| vec![true; lw as usize * lh as usize])
                })
                .collect(),
            "its whole base colour texture (no primitive's coordinates to sample it by)",
        ),
    };
    // every texel sampled, at every level judged, of one alpha
    let mut alpha: Option<u8> = None;
    for ((_, _, rgba), level) in levels.iter().zip(&covered) {
        let Some(level) = level else { continue };
        for (p, _) in rgba.as_chunks::<4>().0.iter().zip(level).filter(|(_, c)| **c) {
            match alpha {
                Some(a) if a != p[3] => return Ok(Judged { uniform: None, over }),
                _ => alpha = Some(p[3]),
            }
        }
    }
    let Some(alpha) = alpha else {
        return Ok(Judged { uniform: None, over });
    };
    let base: Vec<&[u8; 4]> = levels[0]
        .2
        .as_chunks::<4>()
        .0
        .iter()
        .zip(covered[0].as_deref().unwrap_or(&[]))
        .filter(|(_, c)| **c)
        .map(|(p, _)| p)
        .collect();
    let n = base.len().max(1) as f64;
    let mean: [f32; 3] = std::array::from_fn(|c| {
        (base
            .iter()
            .map(|p| f64::from(srgb8_to_linear_f32(p[c])))
            .sum::<f64>()
            / n) as f32
    });
    Ok(Judged {
        uniform: Some((
            m.base_color[3] * f32::from(alpha) / 255.0,
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
                o.insert("uniform".to_owned(), UniformKind::Cutout.text().to_owned());
            }
            UniformAlpha::Frost { opacity, grey } => {
                o.insert("uniform".to_owned(), UniformKind::Frost.text().to_owned());
                o.insert("opacity".to_owned(), opacity.to_string());
                o.insert("grey".to_owned(), grey.to_string());
            }
        }
        o
    }
    /// The layers laid on a surface judged first (`prepare-reskate-laid-layers`): they are decals then, and stay
    /// blended here.
    fn requires(&self) -> &[&'static str] {
        &["pbr.material", "skate.laid_layers"]
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
        decoded: &mut HashMap<Entity, Levels>,
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
        let pass = PREPARE_RESKATE_BLENDED_SURFACES
            .instance(&Options::from([
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
            PREPARE_RESKATE_BLENDED_SURFACES
                .instance(&o(&[("materials", "all"), ("uniform", "cutout")]))
                .is_err()
        );
        assert!(
            PREPARE_RESKATE_BLENDED_SURFACES
                .instance(&o(&[("cutoff", "0.5"), ("uniform", "cutout")]))
                .is_err()
        );
        assert!(
            PREPARE_RESKATE_BLENDED_SURFACES
                .instance(&o(&[("cutoff", "0.5"), ("materials", "all")]))
                .is_err()
        );
        for bad in ["0", "1.5", "NaN"] {
            assert!(
                PREPARE_RESKATE_BLENDED_SURFACES
                    .instance(&o(&[
                        ("cutoff", bad),
                        ("materials", "all"),
                        ("uniform", "cutout")
                    ]))
                    .is_err(),
                "{bad}"
            );
        }
        assert!(
            PREPARE_RESKATE_BLENDED_SURFACES
                .instance(&o(&[("cutoff", "0.5"), ("materials", ""), ("uniform", "cutout")]))
                .is_err()
        );
        let p = PREPARE_RESKATE_BLENDED_SURFACES
            .instance(&o(&[
                ("cutoff", "0.5"),
                ("materials", "a;b"),
                ("uniform", "cutout"),
            ]))
            .unwrap();
        assert_eq!(
            PREPARE_RESKATE_BLENDED_SURFACES
                .instance(&p.options())
                .unwrap()
                .options(),
            p.options()
        );
        assert_eq!(p.options()["materials"], "a;b");
        // frost takes opacity and grey, both, and cutout neither; each in range
        let frost = |extra: &[(&str, &str)]| {
            let mut kv = vec![("cutoff", "0.5"), ("materials", "all"), ("uniform", "frost")];
            kv.extend_from_slice(extra);
            PREPARE_RESKATE_BLENDED_SURFACES.instance(&o(&kv))
        };
        assert!(frost(&[("opacity", "0.35")]).is_err());
        assert!(frost(&[("grey", "0.8")]).is_err());
        assert!(frost(&[("opacity", "0"), ("grey", "0.8")]).is_err());
        assert!(frost(&[("opacity", "0.35"), ("grey", "1.5")]).is_err());
        assert!(
            PREPARE_RESKATE_BLENDED_SURFACES
                .instance(&o(&[
                    ("cutoff", "0.5"),
                    ("materials", "all"),
                    ("uniform", "cutout"),
                    ("opacity", "0.35")
                ]))
                .is_err()
        );
        let p = frost(&[("opacity", "0.35"), ("grey", "0.8")]).unwrap();
        assert_eq!(
            p.options(),
            o(&[
                ("cutoff", "0.5"),
                ("materials", "all"),
                ("uniform", "frost"),
                ("opacity", "0.35"),
                ("grey", "0.8")
            ])
        );
        assert_eq!(
            PREPARE_RESKATE_BLENDED_SURFACES
                .instance(&p.options())
                .unwrap()
                .options(),
            p.options()
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
            // 8 x 8: opaque in columns 0..6 (the island), transparent in 6 and 7 (the gutter)
            let level: Vec<u8> = (0..64)
                .flat_map(|i| {
                    if i % 8 < 6 {
                        [200, 200, 200, 255]
                    } else {
                        [0, 0, 0, 0]
                    }
                })
                .collect();
            w.insert(
                t,
                Texture {
                    width: 8,
                    height: 8,
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
                            // texels 1..4 across and 1..6 down: a bilinear tap reaches columns 0..4, all island
                            Stream::F32x2(vec![[0.125, 0.125], [0.5, 0.125], [0.125, 0.75]]),
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
            log.counts["blended materials judged uniform over the texels its primitives can sample: glass"],
            1
        );
        assert_eq!(log.counts["blended surfaces drawn as nothing"], 1);
        // frost: opaque at max(0.2, 0.35), over the island's colour
        let mut w = World::new();
        let m = glass(&mut w, true);
        frost().run(&mut w, &mut PassLog::default()).unwrap();
        let mat = w.get::<PbrMaterial>(m).unwrap();
        assert_eq!(mat.alpha_mode, AlphaMode::Opaque);
        let island = srgb8_to_linear_f32(200);
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

    /// Nearest: the texels whose square a triangle touches; bilinear: those whose square grown by half a texel it
    /// touches (a tap's reach), clamped or wrapped; a triangle spanning the texture both ways covers it all.
    #[test]
    fn a_footprint_marks_the_texels_a_triangle_can_sample() {
        let thin = [[0.5, 0.5], [2.5, 0.5], [0.5, 0.6]];
        let set = |c: &[bool]| (0..c.len()).filter(|&i| c[i]).collect::<Vec<usize>>();
        let mut c = vec![false; 16];
        assert_eq!(mark(&mut c, 16, (4, 4), [Wrap::Clamp; 2], thin, false), 3);
        assert_eq!(set(&c), [0, 1, 2]);
        // bilinear: column 3 (its grown square reaches 2.5) and row 1 (it reaches 0.5) too; columns and rows below 0
        // clamp onto 0
        let mut c = vec![false; 16];
        assert_eq!(mark(&mut c, 16, (4, 4), [Wrap::Clamp; 2], thin, true), 8);
        assert_eq!(set(&c), (0..8).collect::<Vec<usize>>());
        // past the right edge, repeating: column 0 again
        let mut c = vec![false; 16];
        mark(
            &mut c,
            16,
            (4, 4),
            [Wrap::Repeat; 2],
            [[4.2, 0.2], [4.6, 0.2], [4.2, 0.6]],
            false,
        );
        assert_eq!(set(&c), [0]);
        let mut c = vec![false; 16];
        let newly = mark(
            &mut c,
            16,
            (4, 4),
            [Wrap::Clamp; 2],
            [[-10.0, -10.0], [30.0, -10.0], [-10.0, 30.0]],
            true,
        );
        assert!(c.iter().all(|x| *x));
        assert_eq!(newly, 16);
    }

    /// A coordinate that is not finite or is past FAR cannot say which texels it reaches: the whole level (it
    /// overflowed an `i64` texel index, a panic with overflow checks and a wrapped span without), and the count of
    /// texels newly marked is those that were not.
    #[test]
    fn a_coordinate_that_cannot_be_worked_out_covers_the_level() {
        for far in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 1.0e30, -3.0e9] {
            let mut c = vec![false; 16];
            c[5] = true;
            let newly = mark(
                &mut c,
                15,
                (4, 4),
                [Wrap::Repeat; 2],
                [[0.5, 0.5], [far, 0.5], [0.5, 0.6]],
                true,
            );
            assert!(c.iter().all(|x| *x), "{far}");
            assert_eq!(newly, 15, "{far}");
        }
        // a level all covered is not worked on again: a triangle that would mark nothing new marks none
        let mut c = vec![true; 16];
        assert_eq!(
            mark(
                &mut c,
                0,
                (4, 4),
                [Wrap::Clamp; 2],
                [[0.5, 0.5], [1.5, 0.5], [0.5, 1.5]],
                false
            ),
            0
        );
    }

    /// A triangle with an infinite coordinate is judged over the whole texture (here two texels of different alpha:
    /// varying), not a panic.
    #[test]
    fn an_infinite_coordinate_is_judged_over_the_whole_texture() {
        let mut w = World::new();
        let soft = texture(&mut w, [[255, 255, 255, 10], [255, 255, 255, 250]]);
        let m = tinted(&mut w, "pane", [1.0; 4], Some(soft));
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
                        Stream::F32x2(vec![[0.0, 0.0], [f32::INFINITY, 0.0], [0.0, 0.5]]),
                    ),
                ]),
                indices: None,
                material: Some(m),
            }]),
        );
        let mut log = PassLog::default();
        frost().run(&mut w, &mut log).unwrap();
        assert_eq!(mode(&w, m), AlphaMode::Mask { cutoff: 0.5 });
        assert_eq!(
            log.counts["blended materials judged varying over the texels its primitives can sample: pane"],
            1
        );
    }

    /// A thin triangle whose base-level texels are all opaque, over a texture whose stored mip level 1 is half
    /// transparent there (8 x 4, row 0 opaque and the rest transparent; level 1's row 0 alpha 128): however small the
    /// triangle is at level 1, a pixel can sample that level (its screen derivatives decide), so the alpha varies and it
    /// stays a mask.
    #[test]
    fn a_coarser_level_sampled_is_judged_whatever_the_triangles_size() {
        use iridia_interface::{Filter, Filters, MipFilter};
        use primase_core::pbr::TextureRef;
        let mut w = World::new();
        let tex = w.spawn();
        let base: Vec<u8> = (0..32)
            .flat_map(|i| if i < 8 { [200, 200, 200, 255] } else { [0, 0, 0, 0] })
            .collect();
        let mip1: Vec<u8> = (0..8)
            .flat_map(|i| if i < 4 { [100, 100, 100, 128] } else { [0, 0, 0, 0] })
            .collect();
        w.insert(
            tex,
            Texture {
                width: 8,
                height: 4,
                format: primase_core::TexFormat::Rgba8Unorm,
                levels: vec![base, mip1],
                color_space: primase_core::ColorSpace::Srgb,
                role: primase_core::TexRole::Color,
                normal: None,
            },
        );
        let m = w.spawn();
        w.insert(m, Name("strip".into()));
        w.insert(
            m,
            PbrMaterial {
                alpha_mode: AlphaMode::Blend,
                base_color_texture: Some(TextureRef {
                    filters: Filters {
                        mag: Some(Filter::Nearest),
                        min: Some(Filter::Nearest),
                        mip: Some(MipFilter::Nearest),
                    },
                    ..TextureRef::new(tex)
                }),
                ..PbrMaterial::default()
            },
        );
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
                        // (0.1, 0.1), (7.1, 0.1), (0.1, 0.3) in base texels
                        Stream::F32x2(vec![[0.0125, 0.025], [0.8875, 0.025], [0.0125, 0.075]]),
                    ),
                ]),
                indices: None,
                material: Some(m),
            }]),
        );
        let mut log = PassLog::default();
        PrepareReskateBlendedSurfaces::new(0.75, BlendedSelection::All, UniformAlpha::Cutout)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        let mat = w.get::<PbrMaterial>(m).unwrap();
        assert_eq!(mat.alpha_mode, AlphaMode::Mask { cutoff: 0.75 });
        assert!(mat.base_color_texture.is_some());
        assert_eq!(
            log.counts["blended materials judged varying over the texels its primitives can sample: strip"],
            1
        );
    }

    /// With no primitive to say (no loaded mesh), every texel of every level the slot can sample is judged, and a slot
    /// stating the base level only samples no mip: a base of alpha 51 throughout over an unused level of alpha 0 is
    /// uniform, so frost applies.
    #[test]
    fn a_base_only_slot_with_no_mesh_judges_the_base_only() {
        use iridia_interface::{Filters, MipFilter};
        use primase_core::pbr::TextureRef;
        let mut w = World::new();
        let tex = w.spawn();
        w.insert(
            tex,
            Texture {
                width: 2,
                height: 2,
                format: primase_core::TexFormat::Rgba8Unorm,
                levels: vec![[200, 200, 200, 51].repeat(4), vec![0, 0, 0, 0]],
                color_space: primase_core::ColorSpace::Srgb,
                role: primase_core::TexRole::Color,
                normal: None,
            },
        );
        let m = w.spawn();
        w.insert(m, Name("veil".into()));
        w.insert(
            m,
            PbrMaterial {
                alpha_mode: AlphaMode::Blend,
                base_color_texture: Some(TextureRef {
                    filters: Filters {
                        mag: None,
                        min: None,
                        mip: Some(MipFilter::None),
                    },
                    ..TextureRef::new(tex)
                }),
                ..PbrMaterial::default()
            },
        );
        let mut log = PassLog::default();
        frost().run(&mut w, &mut log).unwrap();
        assert_eq!(w.get::<PbrMaterial>(m).unwrap().alpha_mode, AlphaMode::Opaque);
        assert_eq!(
            log.counts["blended materials judged uniform over its whole base colour texture (no primitive's \
                         coordinates to sample it by): veil"],
            1
        );
    }

    /// A thin triangle across a nearest-sampled 4 x 2 texture, (0.1, 0.1), (3.1, 0.1), (0.1, 0.2) in texels, whose
    /// end texels (0, 0) and (3, 0) are opaque and middle ones (1, 0) and (2, 0) transparent: the triangle samples the
    /// middle though it holds none of their centres, so its alpha varies and it stays a mask.
    #[test]
    fn a_thin_triangle_over_transparent_texels_stays_a_mask() {
        use iridia_interface::{Filter, Filters};
        use primase_core::pbr::TextureRef;
        let mut w = World::new();
        let tex = w.spawn();
        let level: Vec<u8> = (0..8)
            .flat_map(|i| {
                if i == 1 || i == 2 {
                    [0, 0, 0, 0]
                } else {
                    [200, 200, 200, 255]
                }
            })
            .collect();
        w.insert(
            tex,
            Texture {
                width: 4,
                height: 2,
                format: primase_core::TexFormat::Rgba8Unorm,
                levels: vec![level],
                color_space: primase_core::ColorSpace::Srgb,
                role: primase_core::TexRole::Color,
                normal: None,
            },
        );
        let m = w.spawn();
        w.insert(m, Name("pane".into()));
        w.insert(
            m,
            PbrMaterial {
                alpha_mode: AlphaMode::Blend,
                base_color_texture: Some(TextureRef {
                    filters: Filters {
                        mag: Some(Filter::Nearest),
                        min: Some(Filter::Nearest),
                        mip: None,
                    },
                    ..TextureRef::new(tex)
                }),
                ..PbrMaterial::default()
            },
        );
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
                        Stream::F32x2(vec![[0.025, 0.05], [0.775, 0.05], [0.025, 0.1]]),
                    ),
                ]),
                indices: None,
                material: Some(m),
            }]),
        );
        let mut log = PassLog::default();
        frost().run(&mut w, &mut log).unwrap();
        let mat = w.get::<PbrMaterial>(m).unwrap();
        assert_eq!(mat.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
        assert!(mat.base_color_texture.is_some());
        assert_eq!(
            log.counts["blended materials judged varying over the texels its primitives can sample: pane"],
            1
        );
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
