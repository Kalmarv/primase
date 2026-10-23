//! Emissive surfaces as point lights (pass `emissive-lights`): a source that has no lights of its own but draws lamps,
//! bulbs and signs with emissive materials gets `core.light`s where they glow, so a destination that has lights shows the
//! glow as light.
//!
//! What a surface emits is `core.pbr`'s: `emissive` (linear RGB, a factor) times the emissive texture's texel times
//! `emissive_strength`; a material with a zero factor or a zero strength emits nothing. The pass reads the placed
//! prototypes' triangle primitives of such materials, splits each into **pieces** (triangles joined by vertices at one
//! position, within `weld` metres: a bulb, a lamp's panel, a strip), and makes lights of the pieces:
//!
//! - **Colour.** The material's factor times the emissive texture at the piece's texture-coordinate centroid (nearest
//!   texel, the texture's wrap and transform applied; the texture's colour space undone), or, for a material whose name
//!   starts with a prefix of `animated`, the texture's mean (a texture the source scrolls, so any piece shows any texel
//!   in time). A piece whose colour has a luminance (Rec. 709 weights) under `min_luminance` is left out, counted: it
//!   glows no more than the floor says; so is a piece with no area (degenerate triangles) or no radiance.
//! - **Intensity.** The piece's radiance is its colour times `emissive_strength` times `nits_per_strength` (the program's
//!   statement of how bright a unit of strength is, in cd/m2); a light is a point, so its intensity is the radiance's
//!   largest channel times the piece's area times `area_factor` (the part of the surface that faces a viewer: 1 is a
//!   face seen whole), in candela, and the light's colour is the radiance over that channel (largest channel 1), the
//!   way a GTA light is a colour times an intensity. A placement's scale multiplies the area: `|det|^(2/3)`.
//! - **Which pieces, how many lights.** A piece no longer than `big` metres (its largest bounding extent) is a bulb or a
//!   fixture. A prototype holding `string` or more of them is a string: its pieces are ordered along their principal
//!   axis and divided into `per_string` consecutive groups of equal count (rounded down), and each group is one light at
//!   its pieces' mean position carrying their total intensity, in their intensity-weighted colour, so the string's
//!   power is kept by fewer lights. Fewer than `string` pieces: one light each. A longer piece (a strip, a sheet) gets
//!   one light per `strip` metres of its longest bounding axis (at least one), spaced along it, sharing its intensity.
//! - **Range.** `sqrt(intensity / e_min)` metres (the distance where a surface gets `e_min` lux), kept within
//!   `range_min` and `range_max`.
//! - **Shadows.** `shadows=on|off` states `cast_shadows` on every light made.
//!
//! Every option is required: what determines the output is the program's. More lights than `max_total` is an error
//! naming the count, never a silent drop (a destination may drop what is over its table without saying so): the program
//! thins by its options or states a higher total. A prefix in `animated` that no emissive material's name starts with
//! is counted (a program states one list for every world it converts). `animated=none` states none.
//!
//! Reports (counted): the prototypes with emissive pieces, the pieces seen, those left out for their luminance, the
//! small pieces dropped into fewer lights, the strips split, and the lights made (by placement), as an approximation: a
//! point has no size and no facing. Placements are `core.instance`s with a `core.transform`, or an entity that holds a
//! mesh and a transform and is no prototype. An emissive texture the pass cannot read (no `core.texture`, a format the
//! codec lacks, no texture-coordinate stream) is an error naming it, never a guess.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{PbrMaterial, Wrap};
use primase_core::{
    ColorSpace, Component, Entity, Instance, Light, LightKind, Mesh, Name, Semantic, Stream, Texture,
    Topology, Transform, World,
};
use primase_texcodec::{Decoder, TexDecoder};

/// Rec. 709 luminance weights.
const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// How `emissive-lights` makes lights: see the module documentation. Every value is the caller's.
#[derive(Clone, Debug, PartialEq)]
pub struct EmissivePolicy {
    weld: f64,
    big: f64,
    string: usize,
    per_string: usize,
    strip: f64,
    nits_per_strength: f64,
    area_factor: f64,
    min_luminance: f64,
    e_min: f64,
    range: [f64; 2],
    max_total: usize,
    shadows: Option<bool>,
    animated: BTreeSet<String>,
}

