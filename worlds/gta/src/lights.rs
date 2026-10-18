//! `lower-gta-lights{hour,x0,y0,x1,y1}`: every placement's light records ([`GtaLights`] on its prototype) as placed
//! `core.light` entities, each with its record as `gta.light` (its hours among them: a viewer with a time of day uses
//! them), every light or, given `hour`, only those on then, and, given a region, those inside it (GTA ground
//! coordinates; the light's own position decides, as the earlier exporter did). Runs on the read world after the selection (map state and hour
//! resolved, region rules applied), so each light belongs to a placement that is drawn.
//!
//! Per light: an entity with `core.transform` (the placement's transform times the light's position, its local -Z along
//! the light's direction, [`Transform::facing`]) and `core.light`. A record's position and direction are in the space
//! of the bone its `bone_id` tags, as CodeWalker places them: they are taken into the model's space by that bone's pose
//! ([`GtaLights::bones`]), and left as stored when the drawable has no skeleton or no bone has the tag (counted).
//! Then:
//! - kind: point; spot with cones from the record's inner and outer angles (degrees; the outer at most 89.5°);
//!   capsule as a point (its length left out: approximated, counted);
//! - colour: the sRGB bytes in linear light;
//! - intensity: the record's intensity times [`CANDELA_PER_GTA_UNIT`], GTA's lights against its own sun (see there;
//!   UNVERIFIED, counted per light); a target that needs another scale applies it in its own pass;
//! - range: falloff times the placement's scale (at least 0.5 m). GTA's light ends there in a window,
//!   s / ((1 - k) s + k) on s = 1 - (d / range)^2, not inverse-square; the record's falloff and falloff exponent stay
//!   in `gta.light` (the exponent's mapping to k is not read);
//! - shadows unknown.
//!
//! An hour mask of zero means always on, as GTA does.
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, Instance, Light, LightKind, Name, Source, Transform, World};

use crate::components::{GtaLight, GtaLights};
use crate::pose::Pose;
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

/// `v` taken by `pose` (rows `[x, y, z, translation]`), with its translation when `w` is 1.
fn posed(pose: &Pose, v: [f32; 3], w: f32) -> [f32; 3] {
    pose.map(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2] + r[3] * w)
}

/// Candela per unit of GTA's light intensity. GTA's deferred point and sun lights both draw G x light x albedo x N.L
/// (its own pixel shaders, `PS_pointCM_standard` and `PS_directional_just_dir`, run on WARP), so a light compares to
/// the sun in one scale: the extrasunny noon sun (timecycle `light_dir_col` x `light_dir_mult`: luminance
/// 0.8823 x 64 = 56.47) taken as 100,000 lux makes one GTA unit 1,771 lux, a light's candela its intensity x 1,771 lux
/// at 1 m (where its window is about 1). UNVERIFIED, two assumptions: that the game writes colour x intensity into
/// the light's constants with no further factor (CodeWalker doubles it; the game's own code is not read), and the
/// 100,000 lux sun. GTA reference shots at night decide.
pub const CANDELA_PER_GTA_UNIT: f32 = 1771.0;

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
        "GTA's light intensity as candela by its sun (x 1,771: unverified, assumes colour x intensity with no factor \
         and a 100,000 lux noon sun); its windowed falloff as a range; capsules as points"
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
            for (k, (l, bone)) in lights.records.iter().zip(&lights.bones).enumerate() {
                let hours = l.time_flags & 0x00FF_FFFF;
                if self.hour.is_some_and(|h| hours != 0 && hours & (1 << h) == 0) {
                    log.count("lights off at this hour", 1);
                    continue;
                }
                // into the model's space by the bone's pose
                let (position, direction) = match bone {
                    Some(b) => {
                        log.count("lights placed by their bone's pose", 1);
                        (posed(b, l.position, 1.0), posed(b, l.direction, 0.0))
                    }
                    None => {
                        if l.bone_id != 0 {
                            log.count("lights whose bone tag names no bone: placed as stored", 1);
                        }
                        (l.position, l.direction)
                    }
                };
                let at = t.apply(GTA.point(position));
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
                    let d = GTA.direction(direction);
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
                let transform = Transform::facing(at, dir)
                    .or_else(|| Transform::facing(at, [0.0, -1.0, 0.0]))
                    .ok_or("no light basis")?;
                let light = Light {
                    kind,
                    color: l.colour.map(linear),
                    intensity: l.intensity.max(0.0) * CANDELA_PER_GTA_UNIT,
                    range: Some((l.falloff * scale).max(0.5)),
                    cast_shadows: None,
                };
                if !light.intensity.is_finite() || !l.falloff.is_finite() || !at.iter().all(|v| v.is_finite())
                {
                    log.count("lights with values that are not finite left out", 1);
                    continue;
                }
                log.approximate(
                    "light intensity as candela by GTA's noon sun (x 1,771, unverified: colour x intensity with no \
                     factor, a 100,000 lux sun)",
                    1,
                );
                log.approximate("windowed falloff as a range", 1);
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

#[cfg(test)]
mod tests {
    use primase_core::pass::{Pass, PassLog};
    use primase_core::{Instance, Light, Transform, World};

    use crate::components::GtaLights;
    use crate::drawable::LightAttr;

    /// A lamp's light 1 m above its head bone, which stands 6 m up the pole (GTA z): placed 7 m up, at core y 7, not
    /// at the pole's base; a record whose tag names no bone stays at its stored position.
    #[test]
    fn a_light_is_placed_by_the_pose_of_its_bone() {
        let mut w = World::new();
        let proto = w.spawn();
        let record = |bone_id| LightAttr {
            position: [0.0, 0.0, 1.0],
            direction: [0.0, 0.0, -1.0],
            intensity: 1.0,
            falloff: 10.0,
            kind: 1,
            bone_id,
            ..LightAttr::default()
        };
        w.insert(
            proto,
            GtaLights {
                records: vec![record(0x1234), record(0x9999)],
                bones: vec![
                    Some([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 6.0]]),
                    None,
                ],
            },
        );
        let placed = w.spawn();
        w.insert(placed, Transform::IDENTITY);
        w.insert(
            placed,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut log = PassLog::default();
        super::LowerGtaLights {
            hour: None,
            region: None,
        }
        .run(&mut w, &mut log)
        .unwrap();
        let mut heights: Vec<f32> = w
            .iter::<Light>()
            .map(|(e, _)| w.get::<Transform>(e).unwrap().cols[3][1])
            .collect();
        heights.sort_by(f32::total_cmp);
        assert_eq!(heights, vec![1.0, 7.0]);
        assert_eq!(log.counts.get("lights placed by their bone's pose"), Some(&1));
        // intensity 1 by GTA's sun, range its falloff, both approximations counted per light
        for (_, l) in w.iter::<Light>() {
            assert_eq!((l.intensity, l.range), (1771.0, Some(10.0)));
        }
        assert_eq!(log.approximations.get("windowed falloff as a range"), Some(&2));
        assert!(
            log.approximations
                .iter()
                .any(|(k, &n)| k.contains("x 1,771") && n == 2)
        );
    }
}
