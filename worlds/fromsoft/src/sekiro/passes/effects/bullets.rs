//! A bullet about to fly (`fromsoft.bullet`) as `core.particles`: one emitter that flies as the bullet's `BulletParam` row
//! says and starts the bullet's own effect (`sfxId_Bullet`) on its frame, so the effect is carried along the flight.
//!
//! The row's law, as Paramdex describes its fields: the bullet starts at `initVellocity` along its frame's forward, does
//! not accelerate before `accelTime`, then accelerates at `accelInRange` until its travel reaches `dist` and at
//! `accelOutRange` after, its speed held between `minVellocity` and `maxVellocity`; gravity pulls it down at
//! `gravityInRange`, then at `gravityOutRange`; it lives `life` seconds (-1 for ever). An id of -1 names nothing
//! (Paramdex), any other names an effect or a bullet. The lowering reads further, each counted where a row needs it: a
//! `maxVellocity` of 0 as no limit (of the 15,475 rows of the 1.17.3 regulation, 8 start faster than a max of 0 and 12
//! accelerate under one), a `minVellocity` above the max as the max not applied, a `dist` of 0 as the out-of-range law
//! from the start, the range as reached by the forward travel alone (the drop gravity adds is left out), found by
//! stepping the in-range law, and the bullet facing its motion (the pass's choice: the row states no facing). Not
//! carried, counted: homing (`homingAngle` or `homingAngleX` above 0: a lowered world has no target, so the bullet
//! flies straight; the angles are what is tested, as they are the turn a second, so a row that homes automatically
//! (`isEnableAutoHoming`) with neither angle above 0 is not counted), the random range of the life, a follow type other than 0, the hit and deflection effects (nothing
//! is hit in a lowered world), and the bullets it makes on hitting and at intervals.
use super::lower::{blank, end_time};
use super::units;
use fromsoft_formats::param::Bullet;
use primase_core::Entity;
use primase_core::particles::{
    Argument, Curve, Interpolation, Key, NodeMotion, ParticleSystem, Termination, Value,
};
use primase_core::pass::PassLog;

/// The time step of the walk that finds when a bullet's travel reaches its range (seconds).
const RANGE_STEP: f64 = 1e-4;
/// The longest the walk runs for a bullet that lives for ever (seconds).
const RANGE_HORIZON: f64 = 600.0;

pub const MAX_OF_ZERO: &str =
    "bullets whose max speed of 0 is read as no limit while they fly faster or accelerate";
pub const MIN_ABOVE_MAX: &str = "bullets whose min speed is above their max speed: the max not applied";
pub const NO_RANGE: &str = "bullets with no range (dist 0) whose laws in and out of range differ: the out-of-range law from the start";
pub const RANGE_ALONG_FORWARD: &str =
    "bullets' ranges reached by their forward travel alone (the drop gravity adds left out)";
pub const LIFE_RANDOM: &str = "bullets' random life ranges not drawn (they live their life)";
pub const FOLLOWING: &str = "bullets with a follow type other than 0 (not read) fly free of their source";
pub const HIT_EFFECT: &str = "bullets' hit effects not started (nothing is hit)";
pub const FLICK_EFFECT: &str = "bullets' deflection effects not started (nothing deflects them)";
pub const SPAWNED: &str = "bullets made on hitting not made (nothing is hit)";
pub const INTERVAL: &str = "bullets made at intervals not made";
pub const NO_EFFECT: &str = "bullets whose effect the read did not keep fly unseen";
pub const FACING: &str = "bullets drawn facing their motion (the row states no facing)";
pub const HOMING: &str =
    "bullets that home (homingAngle above 0) fly straight: a lowered world has no target";

/// Whether an id of a row names an effect or a bullet: -1 is none.
fn names(id: i32) -> bool {
    id != -1
}

