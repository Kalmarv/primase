//! An effect file's node tree as the emitters of a [`ParticleSystem`].
//!
//! A node becomes one emitter per config its states choose (a node with one config, the usual, one emitter); the children of
//! a node hang from its emitter of the lowest state that chooses a config. What a config holds, one action to a slot, becomes
//! the emitter's fields: the emitter action its spawn, the shape its place and heading, the spread its scatter, the particle
//! actions its law and the appearance its draw. Everything the core description has no field for is counted in [`Notes`].

use super::cx::{Cx, Resolver};
use super::units;
use super::value::{self, Notes};
use crate::pose::placed;
use fromsoft_formats::Game;
use fromsoft_formats::fxr::actions as a;
use fromsoft_formats::fxr::actions::Typed;
use fromsoft_formats::fxr::{Fxr, StaticTransform, TypedConfig, TypedNode};
use primase_core::Transform;
use primase_core::particles::{
    Anchor, Attachment, Axis, Drag, Drive, Emitter, FieldResponse, Follow, Law, Movement, NodeMotion,
    ParticleSystem, Pick, Shape, Spawn, Spread, Termination, Turns, Value,
};

/// An effect lowered, with what the lowering counted.
#[derive(Clone, Debug, PartialEq)]
pub struct Lowered {
    pub system: ParticleSystem,
    pub notes: Notes,
}

/// The node ids the project names.
const ROOT: i16 = 2000;
const PROXY: i16 = 2001;
const LEVELS_OF_DETAIL: i16 = 2002;
const NODE_EMITTER: i16 = 2202;

/// The value of the files' "no limit" distance, which a level-of-detail range's upper bound at or above means none.
pub const NO_LIMIT: f32 = 10_000.0;

/// Lowers `fx` for `game` into a system whose random draws are keyed by `seed`, naming files through `resolver`.
pub fn lower(fx: &Fxr, game: Game, seed: u64, resolver: &mut dyn Resolver) -> Result<Lowered, String> {
    let tree = fx.typed(game).map_err(|e| format!("effect {}: {e}", fx.id))?;
    let states = u32::try_from(fx.states.len().max(1)).unwrap_or(u32::MAX);
    let mut walk = Walk {
        cx: Cx::new(game, resolver),
        emitters: Vec::new(),
        states,
        system: Settings::default(),
    };
    if fx.states.iter().any(|s| !s.conditions.is_empty()) {
        walk.cx
            .notes
            .lose("the conditions that move an effect between its states");
    }
    walk.node(&tree, None, None, "root".to_owned());
    let Walk {
        cx, emitters, system, ..
    } = walk;
    let end = end_time(&emitters);
    let notes = cx.notes;
    let lost = notes.lost.iter().map(|(k, n)| (k.clone(), *n)).collect();
    let system = ParticleSystem {
        seed,
        emitters,
        inputs: cx.inputs.list,
        states,
        warmup: system.warmup,
        time_scale: system.time_scale,
        max_view_distance: system.max_view_distance,
        termination: system.termination,
        end,
        lost,
    };
    Ok(Lowered { system, notes })
}

/// What the root node sets for the whole system.
#[derive(Debug)]
struct Settings {
    warmup: f32,
    time_scale: Option<Value>,
    max_view_distance: Option<f32>,
    termination: Termination,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            warmup: 0.0,
            time_scale: None,
            max_view_distance: None,
            termination: Termination::Natural,
        }
    }
}

struct Walk<'r> {
    cx: Cx<'r>,
    emitters: Vec<Emitter>,
    states: u32,
    system: Settings,
}

/// The actions of one config, one to a slot.
#[derive(Default)]
struct Parts<'a> {
    node_attributes: Option<&'a a::NodeAttributes>,
    particle_attributes: Option<&'a a::ParticleAttributes>,
    modifier: Option<&'a a::ParticleModifier>,
    emitter: Option<&'a Typed>,
    shape: Option<&'a Typed>,
    spread: Option<&'a Typed>,
    particle_movement: Option<&'a Typed>,
    particle_force: Option<&'a Typed>,
    node_movement: Option<&'a Typed>,
    transform: Option<&'a Typed>,
    appearance: Option<&'a Typed>,
    selector: Option<&'a Typed>,
    thresholds: Option<&'a a::LevelsOfDetailThresholds>,
}

fn put<'a, T>(slot: &mut Option<&'a T>, value: &'a T, notes: &mut Notes) {
    if slot.replace(value).is_some() {
        notes.lose("a second action of one slot in a config");
    }
}

