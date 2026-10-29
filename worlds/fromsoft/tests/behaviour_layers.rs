//! A character's behaviour graph read for its layers (`installs.elden`, `keys.elden`, `tools.oodle6`): the player's
//! `Master LayerGenerator` as a `core.controller`, and the pose of its layers composed by `primase_animation`.
//!
//! - L1: `c0000`'s graph has 14 layers: the hand pose (`BasisAnim_CMSG`, nine clips, state 0 at the start, no mask), the
//!   state machine (`Master_SM`, left in the file), ten additive layers whose weights are bound to the variables
//!   `AddBlend09` to `AddBlend00` (initial 0), the speech layer (off at the start, the jaw and lower lip) and the eye blink
//!   layer (on, weight bound to `EyeBlinkBlend` at 1, the four eye bones).
//! - L2: the hand layer under a clip that drives no finger bone (`a000_020000`, a walk) gives the fingers of the hand
//!   clip the layer plays, exactly: the 30 finger bones of the stack's pose equal the hand clip's, where the walk alone
//!   leaves them at the skeleton's rest. (The running game's pose of the same state holds those fingers within 0.0014 rad
//!   of `a010_000009`'s; that comparison is on a capture of the game, kept with the investigation, not repeated here.)

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod common;
mod inputs;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Keys};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_animation::controller::{LayerAction, Slot, evaluate_stack, layer_stack};
use primase_animation::pose::evaluate;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{
    Clip, Component, Controller, ControllerBinding, Entity, JointId, LayerMask, Motion, Name, Rest,
    RigInstance, Skeleton, Transform, World,
};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{CharacterOptions, read_character_with};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HANDS: [&str; 9] = [
    "a000_000009",
    "a002_000009",
    "a003_000009",
    "a010_000009",
    "a012_000009",
    "a013_000009",
    "a014_000009",
    "a015_000009",
    "a016_000009",
];

fn rotation(rest: Rest) -> [f32; 4] {
    match rest {
        Rest::Trs { rotation, .. } => rotation,
        Rest::Affine(_) => panic!("a driven joint is TRS"),
    }
}