/// The system of a bullet of row `row` whose own effect is the system `effect` (`None`: it has none, or the read did not
/// keep it), seeded `seed`; what it cannot carry is counted in `log` and kept in the system's `lost`.
pub(super) fn flight(row: &Bullet, effect: Option<Entity>, seed: u64, log: &mut PassLog) -> ParticleSystem {
    let mut lost: Vec<(String, u32)> = Vec::new();
    let mut lose = |log: &mut PassLog, reason: &str| {
        log.omit(reason, 1);
        lost.push((reason.to_owned(), 1));
    };
    let life = (row.life >= 0.0).then_some(row.life);
    let mut max_speed = (row.max_speed > 0.0).then_some(row.max_speed);
    if max_speed.is_none()
        && (row.initial_speed > 0.0 || row.acceleration_in_range > 0.0 || row.acceleration_out_of_range > 0.0)
    {
        log.approximate(MAX_OF_ZERO, 1);
    }
    let min_speed = Some(row.min_speed);
    if max_speed.is_some_and(|max| row.min_speed > max) {
        log.approximate(MIN_ABOVE_MAX, 1);
        max_speed = None;
    }
    // when the in-range law gives way to the out-of-range law
    let differs = row.acceleration_in_range != row.acceleration_out_of_range
        || row.gravity_in_range != row.gravity_out_of_range;
    let switch = match differs {
        false => None,
        true if row.range <= 0.0 => {
            log.approximate(NO_RANGE, 1);
            Some(0.0)
        }
        true => {
            let reached = reach(row, min_speed, max_speed, life);
            if reached.is_some() && row.gravity_in_range != 0.0 {
                log.approximate(RANGE_ALONG_FORWARD, 1);
            }
            reached
        }
    };
    let out_of_range = |t: f32| switch.is_some_and(|s| t >= s);
    let breaks: Vec<f32> = [Some(0.0), Some(row.acceleration_delay), switch]
        .into_iter()
        .flatten()
        .filter(|t| *t >= 0.0)
        .collect();
    let acceleration = steps(&breaks, |t| {
        if t < row.acceleration_delay {
            0.0
        } else if out_of_range(t) {
            row.acceleration_out_of_range
        } else {
            row.acceleration_in_range
        }
    });
    let lift = steps(&breaks, |t| {
        -if out_of_range(t) {
            row.gravity_out_of_range
        } else {
            row.gravity_in_range
        }
    });

    if row.life_random != 0.0 {
        lose(log, LIFE_RANDOM);
    }
    if row.follow != 0 {
        lose(log, FOLLOWING);
    }
    if row.homing_angle > 0 || row.homing_angle_x > 0 {
        lose(log, HOMING);
    }
    if names(row.hit_effect) {
        lose(log, HIT_EFFECT);
    }
    if names(row.flick_effect) {
        lose(log, FLICK_EFFECT);
    }
    if names(row.spawned_bullet) {
        lose(log, SPAWNED);
    }
    if names(row.interval_bullet) && row.interval_max > 0.0 {
        lose(log, INTERVAL);
    }
    if effect.is_none() && names(row.effect) {
        lose(log, NO_EFFECT);
    }
    log.approximate(FACING, 1);

    let zero = || Value::constant(&[0.0]);
    let one = || Value::constant(&[1.0]);
    let mut emitter = blank(None, Some(format!("bullet {}", row.id)));
    emitter.duration = life;
    emitter.motion = Some(NodeMotion {
        forward: units::FORWARD,
        speed: Value::constant(&[row.initial_speed]),
        speed_multiplier: one(),
        acceleration,
        acceleration_multiplier: one(),
        lift,
        lift_in_world: true,
        displacement: None,
        spin: [zero(), zero(), zero()],
        spin_multiplier: [one(), one(), one()],
        align: true,
        turns: None,
        drag: None,
        min_speed,
        max_speed,
    });
    emitter.spawns = effect.into_iter().collect();
    let emitters = vec![emitter];
    let end = end_time(&emitters);
    ParticleSystem {
        seed,
        emitters,
        inputs: Vec::new(),
        states: 1,
        warmup: 0.0,
        time_scale: None,
        max_view_distance: None,
        termination: Termination::Natural,
        end,
        lost,
    }
}

/// A function of the active time that holds `f`'s value at each of `breaks` (ascending after sorting) until the next: a
/// constant when it has one value.
fn steps(breaks: &[f32], f: impl Fn(f32) -> f32) -> Value {
    let mut at: Vec<f32> = breaks.to_vec();
    at.sort_by(f32::total_cmp);
    at.dedup();
    let keys: Vec<Key> = at
        .iter()
        .map(|&t| Key {
            at: t,
            value: [f(t), 0.0, 0.0, 0.0],
            handle_in: [0.0; 4],
            handle_out: [0.0; 4],
        })
        .collect();
    if keys.windows(2).all(|w| w[0].value == w[1].value) {
        return Value::constant(&[keys.first().map_or(0.0, |k| k.value[0])]);
    }
    Value {
        argument: Argument::ActiveTime,
        curve: Curve {
            width: 1,
            interpolation: Interpolation::Step,
            looped: false,
            keys,
        },
        variation: Vec::new(),
    }
}

