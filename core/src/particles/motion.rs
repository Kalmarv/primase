//! How an emitter's own motion ([`NodeMotion`]) moves it over its active time: [`NodeMotion::advance`] steps a
//! [`MotionState`], so every renderer that plays a description moves its emitters alike.

use super::{NodeMotion, Value, unit};

/// Where an emitter's motion has taken it, in its parent's frame: what [`NodeMotion::advance`] steps. The emitter's
/// frame is its parent's moved by `travelled`, turned by `facing`, then placed (its jitter and placement) and turned by
/// `turned`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionState {
    /// The position its motion has reached (metres).
    pub travelled: [f32; 3],
    /// The speed along its forward gained by acceleration (metres per second), within its speed limits.
    pub gained: f32,
    /// The speed along its lift axis gained by lift (metres per second).
    pub lifted: f32,
    /// Its turn about its own axes by its spin and random turns: a unit quaternion `[x, y, z, w]`.
    pub turned: [f32; 4],
    /// The turn of its whole frame, in its parent's, that faces its forward along its velocity (identity unless it
    /// aligns with its motion): a unit quaternion `[x, y, z, w]`.
    pub facing: [f32; 4],
    /// The random turns taken, and the active time the next is due at (seconds).
    pub turns: u32,
    pub next_turn: f32,
}

impl Default for MotionState {
    /// Not moved: at the origin, at rest, unturned, the first random turn due at once.
    fn default() -> MotionState {
        MotionState {
            travelled: [0.0; 3],
            gained: 0.0,
            lifted: 0.0,
            turned: IDENTITY,
            facing: IDENTITY,
            turns: 0,
            next_turn: 0.0,
        }
    }
}

/// The random stream an emitter's random turns draw on (turn `n` on this plus `n % 64`), with the scope
/// [`TURN_SCOPE`]: above any a source's property modifiers name.
const TURN_STREAM: u32 = 0x4000_0000 + 20;
/// The scope of the random turns' draws: one an emitter's activation count does not reach.
const TURN_SCOPE: u64 = u64::MAX - 1;
/// How far before a random turn's time the active time may be and still take it (seconds): a time stepped in
/// fractions of a second lands within a rounding of the time a turn is due at.
const TURN_TOLERANCE: f32 = 1e-4;

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Whose motion [`NodeMotion::advance`] steps: the system's seed and the emitter's number, which key its random draws,
/// and the system's inputs its values may read.
#[derive(Clone, Copy)]
pub struct Mover<'a> {
    pub seed: u64,
    pub emitter: u32,
    pub input: &'a dyn Fn(u32) -> f32,
}

impl std::fmt::Debug for Mover<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mover")
            .field("seed", &self.seed)
            .field("emitter", &self.emitter)
            .finish_non_exhaustive()
    }
}

impl NodeMotion {
    /// Moves `mover`'s `state` on by `dt` seconds, ending at the active time `active` (seconds), the values read at
    /// `active` (random variation drawn in scope 0). `placement` is
    /// the rotation of the emitter's placement in its parent's frame (a unit quaternion `[x, y, z, w]`): the forward and
    /// a lift not in the world turn with it.
    ///
    /// The speed along the forward is `speed` times its multiplier plus what acceleration times its multiplier has
    /// added, held within `min_speed` and `max_speed`; lift adds speed along +Y (the world's, or the placement's).
    /// The position moves by both, and a displacement, where there is one, replaces it. Spin turns the emitter about
    /// its own axes (Y, then X, then Z), and each random turn, every `interval` seconds from the start, turns its
    /// forward by up to `max_angle` about a random axis across it. An emitter that aligns with its motion faces its
    /// forward along its velocity (`facing`); one at rest keeps the facing it had. Drag is how the particles follow the
    /// emitter's motion, not the motion itself, and is not stepped here.
    pub fn advance(
        &self,
        state: &mut MotionState,
        mover: Mover<'_>,
        active: f32,
        dt: f32,
        placement: [f32; 4],
    ) {
        let Mover { seed, emitter, input } = mover;
        let ev = |v: &Value| v.eval(active, seed, emitter, 0, input);
        let speed = ev(&self.speed)[0] * ev(&self.speed_multiplier)[0];
        let acceleration = ev(&self.acceleration)[0] * ev(&self.acceleration_multiplier)[0];
        let limited = self.limit(speed + state.gained + acceleration * dt);
        state.gained = limited - speed;
        let along = rotate(mul(placement, state.turned), self.forward);
        state.lifted += ev(&self.lift)[0] * dt;
        let up = if self.lift_in_world {
            [0.0, 1.0, 0.0]
        } else {
            rotate(placement, [0.0, 1.0, 0.0])
        };
        let velocity: [f32; 3] = std::array::from_fn(|k| along[k] * limited + up[k] * state.lifted);
        for (t, v) in state.travelled.iter_mut().zip(velocity) {
            *t += v * dt;
        }
        if let Some(d) = &self.displacement {
            let v = ev(d);
            state.travelled = [v[0], v[1], v[2]];
        }
        let spin: [f32; 3] = std::array::from_fn(|k| ev(&self.spin[k])[0] * ev(&self.spin_multiplier[k])[0]);
        if spin != [0.0; 3] {
            let step = euler_yxz(
                (spin[1] * dt).to_radians(),
                (spin[0] * dt).to_radians(),
                (spin[2] * dt).to_radians(),
            );
            state.turned = normalize(mul(state.turned, step));
        }
        if let Some(t) = &self.turns
            && t.interval > 0.0
            && active + TURN_TOLERANCE >= state.next_turn
        {
            let back = [0.0, 0.0, -1.0];
            let turned = random_turn(back, ev(&t.max_angle)[0], seed, emitter, state.turns);
            state.turned = normalize(mul(state.turned, arc(back, turned)));
            state.turns += 1;
            state.next_turn = active + t.interval;
        }
        if self.align {
            let length = velocity.iter().map(|v| v * v).sum::<f32>().sqrt();
            if length > 1e-6 {
                let heading = velocity.map(|v| v / length);
                state.facing = arc(along, heading);
            }
        }
    }

