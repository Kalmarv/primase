//! Emissive surfaces as lights (pass `emissive-lights`): a source that has no lights of its own but draws lamps,
//! bulbs and signs with emissive materials gets `core.light`s where they glow, so a destination that has lights shows the
//! glow as light.
//!
//! What a surface emits is `core.pbr`'s: `emissive` (linear RGB, a factor) times the emissive texture's texel times
//! `emissive_strength`; a material with a zero factor or a zero strength emits nothing. The pass reads the placed
//! prototypes' triangle primitives of such materials, splits each into **pieces** (triangles joined by vertices at one
//! position, within `weld` metres: a bulb, a lamp's panel, a strip), and makes lights of the pieces:
//!
//! - **Colour.** The material's factor times the emissive texture's mean over the piece: each triangle's texel at its
//!   texture-coordinate centroid (nearest texel, the texture's wrap and transform applied; the texture's colour space
//!   undone), weighted by its area, so a piece whose coordinates cross a wrap seam takes the colour it draws with, not
//!   the texel at the mean of its coordinates; or, for a material whose name starts with a prefix of `animated`, the
//!   texture's mean (a texture the source scrolls, so any piece shows any texel in time). A piece whose colour has a
//!   luminance (Rec. 709 weights) under `min_luminance` is left out, counted: it glows no more than the floor says; so
//!   is a piece with no area (degenerate triangles) or no radiance.
//! - **Intensity.** The piece's radiance is its colour times `emissive_strength` times `nits_per_strength` (the program's
//!   statement of how bright a unit of strength is, in cd/m2); a light is a point, so its intensity is the radiance's
//!   largest channel times the piece's area times `area_factor` (the part of the surface that faces a viewer: 1 is a
//!   face seen whole), in candela, and the light's colour is the radiance over that channel (largest channel 1), the
//!   way a lamp's light is a colour times an intensity. A placement's scale multiplies the area: `|det|^(2/3)`.
//! - **Which pieces, how many lights.** A piece no longer than `big` metres (its largest bounding extent) is a bulb or a
//!   fixture. A prototype holding `string` or more of them is a string: its pieces are ordered along their principal
//!   axis and divided into `per_string` consecutive groups of equal count (rounded down), and each group is one light at
//!   its pieces' mean position carrying their total intensity, in their intensity-weighted colour, so the string's
//!   power is kept by fewer lights. Fewer than `string` pieces: one light each. A longer piece (a strip, a sheet) gets
//!   one light per `strip` metres of its longest bounding axis (at least one), spaced along it, sharing its intensity.
//! - **Size.** A light's radius is that of the disc with the area it stands for (one piece's, a string group's mean
//!   piece's, a strip's share): `sqrt(area / pi)`, times `|det|^(1/3)` of the placement.
//! - **Facing.** With `spots=FLAT,INNER,OUTER`, a light whose pieces' area-weighted normals agree (the length of their
//!   sum over their area at least `FLAT`, 1 a plane) is a spot shining along that normal (the triangles' front, by
//!   their winding), cones `INNER` and `OUTER` degrees from its axis: a lamp's panel lights what it faces, not the wall
//!   behind it or the sky above. The others (a bulb, a lantern, a curved sheet) and every light with `spots=none` are
//!   points. The on-axis intensity is the same either way.
//! - **Cones.** `cones=PREFIX:INNER,OUTER;...` gives the spots of every prototype whose name starts with a prefix (the
//!   longest that matches) that prefix's cones instead of `spots`' (degrees from the axis, as there): a program's
//!   statement that a fixture focuses its light (a floodlight's reflector), which its glowing surface does not show. Its
//!   points stay points, and the on-axis intensity is unchanged. A prefix no prototype's name starts with is counted.
//!   `cones=none` states none.
//! - **Down.** `down=PREFIX;...` makes each placement of a prototype whose name starts with a prefix one spot shining
//!   straight down (the world's -Y): its lights folded into one at their intensity-weighted mean position, with their
//!   total intensity, their intensity-weighted colour and the radius of their total area, its cones the prototype's
//!   `cones` or else `spots`'. A program's statement that a fixture with one head lights the ground under it (a street
//!   lamp whose panels face sideways under a shade), which its glowing surface does not show. A prototype it names with
//!   no cone (`spots=none` and no `cones` of its own) is an error; a prefix no prototype's name starts with is counted.
//!   `down=none` states none.
//! - **Scale.** `scale=PREFIX:FACTOR;...` multiplies the intensity of the lights of every prototype whose name starts
//!   with a prefix (the longest that matches) by its factor, before the range: a program's statement that a fixture
//!   lights more than its surface's emission says (a floodlight drawn with a sign's material). A prefix no prototype's
//!   name starts with is counted. `scale=none` states none.
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
//! small pieces dropped into fewer lights, the strips split, the spots and points made, the lights scaled, and the lights
//! made (by placement), as an approximation: a lamp's emitting surface as a sphere or a spot's disc. Placements are `core.instance`s with a `core.transform`, or an entity that holds a
//! mesh and a transform and is no prototype. An emissive texture the pass cannot read (no `core.texture`, a format the
//! codec lacks, no texture-coordinate stream) is an error naming it, never a guess.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, Wrap};
use primase_core::{
    ColorSpace, Component, Entity, Instance, Light, LightKind, Mesh, Name, Semantic, Stream, Texture,
    Topology, Transform, World,
};
use primase_image::sampling::{Address, Levels, Transfer};
use primase_texcodec::{Decoder, TexDecoder};

