//! `core.particles`: a particle effect as a source states it, in game-neutral terms.
//!
//! One component on an entity placed by its [`Transform`](crate::components::Transform): a tree of [`Emitter`]s that follow
//! the entity's frame (or their parents'), emit particles on a schedule from a shape, move them by a law and draw each as one
//! of a few forms ([`Draw`]). Everything a renderer needs is here and in game-neutral words: the curves are constants,
//! steps, lines or cubic Beziers of a stated [`Argument`] (the particle's age, the emitter's active time, the time the particle
//! was emitted), their random variation is seeded and per-stream, blends are the standard few, orientations are named by what
//! they face. A source's own dialect (an engine's curve flavours, its units, its enumerations) is lowered into these by the
//! source's pass, which counts what it could not carry ([`ParticleSystem::lost`]); no renderer needs to know the source.
//!
//! **Determinism.** A system's picture at a time is a function of its description, its seed and its [`Input`]s: random draws
//! are keyed by `(seed, emitter, scope index, stream)` through [`Draw::unit`](Rng::unit), never by the order particles are
//! processed in, so a renderer and a test that step the same description agree.
//!
//! **Units.** Metres, seconds, degrees (angles that a source gives in other units are converted by its pass), linear colour.
//! Angles are degrees because that is what the sources store; the renderer converts.

use serde::Serialize;
use serde_json::Value as Json;

use crate::{Component, Entity, JsonError, Transform, World};

/// What a [`Value`] is a function of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Argument {
    /// Nothing: the curve is read at 0.
    Constant,
    /// Seconds since the particle was emitted.
    Age,
    /// Seconds since the emitter became active (after its delay, or when its state was entered).
    ActiveTime,
    /// The emitter's active time at which the particle was emitted; constant for the particle's life.
    EmissionTime,
}

/// How a [`Curve`] runs between its keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Interpolation {
    /// Holds each key's value until the next key.
    Step,
    /// Joins the keys by straight lines.
    Linear,
    /// Joins the keys by cubic Beziers whose time handles lie a third of the way along the segment, so a segment from key
    /// `a` to key `b` of duration `d` is, for `u = (x - a.at) / d`, the cubic through `a.value`, `a.value + a.out`,
    /// `b.value - b.in` and `b.value` in `u` (handles are offsets in value, per component).
    Bezier,
}

/// One key of a [`Curve`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Key {
    /// The argument value of the key, seconds, ascending along the curve.
    pub at: f32,
    /// The value at the key, in the curve's components (the rest 0).
    pub value: [f32; 4],
    /// For [`Interpolation::Bezier`]: the value offset of the incoming handle (subtracted from this key's value to give the
    /// segment's second control point); 0 otherwise.
    pub handle_in: [f32; 4],
    /// For [`Interpolation::Bezier`]: the value offset of the outgoing handle.
    pub handle_out: [f32; 4],
}

/// A function of one argument with one to four components.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Curve {
    /// The number of components (1 to 4).
    pub width: u8,
    /// How it runs between keys.
    pub interpolation: Interpolation,
    /// Whether the argument wraps at the last key's `at` (the first key then follows it). A loop of no duration is its first
    /// key's value.
    pub looped: bool,
    /// At least one key; a one-key curve is a constant.
    pub keys: Vec<Key>,
}

impl Curve {
    /// A constant of one to four components.
    pub fn constant(values: &[f32]) -> Curve {
        let mut value = [0.0; 4];
        for (slot, v) in value.iter_mut().zip(values) {
            *slot = *v;
        }
        Curve {
            width: values.len().clamp(1, 4) as u8,
            interpolation: Interpolation::Linear,
            looped: false,
            keys: vec![Key {
                at: 0.0,
                value,
                handle_in: [0.0; 4],
                handle_out: [0.0; 4],
            }],
        }
    }

    /// The curve at argument `x`: before the first key the first value, after the last key the last value (or wrapped, for a
    /// loop). Components past [`Curve::width`] are 0.
    pub fn value_at(&self, x: f32) -> [f32; 4] {
        let keys = &self.keys;
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
            return [0.0; 4];
        };
        let x = if self.looped {
            if last.at <= 0.0 {
                return first.value;
            }
            x.rem_euclid(last.at)
        } else {
            x
        };
        if keys.len() == 1 || x <= first.at {
            return first.value;
        }
        if x >= last.at {
            return last.value;
        }
        // the segment [a, b] with a.at <= x < b.at
        let b = keys.partition_point(|k| k.at <= x);
        let (a, b) = (&keys[b - 1], &keys[b]);
        let d = b.at - a.at;
        if d <= 0.0 {
            return b.value;
        }
        let u = (x - a.at) / d;
        let mut out = [0.0; 4];
        for (c, slot) in out
            .iter_mut()
            .enumerate()
            .take(usize::from(self.width.clamp(1, 4)))
        {
            *slot = match self.interpolation {
                Interpolation::Step => a.value[c],
                Interpolation::Linear => a.value[c] + (b.value[c] - a.value[c]) * u,
                Interpolation::Bezier => {
                    let (p0, p1, p2, p3) = (
                        a.value[c],
                        a.value[c] + a.handle_out[c],
                        b.value[c] - b.handle_in[c],
                        b.value[c],
                    );
                    let k = 1.0 - u;
                    k * (k * k * p0 + 3.0 * u * (k * p1 + u * p2)) + u * u * u * p3
                }
            };
        }
        out
    }

    fn check(&self, what: &str) -> Result<(), String> {
        if !(1..=4).contains(&self.width) {
            return Err(format!("{what}: {} components", self.width));
        }
        if self.keys.is_empty() {
            return Err(format!("{what}: no keys"));
        }
        let mut previous = f32::NEG_INFINITY;
        for (i, k) in self.keys.iter().enumerate() {
            if !(k.at.is_finite()
                && k.value
                    .iter()
                    .chain(&k.handle_in)
                    .chain(&k.handle_out)
                    .all(|v| v.is_finite()))
            {
                return Err(format!("{what}: key {i} is not finite"));
            }
            if k.at < previous {
                return Err(format!("{what}: key {i} at {} goes back from {previous}", k.at));
            }
            previous = k.at;
        }
        Ok(())
    }
}

/// Random variation of a [`Value`], drawn from the stream named in each case (a draw is a uniform number in `[0, 1)` per
/// component, keyed as the module says).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Variation {
    /// `base + (2u - 1) * max`: the base varies by up to `max` either way.
    Delta { max: [f32; 4], stream: u32 },
    /// `base + (2u - 1) * max * base`: the base varies by up to the fraction `max` of itself.
    Fraction { max: [f32; 4], stream: u32 },
    /// `base + min + u * (max - min)`: a random addition in a range.
    Range {
        min: [f32; 4],
        max: [f32; 4],
        stream: u32,
    },
    /// `base * factor(input)`: the base scaled by a curve of an [`Input`]'s current value.
    Scaled { input: u32, factor: Curve },
}