#[test]
fn elden_rings_player_layers_are_a_controller_and_the_hand_layer_keeps_the_fingers() -> Result<()> {
    let (Some(dir), Some(keys), Some(dll)) = (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let install = common::install(
        dir,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let mut clips: Vec<String> = HANDS.iter().map(|s| (*s).to_owned()).collect();
    clips.extend(["a000_020000".to_owned(), "a000_098000".to_owned()]);
    let mut w = World::new();
    read_character_with(
        &install,
        "c0000",
        &decoder,
        CharacterOptions {
            animations: Some("c0000".to_owned()),
            animation_clips: Some(clips.clone()),
            ..CharacterOptions::default()
        },
        &mut w,
    )?;
    let (hk, skeleton) = w
        .iter::<Skeleton>()
        .find(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == "c0000 havok skeleton"))
        .map(|(e, s)| (e, s.clone()))
        .unwrap();
    // the player has no model in the read: the rig poses the Havok skeleton itself
    let rig = w.spawn();
    w.insert(rig, RigInstance { skeleton: hk });
    w.insert(rig, Transform::IDENTITY);
    let mut registry = PassRegistry::new();
    primase_havok::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    let mut run = RunReport::default();
    manager.run(
        &mut w,
        &manager.registry.parse(&format!(
            "lower-havok-clips{{absent=neutral,rate=1,root-motion=leave,clips={}}}, lower-havok-layers",
            clips.join(";")
        ))?,
        &mut run,
    )?;
    let log = &run.steps[1].log;
    assert_eq!(log.counts["controllers lowered"], 1, "{log:?}");

    // L1
    let binding = w.get::<ControllerBinding>(rig).expect("the rig is bound").clone();
    let c: Controller = w.get::<Controller>(binding.controller).unwrap().clone();
    assert_eq!(c.layers.len(), 14);
    let hand = &c.layers[0];
    assert_eq!(hand.name, "BasisAnim_CMSG");
    assert_eq!(hand.default_state, Some(0));
    assert_eq!(
        (hand.default_weight, hand.weight_parameter, hand.mask.is_none()),
        (1.0, None, true)
    );
    assert_eq!(
        hand.states.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        HANDS
    );
    assert!(hand.states.iter().all(|s| matches!(s.motion, Motion::Clip(_))));
    assert_eq!(c.layers[1].name, "Master_SM");
    assert!(matches!(&c.layers[1].states[..], [s] if matches!(&s.motion, Motion::Unsupported(_))));
    let adds: Vec<&str> = c.layers[2..12]
        .iter()
        .map(|l| {
            c.parameters[l.weight_parameter.expect("a bound weight")]
                .name
                .as_str()
        })
        .collect();
    assert_eq!(
        adds,
        [
            "AddBlend09",
            "AddBlend08",
            "AddBlend07",
            "AddBlend06",
            "AddBlend05",
            "AddBlend04",
            "AddBlend03",
            "AddBlend02",
            "AddBlend01",
            "AddBlend00"
        ]
    );
    assert!(
        c.parameters
            .iter()
            .filter(|p| p.name.starts_with("AddBlend"))
            .all(|p| p.default == 0.0)
    );
    let speak = &c.layers[12];
    assert_eq!(
        (speak.default_weight, speak.weight_parameter),
        (0.0, None),
        "off at the start"
    );
    assert!(matches!(&speak.mask, Some(LayerMask::Joints(j)) if j.len() == 2));
    let blink = &c.layers[13];
    let eyes = c.parameters[blink.weight_parameter.unwrap()].clone();
    assert_eq!((eyes.name.as_str(), eyes.default), ("EyeBlinkBlend", 1.0));
    assert!(
        matches!(&blink.mask, Some(LayerMask::Joints(j)) if j.len() == 4 && j.iter().all(|m| m.weight == 1.0))
    );
    Component::verify(&c, binding.controller, &w).unwrap();

    // L2: state 3 of the hand layer is a010_000009; the played clip (the walk) takes the state machine's slot
    let values: Vec<f64> = c.parameters.iter().map(|p| p.default).collect();
    let plans = layer_stack(&c, hk, &[Some(3)], &values)?;
    assert!(matches!(plans[0].action, LayerAction::Plays { state: 3, .. }));
    assert!(matches!(plans[1].action, LayerAction::Unplayable(_)));
    assert!(
        plans[2..12].iter().all(|p| p.action == LayerAction::Nothing),
        "the additive layers are at weight 0"
    );
    assert_eq!(plans[12].action, LayerAction::Nothing, "the speech layer is off");
    assert!(
        matches!(plans[13].action, LayerAction::Plays { .. }),
        "the eye blink layer is on"
    );
    let named = |name: &str| -> Entity {
        w.iter::<Clip>()
            .find(|(_, c)| c.name == name)
            .map(|(e, _)| e)
            .unwrap()
    };
    let (walk_e, fist_e) = (named("a000_020000"), named("a010_000009"));
    let (walk, fist) = (w.get::<Clip>(walk_e).unwrap(), w.get::<Clip>(fist_e).unwrap());
    let lookup = |e: Entity| w.get::<Clip>(e);
    let slot = Slot {
        layer: 1,
        clip: walk,
        time: 0.0,
    };
    let stack = evaluate_stack(hk, &skeleton, None, &plans, &lookup, 0.0, Some(slot))?;
    // LAYERS_DUMP=<file>: the rotation of every joint of the stack's pose (compared to the running game's pose offline)
    if let Some(path) = std::env::var_os("LAYERS_DUMP") {
        let lines: Vec<String> = stack
            .pose
            .joints
            .iter()
            .map(|(id, rest)| format!("{}	{:?}", id.0, rotation(*rest)))
            .collect();
        std::fs::write(
            path,
            lines.join(
                "
",
            ),
        )?;
    }
    let alone = evaluate(walk, hk, &skeleton, None, 0.0)?;
    let hand_pose = evaluate(fist, hk, &skeleton, None, 0.0)?;
    let fingers: Vec<JointId> = skeleton
        .joints
        .iter()
        .filter(|j| {
            let n = j.name.to_lowercase();
            n.contains("finger") || n.contains("thumb") || n.contains("palm")
        })
        .map(|j| j.id)
        .collect();
    assert_eq!(fingers.len(), 30);
    let at = |e: &primase_animation::pose::Evaluated, id: JointId| {
        rotation(e.pose.joints.iter().find(|(j, _)| *j == id).unwrap().1)
    };
    let rest = |id: JointId| rotation(skeleton.joints.iter().find(|j| j.id == id).unwrap().rest);
    let mut moved_by_the_hand = 0;
    for id in &fingers {
        assert_eq!(
            at(&stack, *id),
            at(&hand_pose, *id),
            "finger {id:?}: the hand layer's pose survives the walk"
        );
        assert_eq!(
            at(&alone, *id),
            rest(*id),
            "finger {id:?}: the walk alone leaves it at the rest"
        );
        if at(&stack, *id) != rest(*id) {
            moved_by_the_hand += 1;
        }
    }
    assert!(
        moved_by_the_hand >= 24,
        "{moved_by_the_hand} of 30 fingers are off the rest in the hand pose"
    );
    Ok(())
}