/// The option names, in the order the pass prints them.
const KEYS: [&str; 14] = [
    "weld",
    "big",
    "string",
    "per_string",
    "strip",
    "nits_per_strength",
    "area_factor",
    "min_luminance",
    "e_min",
    "range_min",
    "range_max",
    "max_total",
    "shadows",
    "animated",
];

impl EmissivePolicy {
    /// A policy from its values, each finite where a number: `weld`, `big`, `strip`, `nits_per_strength`, `area_factor`
    /// and `e_min` above 0, `min_luminance` not below 0, `string` at least 2, `per_string` at least 1 and below
    /// `string`, `range_min` above 0 and not above `range_max`; `shadows` is `Some(cast)` or `None` (the renderer's
    /// default); `animated` the name prefixes of materials whose texture the source scrolls (none: an empty set).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        weld: f64,
        big: f64,
        string: usize,
        per_string: usize,
        strip: f64,
        nits_per_strength: f64,
        area_factor: f64,
        min_luminance: f64,
        e_min: f64,
        range: [f64; 2],
        max_total: usize,
        shadows: Option<bool>,
        animated: BTreeSet<String>,
    ) -> Result<EmissivePolicy, String> {
        let positive = |what: &str, x: f64| {
            if x.is_finite() && x > 0.0 {
                Ok(())
            } else {
                Err(format!("emissive-lights: {what} {x} is not a number above 0"))
            }
        };
        positive("weld", weld)?;
        positive("big", big)?;
        positive("strip", strip)?;
        positive("nits_per_strength", nits_per_strength)?;
        positive("area_factor", area_factor)?;
        positive("e_min", e_min)?;
        positive("range_min", range[0])?;
        positive("range_max", range[1])?;
        if !(min_luminance.is_finite() && min_luminance >= 0.0) {
            return Err(format!(
                "emissive-lights: min_luminance {min_luminance} is not a number from 0"
            ));
        }
        if string < 2 || per_string < 1 || per_string >= string {
            return Err(format!(
                "emissive-lights: string {string} and per_string {per_string}: a string is at least 2 pieces and \
                 keeps at least 1 and fewer than it has"
            ));
        }
        if range[0] > range[1] {
            return Err(format!(
                "emissive-lights: range_min {} above range_max {}",
                range[0], range[1]
            ));
        }
        if animated.iter().any(|p| p.is_empty() || p.contains(';')) {
            return Err("emissive-lights: an animated prefix is empty or holds ';'".into());
        }
        Ok(EmissivePolicy {
            weld,
            big,
            string,
            per_string,
            strip,
            nits_per_strength,
            area_factor,
            min_luminance,
            e_min,
            range,
            max_total,
            shadows,
            animated,
        })
    }

    /// From pipeline options (every key of the module documentation required, unknown keys refused): numbers as text,
    /// `shadows=on|off`, `animated=<prefix>;<prefix>...` or `none`.
    pub fn from_options(o: &Options) -> Result<EmissivePolicy, String> {
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let text = |k: &str| o.get(k).ok_or_else(|| format!("{k} is required"));
        let number = |k: &str| -> Result<f64, String> {
            text(k)?
                .parse::<f64>()
                .map_err(|_| format!("{k}={} is not a number", o[k]))
        };
        let count = |k: &str| -> Result<usize, String> {
            text(k)?
                .parse::<usize>()
                .map_err(|_| format!("{k}={} is not a count", o[k]))
        };
        let shadows = match text("shadows")?.as_str() {
            "on" => Some(true),
            "off" => Some(false),
            v => return Err(format!("shadows={v}: on or off")),
        };
        let animated = match text("animated")?.as_str() {
            "none" => BTreeSet::new(),
            v => v.split(';').map(str::to_owned).collect(),
        };
        EmissivePolicy::new(
            number("weld")?,
            number("big")?,
            count("string")?,
            count("per_string")?,
            number("strip")?,
            number("nits_per_strength")?,
            number("area_factor")?,
            number("min_luminance")?,
            number("e_min")?,
            [number("range_min")?, number("range_max")?],
            count("max_total")?,
            shadows,
            animated,
        )
    }

    fn to_options(&self) -> Options {
        let mut o = Options::new();
        let mut put = |k: &str, v: String| {
            o.insert(k.to_owned(), v);
        };
        put("weld", self.weld.to_string());
        put("big", self.big.to_string());
        put("string", self.string.to_string());
        put("per_string", self.per_string.to_string());
        put("strip", self.strip.to_string());
        put("nits_per_strength", self.nits_per_strength.to_string());
        put("area_factor", self.area_factor.to_string());
        put("min_luminance", self.min_luminance.to_string());
        put("e_min", self.e_min.to_string());
        put("range_min", self.range[0].to_string());
        put("range_max", self.range[1].to_string());
        put("max_total", self.max_total.to_string());
        put(
            "shadows",
            match self.shadows {
                Some(true) => "on",
                _ => "off",
            }
            .to_owned(),
        );
        put(
            "animated",
            if self.animated.is_empty() {
                "none".to_owned()
            } else {
                self.animated.iter().cloned().collect::<Vec<_>>().join(";")
            },
        );
        o
    }
}