/// A scalar or vector quantity that may vary with time and at random: what every parameter of an emitter is.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Value {
    /// What the curve is a function of.
    pub argument: Argument,
    /// The base value.
    pub curve: Curve,
    /// Variations applied to it, in order.
    pub variation: Vec<Variation>,
}

impl Value {
    /// A constant that does not vary.
    pub fn constant(values: &[f32]) -> Value {
        Value {
            argument: Argument::Constant,
            curve: Curve::constant(values),
            variation: Vec::new(),
        }
    }

    /// Whether the value is a constant that does not vary.
    pub fn is_fixed(&self) -> bool {
        self.variation.is_empty() && (self.argument == Argument::Constant || self.curve.keys.len() == 1)
    }

    fn check(&self, what: &str, inputs: usize) -> Result<(), String> {
        self.curve.check(what)?;
        for v in &self.variation {
            match v {
                Variation::Delta { max, .. } | Variation::Fraction { max, .. } => {
                    if !max.iter().all(|m| m.is_finite()) {
                        return Err(format!("{what}: a variation that is not finite"));
                    }
                }
                Variation::Range { min, max, .. } => {
                    if !min.iter().chain(max).all(|m| m.is_finite()) {
                        return Err(format!("{what}: a variation that is not finite"));
                    }
                }
                Variation::Scaled { input, factor } => {
                    if usize::try_from(*input).is_ok_and(|i| i >= inputs) {
                        return Err(format!("{what}: input {input} of {inputs}"));
                    }
                    factor.check(what)?;
                }
            }
        }
        Ok(())
    }
}

/// The random draw of the module docs: a uniform number in `[0, 1)` keyed by `(seed, emitter, scope, stream, component)`.
/// `scope` is the index that makes the draw belong to one thing: a particle's number in its emitter for a value of the
/// particle's age or emission time, the emitter's activation count for a value of its active time.
pub fn unit(seed: u64, emitter: u32, scope: u64, stream: u32, component: u32) -> f32 {
    fn mix(z: u64) -> u64 {
        let z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    let mut x = mix(seed);
    for v in [u64::from(emitter), scope, u64::from(stream), u64::from(component)] {
        x = mix(x ^ v);
    }
    // the top 24 bits make an exactly representable float in [0, 1)
    (x >> 40) as f32 / 16_777_216.0
}

impl Value {
    /// The value at argument `x` for the draw `scope` of `emitter` in a system of `seed`, with the system's inputs read
    /// through `input` (by index): the curve, then the variations in order.
    pub fn eval(&self, x: f32, seed: u64, emitter: u32, scope: u64, input: &dyn Fn(u32) -> f32) -> [f32; 4] {
        let at = if self.argument == Argument::Constant {
            0.0
        } else {
            x
        };
        let mut v = self.curve.value_at(at);
        let width = usize::from(self.curve.width.clamp(1, 4));
        for var in &self.variation {
            match var {
                Variation::Delta { max, stream } => {
                    for (c, slot) in v.iter_mut().enumerate().take(width) {
                        let u = unit(seed, emitter, scope, *stream, c as u32);
                        *slot += (2.0 * u - 1.0) * max[c];
                    }
                }
                Variation::Fraction { max, stream } => {
                    for (c, slot) in v.iter_mut().enumerate().take(width) {
                        let u = unit(seed, emitter, scope, *stream, c as u32);
                        *slot += (2.0 * u - 1.0) * max[c] * *slot;
                    }
                }
                Variation::Range { min, max, stream } => {
                    for (c, slot) in v.iter_mut().enumerate().take(width) {
                        let u = unit(seed, emitter, scope, *stream, c as u32);
                        *slot += min[c] + u * (max[c] - min[c]);
                    }
                }
                Variation::Scaled { input: i, factor } => {
                    let f = factor.value_at(input(*i));
                    for (c, slot) in v.iter_mut().enumerate().take(width) {
                        *slot *= f[c.min(usize::from(factor.width.clamp(1, 4)) - 1)];
                    }
                }
            }
        }
        v
    }
}

/// A named quantity a program may drive from outside (a source's external values: the weather, a switch). `default` is
/// what a renderer uses when nothing drives it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Input {
    /// The source's name for it.
    pub name: String,
    /// The value when nothing sets it.
    pub default: f32,
}

/// How a thing follows the frame it hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Follow {
    /// Not at all: it keeps its place in the world once made.
    None,
    /// With position and orientation.
    Frame,
    /// With position only.
    Position,
}

/// Which frame a thing hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Anchor {
    /// Its parent emitter's frame (or the system's, for a root).
    Parent,
    /// The system's own frame: the entity's placement, which a source may make an attachment point of another entity.
    System,
}

/// A thing's attachment: which frame, and how it follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Attachment {
    pub anchor: Anchor,
    pub follow: Follow,
}

/// How an emitter makes particles.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Spawn {
    /// One particle, once, when the emitter becomes active.
    Once,
    /// `count` particles every `interval` seconds, `total` times at most, never more than `max_alive` alive.
    Periodic {
        interval: Value,
        count: Value,
        total: Option<u32>,
        max_alive: Option<u32>,
    },
    /// `count` particles each time the emitter has moved `step` metres from where it last emitted.
    Distance {
        step: Value,
        count: Value,
        total: Option<u32>,
        max_alive: Option<u32>,
    },
}

/// Which axis a cylinder runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// Where in its frame an emitter makes a particle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Shape {
    /// The frame's origin.
    Point,
    /// A disc in the XY plane; `concentration` is 0 for uniform over the area, 1 for all at the centre and -1 for all on the
    /// rim.
    Disk { radius: Value, concentration: Value },
    /// A rectangle in the XY plane, with the same `concentration` toward its centre.
    Rectangle { size: [Value; 2], concentration: Value },
    /// A sphere's surface, or its volume.
    Sphere { radius: Value, volume: bool },
    /// A box's surface, or its volume.
    Box { size: [Value; 3], volume: bool },
    /// A cylinder's surface, or its volume.
    Cylinder {
        radius: Value,
        height: Value,
        volume: bool,
        axis: Axis,
    },
}

/// Which way an emitted particle first heads, before the spread.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Heading {
    /// Out of the shape (along the frame's axis the shape faces for a point, disc or rectangle; away from the centre for a
    /// sphere, cylinder or box; for a box volume along a random local axis). A source states the axis in `Shape` terms:
    /// the source's pass puts a point's, disc's and rectangle's facing in `facing`.
    Shape { facing: [f32; 3] },
    /// A fixed direction in the world (a unit vector).
    World([f32; 3]),
    /// A fixed direction in the emitter's frame (a unit vector).
    Frame([f32; 3]),
}

