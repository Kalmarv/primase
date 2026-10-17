//! `lower-gta-lights{hour,x0,y0,x1,y1}`: every placement's light records ([`GtaLights`] on its prototype) as placed
//! `core.light` entities, each with its record as `gta.light` (its hours among them: a viewer with a time of day uses
//! them), every light or, given `hour`, only those on then, and, given a region, those inside it (GTA ground
//! coordinates; the light's own position decides, as the earlier exporter did). Runs on the read world after the selection (map state and hour
//! resolved, region rules applied), so each light belongs to a placement that is drawn.
//!
//! Per light: an entity with `core.transform` (the placement's transform times the light's model-space position, its
//! local -Z along the light's direction, [`Transform::facing`]) and `core.light`:
//! - kind: point; spot with cones from the record's inner and outer angles (degrees; the outer at most 89.5°);
//!   capsule as a point (its length left out: approximated, counted);
//! - colour: the sRGB bytes in linear light;
//! - intensity: candela = intensity x 25 / (4 pi^2), the earlier exporter's calibration (its light energy scale 25
//!   through the skate lights' watts policy), not measured: approximated;
//! - range: falloff times the placement's scale (at least 0.5 m);
//! - shadows unknown.
//!
//! An hour mask of zero means always on, as GTA does. A fragment's lights are placed by their model-space position:
//! its bones' poses are not applied (gta-pose-fragments' job), counted when the bone is not the root.
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, Instance, Light, LightKind, Name, Source, Transform, World};

use crate::components::{GtaLight, GtaLights};
use crate::region::GTA;

/// `lower-gta-lights` (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerGtaLights {
    /// Only the lights on at this hour; `None`: every light (each keeps its hours in its `gta.light`).
    pub hour: Option<u8>,
    /// `[x0, y0, x1, y1]`, GTA ground coordinates; `None`: every light of the placements.
    pub region: Option<[f32; 4]>,
}

impl LowerGtaLights {
    pub fn from_options(o: &Options) -> Result<LowerGtaLights, String> {
        let num = |k: &str| -> Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse().map_err(|_| format!("{k}: not a number")))
                .transpose()
        };
        for k in o.keys() {
            if !["hour", "x0", "y0", "x1", "y1"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let hour = num("hour")?;
        if hour.is_some_and(|h| !(0.0..24.0).contains(&h) || h.fract() != 0.0) {
            return Err("hour: 0-23".into());
        }
        let region = match [num("x0")?, num("y0")?, num("x1")?, num("y1")?] {
            [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
            [None, None, None, None] => None,
            _ => return Err("x0, y0, x1 and y1 go together".into()),
        };
        Ok(LowerGtaLights {
            hour: hour.map(|h| h as u8),
            region,
        })
    }
}

/// GTA's intensity to candela (see the module documentation).
const CANDELA_PER_INTENSITY: f64 = 25.0 / (4.0 * std::f64::consts::PI * std::f64::consts::PI);

fn linear(c: u8) -> f32 {
    primase_image::srgb_to_linear(f64::from(c) / 255.0) as f32
}

impl Pass for LowerGtaLights {
    fn name(&self) -> &'static str {
        "lower-gta-lights"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        if let Some(h) = self.hour {
            o.insert("hour".to_string(), h.to_string());
        }
        if let Some([x0, y0, x1, y1]) = self.region {
            for (k, v) in [("x0", x0), ("y0", y0), ("x1", x1), ("y1", y1)] {
                o.insert(k.to_string(), v.to_string());
            }
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &[
            "core.light",
            "core.transform",
            "core.name",
            "core.source",
            "gta.light",
        ]
    }
    fn approximates(&self) -> &'static str {
        "intensity in candela by the earlier exporter's calibration (x 25 / 4 pi^2); capsules as points; a fragment's \
         lights at their model-space position (bone poses not applied)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut made = Vec::new();
        for (e, i) in w.iter::<Instance>() {
            let Some(lights) = w.get::<GtaLights>(i.prototype) else {
                continue;
            };
            let t = w.get::<Transform>(e).ok_or("a placement without a transform")?;
            let scale = (0..3)
                .map(|c| {
                    t.cols[c]
                        .iter()
                        .map(|v| f64::from(*v).powi(2))
                        .sum::<f64>()
                        .sqrt()
                })
                .fold(0f64, f64::max) as f32;
            let source = w.get::<Source>(e).map(|s| s.0.clone()).unwrap_or_default();
            let name = w
                .get::<Name>(i.prototype)
                .map(|n| n.0.clone())
                .unwrap_or_default();
            for (k, l) in lights.0.iter().enumerate() {
                let hours = l.time_flags & 0x00FF_FFFF;
                if self.hour.is_some_and(|h| hours != 0 && hours & (1 << h) == 0) {
                    log.count("lights off at this hour", 1);
                    continue;
                }
                let at = t.apply(GTA.point(l.position));
                // core (x, y, z) is GTA (x, -z, y)
                let (gx, gy) = (at[0], -at[2]);
                if self
                    .region
                    .is_some_and(|[x0, y0, x1, y1]| !(gx >= x0 && gx < x1 && gy >= y0 && gy < y1))
                {
                    log.count("lights outside the region", 1);
                    continue;
                }
                let dir = {
                    let d = GTA.direction(l.direction);
                    let m: [f32; 3] = std::array::from_fn(|r| (0..3).map(|c| t.cols[c][r] * d[c]).sum());
                    m
                };
                let kind = match l.kind {
                    2 => {
                        let (a, b) = (l.cone_inner_angle, l.cone_outer_angle);
                        let outer = a.max(b).clamp(0.0, 89.5).to_radians();
                        let inner = a.min(b).clamp(0.0, 89.5).to_radians().min(outer);
                        LightKind::Spot {
                            inner_cone: inner,
                            outer_cone: outer,
                        }
                    }
                    4 => {
                        log.approximate("capsule lights as points", 1);
                        LightKind::Point
                    }
                    1 => LightKind::Point,
                    other => {
                        log.count(&format!("lights of unknown kind {other} left out"), 1);
                        continue;
                    }
                };
                if l.bone_id != 0 {
                    log.approximate("lights on a bone placed without its pose", 1);
                }
                let transform = Transform::facing(at, dir)
                    .or_else(|| Transform::facing(at, [0.0, -1.0, 0.0]))
                    .ok_or("no light basis")?;
                let light = Light {
                    kind,
                    color: l.colour.map(linear),
                    intensity: (f64::from(l.intensity.max(0.0)) * CANDELA_PER_INTENSITY) as f32,
                    range: Some((l.falloff * scale).max(0.5)),
                    cast_shadows: None,
                };
                if !light.intensity.is_finite() || !l.falloff.is_finite() || !at.iter().all(|v| v.is_finite())
                {
                    log.count("lights with values that are not finite left out", 1);
                    continue;
                }
                made.push((
                    transform,
                    light,
                    GtaLight(l.clone()),
                    format!("{name} light {k}"),
                    format!("{source}/light {k}"),
                ));
            }
        }
        log.count("lights placed", made.len() as u64);
        for (t, l, g, n, s) in made {
            let e: Entity = w.spawn();
            w.insert(e, t);
            w.insert(e, l);
            w.insert(e, g);
            w.insert(e, Name(n));
            w.insert(e, Source(s));
        }
        Ok(())
    }
}
