//! The GPU particle systems (actions 10000 and 10001): one action that is at once an emitter, a law and an appearance. They
//! are lowered to the ordinary pieces: a periodic spawn, a shape, a law and a quad. What the ordinary pieces have no
//! place for (collision, trace stretching, per-particle random rotation and angular speed, growth acceleration, the
//! second and third axes of a vector speed's randomness) is counted.

use super::cx::Cx;
use super::units;
use super::value::Notes;
use fromsoft_formats::fxr::actions as a;
use fromsoft_formats::fxr::resources::TextureKind;
use primase_core::particles::{
    Anchor, Atlas, Attachment, Axis, Channels, Draw, Drive, Follow, Heading, Law, Movement, Orientation, Quad, Shape, Spawn,
    Spread, Term, Value, Variation,
};

/// What a GPU particle action lowers to: the parts of an emitter it defines.
pub struct Gpu {
    pub spawn: Spawn,
    pub shape: Shape,
    pub heading: Heading,
    pub spread: Spread,
    pub law: Law,
    pub draw: Draw,
}

/// The fields of a GPU particle action that the lowering reads (the two action types store the same ones).
struct GpuIn<'a> {
    follow: &'a fromsoft_formats::fxr::Property,
    gravity: &'a fromsoft_formats::fxr::Property,
    colour: &'a fromsoft_formats::fxr::Property,
    texture: i32,
    normal_map: i32,
    emitter_shape: i32,
    emitter_size: [f32; 3],
    interval: (i32, i32),
    count: (i32, i32, i32),
    limit: Option<i32>,
    duration: i32,
    duration_multiplier: f32,
    speed: ([f32; 3], [f32; 3], [f32; 3]),
    acceleration: ([f32; 3], [f32; 3]),
    size: ([f32; 2], [f32; 2], [f32; 2]),
    size_multiplier: f32,
    uniform: bool,
    rgb: f32,
    alpha: f32,
    colour_range: ([f32; 4], [f32; 4]),
    blend: i32,
    columns: i32,
    total_frames: i32,
    random_frame: bool,
    lighting: i32,
    specular: i32,
    glossiness: f32,
    specularity: f32,
    shadow: f32,
    bloom: Option<[f32; 4]>,
    distance: primase_core::particles::DistanceFade,
    collision: bool,
    trace: bool,
    random_turns: bool,
}

macro_rules! gpu_in {
    ($g:expr) => {{
        let g = $g;
        GpuIn {
            follow: &g.particle_follow_factor,
            gravity: &g.particle_gravity,
            colour: &g.color,
            texture: g.texture,
            normal_map: g.normal_map,
            emitter_shape: g.emitter_shape,
            emitter_size: g.emitter_size,
            interval: (g.emission_interval_min, g.emission_interval_max),
            count: (g.emission_particle_count, g.emission_particle_count_min, g.emission_particle_count_max),
            limit: g.limit_emission_count.then_some(g.emission_count_limit),
            duration: g.particle_duration,
            duration_multiplier: g.particle_duration_multiplier,
            speed: (g.particle_speed, g.particle_speed_min, g.particle_speed_max),
            acceleration: (g.particle_acceleration_min, g.particle_acceleration_max),
            size: (
                [g.particle_size_x, g.particle_size_y],
                [g.particle_size_xmin, g.particle_size_ymin],
                [g.particle_size_xmax, g.particle_size_ymax],
            ),
            size_multiplier: g.particle_size_multiplier,
            uniform: g.particle_uniform_scale,
            rgb: g.rgb_multiplier,
            alpha: g.alpha_multiplier,
            colour_range: (g.color_min, g.color_max),
            blend: g.blend_mode,
            columns: g.columns,
            total_frames: g.total_frames,
            random_frame: g.random_texture_frame,
            lighting: g.lighting,
            specular: g.specular,
            glossiness: g.glossiness,
            specularity: g.specularity,
            shadow: g.shadow_darkness,
            bloom: g.bloom.then_some(g.bloom_color),
            distance: units::distance_fade(
                g.min_fade_distance,
                g.min_distance,
                g.max_fade_distance,
                g.max_distance,
                g.min_distance_threshold,
                g.max_distance_threshold,
            ),
            collision: g.particle_collision,
            trace: g.trace_particles,
            random_turns: g.particle_random_turns,
        }
    }};
}