/// How a particle's first heading is scattered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Spread {
    /// Not at all.
    None,
    /// Inside a cone of half-angle `angle` (degrees); `concentration` 0 spreads uniformly over the cone's area, 1 puts every
    /// particle on the axis and -1 on the cone's edge.
    Cone { angle: Value, concentration: Value },
    /// Inside an elliptical cone of half-angles `x` and `y`.
    Ellipse {
        x: Value,
        y: Value,
        concentration: Value,
    },
    /// Inside a rectangular cone of half-angles `x` and `y`.
    Rectangle {
        x: Value,
        y: Value,
        concentration: Value,
    },
}

/// What drives a particle along its heading.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Drive {
    /// Nothing along the heading (only gravity and forces move it).
    None,
    /// A speed (metres per second) along the current heading: `speed` times `multiplier`, each a function of the particle's
    /// age.
    Speed { speed: Value, multiplier: Value },
    /// An acceleration (metres per second squared) along the current heading, integrated into the speed: `acceleration`
    /// times `multiplier`.
    Acceleration { acceleration: Value, multiplier: Value },
}

/// Random turning of a particle's heading.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Turns {
    /// Seconds between turns.
    pub interval: f32,
    /// The largest turn (degrees).
    pub max_angle: Value,
}

/// Following the emitter's movement though not attached to it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Drag {
    /// 0 not at all, 1 as if attached; negative opposes, above 1 exaggerates.
    pub factor: Value,
    /// Whether rotation is followed as well as movement.
    pub rotation: bool,
}

/// How a particle moves.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Movement {
    /// The drive along the heading.
    pub drive: Drive,
    /// Acceleration along world -Y (metres per second squared; positive falls); it bends the heading.
    pub gravity: Value,
    /// Random turns.
    pub turns: Option<Turns>,
    /// Following the emitter.
    pub drag: Option<Drag>,
    /// How force fields move the particle; `None` when the particle ignores them.
    pub fields: Option<FieldResponse>,
}

/// A particle's response to the force fields of the system: the speed it takes along a field's direction (`speed` times
/// `speed_multiplier`) and the acceleration (`acceleration` times `acceleration_multiplier`), functions of the emitter's
/// active time; the part a source does not state is a constant 0 (and its multiplier 1).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FieldResponse {
    pub speed: Value,
    pub speed_multiplier: Value,
    pub acceleration: Value,
    pub acceleration_multiplier: Value,
}

/// The law every particle of an emitter follows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Law {
    /// Seconds a particle lives; `None` for ever (until the emitter ends).
    pub lifetime: Option<Value>,
    /// How a particle hangs from its emitter's frame.
    pub attachment: Attachment,
    /// The movement; `None` for a particle that stays where it is made.
    pub movement: Option<Movement>,
    /// The speed (metres per second, along the heading) given to a particle when it is emitted, for a particle whose drive is
    /// an acceleration; a function of the emitter's active time.
    pub initial_speed: Value,
    /// Multipliers of every particle's size along its three axes, functions of the time it was emitted; a uniform one has the
    /// first in all three.
    pub size_scale: [Value; 3],
    /// A multiplier of every particle's colour (linear RGBA, unrestricted), a function of the emitter's active time.
    pub tint: Value,
}

/// How a draw's colour is blended with what lies behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Blend {
    /// Replaces what is behind, ignoring alpha.
    Replace,
    /// `src * a + dst * (1 - a)`.
    Alpha,
    /// `dst + src * a`.
    Additive,
    /// `dst - src * a`.
    Subtractive,
    /// `dst * src`.
    Multiply,
}

/// What a flat particle faces.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Orientation {
    /// The camera's image plane.
    CameraPlane,
    /// The camera's position (turning with the view direction).
    Camera,
    /// Toward the camera about the world's vertical axis only.
    CameraYaw,
    /// Toward the camera about the frame's +Y axis only.
    FrameYaw,
    /// A fixed direction in the world (its normal, a unit vector).
    World([f32; 3]),
    /// A fixed direction in the emitter's frame.
    Frame([f32; 3]),
    /// Along its heading.
    Heading,
}

/// A texture applied to a draw: the texture entity and what an atlas is cut into.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Map {
    /// A `core.texture` or `core.texture_file` entity.
    pub texture: Entity,
}

/// A texture atlas cut into cells.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Atlas {
    /// Cells per row.
    pub columns: u32,
    /// Cells in use.
    pub cells: u32,
    /// The cell shown (a fractional value blends two when `blend_cells`), a function of the particle's age.
    pub cell: Value,
    /// Whether neighbouring cells are cross-faded.
    pub blend_cells: bool,
}

/// One factor of a draw's colour: a value (linear RGBA unless it says one channel) multiplied in, optionally clamped to 0..1
/// first.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Term {
    pub value: Value,
    pub clamp: bool,
    pub channels: Channels,
}

/// Which channels a [`Term`] multiplies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Channels {
    Rgba,
    Rgb,
    Alpha,
}

/// Limiting a draw by its distance from the camera. `near` and `far` are each a pair of distances in metres: for `near`
/// `[hidden_at, full_at]` (invisible at or nearer than the first, fully visible at or beyond the second, linear between); for
/// `far` `[full_at, hidden_at]`. `cut_near` and `cut_far` are hard limits (hidden nearer, hidden farther). `None` is no limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct DistanceFade {
    pub near: Option<[f32; 2]>,
    pub far: Option<[f32; 2]>,
    pub cut_near: Option<f32>,
    pub cut_far: Option<f32>,
}

impl DistanceFade {
    /// Whether it limits nothing.
    pub fn is_none(&self) -> bool {
        *self == DistanceFade::default()
    }
}

/// Fading a draw out where it meets geometry (a soft particle): over `distance` metres of depth behind its surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SoftEdge {
    pub distance: f32,
}

/// A camera-facing or fixed flat particle, one textured quad.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Quad {
    /// The colour texture; `None` is white.
    pub map: Option<Map>,
    pub blend: Blend,
    pub orientation: Orientation,
    /// Width and height in metres, functions of the particle's age.
    pub size: [Value; 2],
    /// Whether the height follows the width.
    pub uniform: bool,
    /// Each particle's size is multiplied by a random factor between this and 1, per axis (1 for none).
    pub size_variance: [f32; 2],
    /// Offset from the particle's position in its own axes, metres.
    pub offset: [Value; 3],
    /// Rotation about the three axes (degrees), then spin about them (degrees per second, a function of the age).
    pub rotation: [Value; 3],
    pub spin: [Value; 3],
    /// Whether size applies before rotation.
    pub scale_before_rotation: bool,
    pub atlas: Option<Atlas>,
    /// The factors of the colour, multiplied.
    pub tint: Vec<Term>,
    /// Alpha below this is dropped (0 to 1), and alpha above it stretched to fill the range.
    pub alpha_fade_threshold: Value,
    /// Alpha below this is dropped outright.
    pub alpha_cutoff: Value,
    pub soft: Option<SoftEdge>,
    pub distance: Option<DistanceFade>,
    /// An octagonal quad (cut corners) instead of a rectangle.
    pub octagonal: bool,
    /// A lit surface: a normal map and how it is lit; `None` is unlit.
    pub lit: Option<Lit>,
    /// A bloom tint added to what is drawn, or `None` for none.
    pub bloom: Option<[f32; 4]>,
    /// Whether the texture's colour is multiplied by its alpha first.
    pub premultiply_alpha: bool,
    /// Pushes the draw toward (negative) or away from the camera in depth, metres.
    pub depth_bias: Value,
}