/// The active time at which the forward travel of a bullet of `row` reaches its range under its in-range law (the
/// speed held to `min` and `max` as core's motion holds it), stepped in [`RANGE_STEP`]s over its life (or
/// [`RANGE_HORIZON`]); `None` when it does not reach it.
fn reach(row: &Bullet, min: Option<f32>, max: Option<f32>, life: Option<f32>) -> Option<f32> {
    let limit = |v: f64| {
        let v = max.map_or(v, |m| v.min(f64::from(m)));
        min.map_or(v, |m| v.max(f64::from(m)))
    };
    let horizon = life.map_or(RANGE_HORIZON, f64::from);
    let (range, delay) = (f64::from(row.range), f64::from(row.acceleration_delay));
    let mut speed = limit(f64::from(row.initial_speed));
    let mut travelled = 0.0;
    let mut k = 0u32;
    loop {
        let t = f64::from(k) * RANGE_STEP;
        if travelled >= range {
            return Some(t as f32);
        }
        if t >= horizon {
            return None;
        }
        let a = if t < delay {
            0.0
        } else {
            f64::from(row.acceleration_in_range)
        };
        speed = limit(speed + a * RANGE_STEP);
        travelled += speed * RANGE_STEP;
        k += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shard() -> Bullet {
        Bullet {
            id: 10_400_100,
            effect: 523_022,
            hit_effect: 523_023,
            flick_effect: -1,
            life: 1.25,
            life_random: 0.0,
            range: 12.0,
            gravity_in_range: 0.0,
            gravity_out_of_range: 0.0,
            initial_speed: 13.0,
            acceleration_in_range: 40.0,
            acceleration_out_of_range: 40.0,
            max_speed: 25.0,
            min_speed: 6.0,
            acceleration_delay: 0.32,
            spawned_bullet: -1,
            follow: 0,
            interval_bullet: -1,
            interval_min: 0.0,
            interval_max: 0.0,
            interval_wait: 0.0,
            inherit_speed: false,
            homing_angle: 35,
            homing_angle_x: -1,
            homing_begin: 0.0,
            homing_stop_range: 0.4,
            homing_offset: 0.0,
            auto_homing: true,
        }
    }

    /// Glintstone Shard's row flies at 13 m/s, accelerates at 40 m/s² from 0.32 s and is held at 25 m/s, faces its
    /// motion (counted), lives 1.25 s and starts its effect; its homing and hit effect are counted, not carried.
    #[test]
    fn a_shard_flies_by_its_rows_law() {
        let mut log = PassLog::default();
        let effect = Entity::from_index(7);
        let system = flight(&shard(), Some(effect), 3, &mut log);
        assert_eq!(system.emitters.len(), 1);
        let e = &system.emitters[0];
        assert_eq!(e.duration, Some(1.25));
        assert_eq!(e.spawns, [effect]);
        let m = e.motion.as_ref().unwrap();
        assert!(m.align && m.lift_in_world);
        assert_eq!((m.min_speed, m.max_speed), (Some(6.0), Some(25.0)));
        let at = |v: &Value, t: f32| v.eval(t, 0, 0, 0, &|_| 0.0)[0];
        assert_eq!(at(&m.speed, 0.0), 13.0);
        assert_eq!(at(&m.acceleration, 0.31), 0.0);
        assert_eq!(at(&m.acceleration, 0.32), 40.0);
        assert_eq!(at(&m.acceleration, 1.0), 40.0);
        assert!(m.lift.is_fixed() && at(&m.lift, 0.0) == 0.0);
        assert_eq!(system.lost, [(HOMING.to_owned(), 1), (HIT_EFFECT.to_owned(), 1)]);
        assert_eq!(log.approximations.get(FACING), Some(&1));
        // the effect it starts lives on by its own law: the system's end is not known
        assert_eq!(system.end, None);
    }

    /// Where the laws in and out of range differ, they change at the time the forward travel reaches the range; with
    /// no range the out-of-range law holds from the start.
    #[test]
    fn the_out_of_range_law_begins_where_the_travel_reaches_the_range() {
        let mut log = PassLog::default();
        let row = Bullet {
            initial_speed: 10.0,
            acceleration_delay: 0.0,
            acceleration_in_range: 0.0,
            acceleration_out_of_range: -5.0,
            gravity_in_range: 0.0,
            gravity_out_of_range: 9.8,
            max_speed: 0.0,
            min_speed: 0.0,
            range: 5.0,
            ..shard()
        };
        let system = flight(&row, None, 0, &mut log);
        let m = system.emitters[0].motion.as_ref().unwrap();
        let at = |v: &Value, t: f32| v.eval(t, 0, 0, 0, &|_| 0.0)[0];
        // 5 m at 10 m/s: 0.5 s, within a step
        let keys = &m.acceleration.curve.keys;
        assert_eq!(keys.len(), 2);
        assert!((keys[1].at - 0.5).abs() < 2e-4, "{}", keys[1].at);
        assert_eq!((at(&m.acceleration, 0.4), at(&m.acceleration, 0.6)), (0.0, -5.0));
        assert_eq!((at(&m.lift, 0.4), at(&m.lift, 0.6)), (0.0, -9.8));
        assert_eq!(m.max_speed, None);
        assert_eq!(m.min_speed, Some(0.0));

        let row = Bullet { range: 0.0, ..row };
        let system = flight(&row, None, 0, &mut log);
        let m = system.emitters[0].motion.as_ref().unwrap();
        assert_eq!(at(&m.acceleration, 0.0), -5.0);
        assert!(m.acceleration.is_fixed());
    }

    /// The speed limits as the walk to the range holds them: the max reached under acceleration stops it, the min
    /// holds a start below it.
    #[test]
    fn the_walk_to_the_range_holds_the_speed_limits() {
        let row = Bullet {
            initial_speed: 2.0,
            acceleration_delay: 0.0,
            acceleration_in_range: 100.0,
            range: 30.0,
            ..shard()
        };
        // 2 → held at 6 by the min, then 6 + 100t to 25 by 0.19 s (2.945 m), then 25 m/s for the 27.055 m left
        let t = reach(&row, Some(6.0), Some(25.0), None).unwrap();
        assert!((t - (0.19 + 27.055 / 25.0)).abs() < 1e-3, "{t}");
        assert_eq!(reach(&row, Some(0.0), Some(1.0), Some(1.0)), None);
    }
}
