//! `lower-gta-lights{hour,candela-per-unit,x0,y0,x1,y1}`: every placement's light records ([`GtaLights`] on its prototype) as
//! placed `core.light` entities, each with its record as `gta.light` (its hours among them): the required `hour` says
//! which, every light (`hour=every`), only those on at an hour (`hour=0`-`23`), or, with `hour=state` (the hour kept as
//! state), every light with its hours as a `core.condition` on the global `gta.hour` axis ([`crate::components::state::hour_axis`], made when the region has none; a light
//! on every hour, or whose mask is 0 (always on, as GTA reads it), has none), and, given a region, those inside it (GTA ground
//! coordinates; the light's own position decides, as the earlier exporter did). Runs on the read world after the selection (map state and hour
//! resolved, region rules applied), so each light belongs to a placement that is drawn. Each light is declared as its
//! placement's content ([`primase_core::pass::PassLog::place_from`]), so a conversion keeps it, with the part that owns
//! the placement, whether the pass runs in the selection or in a part.
//!
//! Per light: an entity with `core.transform` (the placement's transform times the light's position, its local -Z along
//! the light's direction, [`Transform::facing`]) and `core.light`. A record's position and direction are in the space
//! of the bone its `bone_id` tags, as CodeWalker places them: they are taken into the model's space by that bone's pose,
//! composed from the prototype's bones as stored ([`GtaSkeleton`], [`crate::rage::drawable::Bones::pose`]: a fragment's own
//! pose of the bone, else the skeleton's rest pose up to the root), and left as stored when the drawable has no
//! skeleton or no bone has the tag (counted).
//! Then:
//! - kind: point; spot with cones from the record's inner and outer angles (degrees; the outer at most 89.5°);
//!   capsule as a point (its length left out: approximated, counted);
//! - colour: the sRGB bytes in linear light;
//! - intensity: the record's intensity times the required `candela-per-unit`, the caller's calibration
//!   ([`LowerGtaLights::candela_per_unit`] gives what GTA's own sun suggests, unverified; counted per light); a target
//!   that needs another scale applies it in its own pass;
//! - range: falloff times the placement's scale (at least 0.5 m). GTA's light ends there in a window,
//!   s / ((1 - k) s + k) on s = 1 - (d / range)^2, not inverse-square; the record's falloff and falloff exponent stay
//!   in `gta.light` (the exponent's mapping to k is not read);
//! - shadows unknown.
//!
//! An hour mask of zero means always on, as GTA does.
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::state::{Clause, Condition, Literal};
use primase_core::{Entity, Instance, Light, LightKind, Name, Source, Transform, World};

use crate::components::GTA;
use crate::components::{GtaLight, GtaLights, GtaSkeleton};
use crate::passes::pose::Pose;
use crate::passes::rect::{GroundRect, Hour, X0, X1, Y0, Y1};

/// Which lights `lower-gta-lights` places, by their hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightHours {
    /// Every light, each keeping its hours in its `gta.light` (option `hour=every`).
    Every,
    /// Only the lights on at this hour (option `hour=H`).
    At(Hour),
    /// Every light with its hours as a condition on `gta.hour`, the hour kept as state (option `hour=state`).
    State,
}

/// `lower-gta-lights` (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerGtaLights {
    pub hours: LightHours,
    /// Candela per unit of GTA's light intensity (option `candela-per-unit`, required, finite and above 0): the
    /// caller's calibration, as GTA's light intensity has no stated unit.
    ///
    /// What GTA's own sun suggests, 1771, measured from the extrasunny noon timecycle and not read at run time: GTA's
    /// deferred point and sun lights both draw G x light x albedo x N.L (its own pixel shaders, `PS_pointCM_standard`
    /// and `PS_directional_just_dir`, run on WARP), so a light compares to the sun in one scale: the noon sun
    /// (timecycle `light_dir_col` x `light_dir_mult`: luminance 0.8823 x 64 = 56.47) taken as 100,000 lux makes one
    /// GTA unit 1,771 lux, a light's candela its intensity x 1,771 lux at 1 m (where its window is about 1).
    /// UNVERIFIED, two assumptions: that the game writes colour x intensity into the light's constants with no further
    /// factor (CodeWalker doubles it; the game's own code is not read), and the 100,000 lux sun. GTA reference shots at
    /// night decide.
    pub candela_per_unit: f32,
    /// The lights inside it; `None`: every light of the placements.
    pub region: Option<GroundRect>,
}

impl LowerGtaLights {
    /// From its declared options.
    fn from_given(g: &Given) -> Result<LowerGtaLights, String> {
        let hours = match g.required("hour") {
            "every" => LightHours::Every,
            "state" => LightHours::State,
            v => LightHours::At(
                Hour::parse(v).map_err(|_| format!("hour: {v} is none of 0-23, every and state"))?,
            ),
        };
        let v = g.required("candela-per-unit");
        let candela_per_unit = v
            .parse::<f32>()
            .ok()
            .filter(|c| c.is_finite() && *c > 0.0)
            .ok_or_else(|| format!("candela-per-unit: {v} is not a finite number above 0"))?;
        Ok(LowerGtaLights {
            hours,
            candela_per_unit,
            region: GroundRect::from_given(g)?,
        })
    }
}

