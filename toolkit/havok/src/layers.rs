//! The layers of a Havok Behavior graph's layer generator, as data (`havok.behavior_layers`, [`read_layers`]) and as a
//! controller (`lower-havok-layers`, [`LowerHavokLayers`]).
//!
//! A behaviour graph (the `Behaviors/*.hkx` of a character's `behbnd`) is a tree of generators. Where it holds an
//! `hkbLayerGenerator` (the player's, under a script generator and a blender; an ordinary character's, under a state
//! machine's state), the graph's animation is a stack of `hkbLayer`s: each plays one generator (a clip, a manual selector of
//! clips, a state machine, ...) at a weight, with an optional per-bone weight array (`hkbBoneWeightArray`) that says which
//! bones the layer drives. That stack is what `core.controller`'s layers state; the rest of the graph (the state machines,
//! their transitions, the events that turn layers on and off) stays in the behaviour file.
//!
//! Reading takes no choice: [`read_layers`] copies what the file states of each layer (its generator, weight, whether it is
//! on at the start, the variable its weight is bound to and that variable's initial value, its bone weights) into an owned
//! [`BehaviorLayers`], with the clips a selector chooses among named by their animation names. Lowering ties it to a rig by
//! what the world holds, and states what it did not take:
//!
//! - **layers**: one `core.controller` layer for each `hkbLayer`, in order; the layer is named by its generator. A layer that
//!   is not on at the start (`onByDefault` false) has default weight 0; its on and off events and fades stay in the file
//!   (counted). A layer whose weight is bound to a variable has the variable as a controller parameter (a `Float`, its
//!   initial value the graph's) and the layer's `weight_parameter` names it, unless the layer is off at the start (then the
//!   binding is not lowered, counted);
//! - **states**: a manual selector of clips is a layer with one state for each clip it chooses among, in the order the file
//!   lists them; the file stores no selected index (its `generatorIndexAtActivate` is runtime state, zero in every object of
//!   both installs), so the layer starts in state 0 and which state the game picks is its own (the player's hand pose group
//!   follows its equipment). A clip generator alone is one state. A clip state is the `core.clip` of that animation name bound
//!   to the rig that drives the rig's skeleton ([`clip_for`]); any other generator, a child that is not a clip, and a clip
//!   the world lacks or cannot place is a state with `Motion::Unsupported` naming why. A clip the world lists and has not
//!   loaded (a reader that decodes clips on demand: an entity with a `core.name` and `core.clip_key` and no `core.clip`) is
//!   `Motion::Pending` on that entity; the pass runs again after the clip is loaded and states the `Motion::Clip`;
//! - **masks**: a bone weight array is the layer's mask: each bone of the array with a weight above 0 is a masked joint of
//!   the rig's skeleton (through the joint map when the clips were mapped from the Havok skeleton to a model's; a bone the map
//!   does not pair is left out and counted), and a bone the array does not list, including those past its end, has weight 0.
//!   A weight outside 0 to 1, a non-finite one, or an empty array, is `LayerMask::Unsupported`, counted;
//! - **blending**: every layer is `Override`. The file has no per-layer additive flag: whether the clip is a delta is the
//!   clip's (`lower-havok-clips` states it from the animation's blend hint), and a delta clip composes onto the pose below as
//!   `core.clip` states.

use std::collections::BTreeMap;

use havok_formats::behavior::{
    Behavior, BindingTarget, ClipGenerator, CustomManualSelectorGenerator, Document, FileKind,
    LayerGenerator, Object, View,
};
use primase_core::pass::{Pass, PassLog, PassRegistry, PassSpec};
use primase_core::{
    Clip, ClipKey, ClipTarget, Component, Controller, ControllerBinding, ControllerLayer,
    ControllerParameter, ControllerParameterKind, ControllerState, Entity, JointId, JointMap, LayerBlending,
    LayerMask, MaskedJoint, Motion, Name, RigInstance, World,
};

/// A clip a layer's generator chooses among, or the other thing a selector's child is.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerChild {
    /// An `hkbClipGenerator`: the animation it plays by name, and its playback speed.
    Clip { animation: String, speed: f32 },
    /// Any other generator (a state machine, a blender, a null child): its class and name.
    Other { class: String, name: String },
}

/// What a layer plays.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerContent {
    /// A `CustomManualSelectorGenerator`: its name and the generators it chooses among, in order.
    Selector { name: String, children: Vec<LayerChild> },
    /// A generator that is one thing: a clip, or any other generator.
    One(LayerChild),
}

