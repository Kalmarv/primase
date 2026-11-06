//! What a FromSoft forward program does with the water under its surface, measured by running it
//! ([`crate::water_probe::WaterProbe`]) on scenes that differ in one thing each, in the terms of
//! [`primase_core::WaterBody`]: how what is behind fades with the depth of water in front of it (extinction), what the
//! water returns of its own where it is deep (scatter), how the surface weighs what passes it against the view angle
//! (its Fresnel reflectance at normal incidence, so its ior), how its opacity rises from the shore (shore fade) and which
//! of its textures move its normal, at what tiling, how fast and how strongly (waves). Each is checked against the
//! model's form on further runs: a program that does not follow it is refused with what differed
//! ([`NotMeasured`]), never fitted.
use iridia_exec::ImageSample;
use iridia_exec::probe::affine_uv;
use primase_core::pbr::UvTransform;

use crate::layer_bake::FrameSun;
use crate::water_probe::{ProbeScene, WaterProbe};

/// The distances (metres) the camera is put at above the surface, looking straight down over no depth of water, to find
/// the one every measurement but the far one is made from ([`Measured::distance`]): where what is behind passes the
/// surface the most, the nearest of equals. A program that blends a colour of its own over the surface with the
/// distance from the camera blends least there.
pub const DISTANCES: [f32; 7] = [1., 3., 10., 30., 100., 300., 1000.];
/// How far what passes the surface may differ between [`DISTANCES`] (a share of white) for it not to change with the
/// distance.
const SAME_AT_EVERY_DISTANCE: f32 = 1e-3;
/// The distance of the far measurement of what the water returns ([`Measured::far_scatter`]).
pub const FAR: f32 = 1000.;
/// The depths of water (metres) the extinction is measured over: it is taken from the deepest and checked at the others.
const DEPTHS: [f32; 3] = [1., 2., 4.];
/// How far each depth's extinction may differ from the deepest's, as a share of it, for the fade to be exponential.
const EXPONENTIAL: f32 = 0.02;
/// The extinction (per metre) below which what is behind does not fade with depth.
const CLEAR: f32 = 1e-4;
/// What every material texture reads in every run but a wave's own: a flat normal (`0.5, 0.5` decodes to no tilt) at
/// full alpha, and one half in a mask.
pub const FLAT: [f32; 4] = [0.5, 0.5, 1., 1.];
/// The tilt a wave measurement gives each lane of its texture in turn (`0.5 + TILT`): a tangent-space x or y of
/// `2 * TILT` at strength one.
const TILT: f32 = 0.05;
/// The angle (radians) a tilted texel must turn the normal by for its texture to move the normal: a strength of 1e-3.
const UNMOVED: f32 = 1e-4;
/// How far the angles a texture's two lanes turn the normal by may differ, as a share of the larger, for one strength to
/// carry both.
const LANES_EQUAL: f32 = 0.02;
/// The view elevation (radians above the plane) the wave strengths are measured at: low, where the Fresnel weight moves
/// most with the angle.
const WAVE_ELEVATION: f32 = 0.2617994; // 15 degrees
/// The view elevations (degrees) the Fresnel weight is measured at; the first is the normal incidence it is taken from.
const ELEVATIONS: [f32; 5] = [90., 60., 40., 20., 10.];
/// How far a weight may differ from Schlick's with the measured reflectance for the surface to follow it.
const SCHLICK: f32 = 0.01;