impl<'a> Parts<'a> {
    fn of(config: &'a TypedConfig, notes: &mut Notes) -> Parts<'a> {
        let mut p = Parts::default();
        for action in &config.actions {
            match action {
                Typed::NodeAttributes(x) => put(&mut p.node_attributes, x, notes),
                Typed::ParticleAttributes(x) => put(&mut p.particle_attributes, x, notes),
                Typed::ParticleModifier(x) => put(&mut p.modifier, x, notes),
                Typed::PeriodicEmitter(_) | Typed::EqualDistanceEmitter(_) | Typed::OneTimeEmitter(_) => {
                    put(&mut p.emitter, action, notes)
                }
                Typed::PointEmitterShape(_)
                | Typed::DiskEmitterShape(_)
                | Typed::RectangleEmitterShape(_)
                | Typed::SphereEmitterShape(_)
                | Typed::BoxEmitterShape(_)
                | Typed::CylinderEmitterShape(_) => put(&mut p.shape, action, notes),
                Typed::NoSpread(_)
                | Typed::CircularSpread(_)
                | Typed::EllipticalSpread(_)
                | Typed::RectangularSpread(_) => put(&mut p.spread, action, notes),
                Typed::ParticleAcceleration(_)
                | Typed::ParticleSpeed(_)
                | Typed::ParticleSpeedRandomTurns(_)
                | Typed::ParticleSpeedPartialFollow(_)
                | Typed::ParticleAccelerationRandomTurns(_)
                | Typed::ParticleAccelerationPartialFollow(_) => put(&mut p.particle_movement, action, notes),
                Typed::ParticleForceSpeed(_) | Typed::ParticleForceAcceleration(_) => {
                    put(&mut p.particle_force, action, notes)
                }
                Typed::NodeAcceleration(_)
                | Typed::Unk11(_)
                | Typed::NodeTranslation(_)
                | Typed::NodeSpin(_)
                | Typed::NodeAttachToCamera(_)
                | Typed::NodeAccelerationRandomTurns(_)
                | Typed::NodeAccelerationPartialFollow(_)
                | Typed::NodeAccelerationSpin(_)
                | Typed::NodeSpeed(_)
                | Typed::NodeSpeedRandomTurns(_)
                | Typed::NodeSpeedPartialFollow(_)
                | Typed::NodeSpeedSpin(_) => put(&mut p.node_movement, action, notes),
                Typed::StaticNodeTransform(_) | Typed::RandomNodeTransform(_) => {
                    put(&mut p.transform, action, notes)
                }
                other if is_appearance(other) => put(&mut p.appearance, action, notes),
                Typed::SelectAllNodes(_) | Typed::SelectRandomNode(_) => put(&mut p.selector, action, notes),
                Typed::LevelsOfDetailThresholds(x) => put(&mut p.thresholds, x, notes),
                Typed::NodeSound(_) => notes.lose("a node sound"),
                Typed::NodeForceSpeed(_) | Typed::NodeForceAcceleration(_) => {
                    notes.lose("a node's response to force fields")
                }
                Typed::Unk130(_) => notes.lose("action 130, in every basic config (unknown)"),
                _ => notes.lose("an action in a config that the lowering does not read"),
            }
        }
        p
    }
}

/// Whether an action is of the project's Appearance slot (it draws, lights or forces: what a config's draw is made from).
fn is_appearance(a: &Typed) -> bool {
    matches!(
        a,
        Typed::PointSprite(_)
            | Typed::Line(_)
            | Typed::QuadLine(_)
            | Typed::BillboardEx(_)
            | Typed::MultiTextureBillboardEx(_)
            | Typed::Model(_)
            | Typed::LegacyTracer(_)
            | Typed::Distortion(_)
            | Typed::RadialBlur(_)
            | Typed::PointLight(_)
            | Typed::GPUStandardParticle(_)
            | Typed::GPUStandardCorrectParticle(_)
            | Typed::Tracer(_)
            | Typed::WaterInteraction(_)
            | Typed::LensFlare(_)
            | Typed::RichModel(_)
            | Typed::WindForce(_)
            | Typed::GravityForce(_)
            | Typed::SpotLight(_)
    )
}

/// The configs of a node the system's states choose, each with the states that choose it (empty: every state), the lowest
/// state first; the first entry holds the node's frame.
fn chosen(n: &TypedNode, states: u32, notes: &mut Notes) -> Vec<(usize, Vec<u32>)> {
    match n.state_configs() {
        Some(map) => {
            let mut out: Vec<(usize, Vec<u32>)> = Vec::new();
            for (s, &c) in map.iter().enumerate() {
                let (Ok(c), Ok(s)) = (usize::try_from(c), u32::try_from(s)) else {
                    continue;
                };
                if c >= n.configs.len() {
                    continue;
                }
                match out.iter_mut().find(|(k, _)| *k == c) {
                    Some((_, list)) => list.push(s),
                    None => out.push((c, vec![s])),
                }
            }
            for (c, config) in n.configs.iter().enumerate() {
                if !out.iter().any(|(k, _)| *k == c) {
                    notes.lose("a config that no state chooses");
                    for action in config.actions.iter().filter(|a| is_appearance(a)) {
                        notes.lose(format!(
                            "appearance action {} in a config that no state chooses",
                            action.id()
                        ));
                    }
                }
            }
            for (_, list) in &mut out {
                if u32::try_from(list.len()).is_ok_and(|n| n >= states) {
                    list.clear();
                }
            }
            out
        }
        None => match n.configs.len() {
            0 => Vec::new(),
            1 => vec![(0, Vec::new())],
            _ => {
                notes.lose("a node with several configs and no state-config map");
                vec![(0, Vec::new())]
            }
        },
    }
}

impl Walk<'_> {
    fn push(&mut self, e: Emitter) -> u32 {
        self.emitters.push(e);
        u32::try_from(self.emitters.len() - 1).unwrap_or(u32::MAX)
    }