/// How a lit particle is shaded.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lit {
    pub normal_map: Option<Map>,
    pub specular_map: Option<Map>,
    pub glossiness: f32,
    pub specularity: f32,
    /// Whether the particle takes shadows (0 none to 1 full darkness).
    pub shadow: f32,
}

/// Several textured layers on one quad (masks, noise, scrolling detail).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayeredQuad {
    /// The quad's own colour and shape; its `map` is unused.
    pub quad: Quad,
    pub layers: Vec<Layer>,
}

/// One layer of a [`LayeredQuad`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layer {
    pub map: Option<Map>,
    pub tint: Value,
    /// Texture-space scroll speed per second, offset and scale: functions of the age (offset of nothing).
    pub scroll: [Value; 2],
    pub offset: [Value; 2],
    pub scale: [Value; 2],
}

/// A point sprite: a screen-aligned textured square.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Point {
    pub map: Option<Map>,
    pub blend: Blend,
    /// Edge in metres at the particle, a function of the age.
    pub size: Value,
    pub tint: Vec<Term>,
    pub alpha_cutoff: Value,
    pub soft: Option<SoftEdge>,
    pub distance: Option<DistanceFade>,
    pub bloom: Option<[f32; 4]>,
}

/// A mesh drawn per particle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeshParticle {
    /// A prototype entity (a `core.mesh` with its materials).
    pub mesh: Entity,
    /// An animation clip entity to play on it, with its speed and whether it loops.
    pub clip: Option<MeshClip>,
    pub blend: Blend,
    pub orientation: MeshOrientation,
    /// Size along the three axes, functions of the age.
    pub size: [Value; 3],
    pub size_variance: [f32; 3],
    pub uniform: bool,
    pub rotation: [Value; 3],
    pub spin: [Value; 3],
    pub tint: Vec<Term>,
    /// Texture-space scroll of the mesh's texture.
    pub scroll: [Value; 2],
    pub offset: [Value; 2],
    pub atlas: Option<Atlas>,
    pub soft: Option<SoftEdge>,
    pub distance: Option<DistanceFade>,
    pub lit: bool,
}

/// An animation played on a mesh particle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeshClip {
    pub clip: Entity,
    pub speed: f32,
    pub looped: bool,
}

/// What a mesh particle faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum MeshOrientation {
    /// Its own frame as emitted.
    Frame,
    /// World axes.
    World,
    /// Along its heading.
    Heading,
    /// Facing away from the camera's image plane (the model's front toward the camera).
    CameraPlane,
    /// Toward the camera about the vertical axis.
    CameraYaw,
}

/// A strip left behind a moving particle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Trail {
    pub map: Option<Map>,
    pub blend: Blend,
    pub orientation: TrailOrientation,
    /// Width in metres, a function of the particle's age, times `width_scale`, a function of the time it was emitted.
    pub width: Value,
    pub width_scale: Value,
    /// Seconds between segment starts, how long a segment lives, how many are alive, and how many cuts a completed segment is
    /// subdivided into.
    pub segment_interval: f32,
    pub segment_life: f32,
    pub max_segments: u32,
    pub subdivisions: u32,
    /// The fraction of the texture's width used per segment (for the whole trail when `attached_uv`), the speed the texture
    /// slides along (u per second), and the random offset of each segment's texture (u as a fraction, v a function of the age).
    pub texture_fraction: Value,
    pub scroll_u: Value,
    pub variance_u: f32,
    pub variance_v: Value,
    /// Whether the texture is attached to the strip (sliding with its source) or stays where each segment was made.
    pub attached_uv: bool,
    /// Whether each segment's opacity follows the speed of the source when the segment was made.
    pub dynamic_opacity: bool,
    /// Where along the trail (a fraction of its length from the start, and from the end) opacity has fully faded in and out,
    /// functions of the age: 0 is no fade.
    pub fade_start: Value,
    pub fade_end: Value,
    pub alpha_fade_threshold: Value,
    pub alpha_cutoff: Value,
    pub atlas: Option<Atlas>,
    pub tint: Vec<Term>,
    /// Seconds over which the trail fades after its particle ends.
    pub fade_out: f32,
    /// A refraction of what is behind the strip, by a normal map and an intensity (a function of the emitter's active time).
    pub distortion: Option<TrailDistortion>,
    pub lit: Option<Lit>,
    pub premultiply_alpha: bool,
    pub soft: Option<SoftEdge>,
    pub distance: DistanceFade,
    pub bloom: Option<[f32; 4]>,
}

/// The refraction a trail makes of what is behind it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrailDistortion {
    pub normal_map: Option<Map>,
    pub intensity: Value,
}

/// How a trail's strip faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TrailOrientation {
    /// Toward the camera.
    Camera,
    /// Its width along its emitter frame's z axis: the segment that sweeps the strip lies along that axis (less what of it
    /// runs along the trail).
    Frame,
    /// Flat to the world's up.
    World,
}

/// A line or elongated quad along the particle's heading.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Streak {
    pub map: Option<Map>,
    pub blend: Blend,
    /// Width in metres (a plain line has no width), a function of the time it was emitted, and its multiplier (age).
    pub width: Option<Value>,
    /// Length in metres, a function of the time it was emitted, and its multiplier (age).
    pub length: Value,
    /// Colour at the leading and trailing end, multiplied in with the tint (functions of the emitter's active time).
    pub start_colour: Value,
    pub end_colour: Value,
    pub tint: Vec<Term>,
    pub alpha_cutoff: Value,
    pub soft: Option<SoftEdge>,
    pub distance: DistanceFade,
    pub bloom: Option<[f32; 4]>,
}

/// A sheet that bends the picture behind it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Refraction {
    pub mode: RefractionMode,
    pub shape: RefractionShape,
    pub orientation: Orientation,
    /// The normal map that pushes (for [`RefractionMode::Push`]) and the mask that limits the sheet.
    pub normal_map: Option<Map>,
    pub mask: Option<Map>,
    /// The sheet's offset from the particle and its size along three axes (metres, functions of the age), whether one size
    /// serves all, and each particle's random size factor between this and 1 per axis.
    pub offset: [Value; 3],
    pub size: [Value; 3],
    pub uniform: bool,
    pub size_variance: [f32; 3],
    /// Strength of the push (a function of the age); for a stir its speed in degrees per second; the fraction of the sheet's
    /// inscribed ellipse the effect covers.
    pub intensity: Value,
    pub stir_speed: Value,
    pub radius: Value,
    /// The normal map's texture offset, scroll speed (per second) and scale along u and v.
    pub normal_offset: [Value; 2],
    pub normal_scroll: [Value; 2],
    pub normal_scale: [Value; 2],
    pub tint: Vec<Term>,
    pub blend: Blend,
    pub alpha_cutoff: Value,
    pub distance: DistanceFade,
}