/// `emissive-lights` (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct EmissiveLights(pub EmissivePolicy);

/// [`EmissivePolicy::from_options`] as a pass factory.
pub fn emissive_lights(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(EmissiveLights(EmissivePolicy::from_options(o)?)))
}

/// One emissive piece of a prototype's mesh, in the mesh's own space.
#[derive(Clone, Debug)]
struct Piece {
    centroid: [f64; 3],
    extent: [f64; 3],
    /// The radiance's largest channel times the area times the area factor: candela at scale 1.
    intensity: f64,
    /// Radiance over its largest channel.
    colour: [f64; 3],
}

/// One light of a prototype, in the mesh's own space.
#[derive(Clone, Debug, PartialEq)]
struct Site {
    at: [f64; 3],
    intensity: f64,
    colour: [f32; 3],
}

/// What reading one prototype's mesh found.
#[derive(Debug, Default)]
struct Sites {
    sites: Vec<Site>,
    pieces: u64,
    dark: u64,
    thinned: u64,
    strips: u64,
}

/// A decoded emissive texture.
struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    srgb: bool,
    mean: [f64; 3],
}

impl Decoded {
    /// The texel at `(u, v)` as linear RGB, wrapped as `wrap` says, nearest.
    fn sample(&self, uv: [f32; 2], wrap: [Wrap; 2]) -> [f64; 3] {
        let axis = |x: f32, n: u32, w: Wrap| -> usize {
            let x = f64::from(x);
            let t = match w {
                Wrap::Repeat => x - x.floor(),
                Wrap::Clamp => x.clamp(0.0, 1.0),
                Wrap::Mirror => {
                    let m = x.rem_euclid(2.0);
                    if m > 1.0 { 2.0 - m } else { m }
                }
            };
            ((t * f64::from(n)).floor().max(0.0) as usize).min(n as usize - 1)
        };
        let (x, y) = (
            axis(uv[0], self.width, wrap[0]),
            axis(uv[1], self.height, wrap[1]),
        );
        // x < width and y < height (`axis` keeps them below n), and `decode` checked that `rgba` holds width x height texels
        let o = (y * self.width as usize + x) * 4;
        let px = &self.rgba[o..o + 4];
        std::array::from_fn(|c| self.texel(px[c]))
    }

    fn texel(&self, b: u8) -> f64 {
        if self.srgb {
            primase_image::srgb8_to_linear(b)
        } else {
            f64::from(b) / 255.0
        }
    }

    fn decode(t: &Texture, e: Entity) -> Result<Decoded, String> {
        let level = t
            .levels
            .first()
            .ok_or_else(|| format!("emissive-lights: texture {e:?} holds no level"))?;
        let rgba = Decoder::default()
            .decode_rgba8(t.format, t.width, t.height, level)
            .map_err(|err| format!("emissive-lights: texture {e:?}: {err}"))?;
        if rgba.len() != t.width as usize * t.height as usize * 4 {
            return Err(format!(
                "emissive-lights: texture {e:?}: the codec returned {} bytes for {} x {}",
                rgba.len(),
                t.width,
                t.height
            ));
        }
        let mut d = Decoded {
            width: t.width,
            height: t.height,
            rgba,
            srgb: t.color_space == ColorSpace::Srgb,
            mean: [0.0; 3],
        };
        let n = (d.rgba.len() / 4) as f64;
        let mut sum = [0.0f64; 3];
        for px in d.rgba.as_chunks::<4>().0 {
            for (s, b) in sum.iter_mut().zip(px) {
                *s += d.texel(*b);
            }
        }
        d.mean = sum.map(|s| s / n);
        Ok(d)
    }
}