/// One `hkbLayer` of the graph's layer generator.
#[derive(Clone, Debug, PartialEq)]
pub struct BehaviorLayer {
    pub content: LayerContent,
    /// `blendingControlData.weight`, as stored.
    pub weight: f32,
    /// `blendingControlData.onByDefault`.
    pub on_by_default: bool,
    /// The variable (an index of [`BehaviorLayers::variables`]) `blendingControlData/weight` is bound to.
    pub weight_variable: Option<usize>,
    /// The layer states an event that turns it on or off, or a fade: dynamics this lowering leaves in the file.
    pub has_events_or_fades: bool,
    /// The layer's `hkbBoneWeightArray`, as stored; `None` when it has none.
    pub bone_weights: Option<Vec<f32>>,
}

/// The layers of a behaviour graph's layer generator.
#[derive(Clone, Debug, PartialEq)]
pub struct BehaviorLayers {
    /// The graph's name.
    pub graph: String,
    /// The layer generator's name.
    pub generator: String,
    pub layers: Vec<BehaviorLayer>,
    /// The variables layer weights are bound to: name and the graph's initial value.
    pub variables: Vec<(String, f32)>,
}

fn child_of(object: Option<Object<'_, '_>>) -> havok_formats::Result<LayerChild> {
    let Some(object) = object else {
        return Ok(LayerChild::Other {
            class: "(null)".to_owned(),
            name: String::new(),
        });
    };
    if let Some(clip) = object.view::<ClipGenerator>() {
        return Ok(LayerChild::Clip {
            animation: clip.animation_name()?,
            speed: clip.playback_speed()?,
        });
    }
    // every generator class has a name; a class without one is unnamed
    let name = if object.value().has_field("name") {
        object.value().field("name")?.string()?
    } else {
        String::new()
    };
    Ok(LayerChild::Other {
        class: object.class_name().to_owned(),
        name,
    })
}

/// The graph's layer generator: the one `hkbLayerGenerator` reachable from `root` that no other layer generator contains,
/// found breadth first through the objects' pointers (the player's sits under a script generator and a blender, an
/// ordinary character's under a state machine's state). `None` where the graph has none; an error where it has several,
/// since which one is meant is not stated (both installs' graphs have none or exactly one: 280 of 559 and 79 of 157).
fn layer_generator<'t, 'a>(
    root: Option<Object<'t, 'a>>,
) -> havok_formats::Result<Option<LayerGenerator<'t, 'a>>> {
    let Some(root) = root else { return Ok(None) };
    let mut seen = std::collections::BTreeSet::from([root.offset()]);
    let mut queue = std::collections::VecDeque::from([root]);
    let mut found: Vec<LayerGenerator<'t, 'a>> = Vec::new();
    while let Some(object) = queue.pop_front() {
        if let Some(generator) = object.view::<LayerGenerator>() {
            found.push(generator);
            continue;
        }
        for reference in object.references()? {
            if seen.insert(reference.target.offset()) {
                queue.push_back(reference.target);
            }
        }
    }
    match found[..] {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        ref several => Err(havok_formats::Error::new(
            0,
            format!(
                "{} layer generators that no other contains: which one is the graph's is not stated",
                several.len()
            ),
        )),
    }
}