/// How a refraction sheet bends what is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RefractionMode {
    /// Swirls the picture, turning at `stir_speed`.
    Stir,
    /// Pushes it by the normal map.
    Push,
    /// Twists it about the sheet's centre while its edge stays.
    Twist,
}

/// The sheet's silhouette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RefractionShape {
    Rectangle,
    /// Half an ellipsoid.
    Hemiellipsoid,
    Ellipsoid,
}

/// A blur of the picture away from a point.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RadialBlur {
    /// A mask that limits where the blur applies.
    pub mask: Option<Map>,
    pub blend: Blend,
    /// The area's offset from the particle and its size along two axes (metres, functions of the age), whether one size serves
    /// both.
    pub offset: [Value; 3],
    pub size: [Value; 2],
    pub uniform: bool,
    /// The blur's strength (a function of the age) and how many times it is applied.
    pub radius: Value,
    pub iterations: u32,
    pub tint: Vec<Term>,
    pub alpha_cutoff: Value,
    pub distance: DistanceFade,
}

/// A light that follows the particle or the emitter.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightDraw {
    pub kind: LightKind,
    /// Linear RGB, a function of the age (unrestricted).
    pub colour: Value,
    /// Candela, a function of the age.
    pub intensity: Value,
    /// Metres at which it stops.
    pub range: Value,
    pub shadows: bool,
    /// Seconds over which it fades out when its emitter ends.
    pub fade_out: f32,
}

/// What a [`LightDraw`] is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum LightKind {
    Point,
    /// A cone along the frame's +Z; angles in degrees from the axis.
    Spot {
        inner: f32,
        outer: f32,
    },
}

/// A lens flare: up to four textured layers placed along the line between the source and the screen's centre.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Flare {
    pub layers: Vec<FlareLayer>,
    pub blend: Blend,
    /// The diameter of the source sphere, whose visible fraction sets the opacity, and the seconds the opacity takes to follow it.
    pub source_size: f32,
    pub opacity_transition: f32,
}

/// How a flare layer's offset vector is found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Reflection {
    /// No offset: the flare sits at the source.
    None,
    /// The source's screen position reflected through the screen's centre.
    Radial,
    /// The source's screen position flipped horizontally.
    Horizontal,
}

/// One layer of a [`Flare`]: `count` flares of one texture.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FlareLayer {
    pub map: Option<Map>,
    pub count: u32,
    /// Width and height (constants), whether the width serves both, and each flare's random factor between this and 1 per axis.
    pub width: Value,
    pub height: Value,
    pub uniform: bool,
    pub scale_variation: [f32; 2],
    /// Colour (clamped to 0..1) and a multiplier of it (unrestricted).
    pub colour: Value,
    pub colour_multiplier: [f32; 4],
    pub reflection: Reflection,
    /// A scalar multiplier of the offset vector, and each flare's random factor between this and 1 on it.
    pub offset: f32,
    pub offset_variation: f32,
    /// The radius of a sphere around the source within which the layer is attenuated (negative: none).
    pub attenuation_radius: f32,
    pub bloom: Option<[f32; 4]>,
}

/// What a force field pushes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ForceKind {
    /// A steady push along a direction.
    Wind,
    /// A pull toward a point or along a direction.
    Gravity,
    /// A swirling noise.
    Turbulence,
    /// Cancels other fields' effect.
    Cancel,
}

/// The volume a force field acts in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum ForceVolume {
    /// Everywhere.
    Boundless,
    Box {
        half: [f32; 3],
    },
    Sphere {
        radius: f32,
    },
    Cylinder {
        radius: f32,
        half_height: f32,
    },
}

/// A force field: pushes the particles whose [`Movement::fields`] listen, inside its volume.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForceField {
    pub kind: ForceKind,
    pub volume: ForceVolume,
    /// For a wind: the direction in the emitter's frame (a unit vector); a gravity pulls toward the volume's centre.
    pub direction: [f32; 3],
    /// The strength, a function of the field's active time, times a constant multiplier and a random factor between the two
    /// bounds (a gravity takes their average).
    pub strength: Value,
    pub multiplier: f32,
    pub random_multiplier: [f32; 2],
    /// Seconds over which it fades out when its emitter ends.
    pub fade_out: f32,
}

/// Ripples where the emitter's frame touches water: the water is pushed down to `depth` over `descent` seconds in the shape of
/// `map`, held for `duration` seconds and released.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ripples {
    pub map: Option<Map>,
    pub depth: f32,
    pub size: f32,
    pub descent: f32,
    pub duration: f32,
}

/// What an emitter draws for each particle (or, for a light, field or ripple, instead of particles).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Draw {
    Quad(Quad),
    LayeredQuad(LayeredQuad),
    Point(Point),
    Mesh(MeshParticle),
    Trail(Trail),
    Streak(Streak),
    Refraction(Refraction),
    RadialBlur(RadialBlur),
    Light(LightDraw),
    Flare(Flare),
    Field(ForceField),
    Ripples(Ripples),
}

/// One emitter of a system.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Emitter {
    /// The source's name for it, if it has one.
    pub name: Option<String>,
    /// The emitter this one hangs from; always an earlier one. `None` hangs it from the system's frame.
    pub parent: Option<u32>,
    /// Placement in the parent's frame (metres), in the world's frame.
    pub placement: Transform,
    /// Random placement jitter: translation by up to this much either way (metres), rotation about each axis (degrees).
    pub jitter_translation: [f32; 3],
    pub jitter_rotation: [f32; 3],
    /// How the emitter follows its parent.
    pub attachment: Attachment,
    /// Seconds before it becomes active, and for how long (`None` for ever).
    pub delay: f32,
    pub duration: Option<f32>,
    /// How it moves in its parent's frame over its active time.
    pub motion: Option<NodeMotion>,
    pub spawn: Spawn,
    pub shape: Shape,
    pub heading: Heading,
    pub spread: Spread,
    pub law: Law,
    /// What it draws; `None` for an emitter that only holds children (or an effect-spawning one).
    pub draw: Option<Draw>,
    /// Other effects this emitter starts when it becomes active (a source's references to other effects): the entity of the
    /// system to start, attached to this emitter's frame.
    pub spawns: Vec<Entity>,
    /// Which `states` of the system it exists in; empty for all of them.
    pub states: Vec<u32>,
    /// Display order among the system's draws (lower draws in front, as a source's depth bias states).
    pub order: f32,
    /// The camera distances (metres, from and to) within which the emitter and its children exist; `None` for any distance (a
    /// source's level-of-detail nodes give each child a range).
    pub lod_range: Option<[f32; 2]>,
    /// For an emitter that makes copies of its children rather than particles: which children each emission starts. Its
    /// `spawn` schedules the emissions; each copy runs its children from its own start.
    pub copies: Option<Pick>,
}