/// A constant `base` with a random addition between `lo` and `hi` (a range variation on stream `stream`).
fn ranged(base: f32, lo: f32, hi: f32, stream: u32) -> Value {
    let mut v = Value::constant(&[base]);
    if lo != 0.0 || hi != 0.0 {
        v.variation.push(Variation::Range { min: [lo, 0.0, 0.0, 0.0], max: [hi, 0.0, 0.0, 0.0], stream });
    }
    v
}

fn norm(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

impl Cx<'_> {
    /// A GPU particle system (10000).
    pub fn gpu_standard(&mut self, g: &a::GPUStandardParticle) -> Gpu {
        let input = gpu_in!(g);
        self.gpu(input)
    }

    /// A GPU particle system (10001, "correct").
    pub fn gpu_correct(&mut self, g: &a::GPUStandardCorrectParticle) -> Gpu {
        let input = gpu_in!(g);
        self.gpu(input)
    }

    fn gpu(&mut self, g: GpuIn<'_>) -> Gpu {
        let notes: &mut Notes = &mut self.notes;
        notes.approximate("a GPU particle system as an ordinary emitter, law and quad");
        for (on, what) in [
            (g.collision, "the depth-buffer collision of GPU particles"),
            (g.trace, "the trace stretching of GPU particles"),
            (g.random_turns, "the random turns of GPU particles"),
            (g.random_frame, "a random texture frame per GPU particle"),
        ] {
            if on {
                notes.lose(what);
            }
        }
        notes.lose("the rotation, angular speed and growth of GPU particles");
        // emission: a count that varies by emission, every interval between the bounds
        let interval = ranged(0.0, units::frames(g.interval.0), units::frames(g.interval.1.max(g.interval.0)), 0);
        let count = ranged(g.count.0 as f32, g.count.1 as f32, g.count.2.max(g.count.1) as f32, 1);
        let spawn = Spawn::Periodic {
            interval,
            count,
            total: g.limit.and_then(|n| u32::try_from(n).ok()),
            max_alive: None,
        };
        let size = g.emitter_size;
        let shape = match g.emitter_shape {
            0 | 3 => {
                notes.approximate("a GPU particle line emitter as a box thin on two axes");
                Shape::Box { size: [Value::constant(&[size[0]]), Value::constant(&[0.0]), Value::constant(&[0.0])], volume: true }
            }
            4 => {
                notes.approximate("a GPU particle cylinder emitter's radius and height taken from its size");
                Shape::Cylinder {
                    radius: Value::constant(&[size[0] * 0.5]),
                    height: Value::constant(&[size[1]]),
                    volume: true,
                    axis: Axis::Y,
                }
            }
            1 | 2 | 5 => Shape::Box { size: size.map(|s| Value::constant(&[s])), volume: true },
            _ => {
                notes.lose("a GPU emitter shape the project does not name, taken as a box");
                Shape::Box { size: size.map(|s| Value::constant(&[s])), volume: true }
            }
        };
        // speed: the vector speed plus a random part, as a speed along the heading its direction gives
        let base = g.speed.0;
        let mid = [0, 1, 2].map(|i| base[i] + 0.5 * (g.speed.1[i] + g.speed.2[i]));
        let heading = if norm(mid) > 1e-6 {
            let n = norm(mid);
            Heading::Frame(units::dir([mid[0] / n, mid[1] / n, mid[2] / n]))
        } else {
            Heading::Shape { facing: units::FORWARD }
        };
        let speed = Value::constant(&[norm(mid)]);
        let acc_mid = [0, 1, 2].map(|i| 0.5 * (g.acceleration.0[i] + g.acceleration.1[i]));
        if norm(acc_mid) > 1e-6 {
            notes.lose("the acceleration of GPU particles");
        }
        let gravity = self.age(g.gravity);
        let drag_factor = self.age(g.follow);
        let movement = Movement {
            drive: if norm(mid) > 1e-6 { Drive::Speed { speed, multiplier: Value::constant(&[1.0]) } } else { Drive::None },
            gravity,
            turns: None,
            drag: Some(primase_core::particles::Drag { factor: drag_factor, rotation: false }),
            fields: None,
        };
        let lifetime = units::frames(g.duration) * g.duration_multiplier;
        let law = Law {
            lifetime: (lifetime > 0.0).then(|| Value::constant(&[lifetime])),
            attachment: Attachment { anchor: Anchor::Parent, follow: Follow::None },
            movement: Some(movement),
            initial_speed: Value::constant(&[0.0]),
            size_scale: [Value::constant(&[1.0]), Value::constant(&[1.0]), Value::constant(&[1.0])],
            tint: Value::constant(&[1.0; 4]),
        };
        // appearance: a camera-facing quad of the size between the bounds
        let map = self.map_word(TextureKind::Albedo, g.texture);
        let sx = (g.size.0[0] + 0.5 * (g.size.1[0] + g.size.2[0])) * g.size_multiplier;
        let sy = if g.uniform { sx } else { (g.size.0[1] + 0.5 * (g.size.1[1] + g.size.2[1])) * g.size_multiplier };
        let blend = units::blend(g.blend, &mut self.notes);
        let colour = self.age(g.colour);
        let mut tint = vec![
            Term { value: colour, clamp: false, channels: Channels::Rgba },
            Term { value: Value::constant(&[g.rgb]), clamp: false, channels: Channels::Rgb },
            Term { value: Value::constant(&[g.alpha]), clamp: false, channels: Channels::Alpha },
        ];
        if g.colour_range.0 != [0.0; 4] || g.colour_range.1 != [0.0; 4] {
            self.notes.lose("the random colour variation of GPU particles");
        }
        tint = super::cx::without_identities(tint);
        let atlas = (g.columns > 1 || g.total_frames > 1).then(|| Atlas {
            columns: u32::try_from(g.columns.max(1)).unwrap_or(1),
            cells: u32::try_from(g.total_frames.max(1)).unwrap_or(1),
            cell: Value::constant(&[0.0]),
            blend_cells: false,
        });
        let lit = if units::lit(g.lighting, &mut self.notes) {
            Some(primase_core::particles::Lit {
                normal_map: self.map_word(TextureKind::Normal, g.normal_map),
                specular_map: self.map_word(TextureKind::Specular, g.specular),
                glossiness: g.glossiness,
                specularity: g.specularity,
                shadow: g.shadow,
            })
        } else {
            None
        };
        let zero = || Value::constant(&[0.0]);
        let quad = Quad {
            map,
            blend,
            orientation: Orientation::CameraPlane,
            size: [Value::constant(&[sx]), Value::constant(&[sy])],
            uniform: g.uniform,
            size_variance: [1.0, 1.0],
            offset: [zero(), zero(), zero()],
            rotation: [zero(), zero(), zero()],
            spin: [zero(), zero(), zero()],
            scale_before_rotation: true,
            atlas,
            tint,
            alpha_fade_threshold: zero(),
            alpha_cutoff: zero(),
            soft: None,
            distance: g.distance,
            octagonal: false,
            lit,
            bloom: g.bloom,
            premultiply_alpha: false,
            depth_bias: zero(),
        };
        Gpu { spawn, shape, heading, spread: Spread::None, law, draw: Draw::Quad(quad) }
    }
}