/// Rec. 709 luminance weights.
const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// How `emissive-lights` makes lights: see the module documentation. Every value is the caller's, checked by
/// [`EmissivePolicy::check`] when the pass is made.
#[derive(Clone, Debug, PartialEq)]
pub struct EmissivePolicy {
    /// Metres, above 0: vertices this near join triangles into one piece.
    pub weld: f64,
    /// Metres, above 0: a piece no longer is a bulb or fixture, a longer one a strip.
    pub big: f64,
    /// At least 2: a prototype with this many small pieces or more is a string.
    pub string: usize,
    /// At least 1 and below `string`: the lights a string becomes.
    pub per_string: usize,
    /// Metres, above 0: a strip gets one light per this length.
    pub strip: f64,
    /// Above 0: cd/m2 per unit of emissive strength.
    pub nits_per_strength: f64,
    /// Above 0: the part of a piece's area that faces a viewer.
    pub area_factor: f64,
    /// Not below 0: a darker piece is left out.
    pub min_luminance: f64,
    /// Lux, above 0: a light's range is where a surface gets this much.
    pub e_min: f64,
    /// Metres: the least and the most range, above 0, the least not above the most.
    pub range: [f64; 2],
    /// The most lights: more is an error.
    pub max_total: usize,
    /// `Some(cast)`, or `None`: the renderer's default.
    pub shadows: Option<bool>,
    /// Name prefixes of materials whose texture the source scrolls.
    pub animated: BTreeSet<String>,
    /// Which lights are spots, and their cones; `None`: every light a point.
    pub spots: Option<Spots>,
    /// Prototype name prefixes and the cones of their spots (each as [`Spots`]' inner and outer).
    pub cones: BTreeMap<String, Cone>,
    /// Prototype name prefixes whose placements each give one spot shining straight down.
    pub down: BTreeSet<String>,
    /// Prototype name prefixes and their intensity factors (each finite and above 0).
    pub scale: BTreeMap<String, f64>,
}

/// A spot's cones, degrees from its axis: `0 <= inner <= outer <= 90`, `outer` above 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    pub inner: f64,
    pub outer: f64,
}

impl Cone {
    fn valid(&self) -> bool {
        0.0 <= self.inner && self.inner <= self.outer && self.outer > 0.0 && self.outer <= 90.0
    }

    fn kind(&self) -> LightKind {
        LightKind::Spot {
            inner_cone: self.inner.to_radians() as f32,
            outer_cone: self.outer.to_radians() as f32,
        }
    }
}