/// Which children an emitter that copies its children starts at each emission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Pick {
    /// All of them.
    All,
    /// One, at random.
    One,
}

/// How an emitter moves in its parent's frame over its active time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NodeMotion {
    /// The direction of `speed` and `acceleration` in the emitter's own frame (a unit vector).
    pub forward: [f32; 3],
    /// Speed along `forward` (metres per second) times its multiplier, and acceleration along it (metres per second squared)
    /// times its multiplier, functions of the active time.
    pub speed: Value,
    pub speed_multiplier: Value,
    pub acceleration: Value,
    pub acceleration_multiplier: Value,
    /// Acceleration along the emitter frame's +Y (metres per second squared), or along the world's +Y when `lift_in_world`.
    pub lift: Value,
    pub lift_in_world: bool,
    /// A displacement of the emitter in its parent's frame (metres), a function of the active time.
    pub displacement: Option<Value>,
    /// Spin about the emitter's three axes (degrees per second) times a multiplier, functions of the active time.
    pub spin: [Value; 3],
    pub spin_multiplier: [Value; 3],
    /// Whether the emitter turns to face its motion.
    pub align: bool,
    pub turns: Option<Turns>,
    pub drag: Option<Drag>,
}

/// What happens to a system when what spawned it goes away.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Termination {
    /// Emitters stop and the particles live out their lives.
    Natural,
    /// Everything fades out over this many seconds.
    Fade(f32),
    /// Everything vanishes at once.
    Instant,
}

/// A particle effect.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParticleSystem {
    /// The seed of every random draw of the system.
    pub seed: u64,
    /// The emitters, parents before children.
    pub emitters: Vec<Emitter>,
    /// Quantities a program may drive.
    pub inputs: Vec<Input>,
    /// The number of states the system has (a source's state machine; a renderer plays state 0 unless it is told another).
    pub states: u32,
    /// Seconds the system is run before it is first shown, so that a placed fire is already burning when its area loads.
    pub warmup: f32,
    /// The rate at which the system's time runs, a function of its active time (1 is real time); `None` for 1.
    pub time_scale: Option<Value>,
    /// Beyond this distance (metres) from the camera nothing of the system is drawn; `None` for any distance.
    pub max_view_distance: Option<f32>,
    /// What happens when the thing that spawned the system goes away (a source's termination).
    pub termination: Termination,
    /// Seconds after which a system with no endless emitter has no live particle: the longest an emitter is active plus its
    /// particles' longest life; `None` when something runs for ever. A renderer that loops a system restarts at this time.
    pub end: Option<f32>,
    /// What the source's pass could not carry, by reason, with a count (a placed and lost accounting).
    pub lost: Vec<(String, u32)>,
}

impl Component for ParticleSystem {
    const NAME: &'static str = "core.particles";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Json>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// The textures, meshes and clips the draws name, and the systems the emitters start.
    fn references(&self, out: &mut Vec<Entity>) {
        for e in &self.emitters {
            out.extend(e.spawns.iter().copied());
            if let Some(d) = &e.draw {
                d.references(out);
            }
        }
    }
    /// Emitters in order with parents before children, curves with ascending finite keys, variations naming an input the
    /// system has, an `end` that is finite, and finite numbers throughout the placement.
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        for (i, e) in self.emitters.iter().enumerate() {
            let at = |what: &str| format!("emitter {i}: {what}");
            if let Some(p) = e.parent
                && usize::try_from(p).map_or(true, |p| p >= i)
            {
                return Err(at(&format!("parent {p} is not an earlier emitter")));
            }
            let finite = e
                .placement
                .cols
                .iter()
                .flatten()
                .chain(&e.jitter_translation)
                .chain(&e.jitter_rotation);
            if !finite.chain([&e.delay, &e.order]).all(|v| v.is_finite()) {
                return Err(at("a placement that is not finite"));
            }
            if e.states.iter().any(|&s| s >= self.states.max(1)) {
                return Err(at("a state the system does not have"));
            }
            e.check(self.inputs.len()).map_err(|m| at(&m))?;
        }
        if self.end.is_some_and(|t| !t.is_finite() || t < 0.0) {
            return Err("an end that is not a finite time".into());
        }
        if !(self.warmup.is_finite() && self.warmup >= 0.0) {
            return Err(format!("a warm-up of {} seconds", self.warmup));
        }
        if self
            .max_view_distance
            .is_some_and(|d| !(d.is_finite() && d > 0.0))
        {
            return Err("a view distance that is not a positive number".into());
        }
        if let Some(v) = &self.time_scale {
            v.check("time scale", self.inputs.len())?;
        }
        if let Termination::Fade(t) = self.termination
            && !(t.is_finite() && t >= 0.0)
        {
            return Err(format!("a fade of {t} seconds"));
        }
        Ok(())
    }
}

impl Emitter {
    fn check(&self, inputs: usize) -> Result<(), String> {
        let mut values: Vec<(&str, &Value)> = vec![
            ("initial speed", &self.law.initial_speed),
            ("tint", &self.law.tint),
            ("size scale x", &self.law.size_scale[0]),
            ("size scale y", &self.law.size_scale[1]),
            ("size scale z", &self.law.size_scale[2]),
        ];
        if let Some(v) = &self.law.lifetime {
            values.push(("lifetime", v));
        }
        match &self.spawn {
            Spawn::Once => {}
            Spawn::Periodic { interval, count, .. } => {
                values.extend([("interval", interval), ("count", count)])
            }
            Spawn::Distance { step, count, .. } => values.extend([("step", step), ("count", count)]),
        }
        match &self.shape {
            Shape::Point => {}
            Shape::Disk {
                radius,
                concentration,
            } => values.extend([("radius", radius), ("concentration", concentration)]),
            Shape::Rectangle { size, concentration } => {
                values.extend(size.iter().map(|v| ("size", v)));
                values.push(("concentration", concentration));
            }
            Shape::Sphere { radius, .. } => values.push(("radius", radius)),
            Shape::Box { size, .. } => values.extend(size.iter().map(|v| ("size", v))),
            Shape::Cylinder { radius, height, .. } => values.extend([("radius", radius), ("height", height)]),
        }
        match &self.spread {
            Spread::None => {}
            Spread::Cone { angle, concentration } => {
                values.extend([("angle", angle), ("concentration", concentration)])
            }
            Spread::Ellipse { x, y, concentration } | Spread::Rectangle { x, y, concentration } => {
                values.extend([("angle", x), ("angle", y), ("concentration", concentration)]);
            }
        }
        if let Some(m) = &self.law.movement {
            match &m.drive {
                Drive::None => {}
                Drive::Speed { speed, multiplier } => {
                    values.extend([("speed", speed), ("speed multiplier", multiplier)])
                }
                Drive::Acceleration {
                    acceleration,
                    multiplier,
                } => {
                    values.extend([
                        ("acceleration", acceleration),
                        ("acceleration multiplier", multiplier),
                    ]);
                }
            }
            values.push(("gravity", &m.gravity));
            if let Some(t) = &m.turns {
                values.push(("turn angle", &t.max_angle));
            }
            if let Some(d) = &m.drag {
                values.push(("drag", &d.factor));
            }
            if let Some(f) = &m.fields {
                values.extend([
                    ("field speed", &f.speed),
                    ("field speed multiplier", &f.speed_multiplier),
                    ("field acceleration", &f.acceleration),
                    ("field acceleration multiplier", &f.acceleration_multiplier),
                ]);
            }
        }
        if let Some(n) = &self.motion {
            values.extend([
                ("node speed", &n.speed),
                ("node speed multiplier", &n.speed_multiplier),
                ("node acceleration", &n.acceleration),
                ("node acceleration multiplier", &n.acceleration_multiplier),
                ("node lift", &n.lift),
            ]);
            values.extend(n.displacement.iter().map(|v| ("node displacement", v)));
            values.extend(n.spin.iter().chain(&n.spin_multiplier).map(|v| ("node spin", v)));
            if let Some(t) = &n.turns {
                values.push(("node turn angle", &t.max_angle));
            }
            if let Some(d) = &n.drag {
                values.push(("node drag", &d.factor));
            }
        }
        for (what, v) in values {
            v.check(what, inputs)?;
        }
        if let Some(d) = &self.draw {
            d.check(inputs)?;
        }
        Ok(())
    }
}