/// What the measurements give ([`measure`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// Per metre of depth along the view axis, per channel.
    pub extinction: [f32; 3],
    /// The camera's distance from the surface every measurement but the far one is made from ([`DISTANCES`]).
    pub distance: f32,
    /// What the water returns where it is infinitely deep, over the irradiance on it and the surface's transmission at
    /// normal incidence (`1 - f0`), times pi, seen straight on from [`Measured::distance`]: the body's diffuse
    /// reflectance.
    pub scatter: [f32; 3],
    /// The same seen from [`FAR`], where it differs from `scatter` (a body colour the program blends in with distance).
    pub far_scatter: Option<[f32; 3]>,
    /// The Fresnel reflectance at normal incidence.
    pub f0: f32,
    /// The largest difference of a measured weight from Schlick's with `f0`, over `ELEVATIONS`.
    pub schlick_error: f32,
    /// The water depth over which the surface's opacity rises to its deep value; zero where it does not change.
    pub shore_fade: f32,
    /// The opacity where the water is deep.
    pub deep_opacity: f32,
    /// The textures that move the surface's normal.
    pub waves: Vec<MeasuredWave>,
    /// What the runs showed the model does not carry, by reason (a count each).
    pub not_carried: Vec<String>,
    /// The frame's values the measuring runs stated rather than their scenes gave them ([`WaterProbe::stated`]), by
    /// reason: what the body was measured with that no scene of the game states.
    pub stated: std::collections::BTreeMap<String, u64>,
}

/// One texture that moves the surface's normal ([`Measured::waves`]).
#[derive(Clone, Debug, PartialEq)]
pub struct MeasuredWave {
    /// Its texture register.
    pub register: u32,
    /// The map from the program's coordinate input to the coordinate it samples the texture at, at time 0.
    pub transform: UvTransform,
    /// How far that coordinate moves per second.
    pub velocity: [f32; 2],
    /// The factor on the texture's tangent-space x and y in the surface's normal.
    pub strength: f32,
}

/// Why a program is not measured as a water surface: the same text for every program refused alike, and the values that
/// show it for this one.
#[derive(Clone, Debug, PartialEq)]
pub struct NotMeasured {
    pub reason: String,
    pub detail: String,
}

impl NotMeasured {
    fn new(reason: &str, detail: impl Into<String>) -> Self {
        NotMeasured {
            reason: reason.into(),
            detail: detail.into(),
        }
    }
}

/// Why a program is not a water surface: what is behind its surface does not fade with the depth of water.
pub const NO_EXTINCTION: &str =
    "what is behind its surface does not fade with the depth of water in front of it";

/// Why a program is not measured: a run on the probe's scene gives a value that is not a number, which the frame's values
/// the probe states (rather than the scene) can explain.
pub const NOT_A_NUMBER: &str = "its run on the probe's scene gives a value that is not a number";

impl From<String> for NotMeasured {
    fn from(detail: String) -> Self {
        NotMeasured::new("its program does not run on the probe's scene", detail)
    }
}

fn mean(c: [f32; 4]) -> f32 {
    (c[0] + c[1] + c[2]) / 3.
}

/// The colour of `scene`, every material texture flat ([`FLAT`]).
fn flat(probe: &WaterProbe, scene: &ProbeScene) -> Result<[f32; 4], String> {
    Ok(probe.pixel(scene, &mut |_, _| FLAT)?.colour)
}