/// `spots=FLAT,INNER,OUTER`: a light whose normals agree by at least `flat` (above 0, at most 1) is a spot with cones
/// `inner` and `outer` degrees from its axis (0 <= inner <= outer <= 90, outer above 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spots {
    pub flat: f64,
    pub inner: f64,
    pub outer: f64,
}

impl EmissivePolicy {
    /// Each value within what its field states.
    pub fn check(&self) -> Result<(), String> {
        let positive = |what: &str, x: f64| {
            if x.is_finite() && x > 0.0 {
                Ok(())
            } else {
                Err(format!("emissive-lights: {what} {x} is not a number above 0"))
            }
        };
        positive("weld", self.weld)?;
        positive("big", self.big)?;
        positive("strip", self.strip)?;
        positive("nits_per_strength", self.nits_per_strength)?;
        positive("area_factor", self.area_factor)?;
        positive("e_min", self.e_min)?;
        positive("range_min", self.range[0])?;
        positive("range_max", self.range[1])?;
        if !(self.min_luminance.is_finite() && self.min_luminance >= 0.0) {
            return Err(format!(
                "emissive-lights: min_luminance {} is not a number from 0",
                self.min_luminance
            ));
        }
        if self.string < 2 || self.per_string < 1 || self.per_string >= self.string {
            return Err(format!(
                "emissive-lights: string {} and per_string {}: a string is at least 2 pieces and keeps at least 1 and \
                 fewer than it has",
                self.string, self.per_string
            ));
        }
        if self.range[0] > self.range[1] {
            return Err(format!(
                "emissive-lights: range_min {} above range_max {}",
                self.range[0], self.range[1]
            ));
        }
        if self.animated.iter().any(|p| p.is_empty() || p.contains(';')) {
            return Err("emissive-lights: an animated prefix is empty or holds ';'".into());
        }
        if let Some(Spots { flat, inner, outer }) = self.spots
            && !(flat > 0.0 && flat <= 1.0 && Cone { inner, outer }.valid())
        {
            return Err(format!(
                "emissive-lights: spots={flat},{inner},{outer}: flat above 0 and at most 1, 0 <= inner <= outer <= 90 \
                 degrees, outer above 0"
            ));
        }
        for (prefix, factor) in &self.scale {
            if prefix.is_empty() || prefix.contains([';', ':']) {
                return Err(format!(
                    "emissive-lights: a scale prefix {prefix:?} is empty or holds ';' or ':'"
                ));
            }
            positive(&format!("scale {prefix}"), *factor)?;
        }
        if self.down.iter().any(|p| p.is_empty() || p.contains(';')) {
            return Err("emissive-lights: a down prefix is empty or holds ';'".into());
        }
        for (prefix, cone) in &self.cones {
            if prefix.is_empty() || prefix.contains([';', ':']) {
                return Err(format!(
                    "emissive-lights: a cones prefix {prefix:?} is empty or holds ';' or ':'"
                ));
            }
            if !cone.valid() {
                return Err(format!(
                    "emissive-lights: cones {prefix}:{},{}: 0 <= inner <= outer <= 90 degrees, outer above 0",
                    cone.inner, cone.outer
                ));
            }
        }
        Ok(())
    }