fn push_map(out: &mut Vec<Entity>, m: &Option<Map>) {
    out.extend(m.iter().map(|m| m.texture));
}

impl Draw {
    fn references(&self, out: &mut Vec<Entity>) {
        match self {
            Draw::Quad(q) => {
                push_map(out, &q.map);
                if let Some(l) = &q.lit {
                    push_map(out, &l.normal_map);
                    push_map(out, &l.specular_map);
                }
            }
            Draw::LayeredQuad(q) => {
                push_map(out, &q.quad.map);
                if let Some(l) = &q.quad.lit {
                    push_map(out, &l.normal_map);
                    push_map(out, &l.specular_map);
                }
                for l in &q.layers {
                    push_map(out, &l.map);
                }
            }
            Draw::Point(p) => push_map(out, &p.map),
            Draw::Mesh(m) => {
                out.push(m.mesh);
                out.extend(m.clip.iter().map(|c| c.clip));
            }
            Draw::Trail(t) => {
                push_map(out, &t.map);
                if let Some(d) = &t.distortion {
                    push_map(out, &d.normal_map);
                }
                if let Some(l) = &t.lit {
                    push_map(out, &l.normal_map);
                    push_map(out, &l.specular_map);
                }
            }
            Draw::Streak(s) => push_map(out, &s.map),
            Draw::Refraction(r) => {
                push_map(out, &r.normal_map);
                push_map(out, &r.mask);
            }
            Draw::RadialBlur(b) => push_map(out, &b.mask),
            Draw::Flare(f) => {
                for l in &f.layers {
                    push_map(out, &l.map);
                }
            }
            Draw::Ripples(r) => push_map(out, &r.map),
            Draw::Light(_) | Draw::Field(_) => {}
        }
    }