/// The water surface `probe`'s program draws, measured; [`NO_EXTINCTION`] where it draws none.
pub fn measure(probe: &WaterProbe) -> Result<Measured, NotMeasured> {
    let mut not_carried = Vec::new();
    // the distance: white behind at no depth of water, seen straight down, the camera's distance swept
    let straight = |distance: f32, water: f32| ProbeScene::at(distance, std::f32::consts::FRAC_PI_2, water);
    let mut swept: Vec<(f32, [f32; 4])> = Vec::new();
    for distance in DISTANCES {
        let run = probe.pixel(&straight(distance, 0.), &mut |_, _| FLAT)?;
        if !run.colour.iter().all(|c| c.is_finite()) {
            return Err(NotMeasured::new(
                NOT_A_NUMBER,
                format!(
                    "white behind at no depth of water seen from {distance} m gives {:?}; the frame's values the probe \
                     states: {:?}",
                    run.colour, run.stated
                ),
            ));
        }
        swept.push((distance, run.colour));
    }
    let (distance, surface) = swept
        .iter()
        .copied()
        .reduce(|best, c| if mean(c.1) > mean(best.1) { c } else { best })
        .expect("distances");
    if swept
        .iter()
        .any(|(_, c)| (mean(*c) - mean(surface)).abs() > SAME_AT_EVERY_DISTANCE)
    {
        not_carried.push(DISTANCE_BODY.to_owned());
    }
    // extinction: white behind, seen straight down from that distance, the depth of water swept
    let overhead = |water: f32| straight(distance, water);
    if surface[..3].iter().any(|c| *c <= 0.) {
        return Err(NotMeasured::new(
            "what is behind its surface does not show through it",
            format!("white behind at no depth of water gives {surface:?}"),
        ));
    }
    let fades: Vec<[f32; 4]> = DEPTHS
        .iter()
        .map(|&d| flat(probe, &overhead(d)))
        .collect::<Result<_, _>>()?;
    let deepest = *DEPTHS.last().expect("depths");
    let per_metre =
        |c: [f32; 4], d: f32| -> [f32; 3] { std::array::from_fn(|k| -(c[k] / surface[k]).ln() / d) };
    let extinction = per_metre(*fades.last().expect("depths"), deepest);
    if extinction.iter().all(|s| s.abs() < CLEAR) {
        return Err(NotMeasured::new(
            NO_EXTINCTION,
            format!(
                "white behind at 0 and {deepest} m of water: {surface:?} and {:?}",
                fades.last()
            ),
        ));
    }
    for (c, &d) in fades.iter().zip(&DEPTHS) {
        let at = per_metre(*c, d);
        if (0..3).any(|k| (at[k] - extinction[k]).abs() > EXPONENTIAL * extinction[k].abs().max(CLEAR)) {
            return Err(NotMeasured::new(
                "what is behind its surface does not fade exponentially with the depth of water",
                format!("extinction {at:?} over {d} m, {extinction:?} over {deepest} m"),
            ));
        }
    }
    if extinction.iter().any(|s| !(s.is_finite() && *s >= 0.)) {
        return Err(NotMeasured::new(
            "what is behind its surface grows with the depth of water",
            format!("{extinction:?}"),
        ));
    }
    // shore fade: the opacity over the first metre of water, against the deep opacity
    let deep_opacity = fades.last().expect("depths")[3];
    let shore_fade = shore_fade(probe, &overhead, surface[3], deep_opacity, &mut not_carried)?;
    // Fresnel: white behind at no depth (nothing absorbed), the view elevation swept
    let weights: Vec<(f32, f32)> = ELEVATIONS
        .iter()
        .map(|&e| {
            let e = e.to_radians();
            flat(probe, &ProbeScene::at(distance, e, 0.)).map(|c| (e.sin(), mean(c)))
        })
        .collect::<Result<_, _>>()?;
    let f0 = 1. - weights[0].1;
    if !(0.0..1.).contains(&f0) {
        return Err(NotMeasured::new(
            "what passes its surface is not weighted by a reflectance below one",
            format!("weight {} at normal incidence", weights[0].1),
        ));
    }
    let schlick = |cos: f32| (1. - f0) * (1. - (1. - cos).powi(5));
    let schlick_error = weights
        .iter()
        .map(|&(cos, w)| (w - schlick(cos)).abs())
        .fold(0., f32::max);
    if schlick_error > SCHLICK {
        return Err(NotMeasured::new(
            "what passes its surface is not weighted by Schlick's Fresnel",
            format!("weights {weights:?} against F0 {f0}"),
        ));
    }
    // scatter: black behind and deep, a white sun overhead with no highlight, against no sun
    let scatter_at = |distance: f32| -> Result<[f32; 3], String> {
        let mut scene = ProbeScene::at(distance, std::f32::consts::FRAC_PI_2, 1e4);
        scene.behind = [0.; 3];
        let dark = flat(probe, &scene)?;
        scene.sun = Some(FrameSun {
            direction_to_light: [0., 1., 0.],
            colour: [1.; 3],
            specular_colour: [0.; 3],
        });
        let lit = flat(probe, &scene)?;
        // the irradiance on the surface is the sun's colour (it faces the sun), and what the body returns passes the
        // surface weighted by 1 - F0 (seen straight on): pi times the radiance over both is the reflectance
        Ok(std::array::from_fn(|k| {
            (lit[k] - dark[k]) * std::f32::consts::PI / (1. - f0)
        }))
    };
    let scatter = scatter_at(distance)?;
    if scatter.iter().any(|s| !(-1e-4..=1.).contains(s)) {
        return Err(NotMeasured::new(
            "what it returns where deep is not a reflectance within 0 to 1",
            format!("{scatter:?}"),
        ));
    }
    let scatter = scatter.map(|s| s.max(0.));
    let far = scatter_at(FAR)?.map(|s| s.clamp(0., 1.));
    let far_scatter = (0..3).any(|k| (far[k] - scatter[k]).abs() > 1e-3).then_some(far);
    if far_scatter.is_some() && !not_carried.iter().any(|r| r == DISTANCE_BODY) {
        not_carried.push(DISTANCE_BODY.to_owned());
    }
    let waves = waves(probe, distance, f0, &mut not_carried)?;
    Ok(Measured {
        extinction,
        scatter,
        distance,
        far_scatter,
        f0,
        schlick_error,
        shore_fade,
        deep_opacity,
        waves,
        not_carried,
        stated: probe.stated(),
    })
}