fn luminance(c: [f64; 3]) -> f64 {
    (0..3).map(|k| LUMA[k] * c[k]).sum()
}

/// The principal axis of `points` (unit), by power iteration on their covariance from a fixed start, so the same points
/// give the same axis.
fn principal_axis(points: &[[f64; 3]]) -> [f64; 3] {
    let n = points.len() as f64;
    let mean: [f64; 3] = std::array::from_fn(|a| points.iter().map(|p| p[a]).sum::<f64>() / n);
    let mut cov = [[0.0f64; 3]; 3];
    for p in points {
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += (p[i] - mean[i]) * (p[j] - mean[j]);
            }
        }
    }
    let mut v = [1.0, 0.5, 0.25];
    for _ in 0..64 {
        let next: [f64; 3] = std::array::from_fn(|i| (0..3).map(|j| cov[i][j] * v[j]).sum());
        let len = next.iter().map(|x| x * x).sum::<f64>().sqrt();
        if len == 0.0 {
            break;
        }
        v = next.map(|x| x / len);
    }
    v
}

/// The material's emission as a factor and strength, `None` where it emits nothing.
fn emission(m: &PbrMaterial) -> Option<([f64; 3], f64)> {
    let factor = m.emissive.map(f64::from);
    (factor.iter().any(|&c| c > 0.0) && m.emissive_strength > 0.0)
        .then(|| (factor, f64::from(m.emissive_strength)))
}