/// `lower-gta-lights`' declaration ([`primase_core::pass::PassRegistry::add`]): which hour's lights are drawn, and
/// their calibration, are the caller's choices, no defaults.
pub const LOWER_GTA_LIGHTS: PassSpec = PassSpec {
    name: "lower-gta-lights",
    options: &[
        OptionSpec::required(
            "hour",
            "every",
            "which lights are placed: those on at an hour 0-23, every one with its hours, or every one with its hours \
             as state",
        ),
        OptionSpec::required(
            "candela-per-unit",
            "1771",
            "candela per unit of GTA's light intensity, finite and above 0: the caller's calibration",
        ),
        X0,
        Y0,
        X1,
        Y1,
    ],
    make: |g| Ok(Box::new(LowerGtaLights::from_given(g)?)),
};

/// `v` taken by `pose` (rows `[x, y, z, translation]`), with its translation when `w` is 1.
fn posed(pose: &Pose, v: [f32; 3], w: f32) -> [f32; 3] {
    pose.map(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2] + r[3] * w)
}

fn linear(c: u8) -> f32 {
    primase_image::srgb_to_linear(f64::from(c) / 255.0) as f32
}

impl Pass for LowerGtaLights {
    fn name(&self) -> &'static str {
        "lower-gta-lights"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        let hour = match self.hours {
            LightHours::Every => "every".to_string(),
            LightHours::At(h) => h.to_string(),
            LightHours::State => "state".to_string(),
        };
        o.insert("hour".to_string(), hour);
        o.insert("candela-per-unit".to_string(), self.candela_per_unit.to_string());
        if let Some(r) = self.region {
            r.insert_options(&mut o);
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
            "core.condition",
            "core.state_axis",
        ]
    }
    fn approximates(&self) -> &'static str {
        "GTA's light intensity as candela at the caller's candela-per-unit (GTA's own scale is not stated); its \
         windowed falloff as a range; capsules as points"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut made = Vec::new();
        for (e, i) in w.iter::<Instance>() {
            let Some(lights) = w.get::<GtaLights>(i.prototype) else {
                continue;
            };
            // each record's bone pose, from the prototype's bones
            let bones = w.get::<GtaSkeleton>(i.prototype).map(|s| &s.0);
            let poses = bones.map(crate::rage::drawable::Bones::pose).unwrap_or_default();
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
            for (k, l) in lights.records.iter().enumerate() {
                let bone = bones
                    .and_then(|b| b.bone_of_tag(l.bone_id))
                    .and_then(|b| poses.get(b));
                let hours = l.time_flags & 0x00FF_FFFF;
                let at = match self.hours {
                    LightHours::At(h) => Some(h.get()),
                    _ => None,
                };
                if at.is_some_and(|h| hours != 0 && hours & (1 << h) == 0) {
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
                if self.region.is_some_and(|r| !r.contains(gx, gy)) {
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
                    intensity: l.intensity.max(0.0) * self.candela_per_unit,
                    range: Some((l.falloff * scale).max(0.5)),
                    cast_shadows: None,
                };
                if !light.intensity.is_finite() || !l.falloff.is_finite() || !at.iter().all(|v| v.is_finite())
                {
                    log.count("lights with values that are not finite left out", 1);
                    continue;
                }
                log.approximate("light intensity as candela at the caller's candela-per-unit", 1);
                log.approximate("windowed falloff as a range", 1);
                made.push((
                    e,
                    transform,
                    light,
                    GtaLight(l.clone()),
                    format!("{name} light {k}"),
                    format!("{source}/light {k}"),
                    hours,
                ));
            }
        }
        log.count("lights placed", made.len() as u64);
        // the hour as state: the axis (made when the region has none) and each light's hours on it
        let axis = (self.hours == LightHours::State).then(|| crate::components::state::hour_axis(w));
        for (placement, t, l, g, n, s, hours) in made {
            let e: Entity = w.spawn();
            // the light is its placement's content: a conversion keeps it with the part that owns the placement
            log.place_from(e, placement);
            w.insert(e, t);
            w.insert(e, l);
            w.insert(e, g);
            w.insert(e, Name(n));
            w.insert(e, Source(s));
            if let Some(axis) = axis
                && hours != 0
                && hours != 0x00FF_FFFF
            {
                w.insert(
                    e,
                    Condition(vec![Clause(vec![Literal {
                        axis,
                        values: u64::from(hours),
                    }])]),
                );
                log.count("lights on some hours only: conditioned on gta.hour", 1);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use primase_core::pass::{Pass, PassLog};
    use primase_core::{Instance, Light, Transform, World};

    use crate::components::GtaLights;
    use crate::rage::drawable::LightAttr;

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
            },
        );
        // the head bone (tag 0x1234) 6 m up from the root, as stored: the pose is composed from it
        let rest = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let up = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 6.0]];
        w.insert(
            proto,
            crate::components::GtaSkeleton(crate::rage::drawable::Bones {
                skeleton: Some(crate::rage::drawable::Skeleton {
                    local: vec![rest, up],
                    parents: vec![None, Some(0)],
                    tags: Some(vec![0, 0x1234]),
                    inverse: None,
                }),
                fragment_pose: Vec::new(),
            }),
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
            hours: super::LightHours::Every,
            candela_per_unit: 1771.0,
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
        // intensity 1 at 1771 cd per unit, range its falloff, both approximations counted per light
        for (_, l) in w.iter::<Light>() {
            assert_eq!((l.intensity, l.range), (1771.0, Some(10.0)));
        }
        assert_eq!(log.approximations.get("windowed falloff as a range"), Some(&2));
        assert!(
            log.approximations
                .iter()
                .any(|(k, &n)| k.contains("candela-per-unit") && n == 2)
        );
    }

    /// The hour kept as state: a street lamp's light (hours 0xF0003F: 00-05 and 20-23) is conditioned on a new
    /// gta.hour axis; one always on (mask 0) and one on every hour are not.
    #[test]
    fn lights_keep_their_hours_as_conditions() {
        use primase_core::state::{Condition, StateAxis};
        let mut w = World::new();
        let proto = w.spawn();
        let record = |time_flags| LightAttr {
            intensity: 1.0,
            falloff: 10.0,
            kind: 1,
            time_flags,
            direction: [0.0, 0.0, -1.0],
            ..LightAttr::default()
        };
        w.insert(
            proto,
            GtaLights {
                records: vec![record(0x00F0_003F), record(0), record(0x00FF_FFFF)],
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
            hours: super::LightHours::State,
            candela_per_unit: 1771.0,
            region: None,
        }
        .run(&mut w, &mut log)
        .unwrap();
        let axes: Vec<_> = w.iter::<StateAxis>().map(|(e, a)| (e, a.key.clone())).collect();
        assert_eq!(axes.len(), 1);
        assert_eq!(axes[0].1, "gta.hour");
        let conditions: Vec<&Condition> = w.iter::<Condition>().map(|(_, c)| c).collect();
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].0[0].0[0].axis, axes[0].0);
        assert_eq!(conditions[0].0[0].0[0].values, 0x00F0_003F);
        assert_eq!(w.iter::<Light>().count(), 3);
        assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    }

    /// `hour` and `candela-per-unit` are required: the hour names the lights (every one, those on at an hour, or every
    /// one under the hour as state), the calibration their intensity. The options a pass gives read back as the same
    /// pass.
    #[test]
    fn the_hour_and_the_calibration_are_required() {
        use super::{LOWER_GTA_LIGHTS, LightHours, LowerGtaLights};
        use primase_core::pass::Options;
        let lights = |hour: Option<&str>, candela: Option<&str>| {
            let mut o = Options::new();
            if let Some(v) = hour {
                o.insert("hour".into(), v.into());
            }
            if let Some(v) = candela {
                o.insert("candela-per-unit".into(), v.into());
            }
            LOWER_GTA_LIGHTS.instance(&o)
        };
        assert!(
            lights(None, Some("1771"))
                .err()
                .unwrap()
                .starts_with("hour is required: ")
        );
        assert_eq!(
            lights(Some("noon"), Some("1771")).err().unwrap(),
            "hour: noon is none of 0-23, every and state"
        );
        assert!(lights(Some("24"), Some("1771")).is_err());
        assert!(
            lights(Some("every"), None)
                .err()
                .unwrap()
                .starts_with("candela-per-unit is required: ")
        );
        for bad in ["0", "-1", "inf", "NaN", "bright"] {
            assert_eq!(
                lights(Some("every"), Some(bad)).err().unwrap(),
                format!("candela-per-unit: {bad} is not a finite number above 0")
            );
        }
        let hour = crate::passes::rect::Hour::new(22).unwrap();
        for (v, hours) in [
            ("every", LightHours::Every),
            ("22", LightHours::At(hour)),
            ("state", LightHours::State),
        ] {
            let pass = lights(Some(v), Some("1771")).unwrap();
            let typed = LowerGtaLights {
                hours,
                candela_per_unit: 1771.0,
                region: None,
            };
            assert_eq!(pass.options(), typed.options(), "{v}");
            assert_eq!(pass.options()["candela-per-unit"], "1771", "{v}");
            assert_eq!(
                LOWER_GTA_LIGHTS.instance(&pass.options()).unwrap().options(),
                pass.options(),
                "{v}"
            );
        }
    }
}