    /// Every value of the draw, named for an error.
    fn values<'a>(&'a self, out: &mut Vec<(&'static str, &'a Value)>) {
        fn terms<'a>(out: &mut Vec<(&'static str, &'a Value)>, t: &'a [Term]) {
            out.extend(t.iter().map(|t| ("tint", &t.value)));
        }
        fn atlas<'a>(out: &mut Vec<(&'static str, &'a Value)>, a: &'a Option<Atlas>) {
            out.extend(a.iter().map(|a| ("atlas cell", &a.cell)));
        }
        fn quad_values<'a>(out: &mut Vec<(&'static str, &'a Value)>, q: &'a Quad) {
            out.extend(q.size.iter().map(|v| ("size", v)));
            out.extend(q.offset.iter().map(|v| ("offset", v)));
            out.extend(q.rotation.iter().map(|v| ("rotation", v)));
            out.extend(q.spin.iter().map(|v| ("spin", v)));
            out.extend([
                ("alpha fade threshold", &q.alpha_fade_threshold),
                ("alpha cutoff", &q.alpha_cutoff),
            ]);
            out.push(("depth bias", &q.depth_bias));
            terms(out, &q.tint);
            atlas(out, &q.atlas);
        }
        match self {
            Draw::Quad(q) => quad_values(out, q),
            Draw::LayeredQuad(q) => {
                quad_values(out, &q.quad);
                for l in &q.layers {
                    out.push(("layer tint", &l.tint));
                    out.extend(l.scroll.iter().map(|v| ("layer scroll", v)));
                    out.extend(l.offset.iter().map(|v| ("layer offset", v)));
                    out.extend(l.scale.iter().map(|v| ("layer scale", v)));
                }
            }
            Draw::Point(p) => {
                out.extend([("size", &p.size), ("alpha cutoff", &p.alpha_cutoff)]);
                terms(out, &p.tint);
            }
            Draw::Mesh(m) => {
                out.extend(m.size.iter().map(|v| ("size", v)));
                out.extend(m.rotation.iter().map(|v| ("rotation", v)));
                out.extend(m.spin.iter().map(|v| ("spin", v)));
                out.extend(m.scroll.iter().map(|v| ("scroll", v)));
                out.extend(m.offset.iter().map(|v| ("offset", v)));
                terms(out, &m.tint);
                atlas(out, &m.atlas);
            }
            Draw::Trail(t) => {
                out.extend([
                    ("width", &t.width),
                    ("width scale", &t.width_scale),
                    ("texture fraction", &t.texture_fraction),
                    ("scroll", &t.scroll_u),
                    ("variance", &t.variance_v),
                    ("fade start", &t.fade_start),
                    ("fade end", &t.fade_end),
                    ("alpha fade threshold", &t.alpha_fade_threshold),
                    ("alpha cutoff", &t.alpha_cutoff),
                ]);
                out.extend(t.distortion.iter().map(|d| ("distortion", &d.intensity)));
                terms(out, &t.tint);
                atlas(out, &t.atlas);
            }
            Draw::Streak(s) => {
                out.extend([
                    ("length", &s.length),
                    ("start colour", &s.start_colour),
                    ("end colour", &s.end_colour),
                    ("alpha cutoff", &s.alpha_cutoff),
                ]);
                out.extend(s.width.iter().map(|w| ("width", w)));
                terms(out, &s.tint);
            }
            Draw::Refraction(r) => {
                out.extend(r.offset.iter().map(|v| ("offset", v)));
                out.extend(r.size.iter().map(|v| ("size", v)));
                out.extend([
                    ("intensity", &r.intensity),
                    ("stir speed", &r.stir_speed),
                    ("radius", &r.radius),
                    ("alpha cutoff", &r.alpha_cutoff),
                ]);
                out.extend(
                    r.normal_offset
                        .iter()
                        .chain(&r.normal_scroll)
                        .chain(&r.normal_scale)
                        .map(|v| ("normal map", v)),
                );
                terms(out, &r.tint);
            }
            Draw::RadialBlur(b) => {
                out.extend(b.offset.iter().map(|v| ("offset", v)));
                out.extend(b.size.iter().map(|v| ("size", v)));
                out.extend([("radius", &b.radius), ("alpha cutoff", &b.alpha_cutoff)]);
                terms(out, &b.tint);
            }
            Draw::Light(l) => out.extend([
                ("colour", &l.colour),
                ("intensity", &l.intensity),
                ("range", &l.range),
            ]),
            Draw::Flare(f) => {
                for l in &f.layers {
                    out.extend([
                        ("flare width", &l.width),
                        ("flare height", &l.height),
                        ("flare colour", &l.colour),
                    ]);
                }
            }
            Draw::Field(f) => out.push(("force", &f.strength)),
            Draw::Ripples(_) => {}
        }
    }

    fn check(&self, inputs: usize) -> Result<(), String> {
        let mut values = Vec::new();
        self.values(&mut values);
        for (what, v) in values {
            v.check(what, inputs)?;
        }
        let atlas = match self {
            Draw::Quad(q) => q.atlas.as_ref(),
            Draw::LayeredQuad(q) => q.quad.atlas.as_ref(),
            Draw::Mesh(m) => m.atlas.as_ref(),
            Draw::Trail(t) => t.atlas.as_ref(),
            _ => None,
        };
        if atlas.is_some_and(|a| a.columns == 0 || a.cells == 0) {
            return Err("an atlas of no cells".into());
        }
        if let Draw::Quad(q) = self
            && let Orientation::World(n) | Orientation::Frame(n) = q.orientation
            && !n.iter().all(|v| v.is_finite())
        {
            return Err("an orientation that is not finite".into());
        }
        if let Draw::Trail(t) = self
            && !(t.segment_interval.is_finite() && t.segment_life.is_finite() && t.fade_out.is_finite())
        {
            return Err("a trail time that is not finite".into());
        }
        if let Draw::LayeredQuad(q) = self
            && q.layers.is_empty()
        {
            return Err("a layered quad of no layers".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(at: f32, v: f32) -> Key {
        Key {
            at,
            value: [v, 0.0, 0.0, 0.0],
            handle_in: [0.0; 4],
            handle_out: [0.0; 4],
        }
    }

    fn curve(interpolation: Interpolation, keys: Vec<Key>) -> Curve {
        Curve {
            width: 1,
            interpolation,
            looped: false,
            keys,
        }
    }

    #[test]
    fn curves_hold_run_and_loop() {
        let lin = curve(Interpolation::Linear, vec![key(0.0, 0.0), key(2.0, 10.0)]);
        assert_eq!(lin.value_at(-5.0)[0], 0.0);
        assert_eq!(lin.value_at(1.0)[0], 5.0);
        assert_eq!(lin.value_at(9.0)[0], 10.0);
        let step = curve(
            Interpolation::Step,
            vec![key(0.0, 1.0), key(2.0, 2.0), key(4.0, 3.0)],
        );
        assert_eq!(step.value_at(1.9)[0], 1.0);
        assert_eq!(step.value_at(2.0)[0], 2.0);
        let mut looped = lin.clone();
        looped.looped = true;
        assert_eq!(looped.value_at(3.0)[0], 5.0);
        let mut still = curve(Interpolation::Linear, vec![key(0.0, 7.0)]);
        still.looped = true;
        assert_eq!(still.value_at(100.0)[0], 7.0);
    }

    #[test]
    fn a_bezier_segment_is_the_cubic_through_its_handles() {
        let mut a = key(0.0, 0.0);
        a.handle_out = [1.0, 0.0, 0.0, 0.0];
        let mut b = key(1.0, 1.0);
        b.handle_in = [1.0, 0.0, 0.0, 0.0];
        let c = curve(Interpolation::Bezier, vec![a, b]);
        // control points 0, 1, 0, 1 in u
        let want = |u: f32| {
            let k = 1.0 - u;
            3.0 * k * k * u + u * u * u
        };
        for u in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((c.value_at(u)[0] - want(u)).abs() < 1e-6, "{u}");
        }
    }

    #[test]
    fn draws_are_deterministic_uniform_and_keyed() {
        let a = unit(7, 1, 2, 3, 0);
        assert_eq!(a, unit(7, 1, 2, 3, 0));
        assert!((0.0..1.0).contains(&a));
        // any key changes the draw
        for other in [
            unit(8, 1, 2, 3, 0),
            unit(7, 2, 2, 3, 0),
            unit(7, 1, 3, 3, 0),
            unit(7, 1, 2, 4, 0),
            unit(7, 1, 2, 3, 1),
        ] {
            assert_ne!(a, other);
        }
        // roughly uniform over many scopes
        let n = 20_000;
        let mean = (0..n).map(|i| f64::from(unit(1, 0, i, 0, 0))).sum::<f64>() / n as f64;
        assert!((mean - 0.5).abs() < 0.01, "{mean}");
    }

    #[test]
    fn variations_apply_in_order_within_their_bounds() {
        let mut v = Value::constant(&[10.0]);
        v.variation.push(Variation::Delta {
            max: [2.0; 4],
            stream: 0,
        });
        v.variation.push(Variation::Fraction {
            max: [0.1; 4],
            stream: 1,
        });
        let none = |_: u32| 0.0;
        for scope in 0..200 {
            let x = v.eval(0.0, 5, 0, scope, &none)[0];
            assert!((8.0 * 0.9..=12.0 * 1.1).contains(&x), "{x}");
        }
        // the same scope gives the same value
        assert_eq!(v.eval(0.0, 5, 0, 9, &none), v.eval(3.0, 5, 0, 9, &none));
        // an input-scaled value reads its curve at the input
        let mut scaled = Value::constant(&[4.0]);
        scaled.variation.push(Variation::Scaled {
            input: 0,
            factor: curve(Interpolation::Linear, vec![key(0.0, 0.0), key(1.0, 1.0)]),
        });
        assert_eq!(scaled.eval(0.0, 1, 0, 0, &|_| 0.25)[0], 1.0);
    }

    #[test]
    fn a_constant_value_is_fixed() {
        let v = Value::constant(&[1.0, 2.0]);
        assert!(v.is_fixed());
        assert_eq!(v.curve.value_at(9.0)[..2], [1.0, 2.0]);
        let mut w = v.clone();
        w.variation.push(Variation::Delta {
            max: [0.1; 4],
            stream: 3,
        });
        assert!(!w.is_fixed());
    }
}