impl EmissiveLights {
    /// The sites of `mesh` (a prototype's or a placed entity's own), `name` naming it in an error.
    fn read_mesh(
        &self,
        w: &World,
        mesh: &Mesh,
        name: &str,
        textures: &mut BTreeMap<Entity, Decoded>,
        animated_used: &mut BTreeSet<String>,
    ) -> Result<Sites, String> {
        let p = &self.0;
        let mut found = Sites::default();
        let mut pieces: Vec<Piece> = Vec::new();
        for (n, prim) in mesh.primitives().iter().enumerate() {
            let Some(material) = prim
                .material
                .and_then(|m| w.get::<PbrMaterial>(m).map(|pbr| (m, pbr)))
            else {
                continue;
            };
            let Some((factor, strength)) = emission(material.1) else {
                continue;
            };
            if prim.topology != Topology::Triangles {
                continue;
            }
            let Some(pos) = prim.positions() else { continue };
            // a material with no name matches no prefix (a prefix is never empty)
            let material_name = w
                .get::<Name>(material.0)
                .map(|n| n.0.as_str())
                .unwrap_or_default();
            let animated = p.animated.iter().find(|a| material_name.starts_with(a.as_str()));
            if let Some(a) = animated {
                animated_used.insert(a.clone());
            }
            // the emissive texture, decoded once
            let texture = match &material.1.emissive_texture {
                Some(r) => {
                    let t = w.get::<Texture>(r.texture).ok_or_else(|| {
                        format!(
                            "emissive-lights: {name}'s primitive {n}: emissive texture {:?} is no core.texture",
                            r.texture
                        )
                    })?;
                    if let std::collections::btree_map::Entry::Vacant(slot) = textures.entry(r.texture) {
                        slot.insert(Decoded::decode(t, r.texture)?);
                    }
                    let uvs = match prim.streams.get(&Semantic::TexCoord(r.texcoord)) {
                        Some(Stream::F32x2(v)) if v.len() == pos.len() => v.as_slice(),
                        Some(Stream::F32x2(v)) => {
                            return Err(format!(
                                "emissive-lights: {name}'s primitive {n}: TexCoord({}) has {} entries for {} vertices",
                                r.texcoord,
                                v.len(),
                                pos.len()
                            ));
                        }
                        _ => {
                            return Err(format!(
                                "emissive-lights: {name}'s primitive {n} has an emissive texture and no TexCoord({})",
                                r.texcoord
                            ));
                        }
                    };
                    Some((r, uvs))
                }
                None => None,
            };
            let indices: Vec<u32> = match &prim.indices {
                Some(ix) => ix.clone(),
                None => (0..pos.len() as u32).collect(),
            };
            // vertices welded by position
            let mut weld_of: Vec<usize> = Vec::with_capacity(pos.len());
            let mut keys: BTreeMap<[i64; 3], usize> = BTreeMap::new();
            for v in pos {
                let key = std::array::from_fn(|a| (f64::from(v[a]) / p.weld).round() as i64);
                let next = keys.len();
                weld_of.push(*keys.entry(key).or_insert(next));
            }
            let mut parent: Vec<usize> = (0..keys.len()).collect();
            // union-find over weld ids: every id is below `parent.len()` (they number the distinct welded positions)
            fn find(parent: &mut [usize], mut a: usize) -> usize {
                while parent[a] != a {
                    parent[a] = parent[parent[a]];
                    a = parent[a];
                }
                a
            }
            let corner = |k: u32| -> Result<usize, String> {
                if (k as usize) < pos.len() {
                    Ok(k as usize)
                } else {
                    Err(format!(
                        "emissive-lights: {name}'s primitive {n} indexes vertex {k} of {}",
                        pos.len()
                    ))
                }
            };
            // from here every vertex index is below `pos.len()` (`corner` refuses any other), and `weld_of`, `pos` and (when
            // read) `uvs` have one entry per vertex, so they are indexed directly
            let mut tris: Vec<[usize; 3]> = Vec::new();
            for c in indices.as_chunks::<3>().0 {
                let t = [corner(c[0])?, corner(c[1])?, corner(c[2])?];
                let ids = t.map(|v| weld_of[v]);
                let (a, b) = (find(&mut parent, ids[0]), find(&mut parent, ids[1]));
                parent[b] = a;
                let (a, c2) = (find(&mut parent, ids[0]), find(&mut parent, ids[2]));
                parent[c2] = a;
                tris.push(t);
            }
            let mut groups: BTreeMap<usize, Vec<[usize; 3]>> = BTreeMap::new();
            for t in tris {
                let root = find(&mut parent, weld_of[t[0]]);
                groups.entry(root).or_default().push(t);
            }
            for tris in groups.values() {
                let mut seen: BTreeSet<usize> = BTreeSet::new();
                let mut welded: BTreeSet<usize> = BTreeSet::new();
                let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                let mut sum = [0.0f64; 3];
                let mut area = 0.0f64;
                for t in tris {
                    let v: [[f64; 3]; 3] = t.map(|k| pos[k].map(f64::from));
                    let (e1, e2): ([f64; 3], [f64; 3]) = (
                        std::array::from_fn(|a| v[1][a] - v[0][a]),
                        std::array::from_fn(|a| v[2][a] - v[0][a]),
                    );
                    let cross = [
                        e1[1] * e2[2] - e1[2] * e2[1],
                        e1[2] * e2[0] - e1[0] * e2[2],
                        e1[0] * e2[1] - e1[1] * e2[0],
                    ];
                    area += 0.5 * cross.iter().map(|x| x * x).sum::<f64>().sqrt();
                    for k in t {
                        seen.insert(*k);
                        if welded.insert(weld_of[*k]) {
                            let q = pos[*k].map(f64::from);
                            for a in 0..3 {
                                lo[a] = lo[a].min(q[a]);
                                hi[a] = hi[a].max(q[a]);
                                sum[a] += q[a];
                            }
                        }
                    }
                }
                let centroid: [f64; 3] = std::array::from_fn(|a| sum[a] / welded.len() as f64);
                found.pieces += 1;
                // the piece's colour
                let mut colour = factor;
                if let Some((r, uvs)) = texture {
                    let d = textures
                        .get(&r.texture)
                        .ok_or_else(|| format!("emissive-lights: texture {:?} not decoded", r.texture))?;
                    let texel = if animated.is_some() {
                        d.mean
                    } else {
                        let mut uv = [0.0f64; 2];
                        for k in &seen {
                            let t = uvs[*k];
                            uv[0] += f64::from(t[0]);
                            uv[1] += f64::from(t[1]);
                        }
                        let at = [
                            (uv[0] / seen.len() as f64) as f32,
                            (uv[1] / seen.len() as f64) as f32,
                        ];
                        let at = r.transform.map_or(at, |t| t.apply(at));
                        d.sample(at, r.wrap)
                    };
                    colour = std::array::from_fn(|c| factor[c] * texel[c]);
                }
                if luminance(colour) < p.min_luminance {
                    found.dark += 1;
                    continue;
                }
                let radiance: [f64; 3] = colour.map(|c| c * strength * p.nits_per_strength);
                let peak = radiance.iter().copied().fold(0.0, f64::max);
                // a piece with no light to give: no radiance, or no area (degenerate triangles)
                if peak <= 0.0 || area <= 0.0 {
                    found.dark += 1;
                    continue;
                }
                pieces.push(Piece {
                    centroid,
                    extent: std::array::from_fn(|a| hi[a] - lo[a]),
                    intensity: peak * area * p.area_factor,
                    colour: radiance.map(|c| c / peak),
                });
            }
        }
        let (small, long): (Vec<Piece>, Vec<Piece>) = pieces
            .into_iter()
            .partition(|q| q.extent.iter().copied().fold(0.0, f64::max) <= p.big);
        let shown = |c: [f64; 3]| c.map(|x| x as f32);
        if small.len() >= p.string {
            let axis = principal_axis(&small.iter().map(|q| q.centroid).collect::<Vec<_>>());
            let mut order: Vec<usize> = (0..small.len()).collect();
            let along = |i: usize| -> f64 { (0..3).map(|a| small[i].centroid[a] * axis[a]).sum() };
            order.sort_by(|&a, &b| along(a).total_cmp(&along(b)).then(a.cmp(&b)));
            let n = small.len();
            for g in 0..p.per_string {
                let (from, to) = (g * n / p.per_string, (g + 1) * n / p.per_string);
                // `per_string` < `string` <= n, so every group holds at least n / per_string >= 1 pieces
                let group: Vec<&Piece> = order[from..to].iter().map(|&i| &small[i]).collect();
                let total: f64 = group.iter().map(|q| q.intensity).sum();
                let at: [f64; 3] = std::array::from_fn(|a| {
                    group.iter().map(|q| q.centroid[a]).sum::<f64>() / group.len() as f64
                });
                let mix: [f64; 3] = std::array::from_fn(|c| {
                    group.iter().map(|q| q.colour[c] * q.intensity).sum::<f64>() / total
                });
                let top = mix.iter().copied().fold(0.0, f64::max);
                found.sites.push(Site {
                    at,
                    intensity: total,
                    colour: shown(mix.map(|c| c / top)),
                });
            }
            found.thinned += (n - p.per_string) as u64;
        } else {
            for q in &small {
                found.sites.push(Site {
                    at: q.centroid,
                    intensity: q.intensity,
                    colour: shown(q.colour),
                });
            }
        }
        for q in &long {
            // the longest axis, the lowest index on a tie
            let axis = if q.extent[0] >= q.extent[1] && q.extent[0] >= q.extent[2] {
                0
            } else if q.extent[1] >= q.extent[2] {
                1
            } else {
                2
            };
            let k = ((q.extent[axis] / p.strip).ceil() as usize).max(1);
            for t in 0..k {
                let mut at = q.centroid;
                at[axis] += ((t as f64 + 0.5) / k as f64 - 0.5) * q.extent[axis];
                found.sites.push(Site {
                    at,
                    intensity: q.intensity / k as f64,
                    colour: shown(q.colour),
                });
            }
            found.strips += 1;
        }
        Ok(found)
    }
}