    /// From the options [`EMISSIVE_LIGHTS`] declares: numbers as text, `animated=<prefix>;<prefix>...` or `none`.
    fn from_given(g: &Given) -> Result<EmissivePolicy, String> {
        let number = |k: &str| -> Result<f64, String> {
            let v = g.required(k);
            v.parse::<f64>().map_err(|_| format!("{k}={v} is not a number"))
        };
        let count = |k: &str| -> Result<usize, String> {
            let v = g.required(k);
            v.parse::<usize>().map_err(|_| format!("{k}={v} is not a count"))
        };
        let shadows = Some(g.choice::<Shadows>("shadows") == Shadows::On);
        let animated = match g.required("animated") {
            "none" => BTreeSet::new(),
            v => v.split(';').map(str::to_owned).collect(),
        };
        let spots = match g.required("spots") {
            "none" => None,
            v => {
                let n: Vec<f64> = v
                    .split(',')
                    .map(|x| {
                        x.parse::<f64>()
                            .map_err(|_| format!("spots={v}: FLAT,INNER,OUTER or none"))
                    })
                    .collect::<Result<_, _>>()?;
                let [flat, inner, outer] = n[..] else {
                    return Err(format!("spots={v}: FLAT,INNER,OUTER or none"));
                };
                Some(Spots { flat, inner, outer })
            }
        };
        let scale = match g.required("scale") {
            "none" => BTreeMap::new(),
            v => v
                .split(';')
                .map(|item| {
                    let (prefix, factor) = item
                        .split_once(':')
                        .ok_or_else(|| format!("scale={v}: PREFIX:FACTOR;... or none"))?;
                    let factor = factor
                        .parse::<f64>()
                        .map_err(|_| format!("scale={v}: {factor} is not a number"))?;
                    Ok((prefix.to_owned(), factor))
                })
                .collect::<Result<_, String>>()?,
        };
        let cones = match g.required("cones") {
            "none" => BTreeMap::new(),
            v => v
                .split(';')
                .map(|item| {
                    let bad = || format!("cones={v}: PREFIX:INNER,OUTER;... or none");
                    let (prefix, angles) = item.split_once(':').ok_or_else(bad)?;
                    let (inner, outer) = angles.split_once(',').ok_or_else(bad)?;
                    let inner = inner.parse::<f64>().map_err(|_| bad())?;
                    let outer = outer.parse::<f64>().map_err(|_| bad())?;
                    Ok((prefix.to_owned(), Cone { inner, outer }))
                })
                .collect::<Result<_, String>>()?,
        };
        let down = match g.required("down") {
            "none" => BTreeSet::new(),
            v => v.split(';').map(str::to_owned).collect(),
        };
        let p = EmissivePolicy {
            weld: number("weld")?,
            big: number("big")?,
            string: count("string")?,
            per_string: count("per_string")?,
            strip: number("strip")?,
            nits_per_strength: number("nits_per_strength")?,
            area_factor: number("area_factor")?,
            min_luminance: number("min_luminance")?,
            e_min: number("e_min")?,
            range: [number("range_min")?, number("range_max")?],
            max_total: count("max_total")?,
            shadows,
            animated,
            spots,
            cones,
            down,
            scale,
        };
        p.check()?;
        Ok(p)
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
            if self.shadows == Some(true) {
                Shadows::On
            } else {
                Shadows::Off
            }
            .text()
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
        put(
            "spots",
            self.spots.map_or("none".to_owned(), |s| {
                format!("{},{},{}", s.flat, s.inner, s.outer)
            }),
        );
        put(
            "cones",
            if self.cones.is_empty() {
                "none".to_owned()
            } else {
                self.cones
                    .iter()
                    .map(|(k, c)| format!("{k}:{},{}", c.inner, c.outer))
                    .collect::<Vec<_>>()
                    .join(";")
            },
        );
        put(
            "down",
            if self.down.is_empty() {
                "none".to_owned()
            } else {
                self.down.iter().cloned().collect::<Vec<_>>().join(";")
            },
        );
        put(
            "scale",
            if self.scale.is_empty() {
                "none".to_owned()
            } else {
                self.scale
                    .iter()
                    .map(|(k, v)| format!("{k}:{v}"))
                    .collect::<Vec<_>>()
                    .join(";")
            },
        );
        o
    }
}

/// `emissive-lights` (see the module documentation). Made by [`EmissiveLights::new`].
#[derive(Clone, Debug, PartialEq)]
pub struct EmissiveLights(EmissivePolicy);

impl EmissiveLights {
    /// The pass with policy `p`, checked ([`EmissivePolicy::check`]).
    pub fn new(p: EmissivePolicy) -> Result<EmissiveLights, String> {
        p.check()?;
        Ok(EmissiveLights(p))
    }

    pub fn policy(&self) -> &EmissivePolicy {
        &self.0
    }
}

/// Whether every light made casts shadows (option `shadows`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shadows {
    Off,
    On,
}