    /// The node `n` under `parent` (the emitter whose frame it hangs from), within the level-of-detail range `lod`.
    fn node(&mut self, n: &TypedNode, parent: Option<u32>, lod: Option<[f32; 2]>, name: String) {
        let mut frame = parent;
        match n.id {
            ROOT => self.root(n),
            PROXY => {
                for action in &n.actions {
                    if let Typed::SFXReference(r) = action {
                        let mut e = blank(parent, Some(format!("{name} spawns {}", r.sfx)));
                        e.lod_range = lod;
                        match self.cx.resolver.effect(r.sfx) {
                            Some(entity) => e.spawns.push(entity),
                            None => self
                                .cx
                                .notes
                                .lose("a spawned effect that is not in the effect binders or closes a cycle"),
                        }
                        frame = Some(self.push(e));
                    }
                }
            }
            _ => {
                let chosen = chosen(n, self.states, &mut self.cx.notes);
                let mut first = true;
                let mut thresholds = None;
                for (c, states) in chosen {
                    let Some(config) = n.configs.get(c) else { continue };
                    let label = format!("{name} config {c}");
                    let (emitter, limits) = self.config(n, config, parent, lod, states, label);
                    let index = self.push(emitter);
                    if first {
                        frame = Some(index);
                        thresholds = limits;
                        first = false;
                    }
                }
                if n.configs.len() > 1 && self.states > 1 {
                    self.cx.notes.approximate(
                        "the children of a node with several configs hang from its first state's config",
                    );
                }
                if n.id == LEVELS_OF_DETAIL && n.children.len() > 5 {
                    self.cx
                        .notes
                        .lose("a level-of-detail node with more than five children");
                }
                for (i, child) in n.children.iter().enumerate() {
                    let range = match (n.id, thresholds) {
                        (LEVELS_OF_DETAIL, Some(t)) => {
                            let upper = t.get(i).copied().unwrap_or(NO_LIMIT);
                            let lower = if i == 0 {
                                0.0
                            } else {
                                t.get(i - 1).copied().unwrap_or(0.0)
                            };
                            self.cx
                                .notes
                                .approximate("level-of-detail ranges taken as ascending distance thresholds");
                            Some([lower, upper])
                        }
                        _ => lod,
                    };
                    self.node(child, frame, range, format!("{name}/{i}"));
                }
                return;
            }
        }
        for (i, child) in n.children.iter().enumerate() {
            self.node(child, frame, lod, format!("{name}/{i}"));
        }
    }