/// What a lowering counts where what the surface draws changes with the camera's distance from it (what passes it, or
/// what it returns of its own: [`Measured::far_scatter`]): a colour of its own the program blends in with the distance,
/// which the model does not carry.
pub const DISTANCE_BODY: &str = "a colour of its own blended over the surface with the distance from the camera (the \
    surface where it blends least carried)";

/// What a lowering counts where the opacity does not rise linearly from zero at the shore.
pub const SHORE_NOT_LINEAR: &str =
    "opacity at the shore not a linear rise from zero (the depth it reaches its deep value carried)";

/// The depth of water over which the opacity rises from `shore` (at no depth) to `deep`, found by halving; zero where
/// they are equal. Not a linear rise from zero is counted ([`SHORE_NOT_LINEAR`]).
fn shore_fade(
    probe: &WaterProbe,
    overhead: &dyn Fn(f32) -> ProbeScene,
    shore: f32,
    deep: f32,
    not_carried: &mut Vec<String>,
) -> Result<f32, NotMeasured> {
    if (deep - shore).abs() < 1e-4 {
        return Ok(0.);
    }
    let opacity = |d: f32| flat(probe, &overhead(d)).map(|c| c[3]);
    let (mut lo, mut hi) = (0f32, *DEPTHS.last().expect("depths"));
    if (opacity(hi)? - deep).abs() > 1e-4 {
        return Err(NotMeasured::new(
            "its opacity does not settle within the depths measured",
            format!("{} at {hi} m, {deep} deep", opacity(hi)?),
        ));
    }
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if (opacity(mid)? - deep).abs() <= 1e-4 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let fade = hi;
    let half = opacity(0.5 * fade)?;
    let linear = shore.abs() < 1e-4 && (half - 0.5 * deep).abs() <= 0.01 * deep.abs().max(1e-4);
    if !linear {
        not_carried.push(SHORE_NOT_LINEAR.to_owned());
    }
    Ok(fade)
}

/// What a lowering counts where a texture moving the normal is sampled at more than one coordinate in a run (layers of
/// one texture, a coordinate offset by another texture): the first sample's carried.
pub const WAVE_SAMPLED_TWICE: &str = "wave texture sampled at more than one coordinate (the first carried)";

/// What a lowering counts where a texture's two lanes turn the normal by different angles (`LANES_EQUAL`).
pub const WAVE_LANES_UNEQUAL: &str =
    "wave texture turning the normal by different strengths in its two lanes (the first's carried)";

/// What a lowering counts where a wave's coordinate does not move uniformly with time.
pub const WAVE_NOT_UNIFORM: &str =
    "wave coordinate not moving uniformly with time (its first second carried)";

/// The first coordinate the run sampled `register` at, and whether it sampled it at another.
fn sampled(samples: &[ImageSample<u32>], register: u32) -> Option<([f32; 2], bool)> {
    let mut at = samples
        .iter()
        .filter(|s| s.texture == register)
        .map(|s| [s.coord[0], s.coord[1]]);
    let first = at.next()?;
    Some((first, at.any(|c| c != first)))
}