impl Choice for Shadows {
    const VALUES: &'static [(&'static str, Self)] = &[("off", Shadows::Off), ("on", Shadows::On)];
}

/// `emissive-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): what the emission becomes, how a string
/// is thinned and what is too many are the program's, every option required (see the module documentation).
pub const EMISSIVE_LIGHTS: PassSpec = PassSpec {
    name: "emissive-lights",
    options: &[
        OptionSpec::required(
            "weld",
            "0.001",
            "metres, above 0: vertices this near join triangles into one piece",
        ),
        OptionSpec::required(
            "big",
            "2.5",
            "metres, above 0: a piece no longer is a bulb or fixture, a longer one a strip",
        ),
        OptionSpec::required(
            "string",
            "8",
            "at least 2: a prototype with this many small pieces or more is a string",
        ),
        OptionSpec::required(
            "per_string",
            "6",
            "at least 1 and below string: the lights a string becomes",
        ),
        OptionSpec::required(
            "strip",
            "8",
            "metres, above 0: a strip gets one light per this length",
        ),
        OptionSpec::required(
            "nits_per_strength",
            "558.5",
            "cd/m2 per unit of emissive strength, above 0: how bright the program states a unit is",
        ),
        OptionSpec::required(
            "area_factor",
            "1",
            "above 0: the part of a piece's area that faces a viewer (1: a face seen whole)",
        ),
        OptionSpec::required(
            "min_luminance",
            "0.05",
            "not below 0: a piece whose colour's luminance is under it is left out, counted",
        ),
        OptionSpec::required(
            "e_min",
            "2",
            "lux, above 0: a light's range is where a surface gets this much",
        ),
        OptionSpec::required("range_min", "3", "metres, above 0: the least range"),
        OptionSpec::required("range_max", "30", "metres, not below range_min: the most range"),
        OptionSpec::required(
            "max_total",
            "3500",
            "the most lights: more is an error naming the count",
        ),
        OptionSpec::choice::<Shadows>("shadows", "whether every light made casts shadows"),
        OptionSpec::required(
            "animated",
            "none",
            "name prefixes of materials whose texture the source scrolls, `;` between, or none",
        ),
        OptionSpec::required(
            "spots",
            "none",
            "FLAT,INNER,OUTER (normals agreeing by at least FLAT, cones in degrees): spots along a flat piece's \
             normal; or none: every light a point",
        ),
        OptionSpec::required(
            "cones",
            "none",
            "PREFIX:INNER,OUTER;...: the spots of prototypes whose names start with a prefix get these cones (degrees \
             from the axis) instead of spots'; or none",
        ),
        OptionSpec::required(
            "down",
            "none",
            "PREFIX;...: each placement of a prototype whose name starts with a prefix gives one spot shining straight              down (its lights folded into one, its cones the prototype's cones or else spots'); or none",
        ),
        OptionSpec::required(
            "scale",
            "none",
            "PREFIX:FACTOR;...: intensity factors for prototypes whose names start with a prefix; or none",
        ),
    ],
    make: |g| Ok(Box::new(EmissiveLights(EmissivePolicy::from_given(g)?))),
};