impl Pass for EmissiveLights {
    fn name(&self) -> &'static str {
        "emissive-lights"
    }
    fn options(&self) -> Options {
        self.0.to_options()
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[Mesh::NAME, PbrMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Light::NAME, Transform::NAME, Name::NAME]
    }
    fn approximates(&self) -> &'static str {
        "an emissive surface as point lights at the centres of its pieces: radiance times area as intensity (a point has \
         no size and no facing), a string's power in fewer lights, a strip as lights spaced along it"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let p = &self.0;
        // the placements: (the placement, the entity whose mesh it draws)
        let prototypes: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let mut placements: Vec<(Entity, Entity)> = w
            .iter::<Instance>()
            .filter(|(e, _)| w.has::<Transform>(*e))
            .map(|(e, i)| (e, i.prototype))
            .collect();
        placements.extend(
            w.iter::<Mesh>()
                .map(|(e, _)| e)
                .filter(|e| !prototypes.contains(e) && w.has::<Transform>(*e))
                .map(|e| (e, e)),
        );
        // each drawn mesh read once
        let mut textures: BTreeMap<Entity, Decoded> = BTreeMap::new();
        let mut animated_used: BTreeSet<String> = BTreeSet::new();
        let mut read: BTreeMap<Entity, Sites> = BTreeMap::new();
        for &(_, holder) in &placements {
            if read.contains_key(&holder) {
                continue;
            }
            let Some(mesh) = w.get::<Mesh>(holder) else {
                continue;
            };
            let name = w
                .get::<Name>(holder)
                .map(|n| n.0.clone())
                .unwrap_or_else(|| format!("{holder:?}"));
            let sites = self.read_mesh(w, mesh, &name, &mut textures, &mut animated_used)?;
            if sites.pieces > 0 {
                log.count("prototypes with emissive pieces", 1);
                log.count("emissive pieces seen", sites.pieces);
                log.count(
                    "pieces left out: luminance under the floor or no area",
                    sites.dark,
                );
                log.count("pieces folded into fewer lights (strings)", sites.thinned);
                log.count("strips split into lights", sites.strips);
            }
            read.insert(holder, sites);
        }
        // a prefix that no emissive material of this world starts with is counted, not an error: one program states one
        // list for every world it converts, and a world may hold none of them
        log.count(
            "animated prefixes that no emissive material's name starts with",
            p.animated.iter().filter(|a| !animated_used.contains(*a)).count() as u64,
        );
        // the lights, counted before any is made
        let total: usize = placements
            .iter()
            .map(|(_, holder)| read.get(holder).map_or(0, |s| s.sites.len()))
            .sum();
        if total > p.max_total {
            return Err(format!(
                "emissive-lights: {total} lights, over max_total {}: thin by the options (per_string, strip, \
                 min_luminance) or state a higher total; a destination may drop the lights over its table without \
                 saying so",
                p.max_total
            ));
        }
        let mut made: Vec<(Entity, Transform, Light, String)> = Vec::new();
        for (placement, holder) in &placements {
            let (Some(sites), Some(t)) = (read.get(holder), w.get::<Transform>(*placement)) else {
                continue;
            };
            let name = w.get::<Name>(*holder).map(|n| n.0.clone()).unwrap_or_default();
            let scale2 = t.determinant().abs().powf(2.0 / 3.0);
            for (k, s) in sites.sites.iter().enumerate() {
                let at = t.apply(s.at.map(|x| x as f32));
                let intensity = (s.intensity * scale2) as f32;
                let range = (f64::from(intensity) / p.e_min)
                    .sqrt()
                    .clamp(p.range[0], p.range[1]) as f32;
                if !(intensity.is_finite() && at.iter().all(|v| v.is_finite())) {
                    return Err(format!("emissive-lights: {name} light {k} is not finite"));
                }
                made.push((
                    *placement,
                    Transform {
                        cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], at],
                    },
                    Light {
                        kind: LightKind::Point,
                        color: s.colour,
                        intensity,
                        range: Some(range),
                        cast_shadows: p.shadows,
                    },
                    format!("{name} light {k}"),
                ));
            }
        }
        log.count("emissive lights made", made.len() as u64);
        log.approximate(
            "emissive surfaces as point lights (radiance times area as intensity)",
            made.len() as u64,
        );
        for (placement, t, l, n) in made {
            let e = w.spawn();
            log.place_from(e, placement);
            w.insert(e, t);
            w.insert(e, l);
            w.insert(e, Name(n));
        }
        Ok(())
    }
}