/// The layers of `document`'s layer generator ([`layer_generator`]): `None` unless the file is a behaviour graph that has one.
pub fn read_layers(document: &Document<'_, '_>) -> havok_formats::Result<Option<BehaviorLayers>> {
    if document.kind() != FileKind::Behavior {
        return Ok(None);
    }
    let behavior: Behavior<'_, '_> = document.behavior()?;
    let Some(generator) = layer_generator(behavior.graph().root_generator()?)? else {
        return Ok(None);
    };
    let variables = behavior.variables()?;
    let mut bound: Vec<(String, f32)> = Vec::new();
    let mut layers = Vec::new();
    for layer in generator.layers()? {
        let Some(layer) = layer else {
            return Err(havok_formats::Error::new(
                0,
                "a layer of the layer generator is a null pointer",
            ));
        };
        let control = layer.blending_control_data()?;
        let members = behavior.bindings(Object::new(layer.raw()))?;
        let weight_variable = match members
            .iter()
            .find(|m| m.member_path == "blendingControlData/weight")
        {
            Some(m) if m.target == BindingTarget::Variable => {
                let initial = variables
                    .get(m.index)
                    .and_then(|v| v.initial_real())
                    .ok_or_else(|| {
                        havok_formats::Error::new(
                            0,
                            format!(
                                "the layer's weight is bound to variable {} ({}), which is not a real",
                                m.index, m.name
                            ),
                        )
                    })?;
                Some(match bound.iter().position(|(n, _)| *n == m.name) {
                    Some(at) => at,
                    None => {
                        bound.push((m.name.clone(), initial));
                        bound.len() - 1
                    }
                })
            }
            _ => None,
        };
        let content = match layer.generator()? {
            Some(g) => match g.view::<CustomManualSelectorGenerator>() {
                Some(selector) => LayerContent::Selector {
                    name: selector.name()?,
                    children: selector
                        .generators()?
                        .into_iter()
                        .map(child_of)
                        .collect::<havok_formats::Result<_>>()?,
                },
                None => LayerContent::One(child_of(Some(g))?),
            },
            None => LayerContent::One(child_of(None)?),
        };
        layers.push(BehaviorLayer {
            content,
            weight: control.weight()?,
            on_by_default: control.on_by_default()?,
            weight_variable,
            has_events_or_fades: control.on_event_id()? >= 0
                || control.off_event_id()? >= 0
                || control.fade_in_duration()? > 0.0
                || control.fade_out_duration()? > 0.0,
            bone_weights: match layer.bone_weights()? {
                Some(w) => Some(w.bone_weights()?),
                None => None,
            },
        });
    }
    Ok(Some(BehaviorLayers {
        graph: behavior.graph().name()?,
        generator: generator.name()?,
        layers,
        variables: bound,
    }))
}

/// `havok.behavior_layers`: the layers of a character's behaviour graph ([`BehaviorLayers`]), on the entity that holds the
/// character's Havok skeleton (the `core.skeleton` made of the file the graph binds), so whatever carries the skeleton
/// carries its layers.
#[derive(Clone, Debug, PartialEq)]
pub struct HavokBehaviorLayers(pub BehaviorLayers);