/// One emissive piece of a prototype's mesh, in the mesh's own space.
#[derive(Clone, Debug)]
struct Piece {
    centroid: [f64; 3],
    extent: [f64; 3],
    area: f64,
    /// The sum of its triangles' area vectors (normal times area, by their winding).
    normal: [f64; 3],
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
    /// The disc radius of the area it stands for, mesh units.
    radius: f64,
    /// The unit normal it shines along, where its pieces are flat enough to be a spot.
    facing: Option<[f64; 3]>,
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

/// `sites` (at least one) as one light: at their intensity-weighted mean position, with their total intensity, their
/// intensity-weighted colour (largest channel 1) and the radius of a disc of their total area; no facing (the caller
/// aims it).
fn fold(sites: &[Site]) -> Site {
    let total: f64 = sites.iter().map(|s| s.intensity).sum();
    let weight = |s: &Site| {
        if total > 0.0 {
            s.intensity / total
        } else {
            1.0 / sites.len() as f64
        }
    };
    let at = std::array::from_fn(|a| sites.iter().map(|s| weight(s) * s.at[a]).sum());
    let mix: [f64; 3] =
        std::array::from_fn(|c| sites.iter().map(|s| weight(s) * f64::from(s.colour[c])).sum());
    let top = mix.iter().copied().fold(0.0, f64::max);
    Site {
        at,
        intensity: total,
        colour: mix.map(|c| if top > 0.0 { (c / top) as f32 } else { 0.0 }),
        radius: sites.iter().map(|s| s.radius * s.radius).sum::<f64>().sqrt(),
        facing: None,
    }
}

/// A decoded emissive texture: its base level, sampled at the texel a coordinate falls in, and its mean.
struct Decoded {
    levels: Levels,
    transfer: Transfer,
    mean: [f64; 3],
}

impl Decoded {
    /// The texel `uv` falls in as linear RGB, addressed as `wrap` says ([`Levels::point`]); a coordinate that isn't
    /// finite is refused.
    fn sample(&self, uv: [f32; 2], wrap: [Wrap; 2]) -> Result<[f64; 3], String> {
        let address = wrap.map(|w| match w {
            Wrap::Repeat => Address::Wrap,
            Wrap::Clamp => Address::Clamp,
            Wrap::Mirror => Address::Mirror,
        });
        let v = self
            .levels
            .point(0, uv.map(f64::from), address, self.transfer)
            .map_err(|err| format!("emissive-lights: sampling at {uv:?}: {err}"))?;
        Ok([v[0], v[1], v[2]])
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
        let transfer = match t.color_space {
            ColorSpace::Srgb => Transfer::SrgbColour,
            ColorSpace::Linear => Transfer::Stored,
        };
        let n = (rgba.len() / 4) as f64;
        let mut sum = [0.0f64; 3];
        for px in rgba.as_chunks::<4>().0 {
            for (c, s) in sum.iter_mut().enumerate() {
                *s += transfer.channel(c, px[c]);
            }
        }
        let levels = Levels::one(t.width, t.height, rgba)
            .map_err(|err| format!("emissive-lights: texture {e:?}: {err}"))?;
        Ok(Decoded {
            levels,
            transfer,
            mean: sum.map(|s| s / n),
        })
    }
}

fn luminance(c: [f64; 3]) -> f64 {
    (0..3).map(|k| LUMA[k] * c[k]).sum()
}

/// The principal axis of `points` (unit), by power iteration on their covariance from a fixed start, so the same points
/// give the same axis.
/// The direction normal `n` (mesh space) faces under the placement `t`: its linear part's inverse transpose times `n`
/// (`(n.x c1 x c2 + n.y c2 x c0 + n.z c0 x c1) / det` over the columns `c`), so a scaled placement keeps it normal to
/// the surface and a mirroring one keeps it on the front side, which a renderer keeps by flipping the winding. Not
/// unit length; zero for a singular placement (no direction: the light stays a point).
fn normal_through(t: &Transform, n: [f64; 3]) -> [f64; 3] {
    let c = [0, 1, 2].map(|i| t.cols[i].map(f64::from));
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let (a, b, d) = (cross(c[1], c[2]), cross(c[2], c[0]), cross(c[0], c[1]));
    let det = t.determinant();
    if det == 0.0 {
        return [0.0; 3];
    }
    std::array::from_fn(|i| (n[0] * a[i] + n[1] * b[i] + n[2] * d[i]) / det)
}

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
                let mut welded: BTreeSet<usize> = BTreeSet::new();
                let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                let mut sum = [0.0f64; 3];
                let mut area = 0.0f64;
                let mut normal = [0.0f64; 3];
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
                    for a in 0..3 {
                        normal[a] += 0.5 * cross[a];
                    }
                    for k in t {
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
                        // each triangle's texel at its own coordinates' centroid, weighted by its area: a piece's
                        // coordinates may cross a wrap seam (u 0.09 and 1.09 on one welded piece), where their mean
                        // falls on another part of the texture
                        let mut sum = [0.0f64; 3];
                        let mut weight = 0.0f64;
                        for t in tris {
                            let v: [[f64; 3]; 3] = t.map(|k| pos[k].map(f64::from));
                            let (e1, e2): ([f64; 3], [f64; 3]) = (
                                std::array::from_fn(|a| v[1][a] - v[0][a]),
                                std::array::from_fn(|a| v[2][a] - v[0][a]),
                            );
                            let a = 0.5
                                * [
                                    e1[1] * e2[2] - e1[2] * e2[1],
                                    e1[2] * e2[0] - e1[0] * e2[2],
                                    e1[0] * e2[1] - e1[1] * e2[0],
                                ]
                                .iter()
                                .map(|x| x * x)
                                .sum::<f64>()
                                .sqrt();
                            if a <= 0.0 {
                                continue;
                            }
                            let at: [f32; 2] = std::array::from_fn(|c| {
                                (t.iter().map(|&k| f64::from(uvs[k][c])).sum::<f64>() / 3.0) as f32
                            });
                            let at = r.transform.map_or(at, |t| t.apply(at));
                            let texel = d.sample(at, r.wrap)?;
                            for c in 0..3 {
                                sum[c] += a * texel[c];
                            }
                            weight += a;
                        }
                        // a piece with no area is left out below; its colour is not read
                        if weight > 0.0 {
                            sum.map(|s| s / weight)
                        } else {
                            [0.0; 3]
                        }
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
                    area,
                    normal,
                    intensity: peak * area * p.area_factor,
                    colour: radiance.map(|c| c / peak),
                });
            }
        }
        let (small, long): (Vec<Piece>, Vec<Piece>) = pieces
            .into_iter()
            .partition(|q| q.extent.iter().copied().fold(0.0, f64::max) <= p.big);
        let shown = |c: [f64; 3]| c.map(|x| x as f32);
        // the radius of a disc of `area`; the direction a light of these pieces shines along, where they are flat
        let disc = |area: f64| (area / std::f64::consts::PI).sqrt();
        let facing = |pieces: &[&Piece]| -> Option<[f64; 3]> {
            let spots = p.spots?;
            let area: f64 = pieces.iter().map(|q| q.area).sum();
            let n: [f64; 3] = std::array::from_fn(|a| pieces.iter().map(|q| q.normal[a]).sum::<f64>());
            let len = n.iter().map(|x| x * x).sum::<f64>().sqrt();
            (area > 0.0 && len >= spots.flat * area).then(|| n.map(|x| x / len))
        };
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
                let area: f64 = group.iter().map(|q| q.area).sum();
                found.sites.push(Site {
                    at,
                    intensity: total,
                    colour: shown(mix.map(|c| c / top)),
                    radius: disc(area / group.len() as f64),
                    facing: facing(&group),
                });
            }
            found.thinned += (n - p.per_string) as u64;
        } else {
            for q in &small {
                found.sites.push(Site {
                    at: q.centroid,
                    intensity: q.intensity,
                    colour: shown(q.colour),
                    radius: disc(q.area),
                    facing: facing(&[q]),
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
                    radius: disc(q.area / k as f64),
                    facing: facing(&[q]),
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
        let mut down_used: BTreeSet<&str> = BTreeSet::new();
        let mut downward: BTreeSet<Entity> = BTreeSet::new();
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
            let mut sites = self.read_mesh(w, mesh, &name, &mut textures, &mut animated_used)?;
            // a fixture `down` names: its lights as one, aimed below
            if let Some(prefix) = p.down.iter().find(|d| name.starts_with(d.as_str()))
                && !sites.sites.is_empty()
            {
                down_used.insert(prefix.as_str());
                log.count(
                    "lights folded into one spot aimed down (down)",
                    sites.sites.len() as u64 - 1,
                );
                sites.sites = vec![fold(&sites.sites)];
                downward.insert(holder);
            }
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
        let spot_kind = p.spots.map(|s| LightKind::Spot {
            inner_cone: s.inner.to_radians() as f32,
            outer_cone: s.outer.to_radians() as f32,
        });
        let mut scale_used: BTreeSet<&str> = BTreeSet::new();
        let mut cones_used: BTreeSet<&str> = BTreeSet::new();
        let mut made: Vec<(Entity, Transform, Light, String)> = Vec::new();
        for (placement, holder) in &placements {
            let (Some(sites), Some(t)) = (read.get(holder), w.get::<Transform>(*placement)) else {
                continue;
            };
            let name = w.get::<Name>(*holder).map(|n| n.0.clone()).unwrap_or_default();
            // the longest prefix of `scale` the prototype's name starts with
            let factor = p
                .scale
                .iter()
                .filter(|(prefix, _)| name.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map_or(1.0, |(prefix, factor)| {
                    scale_used.insert(prefix.as_str());
                    *factor
                });
            // the longest prefix of `cones` the prototype's name starts with
            let cone = p
                .cones
                .iter()
                .filter(|(prefix, _)| name.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(prefix, cone)| {
                    cones_used.insert(prefix.as_str());
                    cone.kind()
                });
            let det = t.determinant();
            let scale2 = det.abs().powf(2.0 / 3.0);
            let scale1 = det.abs().cbrt();
            for (k, s) in sites.sites.iter().enumerate() {
                let at = t.apply(s.at.map(|x| x as f32));
                let intensity = (s.intensity * scale2 * factor) as f32;
                let range = (f64::from(intensity) / p.e_min)
                    .sqrt()
                    .clamp(p.range[0], p.range[1]) as f32;
                if !(intensity.is_finite() && at.iter().all(|v| v.is_finite())) {
                    return Err(format!("emissive-lights: {name} light {k} is not finite"));
                }
                // a fixture `down` names shines straight down, with its prototype's cones or else spots'; any other
                // spot along its pieces' normal as the placement turns it ([`normal_through`])
                let aimed = if downward.contains(holder) {
                    let kind = cone.or(spot_kind).ok_or_else(|| {
                        format!(
                            "emissive-lights: down names {name}, which has no cone: state spots or a cones prefix for it"
                        )
                    })?;
                    log.count("spots aimed straight down (down)", 1);
                    Transform::facing(at, [0.0, -1.0, 0.0]).map(|f| (f, kind))
                } else {
                    match (s.facing, spot_kind) {
                        (Some(n), Some(kind)) => {
                            let d = normal_through(t, n);
                            Transform::facing(at, d.map(|x| x as f32)).map(|f| (f, cone.unwrap_or(kind)))
                        }
                        _ => None,
                    }
                };
                if aimed.is_some() && cone.is_some() {
                    log.count("spots given their prototype's cones", 1);
                }
                if factor != 1.0 {
                    log.count("lights scaled by their prototype's factor", 1);
                }
                let (transform, kind) = match aimed {
                    Some(aimed) => {
                        log.count("spots along their pieces' normal", 1);
                        aimed
                    }
                    None => {
                        log.count("points", 1);
                        (
                            Transform {
                                cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], at],
                            },
                            LightKind::Point,
                        )
                    }
                };
                made.push((
                    *placement,
                    transform,
                    Light {
                        kind,
                        color: s.colour,
                        intensity,
                        range: Some(range),
                        cast_shadows: p.shadows,
                        radius: Some((s.radius * scale1) as f32),
                    },
                    format!("{name} light {k}"),
                ));
            }
        }
        log.count(
            "scale prefixes that no prototype's name starts with",
            p.scale
                .keys()
                .filter(|k| !scale_used.contains(k.as_str()))
                .count() as u64,
        );
        log.count(
            "cones prefixes that no prototype's name starts with",
            p.cones
                .keys()
                .filter(|k| !cones_used.contains(k.as_str()))
                .count() as u64,
        );
        log.count(
            "down prefixes that no prototype's name starts with",
            p.down.iter().filter(|k| !down_used.contains(k.as_str())).count() as u64,
        );
        log.count("emissive lights made", made.len() as u64);
        log.approximate(
            "emissive surfaces as lights (radiance times area as intensity; the surface as a sphere, or a disc's spot)",
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