    /// `speed` held within the motion's limits.
    fn limit(&self, speed: f32) -> f32 {
        let speed = self.min_speed.map_or(speed, |min| speed.max(min));
        self.max_speed.map_or(speed, |max| speed.min(max))
    }
}

/// `v` turned by up to `max_degrees` about a random axis across it, from the draws of turn number `n`.
fn random_turn(v: [f32; 3], max_degrees: f32, seed: u64, emitter: u32, n: u32) -> [f32; 3] {
    let draw = |component| unit(seed, emitter, TURN_SCOPE, TURN_STREAM + n % 64, component);
    let (t1, t2) = basis(v);
    let alpha = max_degrees.to_radians() * draw(1);
    let phi = std::f32::consts::TAU * draw(2);
    let turned: [f32; 3] =
        std::array::from_fn(|k| v[k] * alpha.cos() + (t1[k] * phi.cos() + t2[k] * phi.sin()) * alpha.sin());
    let length = turned.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length.is_finite() && length > 0.0 {
        turned.map(|x| x / length)
    } else {
        v
    }
}

/// Two unit vectors across `dir` and across each other.
fn basis(dir: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let dir = unit_or_zero(dir);
    let helper = if dir[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let t1 = unit_or_zero(cross(dir, helper));
    let t2 = unit_or_zero(cross(dir, t1));
    (t1, t2)
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit_or_zero(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length.is_finite() && length > 0.0 {
        v.map(|x| x / length)
    } else {
        [0.0; 3]
    }
}

/// The product `a b` of two quaternions `[x, y, z, w]`: `b`'s turn, then `a`'s.
fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let length = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length.is_finite() && length > 0.0 {
        q.map(|x| x / length)
    } else {
        IDENTITY
    }
}

/// `v` turned by the unit quaternion `q`.
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let w = q[3];
    let uv = cross(u, v);
    let uuv = cross(u, uv);
    std::array::from_fn(|k| v[k] + 2.0 * (w * uv[k] + uuv[k]))
}

/// The turn about Y by `y`, then X by `x`, then Z by `z` (radians), applied to a vector Z first: `qy qx qz`.
fn euler_yxz(y: f32, x: f32, z: f32) -> [f32; 4] {
    let about = |axis: usize, angle: f32| {
        let mut q = [0.0, 0.0, 0.0, (angle / 2.0).cos()];
        q[axis] = (angle / 2.0).sin();
        q
    };
    mul(mul(about(1, y), about(0, x)), about(2, z))
}