    /// The root node's own actions: what they set for the whole system.
    fn root(&mut self, n: &TypedNode) {
        let mut termination = None;
        for action in &n.actions {
            match action {
                Typed::SimulateTermination(_) => termination = termination.or(Some(Termination::Natural)),
                Typed::FadeTermination(f) => {
                    termination = termination.or(Some(Termination::Fade(value::number(&f.duration))))
                }
                Typed::InstantTermination(_) => termination = termination.or(Some(Termination::Instant)),
                Typed::Unk10500(u) => {
                    let rate = self.cx.active(&u.rate_of_time);
                    if !(rate.is_fixed() && value::number_of(&rate) == 1.0) {
                        self.system.time_scale = Some(rate);
                    }
                    if u.limit_view_distance && u.max_view_distance > 0.0 {
                        self.system.max_view_distance = Some(u.max_view_distance);
                    }
                    self.system.warmup = units::frames(u.initial_simulation_time).max(0.0);
                }
                Typed::Unk10100(_) | Typed::Unk10400(_) => {
                    self.cx.notes.lose("the unknown root actions 10100 and 10400")
                }
                Typed::StateConfigMap(_) => {}
                _ => self
                    .cx
                    .notes
                    .lose("an action on the root node that the lowering does not read"),
            }
        }
        if n.actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    Typed::SimulateTermination(_) | Typed::FadeTermination(_) | Typed::InstantTermination(_)
                )
            })
            .count()
            > 1
        {
            self.cx.notes.lose("more than one termination action on the root");
        }
        self.system.termination = termination.unwrap_or(Termination::Natural);
    }

    /// The emitter of one config of node `n`, and (for a level-of-detail node) the thresholds its config holds.
    fn config(
        &mut self,
        n: &TypedNode,
        config: &TypedConfig,
        parent: Option<u32>,
        lod: Option<[f32; 2]>,
        states: Vec<u32>,
        name: String,
    ) -> (Emitter, Option<[f32; 5]>) {
        let parts = Parts::of(config, &mut self.cx.notes);
        let mut e = blank(parent, Some(name));
        e.states = states;
        e.lod_range = lod;
        // placement
        match parts.transform {
            Some(Typed::StaticNodeTransform(t)) => e.placement = transform(t.offset, t.rotation),
            Some(Typed::RandomNodeTransform(t)) => {
                e.placement = transform(t.offset, t.rotation);
                e.jitter_translation = t.offset_variance;
                e.jitter_rotation = t.rotation_variance;
            }
            _ => {}
        }
        // life
        if let Some(attrs) = parts.node_attributes {
            e.delay = attrs.delay.max(0.0);
            let duration = value::number(&attrs.duration);
            e.duration = (duration >= 0.0).then_some(duration);
            e.attachment = units::attachment(attrs.attachment, &mut self.cx.notes);
            e.order = attrs.depth_bias;
        }
        // movement of the emitter
        if let Some(m) = parts.node_movement {
            e.motion = self.node_motion(m);
        }
        // spawn, shape, spread
        match parts.emitter {
            Some(Typed::PeriodicEmitter(p)) => {
                e.spawn = Spawn::Periodic {
                    interval: self.cx.active(&p.interval),
                    count: self.cx.active(&p.per_emission),
                    total: value::count(&p.total_emissions),
                    max_alive: value::count(&p.max_concurrent),
                };
            }
            Some(Typed::EqualDistanceEmitter(p)) => {
                e.spawn = Spawn::Distance {
                    step: self.cx.active(&p.threshold),
                    count: Value::constant(&[1.0]),
                    total: value::count(&p.total_emissions),
                    max_alive: value::count(&p.max_concurrent),
                };
            }
            _ => {}
        }
        if let Some(s) = parts.shape {
            let (shape, heading) = self.shape(s);
            e.shape = shape;
            e.heading = heading;
        }
        if let Some(s) = parts.spread {
            e.spread = self.spread(s);
        }
        // the law of the particles
        if let Some(p) = parts.particle_attributes {
            let life = value::number(&p.duration);
            e.law.lifetime = (life >= 0.0).then(|| Value::constant(&[life]));
            e.law.attachment = units::attachment(p.attachment, &mut self.cx.notes);
        }
        if let Some(m) = parts.modifier {
            e.law.initial_speed = self.cx.active(&m.speed);
            let x = self.cx.emitted(&m.scale_x);
            e.law.size_scale = if m.uniform_scale {
                [x.clone(), x.clone(), x]
            } else {
                [x, self.cx.emitted(&m.scale_y), self.cx.emitted(&m.scale_z)]
            };
            e.law.tint = self.cx.active(&m.color);
        }
        e.law.movement = self.movement(parts.particle_movement, parts.particle_force);
        // the draw
        match parts.appearance {
            Some(Typed::GPUStandardParticle(g)) => {
                let gpu = self.cx.gpu_standard(g);
                self.adopt(&mut e, gpu);
            }
            Some(Typed::GPUStandardCorrectParticle(g)) => {
                let gpu = self.cx.gpu_correct(g);
                self.adopt(&mut e, gpu);
            }
            Some(action) => e.draw = self.cx.appearance(action),
            None => {}
        }
        if let (Some(action), true) = (parts.appearance, e.draw.is_some()) {
            self.cx.notes.place(format!("draw of action {}", action.id()));
        }
        // an emitter of nodes
        if n.id == NODE_EMITTER {
            e.copies = Some(match parts.selector {
                Some(Typed::SelectRandomNode(_)) => Pick::One,
                _ => Pick::All,
            });
        } else if parts.selector.is_some() {
            self.cx
                .notes
                .lose("a node selector on a node that is not a node emitter");
        }
        let limits = parts.thresholds.map(|t| {
            if let Some(d) = Some(value::number(&t.duration)).filter(|d| *d >= 0.0) {
                e.duration = Some(d);
            }
            [
                t.threshold0,
                t.threshold1,
                t.threshold2,
                t.threshold3,
                t.threshold4,
            ]
            .map(|v| v as f32)
        });
        (e, limits)
    }

    /// A GPU particle action defines the spawn, shape, law and draw of its emitter at once.
    fn adopt(&mut self, e: &mut Emitter, gpu: super::gpu::Gpu) {
        e.spawn = gpu.spawn;
        e.shape = gpu.shape;
        e.heading = gpu.heading;
        e.spread = gpu.spread;
        e.law = gpu.law;
        e.draw = Some(gpu.draw);
    }

    fn shape(&mut self, s: &Typed) -> (Shape, primase_core::particles::Heading) {
        use primase_core::particles::Heading;
        let facing = Heading::Shape {
            facing: units::FORWARD,
        };
        match s {
            Typed::PointEmitterShape(p) => (Shape::Point, units::heading(p.direction, &mut self.cx.notes)),
            Typed::DiskEmitterShape(d) => (
                Shape::Disk {
                    radius: self.cx.active(&d.radius),
                    concentration: self.cx.active(&d.distribution),
                },
                units::heading(d.direction, &mut self.cx.notes),
            ),
            Typed::RectangleEmitterShape(r) => (
                Shape::Rectangle {
                    size: [self.cx.active(&r.size_x), self.cx.active(&r.size_y)],
                    concentration: self.cx.active(&r.distribution),
                },
                units::heading(r.direction, &mut self.cx.notes),
            ),
            Typed::SphereEmitterShape(r) => (
                Shape::Sphere {
                    radius: self.cx.active(&r.radius),
                    volume: r.emit_inside,
                },
                facing,
            ),
            Typed::BoxEmitterShape(b) => (
                Shape::Box {
                    size: [
                        self.cx.active(&b.size_x),
                        self.cx.active(&b.size_y),
                        self.cx.active(&b.size_z),
                    ],
                    volume: b.emit_inside,
                },
                units::heading(b.direction, &mut self.cx.notes),
            ),
            Typed::CylinderEmitterShape(c) => (
                Shape::Cylinder {
                    radius: self.cx.active(&c.radius),
                    height: self.cx.active(&c.height),
                    volume: c.emit_inside,
                    axis: if c.y_axis { Axis::Y } else { Axis::Z },
                },
                units::heading(c.direction, &mut self.cx.notes),
            ),
            _ => (Shape::Point, facing),
        }
    }

    fn spread(&mut self, s: &Typed) -> Spread {
        match s {
            Typed::CircularSpread(c) => Spread::Cone {
                angle: self.cx.active(&c.angle),
                concentration: self.cx.active(&c.distribution),
            },
            Typed::EllipticalSpread(c) => Spread::Ellipse {
                x: self.cx.active(&c.angle_x),
                y: self.cx.active(&c.angle_y),
                concentration: self.cx.active(&c.distribution),
            },
            Typed::RectangularSpread(c) => Spread::Rectangle {
                x: self.cx.active(&c.angle_x),
                y: self.cx.active(&c.angle_y),
                concentration: self.cx.active(&c.distribution),
            },
            _ => Spread::None,
        }
    }

    /// How particles move: the drive and gravity of the movement action, its random turns and partial following, and the
    /// response to force fields.
    fn movement(&mut self, movement: Option<&Typed>, force: Option<&Typed>) -> Option<Movement> {
        let fields = force.map(|f| match f {
            Typed::ParticleForceSpeed(x) => FieldResponse {
                speed: self.cx.active(&x.speed),
                speed_multiplier: self.cx.active(&x.speed_multiplier),
                acceleration: Value::constant(&[0.0]),
                acceleration_multiplier: Value::constant(&[1.0]),
            },
            Typed::ParticleForceAcceleration(x) => FieldResponse {
                speed: Value::constant(&[0.0]),
                speed_multiplier: Value::constant(&[1.0]),
                acceleration: self.cx.active(&x.acceleration),
                acceleration_multiplier: self.cx.active(&x.acceleration_multiplier),
            },
            _ => FieldResponse {
                speed: Value::constant(&[0.0]),
                speed_multiplier: Value::constant(&[1.0]),
                acceleration: Value::constant(&[0.0]),
                acceleration_multiplier: Value::constant(&[1.0]),
            },
        });
        let Some(m) = movement else {
            return fields.map(|f| Movement {
                drive: Drive::None,
                gravity: Value::constant(&[0.0]),
                turns: None,
                drag: None,
                fields: Some(f),
            });
        };
        let turns = |this: &mut Self, angle: &fromsoft_formats::fxr::Property, interval: i32| Turns {
            interval: units::turn_interval(interval),
            max_angle: this.cx.age(angle),
        };
        let drag = |this: &mut Self, factor: &fromsoft_formats::fxr::Property, rotation: bool| Drag {
            factor: this.cx.age(factor),
            rotation,
        };
        let (drive, gravity, turn, follow) = match m {
            Typed::ParticleAcceleration(x) => (
                Drive::Acceleration {
                    acceleration: self.cx.age(&x.acceleration),
                    multiplier: self.cx.age(&x.acceleration_multiplier),
                },
                self.cx.age(&x.gravity),
                None,
                None,
            ),
            Typed::ParticleSpeed(x) => (
                Drive::Speed {
                    speed: self.cx.age(&x.speed),
                    multiplier: self.cx.age(&x.speed_multiplier),
                },
                self.cx.age(&x.gravity),
                None,
                None,
            ),
            Typed::ParticleSpeedRandomTurns(x) => (
                Drive::Speed {
                    speed: self.cx.age(&x.speed),
                    multiplier: self.cx.age(&x.speed_multiplier),
                },
                self.cx.age(&x.gravity),
                Some(turns(self, &x.max_turn_angle, x.turn_interval)),
                None,
            ),
            Typed::ParticleSpeedPartialFollow(x) => (
                Drive::Speed {
                    speed: self.cx.age(&x.speed),
                    multiplier: self.cx.age(&x.speed_multiplier),
                },
                self.cx.age(&x.gravity),
                Some(turns(self, &x.max_turn_angle, x.turn_interval)),
                Some(drag(self, &x.follow_factor, x.follow_rotation)),
            ),
            Typed::ParticleAccelerationRandomTurns(x) => (
                Drive::Acceleration {
                    acceleration: self.cx.age(&x.acceleration),
                    multiplier: self.cx.age(&x.acceleration_multiplier),
                },
                self.cx.age(&x.gravity),
                Some(turns(self, &x.max_turn_angle, x.turn_interval)),
                None,
            ),
            Typed::ParticleAccelerationPartialFollow(x) => (
                Drive::Acceleration {
                    acceleration: self.cx.age(&x.acceleration),
                    multiplier: self.cx.age(&x.acceleration_multiplier),
                },
                self.cx.age(&x.gravity),
                Some(turns(self, &x.max_turn_angle, x.turn_interval)),
                Some(drag(self, &x.follow_factor, x.follow_rotation)),
            ),
            _ => return None,
        };
        Some(Movement {
            drive,
            gravity,
            turns: turn,
            drag: follow,
            fields,
        })
    }

    /// How an emitter moves in its parent's frame; `None` for an action the core description has no motion for.
    fn node_motion(&mut self, m: &Typed) -> Option<NodeMotion> {
        let zero = || Value::constant(&[0.0]);
        let one = || Value::constant(&[1.0]);
        let mut out = NodeMotion {
            forward: units::FORWARD,
            speed: zero(),
            speed_multiplier: one(),
            acceleration: zero(),
            acceleration_multiplier: one(),
            lift: zero(),
            lift_in_world: false,
            displacement: None,
            spin: [zero(), zero(), zero()],
            spin_multiplier: [one(), one(), one()],
            align: false,
            turns: None,
            drag: None,
        };
        let turns = |this: &mut Self, angle: &fromsoft_formats::fxr::Property, interval: i32| Turns {
            interval: units::turn_interval(interval),
            max_angle: this.cx.active(angle),
        };
        let drag = |this: &mut Self, factor: &fromsoft_formats::fxr::Property, rotation: bool| Drag {
            factor: this.cx.active(factor),
            rotation,
        };
        match m {
            Typed::NodeAcceleration(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.acceleration = self.cx.active(&x.acceleration_z);
                out.acceleration_multiplier = self.cx.active(&x.acceleration_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.align = x.unk_align_with_motion != 0;
            }
            Typed::NodeAccelerationRandomTurns(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.acceleration = self.cx.active(&x.acceleration_z);
                out.acceleration_multiplier = self.cx.active(&x.acceleration_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.align = x.unk_align_with_motion != 0;
                out.turns = Some(turns(self, &x.max_turn_angle, x.turn_interval));
            }
            Typed::NodeAccelerationPartialFollow(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.acceleration = self.cx.active(&x.acceleration_z);
                out.acceleration_multiplier = self.cx.active(&x.acceleration_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.lift_in_world = true;
                out.align = x.unk_align_with_motion != 0;
                out.turns = Some(turns(self, &x.max_turn_angle, x.turn_interval));
                out.drag = Some(drag(self, &x.follow_factor, x.follow_rotation));
            }
            Typed::NodeAccelerationSpin(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.acceleration = self.cx.active(&x.acceleration_z);
                out.acceleration_multiplier = self.cx.active(&x.acceleration_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.align = x.unk_align_with_motion != 0;
                self.spin(
                    &mut out,
                    [
                        (&x.angular_speed_x, &x.angular_speed_multiplier_x),
                        (&x.angular_speed_y, &x.angular_speed_multiplier_y),
                        (&x.angular_speed_z, &x.angular_speed_multiplier_z),
                    ],
                );
            }
            Typed::NodeSpeed(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.speed_multiplier = self.cx.active(&x.speed_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.align = x.unk_align_with_motion != 0;
            }
            Typed::NodeSpeedRandomTurns(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.speed_multiplier = self.cx.active(&x.speed_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.align = x.unk_align_with_motion != 0;
                out.turns = Some(turns(self, &x.max_turn_angle, x.turn_interval));
            }
            Typed::NodeSpeedPartialFollow(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.speed_multiplier = self.cx.active(&x.speed_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.lift_in_world = true;
                out.align = x.unk_align_with_motion != 0;
                out.turns = Some(turns(self, &x.max_turn_angle, x.turn_interval));
                out.drag = Some(drag(self, &x.follow_factor, x.follow_rotation));
            }
            Typed::NodeSpeedSpin(x) => {
                out.speed = self.cx.fixed(&x.speed_z);
                out.speed_multiplier = self.cx.active(&x.speed_multiplier_z);
                out.lift = self.cx.active(&x.acceleration_y);
                out.align = x.unk_align_with_motion != 0;
                self.spin(
                    &mut out,
                    [
                        (&x.angular_speed_x, &x.angular_speed_multiplier_x),
                        (&x.angular_speed_y, &x.angular_speed_multiplier_y),
                        (&x.angular_speed_z, &x.angular_speed_multiplier_z),
                    ],
                );
            }
            Typed::NodeSpin(x) => {
                self.spin(
                    &mut out,
                    [
                        (&x.angular_speed_x, &x.angular_speed_multiplier_x),
                        (&x.angular_speed_y, &x.angular_speed_multiplier_y),
                        (&x.angular_speed_z, &x.angular_speed_multiplier_z),
                    ],
                );
            }
            Typed::NodeTranslation(x) => {
                let mut v = self.cx.active(&x.translation);
                // the file's z is core's -z
                for key in &mut v.curve.keys {
                    key.value[2] = -key.value[2];
                    key.handle_in[2] = -key.handle_in[2];
                    key.handle_out[2] = -key.handle_out[2];
                }
                out.displacement = Some(v);
                out.align = x.unk_align_with_motion != 0;
            }
            Typed::NodeAttachToCamera(_) => {
                self.cx.notes.lose("a node attached to the camera");
                return None;
            }
            Typed::Unk11(_) => {
                self.cx.notes.lose("action 11 (unknown node movement)");
                return None;
            }
            _ => return None,
        }
        Some(out)
    }

    /// A node's spin about its three axes (degrees per second times a multiplier), signed for the mirrored frame.
    fn spin(
        &mut self,
        out: &mut NodeMotion,
        s: [(&fromsoft_formats::fxr::Property, &fromsoft_formats::fxr::Property); 3],
    ) {
        for (i, (speed, multiplier)) in s.into_iter().enumerate() {
            let sign = if i < 2 { -1.0 } else { 1.0 };
            let (a, b) = (self.cx.active(speed), self.cx.active(multiplier));
            if let Some(slot) = out.spin.get_mut(i) {
                *slot = value::scaled(a, sign);
            }
            if let Some(slot) = out.spin_multiplier.get_mut(i) {
                *slot = b;
            }
        }
    }
}

/// A blank emitter: hung from `parent`, attached to it, active at once and for ever, making nothing.
fn blank(parent: Option<u32>, name: Option<String>) -> Emitter {
    Emitter {
        name,
        parent,
        placement: Transform::IDENTITY,
        jitter_translation: [0.0; 3],
        jitter_rotation: [0.0; 3],
        attachment: Attachment {
            anchor: Anchor::Parent,
            follow: Follow::Frame,
        },
        delay: 0.0,
        duration: None,
        motion: None,
        spawn: Spawn::Once,
        shape: Shape::Point,
        heading: primase_core::particles::Heading::Shape {
            facing: units::FORWARD,
        },
        spread: Spread::None,
        law: Law {
            lifetime: None,
            attachment: Attachment {
                anchor: Anchor::Parent,
                follow: Follow::Frame,
            },
            movement: None,
            initial_speed: Value::constant(&[0.0]),
            size_scale: [
                Value::constant(&[1.0]),
                Value::constant(&[1.0]),
                Value::constant(&[1.0]),
            ],
            tint: Value::constant(&[1.0; 4]),
        },
        draw: None,
        spawns: Vec::new(),
        states: Vec::new(),
        order: 0.0,
        lod_range: None,
        copies: None,
    }
}

/// The static node transform (an offset and rotations in degrees, as stored) as a placement in core's frame.
fn transform(offset: [f32; 3], rotation_degrees: [f32; 3]) -> Transform {
    let m = StaticTransform {
        offset,
        rotation_degrees,
    }
    .matrix();
    placed(m.map(|c| c.map(|x| x as f32)))
}

/// Seconds after which no particle of the system is alive and nothing more is emitted; `None` when something never ends.
fn end_time(emitters: &[Emitter]) -> Option<f32> {
    let mut starts: Vec<f32> = Vec::with_capacity(emitters.len());
    let mut end = 0.0f32;
    for e in emitters {
        let base = e
            .parent
            .and_then(|p| starts.get(usize::try_from(p).ok()?).copied())
            .unwrap_or(0.0);
        let start = base + e.delay;
        starts.push(start);
        if e.draw.is_none() && e.spawns.is_empty() {
            continue;
        }
        let emits_until = match (&e.spawn, e.duration) {
            (Spawn::Once, _) => start,
            (_, Some(d)) => start + d,
            (
                Spawn::Periodic {
                    interval,
                    total: Some(n),
                    ..
                },
                None,
            ) if interval.is_fixed() => start + value::number_of(interval) * *n as f32,
            _ => return None,
        };
        let life = match &e.law.lifetime {
            Some(v) if v.is_fixed() => value::number_of(v),
            Some(_) => return None,
            None if matches!(
                e.draw,
                Some(primase_core::particles::Draw::Light(_)) | Some(primase_core::particles::Draw::Field(_))
            ) =>
            {
                0.0
            }
            None => return None,
        };
        end = end.max(emits_until + life);
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_static_transform_is_placed_in_cores_frame() {
        // a plain offset along the file's +Z is core's -Z
        let t = transform([1.0, 2.0, 3.0], [0.0; 3]);
        assert_eq!(t.cols[3], [1.0, 2.0, -3.0]);
        assert_eq!(t.cols[0], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_blank_emitter_hangs_from_its_parent_and_draws_nothing() {
        let e = blank(Some(3), Some("n".into()));
        assert_eq!(e.parent, Some(3));
        assert!(e.draw.is_none() && e.spawns.is_empty() && e.states.is_empty());
        assert_eq!(e.delay, 0.0);
    }

    #[test]
    fn the_end_of_a_system_is_when_its_last_particle_dies() {
        let mut once = blank(None, None);
        once.draw = Some(primase_core::particles::Draw::Point(
            primase_core::particles::Point {
                map: None,
                blend: primase_core::particles::Blend::Alpha,
                size: Value::constant(&[1.0]),
                tint: Vec::new(),
                alpha_cutoff: Value::constant(&[0.0]),
                soft: None,
                distance: Default::default(),
                bloom: None,
            },
        ));
        once.delay = 1.0;
        once.law.lifetime = Some(Value::constant(&[2.0]));
        assert_eq!(end_time(std::slice::from_ref(&once)), Some(3.0));
        // an endless particle: no end
        once.law.lifetime = None;
        assert_eq!(end_time(&[once]), None);
        // nothing drawn: ends at once
        assert_eq!(end_time(&[blank(None, None)]), Some(0.0));
    }
}