impl Component for HavokBehaviorLayers {
    const NAME: &'static str = "havok.behavior_layers";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// `lower-havok-layers`: each `havok.behavior_layers` as a `core.controller` and its `core.controller_binding` on the rig that
/// poses its skeleton (see the module documentation). No options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowerHavokLayers;

/// `lower-havok-layers`' declaration ([`PassRegistry::add`]): no options.
pub const LOWER_HAVOK_LAYERS: PassSpec = PassSpec {
    name: "lower-havok-layers",
    options: &[],
    make: |_| Ok(Box::new(LowerHavokLayers)),
};

/// Register this crate's layer pass: [`LOWER_HAVOK_LAYERS`].
pub fn register(reg: &mut PassRegistry) {
    reg.add(&LOWER_HAVOK_LAYERS);
}

/// Where a behaviour graph's skeleton is posed: the rig, its skeleton, and the Havok bone to that skeleton's joint.
struct Target {
    rig: Entity,
    skeleton: Entity,
    /// `None`: the rig poses the Havok skeleton itself (a bone is the joint of its index).
    map: Option<BTreeMap<JointId, JointId>>,
}

fn target_of(w: &World, havok: Entity) -> Result<Target, String> {
    let rigs_of = |skeleton: Entity| -> Vec<Entity> {
        w.iter::<RigInstance>()
            .filter(|(_, r)| r.skeleton == skeleton)
            .map(|(e, _)| e)
            .collect()
    };
    let direct = rigs_of(havok);
    if let [rig] = direct[..] {
        return Ok(Target {
            rig,
            skeleton: havok,
            map: None,
        });
    }
    if direct.len() > 1 {
        return Err(format!("{} rigs pose the Havok skeleton itself", direct.len()));
    }
    // a program that maps clips one at a time leaves one joint map for each; maps of one pairing are one map
    let mut maps: Vec<&JointMap> = Vec::new();
    for (_, m) in w.iter::<JointMap>().filter(|(_, m)| m.from == havok) {
        if !maps.iter().any(|seen| seen.to == m.to && seen.pairs == m.pairs) {
            maps.push(m);
        }
    }
    let [map] = maps[..] else {
        return Err(format!(
            "no rig poses the Havok skeleton and {} different joint maps leave it",
            maps.len()
        ));
    };
    match rigs_of(map.to)[..] {
        [rig] => Ok(Target {
            rig,
            skeleton: map.to,
            map: Some(map.pairs.iter().copied().collect()),
        }),
        ref rigs => Err(format!(
            "{} rigs pose the skeleton the joint map leads to",
            rigs.len()
        )),
    }
}

/// The motion a clip state of `animation` has for the rig of `skeleton`: the `core.clip` of that name that drives the
/// skeleton's joints, a [`Motion::Pending`] on the one clip entity of that name the world lists and has not loaded
/// (`core.name` and `core.clip_key`, no `core.clip`), or `Err` saying why there is none (a named refusal).
fn clip_for(w: &World, animation: &str, skeleton: Entity) -> Result<Motion, String> {
    let named: Vec<Entity> = w
        .iter::<Clip>()
        .filter(|(_, c)| c.name == animation)
        .map(|(e, _)| e)
        .collect();
    let [clip] = named[..] else {
        if !named.is_empty() {
            return Err(format!("{} clips are named {animation}", named.len()));
        }
        let listed: Vec<Entity> = w
            .iter::<ClipKey>()
            .map(|(e, _)| e)
            .filter(|e| !w.has::<Clip>(*e) && w.get::<Name>(*e).is_some_and(|n| n.0 == animation))
            .collect();
        return match listed[..] {
            [] => Err(format!("the world has no clip named {animation}")),
            [entity] => Ok(Motion::Pending(entity)),
            _ => Err(format!("{} listed clips are named {animation}", listed.len())),
        };
    };
    let drives = w.get::<Clip>(clip).is_some_and(|c| {
        c.channels
            .iter()
            .any(|ch| matches!(&ch.target, ClipTarget::Joint { skeleton: s, .. } if *s == skeleton))
    });
    if drives {
        Ok(Motion::Clip(clip))
    } else {
        Err(format!(
            "clip {animation} drives no joint of the rig's skeleton (it is not mapped onto it)"
        ))
    }
}

fn state_of(child: &LayerChild, w: &World, target: &Target, log: &mut PassLog) -> ControllerState {
    let (name, motion, speed) = match child {
        LayerChild::Clip { animation, speed } => match clip_for(w, animation, target.skeleton) {
            Ok(motion) => {
                if matches!(motion, Motion::Pending(_)) {
                    log.count(
                        "layer states whose clip is listed and not loaded yet (pending)",
                        1,
                    );
                }
                (animation.clone(), motion, *speed)
            }
            Err(why) => {
                log.count(
                    "layer states whose clip is not in the world for the rig (named)",
                    1,
                );
                (animation.clone(), Motion::Unsupported(why), *speed)
            }
        },
        LayerChild::Other { class, name } => {
            log.count("layer states that are not clips (named)", 1);
            (
                if name.is_empty() {
                    class.clone()
                } else {
                    name.clone()
                },
                Motion::Unsupported(format!("{class} {name:?} stays in the behaviour file")),
                1.0,
            )
        }
    };
    ControllerState {
        name,
        motion,
        speed: f64::from(speed),
        speed_parameter: None,
        cycle_offset: 0.0,
        cycle_offset_parameter: None,
    }
}

fn mask_of(weights: &[f32], target: &Target, log: &mut PassLog) -> LayerMask {
    if weights.is_empty() {
        log.omit("layer masks not lowered: an empty bone weight array", 1);
        return LayerMask::Unsupported("its bone weight array is empty".to_owned());
    }
    if weights.iter().any(|w| !w.is_finite() || !(0.0..=1.0).contains(w)) {
        log.omit(
            "layer masks not lowered: a bone weight outside 0 to 1 (never clamped)",
            1,
        );
        return LayerMask::Unsupported("a bone weight is outside 0 to 1".to_owned());
    }
    let mut joints = Vec::new();
    let mut unpaired = 0;
    for (bone, &weight) in weights.iter().enumerate() {
        if weight <= 0.0 {
            continue;
        }
        let havok = JointId(bone as u32);
        let joint = match &target.map {
            None => Some(havok),
            Some(map) => map.get(&havok).copied(),
        };
        match joint {
            Some(joint) => joints.push(MaskedJoint {
                skeleton: target.skeleton,
                joint,
                weight,
            }),
            None => unpaired += 1,
        }
    }
    if unpaired > 0 {
        log.count(
            "masked bones the rig's skeleton has no joint for (left out of the mask)",
            unpaired,
        );
    }
    log.count("layers with a bone weight array", 1);
    LayerMask::Joints(joints)
}

fn controller_for(b: &BehaviorLayers, target: &Target, w: &World, log: &mut PassLog) -> Controller {
    let skeleton_joints = w
        .get::<primase_core::Skeleton>(target.skeleton)
        .map_or(0, |s| s.joints.len());
    let mut parameters: Vec<ControllerParameter> = Vec::new();
    let mut layers = Vec::new();
    for (index, layer) in b.layers.iter().enumerate() {
        let (states, name) = match &layer.content {
            LayerContent::Selector { name, children } => (
                children
                    .iter()
                    .map(|c| state_of(c, w, target, log))
                    .collect::<Vec<_>>(),
                name.clone(),
            ),
            LayerContent::One(child) => {
                let state = state_of(child, w, target, log);
                let name = state.name.clone();
                (vec![state], name)
            }
        };
        if layer.has_events_or_fades {
            log.count(
                "layers with on/off events or fades (left in the behaviour file)",
                1,
            );
        }
        let weight_parameter = match layer.weight_variable {
            Some(v) if layer.on_by_default => {
                let (variable, initial) = &b.variables[v];
                let at = parameters
                    .iter()
                    .position(|p| p.name == *variable)
                    .unwrap_or_else(|| {
                        parameters.push(ControllerParameter {
                            name: variable.clone(),
                            kind: ControllerParameterKind::Float,
                            default: f64::from(*initial),
                        });
                        parameters.len() - 1
                    });
                Some(at)
            }
            Some(_) => {
                log.count(
                    "layer weight bindings not lowered: the layer is off at the start",
                    1,
                );
                None
            }
            None => None,
        };
        if !layer.on_by_default {
            log.count("layers off at the start (default weight 0)", 1);
        }
        if let Some(weights) = &layer.bone_weights
            && weights.len() < skeleton_joints
        {
            log.count(
                "bone weight arrays shorter than the skeleton (later bones have weight 0)",
                1,
            );
        }
        layers.push(ControllerLayer {
            name: if name.trim().is_empty() {
                format!("layer {index}")
            } else {
                name
            },
            default_state: (!states.is_empty()).then_some(0),
            states,
            default_weight: if layer.on_by_default { layer.weight } else { 0.0 },
            weight_parameter,
            blending: LayerBlending::Override,
            mask: layer
                .bone_weights
                .as_deref()
                .map(|weights| mask_of(weights, target, log)),
        });
    }
    Controller {
        name: format!("{} / {}", b.graph, b.generator),
        parameters,
        layers,
    }
}

impl Pass for LowerHavokLayers {
    fn name(&self) -> &'static str {
        "lower-havok-layers"
    }
    fn requires(&self) -> &[&'static str] {
        &[HavokBehaviorLayers::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.controller", "core.controller_binding"]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[HavokBehaviorLayers::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.controller"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let all: Vec<(Entity, BehaviorLayers)> = w
            .iter::<HavokBehaviorLayers>()
            .map(|(e, b)| (e, b.0.clone()))
            .collect();
        log.count("behaviour graphs with a root layer generator", all.len() as u64);
        for (skeleton, b) in all {
            let target = match target_of(w, skeleton) {
                Ok(t) => t,
                Err(why) => {
                    log.omit(&format!("behaviour graphs not bound to a rig: {why}"), 1);
                    continue;
                }
            };
            let controller = controller_for(&b, &target, w, log);
            let existing = w
                .get::<ControllerBinding>(target.rig)
                .map(|binding| binding.controller)
                .filter(|c| w.has::<Controller>(*c));
            let entity = existing.unwrap_or_else(|| w.spawn());
            w.insert(entity, controller);
            let binding = ControllerBinding {
                controller: entity,
                rig: target.rig,
            };
            if w.get::<ControllerBinding>(target.rig) != Some(&binding) {
                w.insert(target.rig, binding);
            }
            log.count("controllers lowered", 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::Options;
    use primase_core::{
        Channel, ClipBinding, Extrapolation, Interpolation, Joint, Property, Rest, Skeleton, Transform,
    };

    fn skeleton(w: &mut World, joints: u32) -> Entity {
        let rest = Rest::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        let e = w.spawn();
        w.insert(
            e,
            Skeleton {
                joints: (0..joints)
                    .map(|i| Joint {
                        id: JointId(i),
                        name: format!("j{i}"),
                        parent: None,
                        rest,
                    })
                    .collect(),
            },
        );
        e
    }

    /// A clip named `name` that rotates joint 0 of `skeleton`.
    fn clip(w: &mut World, name: &str, skeleton: Entity) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Clip {
                name: name.into(),
                duration: 1.0,
                extrapolation: Extrapolation::Hold,
                channels: vec![Channel {
                    target: ClipTarget::Joint {
                        skeleton,
                        joint: JointId(0),
                    },
                    property: Property::Rotation,
                    interpolation: Interpolation::Step,
                    times: vec![0.0],
                    values: vec![0.0, 0.0, 0.0, 1.0],
                }],
                root_motion: None,
                additive: None,
            },
        );
        e
    }

    fn selector(children: &[&str]) -> LayerContent {
        LayerContent::Selector {
            name: "Basis_CMSG".into(),
            children: children
                .iter()
                .map(|c| {
                    if let Some(class) = c.strip_prefix('!') {
                        LayerChild::Other {
                            class: class.into(),
                            name: "inner".into(),
                        }
                    } else {
                        LayerChild::Clip {
                            animation: (*c).into(),
                            speed: 1.0,
                        }
                    }
                })
                .collect(),
        }
    }

    fn layer(content: LayerContent) -> BehaviorLayer {
        BehaviorLayer {
            content,
            weight: 1.0,
            on_by_default: true,
            weight_variable: None,
            has_events_or_fades: false,
            bone_weights: None,
        }
    }

    fn layers() -> BehaviorLayers {
        let mut add = layer(LayerContent::One(LayerChild::Clip {
            animation: "add".into(),
            speed: 1.0,
        }));
        add.weight_variable = Some(0);
        let mut blink = layer(LayerContent::One(LayerChild::Clip {
            animation: "blink".into(),
            speed: 1.0,
        }));
        blink.weight_variable = Some(1);
        blink.bone_weights = Some(vec![0.0, 1.0, 0.5]);
        let mut speak = layer(selector(&["blink"]));
        speak.on_by_default = false;
        speak.weight_variable = Some(0);
        speak.has_events_or_fades = true;
        let mut wild = layer(selector(&["blink"]));
        wild.bone_weights = Some(vec![0.0, 1.5]);
        let mut empty = layer(selector(&["blink"]));
        empty.bone_weights = Some(vec![]);
        BehaviorLayers {
            graph: "c0000".into(),
            generator: "Master LayerGenerator".into(),
            layers: vec![
                layer(selector(&[
                    "hand_a",
                    "hand_b",
                    "hand_missing",
                    "!hkbStateMachine",
                ])),
                layer(LayerContent::One(LayerChild::Other {
                    class: "hkbStateMachine".into(),
                    name: "Master_SM".into(),
                })),
                add,
                blink,
                speak,
                wild,
                empty,
            ],
            variables: vec![("AddBlend03".into(), 0.0), ("EyeBlinkBlend".into(), 1.0)],
        }
    }

    /// The Havok skeleton (3 bones), the model's (joints 10, 11, 12 for bones 0, 1, 2 but bone 2 unpaired), a rig on the
    /// model's, and the clips on the model's skeleton.
    fn world() -> (World, Entity, Entity, Entity) {
        let mut w = World::new();
        let havok = skeleton(&mut w, 3);
        let model = w.spawn();
        let rest = Rest::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        w.insert(
            model,
            Skeleton {
                joints: (10..13)
                    .map(|i| Joint {
                        id: JointId(i),
                        name: format!("m{i}"),
                        parent: None,
                        rest,
                    })
                    .collect(),
            },
        );
        let map = w.spawn();
        w.insert(
            map,
            JointMap {
                from: havok,
                to: model,
                pairs: vec![(JointId(0), JointId(10)), (JointId(1), JointId(11))],
            },
        );
        let rig = w.spawn();
        w.insert(rig, RigInstance { skeleton: model });
        w.insert(rig, Transform::IDENTITY);
        for name in ["hand_a", "hand_b", "add", "blink"] {
            let c = clip(&mut w, name, model);
            let b = w.spawn();
            w.insert(
                b,
                ClipBinding {
                    clip: c,
                    rig: Some(rig),
                    meshes: vec![],
                },
            );
        }
        // a clip still on the Havok skeleton (not mapped) is named, not played
        clip(&mut w, "hand_missing", havok);
        w.insert(havok, HavokBehaviorLayers(layers()));
        (w, havok, model, rig)
    }

    fn run(w: &mut World) -> PassLog {
        let pass = LOWER_HAVOK_LAYERS.instance(&Options::new()).expect("no options");
        let mut log = PassLog::default();
        pass.run(w, &mut log).expect("the pass runs");
        log
    }

    #[test]
    fn a_behaviour_graphs_layers_are_a_controller_on_the_rig_that_poses_the_skeleton() {
        let (mut w, _, model, rig) = world();
        let log = run(&mut w);
        let binding = w.get::<ControllerBinding>(rig).expect("the rig is bound").clone();
        assert_eq!(binding.rig, rig);
        let c = w.get::<Controller>(binding.controller).unwrap().clone();
        assert_eq!(c.name, "c0000 / Master LayerGenerator");
        assert_eq!(c.layers.len(), 7);
        // the selector's layer: a state for each child; clips found by name on the model's skeleton; the rest named
        let basis = &c.layers[0];
        assert_eq!(basis.name, "Basis_CMSG");
        assert_eq!(basis.default_state, Some(0));
        assert_eq!(basis.default_weight, 1.0);
        assert_eq!(basis.blending, LayerBlending::Override);
        assert_eq!(basis.mask, None);
        assert_eq!(basis.states.len(), 4);
        assert!(matches!(basis.states[0].motion, Motion::Clip(_)));
        assert!(matches!(basis.states[1].motion, Motion::Clip(_)));
        assert!(
            matches!(&basis.states[2].motion, Motion::Unsupported(why) if why.contains("drives no joint of the rig's skeleton")),
            "{:?}",
            basis.states[2].motion
        );
        assert!(
            matches!(&basis.states[3].motion, Motion::Unsupported(why) if why.contains("stays in the behaviour file"))
        );
        // the state machine's layer is one named state
        assert_eq!(c.layers[1].name, "Master_SM");
        assert_eq!(c.layers[1].states.len(), 1);
        // weights bound to variables are parameters (Float, the graph's initial value), shared by name
        assert_eq!(c.parameters.len(), 2);
        assert_eq!(
            (c.parameters[0].name.as_str(), c.parameters[0].default),
            ("AddBlend03", 0.0)
        );
        assert_eq!(
            (c.parameters[1].name.as_str(), c.parameters[1].default),
            ("EyeBlinkBlend", 1.0)
        );
        assert_eq!(c.layers[2].weight_parameter, Some(0));
        assert_eq!(c.layers[3].weight_parameter, Some(1));
        // a bone weight array is a mask of the model's joints: bone 1 at 1, bone 2 (no pair) left out, bone 0 weight 0
        assert_eq!(
            c.layers[3].mask,
            Some(LayerMask::Joints(vec![MaskedJoint {
                skeleton: model,
                joint: JointId(11),
                weight: 1.0
            }]))
        );
        // off at the start: weight 0, the binding not lowered; events counted
        assert_eq!(c.layers[4].default_weight, 0.0);
        assert_eq!(c.layers[4].weight_parameter, None);
        // a weight outside 0 to 1 and an empty array are named refusals
        assert!(
            matches!(&c.layers[5].mask, Some(LayerMask::Unsupported(why)) if why.contains("outside 0 to 1"))
        );
        assert!(matches!(&c.layers[6].mask, Some(LayerMask::Unsupported(why)) if why.contains("empty")));
        assert_eq!(log.counts["controllers lowered"], 1);
        assert_eq!(log.counts["layers off at the start (default weight 0)"], 1);
        assert_eq!(
            log.counts["layer weight bindings not lowered: the layer is off at the start"],
            1
        );
        assert_eq!(
            log.counts["layers with on/off events or fades (left in the behaviour file)"],
            1
        );
        assert_eq!(
            log.counts["masked bones the rig's skeleton has no joint for (left out of the mask)"],
            1
        );
        assert_eq!(
            log.omitted["layer masks not lowered: a bone weight outside 0 to 1 (never clamped)"],
            1
        );
        assert_eq!(
            log.omitted["layer masks not lowered: an empty bone weight array"],
            1
        );
        Component::verify(&c, binding.controller, &w).unwrap();
    }

    #[test]
    fn a_second_run_replaces_the_controller_in_place_and_a_skeleton_with_no_rig_is_named() {
        let (mut w, _, _, rig) = world();
        run(&mut w);
        let first = w.get::<ControllerBinding>(rig).unwrap().controller;
        run(&mut w);
        assert_eq!(w.get::<ControllerBinding>(rig).unwrap().controller, first);
        assert_eq!(w.iter::<Controller>().count(), 1);
        // no rig poses the skeleton the joint map leads to
        let (mut w, _, model, rig) = world();
        w.remove::<RigInstance>(rig);
        let _ = model;
        let log = run(&mut w);
        assert_eq!(w.iter::<Controller>().count(), 0);
        assert_eq!(
            log.omitted["behaviour graphs not bound to a rig: 0 rigs pose the skeleton the joint map leads to"],
            1
        );
        // clips mapped one at a time leave one joint map each: the same pairing is one map, a different one is named
        let (mut w, havok, model, rig) = world();
        let map = w.iter::<JointMap>().next().map(|(_, m)| m.clone()).unwrap();
        let again = w.spawn();
        w.insert(again, map.clone());
        let log = run(&mut w);
        assert_eq!(log.counts["controllers lowered"], 1, "{log:?}");
        assert!(w.get::<ControllerBinding>(rig).is_some());
        let other = w.spawn();
        w.insert(
            other,
            JointMap {
                from: havok,
                to: model,
                pairs: vec![(JointId(0), JointId(12))],
            },
        );
        let log = run(&mut w);
        assert_eq!(
            log.omitted["behaviour graphs not bound to a rig: no rig poses the Havok skeleton and 2 different joint maps leave it"],
            1
        );
    }

    #[test]
    fn a_clip_the_world_lists_and_has_not_loaded_is_pending_until_it_is() {
        let (mut w, havok, model, rig) = world();
        let mut b = layers();
        b.layers[0] = layer(selector(&["hand_a", "hand_listed", "hand_twice"]));
        w.insert(havok, HavokBehaviorLayers(b));
        let listed = |w: &mut World, name: &str, key: &str| {
            let e = w.spawn();
            w.insert(e, Name(name.into()));
            w.insert(e, ClipKey(key.into()));
            e
        };
        let wanted = listed(&mut w, "hand_listed", "opaque:1");
        listed(&mut w, "hand_twice", "opaque:2");
        listed(&mut w, "hand_twice", "opaque:3");
        let log = run(&mut w);
        let binding = w.get::<ControllerBinding>(rig).unwrap().clone();
        let c = w.get::<Controller>(binding.controller).unwrap().clone();
        let basis = &c.layers[0];
        assert!(matches!(basis.states[0].motion, Motion::Clip(_)));
        assert_eq!(basis.states[1].motion, Motion::Pending(wanted));
        assert!(
            matches!(&basis.states[2].motion, Motion::Unsupported(why) if why.contains("2 listed clips are named hand_twice"))
        );
        assert_eq!(
            log.counts["layer states whose clip is listed and not loaded yet (pending)"],
            1
        );
        Component::verify(&c, binding.controller, &w).unwrap();
        let mut refs = Vec::new();
        Component::references(&c, &mut refs);
        assert!(refs.contains(&wanted));
        // loaded in place on the listed entity and bound to the rig, the state plays it after the next run
        let loaded = clip(&mut w, "hand_listed", model);
        let decoded = w.get::<Clip>(loaded).unwrap().clone();
        w.despawn(loaded);
        w.insert(wanted, decoded);
        let bound = w.spawn();
        w.insert(
            bound,
            ClipBinding {
                clip: wanted,
                rig: Some(rig),
                meshes: vec![],
            },
        );
        let log = run(&mut w);
        assert!(
            !log.counts
                .contains_key("layer states whose clip is listed and not loaded yet (pending)")
        );
        let c = w.get::<Controller>(binding.controller).unwrap();
        assert_eq!(c.layers[0].states[1].motion, Motion::Clip(wanted));
    }

    #[test]
    fn the_pass_registers_and_takes_no_options() {
        let mut registry = PassRegistry::new();
        register(&mut registry);
        assert!(registry.describe("lower-havok-layers").is_some());
        assert!(registry.make("lower-havok-layers", &Options::new()).is_ok());
        assert!(!registry.requires_options("lower-havok-layers"));
    }
}