/// The textures whose texels move the surface's normal: each tilted alone, seen from low over no depth of water, where
/// the weight of what passes moves with the normal's angle to the view.
fn waves(
    probe: &WaterProbe,
    distance: f32,
    f0: f32,
    not_carried: &mut Vec<String>,
) -> Result<Vec<MeasuredWave>, NotMeasured> {
    // two views from low: one along the tangent (from +x), one along the bitangent (the same turned a quarter about
    // the normal, from +z); a texel tilts the normal towards some direction in the surface's plane, which each view sees
    // the part of that lies in its own vertical plane
    let scene = ProbeScene::at(distance, WAVE_ELEVATION, 0.);
    let across = ProbeScene {
        camera: [0., scene.camera[1], scene.camera[0]],
        ..scene
    };
    let views = [scene, across];
    let levels = [mean(flat(probe, &scene)?), mean(flat(probe, &across)?)];
    // the cosine of the view to the normal a weight shows, by Schlick's with the measured reflectance
    let cosine = |w: f32| 1. - (1. - (w / (1. - f0)).clamp(0., 1.)).powf(0.2);
    // the normal's angle towards a view: the cosine to the view is sin(elevation + angle)
    let towards = |w: f32, level: f32| cosine(w).clamp(-1., 1.).asin() - cosine(level).clamp(-1., 1.).asin();
    let mut found = Vec::new();
    for &register in &probe.material {
        // the angle the texture's lane `lane` tilted by TILT turns the normal by, whatever its direction
        let tilt = |lane: usize| -> Result<f32, NotMeasured> {
            let mut texel = FLAT;
            texel[lane] += TILT;
            let mut angles = [0f32; 2];
            for (k, view) in views.iter().enumerate() {
                let w = mean(
                    probe
                        .pixel(view, &mut |r, _| if r == register { texel } else { FLAT })?
                        .colour,
                );
                angles[k] = towards(w, levels[k]);
            }
            Ok(angles[0].hypot(angles[1]))
        };
        let (x, y) = (tilt(0)?, tilt(1)?);
        if x.max(y) < UNMOVED {
            continue;
        }
        if (x - y).abs() > LANES_EQUAL * x.max(y) {
            not_carried.push(WAVE_LANES_UNEQUAL.to_owned());
        }
        let strength = x.sin() / (2. * TILT);
        let mut points = Vec::new();
        let mut twice = false;
        for uv in [[0., 0.], [1., 0.], [0., 1.], [1., 1.], [0.5, 0.25]] {
            let run = probe.pixel(&ProbeScene { uv, ..scene }, &mut |_, _| FLAT)?;
            let (coord, again) = sampled(&run.samples, register).ok_or_else(|| {
                NotMeasured::new(
                    "a wave texture is not sampled in every run",
                    format!("t{register}"),
                )
            })?;
            twice |= again;
            points.push((uv, coord));
        }
        let map = affine_uv(&points).map_err(|e| {
            NotMeasured::new(
                "a wave's coordinate is not an affine map of the input's",
                format!("t{register}: {e}"),
            )
        })?;
        let at = |time: f32| -> Result<[f32; 2], NotMeasured> {
            let run = probe.pixel(&ProbeScene { time, ..scene }, &mut |_, _| FLAT)?;
            Ok(sampled(&run.samples, register).expect("sampled at time 0").0)
        };
        let (t0, t1, t2) = (at(0.)?, at(1.)?, at(2.)?);
        let velocity = [t1[0] - t0[0], t1[1] - t0[1]];
        if (0..2).any(|k| ((t2[k] - t0[k]) - 2. * velocity[k]).abs() > 1e-4 * velocity[k].abs().max(1.)) {
            not_carried.push(WAVE_NOT_UNIFORM.to_owned());
        }
        if twice {
            not_carried.push(WAVE_SAMPLED_TWICE.to_owned());
        }
        found.push(MeasuredWave {
            register,
            transform: UvTransform {
                offset: map.offset.map(|v| v as f32),
                scale: map.scale.map(|v| v as f32),
                rotation: 0.,
            },
            velocity,
            strength,
        });
    }
    Ok(found)
}