/// The shortest turn taking the unit vector `from` to the unit vector `to`; opposite ones turn half about an axis
/// across `from`.
fn arc(from: [f32; 3], to: [f32; 3]) -> [f32; 4] {
    let d = dot(from, to);
    if d < -1.0 + 1e-6 {
        let (axis, _) = basis(from);
        return [axis[0], axis[1], axis[2], 0.0];
    }
    let c = cross(from, to);
    let s = (2.0 * (1.0 + d)).sqrt();
    [c[0] / s, c[1] / s, c[2] / s, s / 2.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::particles::{Turns, Value};

    fn v(x: f32) -> Value {
        Value::constant(&[x])
    }

    fn motion() -> NodeMotion {
        NodeMotion {
            forward: [0.0, 0.0, -1.0],
            speed: v(0.0),
            speed_multiplier: v(1.0),
            acceleration: v(0.0),
            acceleration_multiplier: v(1.0),
            lift: v(0.0),
            lift_in_world: false,
            displacement: None,
            spin: [v(0.0), v(0.0), v(0.0)],
            spin_multiplier: [v(1.0), v(1.0), v(1.0)],
            align: false,
            turns: None,
            drag: None,
            min_speed: None,
            max_speed: None,
        }
    }

    /// Plays `m` for `seconds` in steps of 1/120 s from active time 0, from rest.
    fn play(m: &NodeMotion, seconds: f32) -> MotionState {
        let mut s = MotionState::default();
        let mover = Mover {
            seed: 7,
            emitter: 0,
            input: &|_| 0.0,
        };
        let dt = 1.0 / 120.0;
        let steps = (seconds / dt).round() as u32;
        for i in 0..steps {
            m.advance(&mut s, mover, i as f32 * dt, dt, IDENTITY);
        }
        s
    }

    fn close(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
    }

    #[test]
    fn acceleration_is_held_within_the_speed_limits() {
        // from 6 m/s at 40 m/s² up to 20 m/s: 0.35 s to reach it, then on at 20
        let mut m = motion();
        m.acceleration = v(40.0);
        m.min_speed = Some(6.0);
        m.max_speed = Some(20.0);
        let s = play(&m, 1.0);
        assert!((s.gained - 20.0).abs() < 1e-3, "{}", s.gained);
        // 6*0.35 + 40*0.35²/2 + 20*0.65 = 2.1 + 2.45 + 13
        assert!(close(s.travelled, [0.0, 0.0, -17.55], 0.2), "{:?}", s.travelled);
        // a deceleration from the limit answers at once: it does not first spend what was clamped away
        let mut m = motion();
        m.speed = v(30.0);
        m.max_speed = Some(10.0);
        let s = play(&m, 1.0);
        assert!(close(s.travelled, [0.0, 0.0, -10.0], 1e-3), "{:?}", s.travelled);
    }

    #[test]
    fn an_aligned_emitter_faces_its_curving_path_and_one_not_aligned_does_not() {
        let mut m = motion();
        m.speed = v(10.0);
        m.lift = v(-10.0);
        m.lift_in_world = true;
        let s = play(&m, 1.0);
        assert_eq!(s.facing, IDENTITY);
        m.align = true;
        let s = play(&m, 1.0);
        // after 1 s the velocity is (0, -10, -10): the forward is pitched 45 degrees down
        let forward = rotate(s.facing, [0.0, 0.0, -1.0]);
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(forward, [0.0, -half, -half], 1e-2), "{forward:?}");
    }

    #[test]
    fn random_turns_come_every_interval_and_are_the_same_each_play() {
        let mut m = motion();
        m.speed = v(1.0);
        m.turns = Some(Turns {
            interval: 0.1,
            max_angle: v(30.0),
        });
        let a = play(&m, 1.0);
        // turns at 0.0, 0.1, ..., 0.9: each when the active time, in steps of 1/120 s, reaches the time it is due at
        assert_eq!(a.turns, 10);
        assert_eq!(a, play(&m, 1.0));
        assert_ne!(a.turned, IDENTITY);
    }

    #[test]
    fn a_displacement_places_the_emitter_where_it_says() {
        let mut m = motion();
        m.speed = v(5.0);
        m.displacement = Some(Value::constant(&[1.0, 2.0, 3.0]));
        assert_eq!(play(&m, 0.5).travelled, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn quaternion_helpers_agree_with_a_quarter_turn() {
        let q = euler_yxz(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        assert!(close(rotate(q, [0.0, 0.0, -1.0]), [-1.0, 0.0, 0.0], 1e-6));
        let a = arc([0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
        assert!(close(rotate(a, [0.0, 0.0, -1.0]), [1.0, 0.0, 0.0], 1e-6));
        let back = arc([0.0, 0.0, -1.0], [0.0, 0.0, 1.0]);
        assert!(close(rotate(back, [0.0, 0.0, -1.0]), [0.0, 0.0, 1.0], 1e-6));
    }
}
