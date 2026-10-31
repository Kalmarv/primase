//! `hold-fromsoft-weapons{grip=one-handed|two-handed-right|two-handed-left}`: the weapons a character holds, where the
//! game holds them. The read keeps, on the root of each weapon model's asset (a form of the weapon's part: the katana
//! `wp_a_0300_l`, its scabbard `wp_a_0300_1_l`), which hand slot it is in, which weapon row it is and the character's dummy
//! polygon each way of holding puts it at (`fromsoft.held_weapon`, from `WepAbsorpPosParam`). This pass, after
//! `place-assets` gave every asset its rig, attaches each model's rig (`core.attachment`) to the socket of that dummy
//! polygon on the character's rig, for the grip the program states (how the player holds the weapons is the player's
//! state, as which weapons they hold is):
//!
//! - `one-handed`: each weapon in its own hand (`right_<n>`, `left_<n>`);
//! - `two-handed-right`: the right hand's weapons in both hands (`both_<n>`), the left's hung (`leftHang_<n>`);
//! - `two-handed-left`: the left's in both hands (`leftBoth_<n>`), the right's hung (`rightHang_<n>`).
//!
//! The character's rig is the one rig (not a held weapon's) whose skeleton has the socket (`dummy <id>`, the player's
//! dummy polygons carried onto the assembled skeleton by `assemble-character`). Elden Ring's hold rows name a body part's
//! polygon `n` as `2000 + n` (its sockets are so named, [`crate::sekiro::effect_data::BODY_PART_DUMMIES`]), and say how
//! each slot shows its model ([`Shown`]): such a number shown in a hand is the character's own polygon `n` (the Caestus
//! held in both hands shows its second fist, `2117`, on the left hand, where the character's polygon 117 is); shown on
//! the body, the body part's. If the body part lacks that polygon, placement is refused by model name and the model
//! is omitted. This is an implementation limit, not a visibility rule: Elden Ring 2.7's native attachment path
//! (9eef30 -> 9f4f80 -> 9eb580 -> b477d0) falls back to the base character's polygon `n`, independently of model state.
//! The saved two-handed Uchigatana hold has a visible stowed Longbow at 2040 although the altered chest lacks polygon
//! 40. Conversely, Leontiel's model 1 at 2116 was absent in three captured idle holds; its visibility condition is
//! unresolved. This pass does not evaluate that condition or apply the body-to-base fallback. One a held weapon's
//! model has, or shown in a way no stock slot uses,
//! is refused. A model the grip puts at no dummy polygon, or at one no rig has, stays where it is, counted; a socket two
//! rigs have is refused. The rig's
//! `core.transform` is where the attachment puts it with the character's rig in its pose as read (its `core.rig_pose`,
//! else its rest).
//!
//! The model's frame at its socket ([`HOLD_TURN`]) is a reading of the community's tools, not checked against the
//! game: reported as an approximation on every model it places.
use crate::sekiro::effect_data::{BODY_PART_DUMMIES, HandSlot, HeldWeapon, Hold, Holds, Shown};
use primase_animation::pose::{affine, compose, locals, model_transforms, transform};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Asset, Attachment, ClipBinding, Component, Entity, RigInstance, RigPose, Role, Skeleton, Sockets,
    Transform, World,
};
use std::collections::BTreeSet;

/// How the player holds the weapons (`grip=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// Each weapon in its own hand.
    OneHanded,
    /// The right hand's weapons in both hands, the left's hung.
    TwoHandedRight,
    /// The left hand's weapons in both hands, the right's hung.
    TwoHandedLeft,
}

impl Choice for Grip {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("one-handed", Grip::OneHanded),
        ("two-handed-right", Grip::TwoHandedRight),
        ("two-handed-left", Grip::TwoHandedLeft),
    ];
}

impl Holds {
    /// Where a model of a weapon in the left hand's slots (`left`) or the right's stands in `grip`.
    pub fn in_grip(&self, left: bool, grip: Grip) -> Option<Hold> {
        match (left, grip) {
            (false, Grip::OneHanded) => self.right,
            (false, Grip::TwoHandedRight) => self.both,
            (false, Grip::TwoHandedLeft) => self.right_hang,
            (true, Grip::OneHanded) => self.left,
            (true, Grip::TwoHandedRight) => self.left_hang,
            (true, Grip::TwoHandedLeft) => self.left_both,
        }
    }
}

/// A held model's frame in its socket's frame: a half turn about the socket's up axis (the dummy polygon's upward
/// direction; x and z reversed). The community's reading: the Sekiro animation bridge's weapon attachment
/// (`standalone/weapon_assets.py`: "DS3 and Sekiro use the same net Y half-turn in a dummy's frame", GPL-3.0) and the
/// stock data agree with it this far: with it the curve of Sekiro's katana (its dummy polygons bend towards the model's
/// +x along the blade) faces up and its edge down when the Wolf's arm holds it out at rest. Not checked against the
/// game.
pub const HOLD_TURN: Transform = Transform {
    cols: [
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 0.0],
    ],
};

/// `hold-fromsoft-weapons` (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldFromSoftWeapons {
    pub grip: Grip,
}

/// `hold-fromsoft-weapons`' declaration ([`primase_core::pass::PassRegistry::add`]).
pub const HOLD_FROMSOFT_WEAPONS: PassSpec = PassSpec {
    name: "hold-fromsoft-weapons",
    options: &[OptionSpec::choice::<Grip>(
        "grip",
        "how the weapons are held: each in its hand, the right hand's in both hands, or the left's",
    )],
    make: |g: &Given| {
        Ok(Box::new(HoldFromSoftWeapons {
            grip: g.choice::<Grip>("grip"),
        }))
    },
};

/// Leave the held model whose asset's root is `root` (its rig instance `rig`) out of the scene: what only its asset reaches,
/// its rig instance and what refers to them alone (the placements `place-assets` made of its parts), despawned and
/// declared; what also refers to other entities (a clip binding of several meshes) stops naming them
/// ([`primase_core::ledger::despawn`]).
fn leave_out(w: &mut World, root: Entity, rig: Entity, log: &mut PassLog, reason: &str) {
    let others: Vec<Entity> = w.iter::<Asset>().map(|(e, _)| e).filter(|e| *e != root).collect();
    let shared = w.closure(others);
    let mut gone: BTreeSet<Entity> = w.closure([root, rig]).difference(&shared).copied().collect();
    loop {
        let referring: Vec<Entity> = w
            .entities()
            .filter(|e| !gone.contains(e) && !shared.contains(e))
            .filter(|e| {
                let refs = w.references(*e);
                !refs.is_empty() && refs.iter().all(|r| gone.contains(r))
            })
            .collect();
        if referring.is_empty() {
            break;
        }
        gone.extend(referring);
    }
    for e in gone {
        primase_core::ledger::despawn(w, e, log, reason);
    }
}

/// The rig instances of `w`: each with the skeleton it poses.
fn rigs(w: &World) -> Vec<(Entity, Entity)> {
    w.iter::<RigInstance>().map(|(e, r)| (e, r.skeleton)).collect()
}

impl Pass for HoldFromSoftWeapons {
    fn name(&self) -> &'static str {
        "hold-fromsoft-weapons"
    }
    fn options(&self) -> Options {
        Options::from([("grip".to_owned(), self.grip.text().to_owned())])
    }
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    /// A world with no held weapon is left as it is.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[Attachment::NAME]
    }
    /// A model left out of the scene stops being listed by the assets and clip bindings that name its entities.
    fn modifies(&self) -> &[&'static str] {
        &[Transform::NAME, Asset::NAME, ClipBinding::NAME]
    }
    /// A model whose placement is refused is omitted, with its name and reason in the pass log.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "a held model's frame at its dummy polygon turned half about the polygon's upward direction, the community tools' reading, not checked against the game"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // each held model: its asset's rig skeleton and what the read says of it
        let mut held: Vec<(Entity, String, HeldWeapon, Entity)> = Vec::new();
        for (root, h) in w.iter::<HeldWeapon>() {
            let label = w
                .get::<primase_core::Name>(root)
                .map_or_else(|| format!("{root:?}"), |n| n.0.clone());
            let asset = w
                .get::<Asset>(root)
                .ok_or_else(|| format!("{label}: fromsoft.held_weapon on what is not an asset's root"))?;
            let skeleton = asset
                .members
                .iter()
                .find(|(_, r)| *r == Role::Rig)
                .map(|(e, _)| *e)
                .ok_or_else(|| format!("{label}: a held weapon's asset with no rig"))?;
            held.push((root, label, *h, skeleton));
        }
        let held_skeletons: BTreeSet<Entity> = held.iter().map(|(_, _, _, s)| *s).collect();
        let held_slots: BTreeSet<HandSlot> = held.iter().map(|(_, _, h, _)| h.slot).collect();
        let all = rigs(w);
        let mut attached = 0u64;
        let mut at_own = 0u64;
        let mut left_out = 0u64;
        for (root, label, h, skeleton) in held {
            let weapon_rig = match all
                .iter()
                .filter(|(_, s)| *s == skeleton)
                .collect::<Vec<_>>()
                .as_slice()
            {
                [(rig, _)] => *rig,
                [] => return Err(format!("{label}: its rig is not placed (run place-assets first)")),
                more => return Err(format!("{label}: {} rigs pose its skeleton", more.len())),
            };
            let Some(Hold { dummy, shown }) = h.holds.in_grip(h.slot.left, self.grip) else {
                log.omit(
                    "held weapon models the grip puts at no dummy polygon, left where they are",
                    1,
                );
                continue;
            };
            // the rigs whose skeleton has polygon `id`'s socket: the character's (`held` false), or the held weapons'
            let holders_of = |id: i32, held: bool| -> Vec<(Entity, Entity, primase_core::Socket)> {
                let name = crate::pose::dummy_socket_name(id);
                all.iter()
                    .filter(|(_, s)| held_skeletons.contains(s) == held)
                    .filter_map(|(rig, s)| {
                        let socket = w.get::<Sockets>(*s)?.0.iter().find(|k| k.name == name)?.clone();
                        Some((*rig, *s, socket))
                    })
                    .collect()
            };
            // a body part's polygon `2000 + n`: where the slot shows the model says whose polygon `n` it is
            let mut used = dummy;
            let mut holders = holders_of(dummy, false);
            if (BODY_PART_DUMMIES..BODY_PART_DUMMIES + 1_000).contains(&dummy) {
                let n = dummy - BODY_PART_DUMMIES;
                match shown {
                    // in a hand: the character's own polygon `n` (the Caestus's second fist, 2117, on the left hand)
                    Some(Shown::LeftHand | Shown::RightHand) => {
                        (used, holders) = (n, holders_of(n, false));
                        at_own += 1;
                    }
                    // Socket existence does not establish visibility. Do not silently turn a missing body socket
                    // into a claim that the game hides the model, or draw it using an unevaluated visibility state.
                    Some(Shown::Body) | None if holders.is_empty() => {
                        let on_weapons: Vec<i32> = held_slots
                            .iter()
                            .map(|slot| slot.global_id(n))
                            .filter(|id| !holders_of(*id, true).is_empty())
                            .collect();
                        if !on_weapons.is_empty() {
                            return Err(format!(
                                "{label}: shown on the body at {dummy}, a polygon no worn body part has and a held weapon's model has ({on_weapons:?}): what the game does with it is not known"
                            ));
                        }
                        let reason = format!(
                            "{label}: placement refused at body dummy {dummy}: no worn body part has this socket; base-character dummy {n} fallback is not implemented and model visibility is unresolved"
                        );
                        log.omit(&reason, 1);
                        leave_out(w, root, weapon_rig, log, &reason);
                        left_out += 1;
                        continue;
                    }
                    Some(Shown::Body) | None => {}
                    Some(Shown::Other(v)) => {
                        return Err(format!(
                            "{label}: at {dummy} shown as {v} (dispPosType), a value no stock slot uses: where the game puts it is not known"
                        ));
                    }
                }
            }
            let name = crate::pose::dummy_socket_name(used);
            let (holder, holder_skeleton, socket) = match holders.as_slice() {
                [one] => one.clone(),
                [] => {
                    log.omit(
                        "held weapon models whose dummy polygon no character rig has, left where they are",
                        1,
                    );
                    continue;
                }
                more => {
                    return Err(format!(
                        "{label}: {} rigs have the socket {name}: which holds the weapon is not said",
                        more.len()
                    ));
                }
            };
            // where the attachment puts the model with the character in its pose as read
            let skeleton = w
                .get::<Skeleton>(holder_skeleton)
                .ok_or("a rig's skeleton holds no core.skeleton")?;
            let model =
                model_transforms(skeleton, &locals(skeleton, w.get::<RigPose>(holder))).map_err(|e| e.0)?;
            let joint = model
                .get(&socket.joint)
                .ok_or_else(|| format!("socket {name} is on a joint its skeleton does not pose"))?;
            let placed = w.get::<Transform>(holder).copied().unwrap_or(Transform::IDENTITY);
            let at = compose(
                &affine(&placed),
                &compose(&compose(joint, &affine(&socket.offset)), &affine(&HOLD_TURN)),
            );
            w.insert(weapon_rig, transform(&at));
            w.insert(
                weapon_rig,
                Attachment {
                    rig: holder,
                    socket: socket.id,
                    offset: HOLD_TURN,
                },
            );
            attached += 1;
        }
        log.count("held weapon models attached at their dummy polygon", attached);
        log.count(
            "held weapon models shown in a hand at a body part's polygon number (2000 + n), held at the character's own polygon n",
            at_own,
        );
        log.count(
            "held weapon models with unresolved placement at a missing body socket, omitted by name",
            left_out,
        );
        if attached > 0 {
            log.approximate(
                "a held model's frame at its dummy polygon turned half about the polygon's upward direction (the community tools' reading, not checked against the game)",
                attached,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::effect_data::HandSlot;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{Joint, JointId, Name, Rest, Socket, SocketId};

    fn at(t: [f32; 3]) -> Rest {
        Rest::Trs {
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    fn translate(t: [f32; 3]) -> Transform {
        Transform {
            cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], t],
        }
    }

    /// A rig (an asset root, its skeleton and its placed rig instance) of the joints `joints` (name, parent, translation).
    fn rig(w: &mut World, label: &str, joints: &[(&str, Option<u32>, [f32; 3])]) -> (Entity, Entity, Entity) {
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: joints
                    .iter()
                    .enumerate()
                    .map(|(i, &(name, parent, t))| Joint {
                        id: JointId(i as u32),
                        name: name.to_owned(),
                        parent: parent.map(JointId),
                        rest: at(t),
                    })
                    .collect(),
            },
        );
        let root = w.spawn();
        w.insert(root, Name(label.to_owned()));
        w.insert(
            root,
            Asset {
                members: vec![(skeleton, Role::Rig)],
            },
        );
        let instance = w.spawn();
        w.insert(instance, RigInstance { skeleton });
        w.insert(instance, Transform::IDENTITY);
        (root, skeleton, instance)
    }

    fn holds(right: Option<i32>, both: Option<i32>, right_hang: Option<i32>) -> Holds {
        Holds {
            right: right.map(Hold::at),
            left: None,
            both: both.map(Hold::at),
            left_hang: None,
            right_hang: right_hang.map(Hold::at),
            left_both: None,
        }
    }

    /// The Wolf's case in small: a character rig standing at x = 10 whose hand (joint 1, 1 up) holds socket `dummy 20`
    /// 0.5 along x, and `dummy 147` (a hip); the katana (model 0) held one-handed at `dummy 20`, the scabbard (model 1)
    /// at `dummy 147`, a third model the row puts nowhere.
    fn world() -> (World, [Entity; 4]) {
        let mut w = World::new();
        let (_, character, holder) = rig(
            &mut w,
            "assembled",
            &[("Root", None, [0.0; 3]), ("R_Weapon", Some(0), [0.0, 1.0, 0.0])],
        );
        w.insert(holder, translate([10.0, 0.0, 0.0]));
        w.insert(
            character,
            Sockets(vec![
                Socket {
                    id: SocketId(0),
                    name: "dummy 20".into(),
                    joint: JointId(1),
                    offset: translate([0.5, 0.0, 0.0]),
                },
                Socket {
                    id: SocketId(1),
                    name: "dummy 147".into(),
                    joint: JointId(0),
                    offset: translate([0.0, 0.9, -0.2]),
                },
            ]),
        );
        let slot = HandSlot {
            left: false,
            index: 0,
        };
        let mut out = [holder; 4];
        for (i, (label, h)) in [
            ("wp_a_0300_l", holds(Some(20), Some(20), Some(147))),
            ("wp_a_0300_1_l", holds(Some(147), Some(147), Some(147))),
            ("wp_a_0300_2_l", holds(None, None, None)),
        ]
        .into_iter()
        .enumerate()
        {
            let (root, _, instance) = rig(&mut w, label, &[("WP_A_0300", None, [0.0; 3])]);
            w.insert(
                root,
                HeldWeapon {
                    slot,
                    row: 8000000,
                    model: i as u8,
                    holds: h,
                },
            );
            out[i + 1] = instance;
        }
        out[0] = holder;
        (w, out)
    }

    fn run(w: &mut World, pipeline: &str) -> Result<RunReport, String> {
        let mut r = PassRegistry::new();
        r.add(&HOLD_FROMSOFT_WEAPONS);
        let m = PassManager::new(r).with_pass_checks(true);
        let mut report = RunReport::default();
        m.run(
            w,
            &m.registry.parse(pipeline).map_err(|e| e.to_string())?,
            &mut report,
        )
        .map_err(|e| e.to_string())?;
        Ok(report)
    }

    /// One-handed, the katana follows the hand's socket and the scabbard the hip's, each turned half about y and placed
    /// where the character's rest puts them; the model the row puts nowhere is counted and stays.
    #[test]
    fn each_model_is_attached_at_its_dummy_polygon_for_the_grip() {
        let (mut w, [holder, katana, scabbard, third]) = world();
        let report = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap();
        let a = w.get::<Attachment>(katana).unwrap();
        assert_eq!((a.rig, a.socket, a.offset), (holder, SocketId(0), HOLD_TURN));
        // x 10 (the rig) + 0.5 (the socket), y 1 (the hand); the frame turned: x and z reversed
        assert_eq!(
            w.get::<Transform>(katana).unwrap().cols,
            [
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, -1.0],
                [10.5, 1.0, 0.0]
            ]
        );
        assert_eq!(w.get::<Attachment>(scabbard).unwrap().socket, SocketId(1));
        assert_eq!(w.get::<Transform>(scabbard).unwrap().cols[3], [10.0, 0.9, -0.2]);
        assert!(w.get::<Attachment>(third).is_none());
        assert_eq!(w.get::<Transform>(third), Some(&Transform::IDENTITY));
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["held weapon models attached at their dummy polygon"],
            2
        );
        assert_eq!(
            log.omitted["held weapon models the grip puts at no dummy polygon, left where they are"],
            1
        );
        assert_eq!(w.verify_components(), vec![]);
    }

    /// Two-handed on the left, a right-hand weapon is hung (`rightHang`: the hip); a dummy polygon no rig has is counted;
    /// a socket two character rigs have is refused; the grip is required and printed back.
    #[test]
    fn the_grip_chooses_the_dummy_polygon_and_what_is_not_there_is_counted_or_refused() {
        let (mut w, [_, katana, ..]) = world();
        run(&mut w, "hold-fromsoft-weapons{grip=two-handed-left}").unwrap();
        assert_eq!(w.get::<Attachment>(katana).unwrap().socket, SocketId(1));

        let (mut w, [_, katana, ..]) = world();
        let root = w
            .iter::<HeldWeapon>()
            .find(|(r, _)| w.get::<Name>(*r).is_some_and(|n| n.0 == "wp_a_0300_l"))
            .unwrap()
            .0;
        w.get_mut::<HeldWeapon>(root).unwrap().holds.right = Some(Hold::at(99));
        let report = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap();
        assert!(w.get::<Attachment>(katana).is_none());
        assert_eq!(
            report.steps[0].log.omitted["held weapon models whose dummy polygon no character rig has, left where they are"],
            1
        );

        let (mut w, _) = world();
        let (_, second, _) = rig(&mut w, "another", &[("Root", None, [0.0; 3])]);
        w.insert(
            second,
            Sockets(vec![Socket {
                id: SocketId(0),
                name: "dummy 20".into(),
                joint: JointId(0),
                offset: Transform::IDENTITY,
            }]),
        );
        assert!(
            run(&mut w, "hold-fromsoft-weapons{grip=one-handed}")
                .unwrap_err()
                .contains("2 rigs have the socket dummy 20")
        );

        let (mut w, _) = world();
        assert!(
            run(&mut w, "hold-fromsoft-weapons")
                .unwrap_err()
                .contains("grip is required")
        );
        assert_eq!(
            HoldFromSoftWeapons {
                grip: Grip::TwoHandedRight
            }
            .options()["grip"],
            "two-handed-right"
        );
    }

    /// Elden Ring's scabbard with the sword drawn hangs at a body part's polygon (`2069`: the body part's polygon 69, named
    /// so on the character's skeleton, shown on the body), not at the character's own polygon 69. A body part polygon
    /// number shown in a hand (the Caestus's second fist, `2117`) is the character's own polygon; one shown on the body
    /// that no worn part has leaves the model out of the scene; one a held weapon's model has, or a way of showing no stock
    /// slot uses, is refused.
    #[test]
    fn a_body_part_polygon_number_is_taken_by_how_the_slot_shows_the_model() {
        let socket = |id: u32, name: &str, z: f32| Socket {
            id: SocketId(id),
            name: name.into(),
            joint: JointId(0),
            offset: translate([0.0, 0.9, z]),
        };
        let scabbard_root = |w: &World| {
            w.iter::<HeldWeapon>()
                .find(|(r, _)| w.get::<Name>(*r).is_some_and(|n| n.0 == "wp_a_0300_1_l"))
                .unwrap()
                .0
        };
        let scabbard_at = |w: &mut World, dummy: i32, shown: Shown| {
            let root = scabbard_root(w);
            w.get_mut::<HeldWeapon>(root).unwrap().holds.right = Some(Hold {
                dummy,
                shown: Some(shown),
            });
        };
        let character = |w: &World| {
            w.iter::<Sockets>()
                .find(|(_, s)| s.0.iter().any(|k| k.name == "dummy 20"))
                .unwrap()
                .0
        };
        let with_sockets = |sockets: Vec<Socket>| {
            let (mut w, models) = world();
            let c = character(&w);
            w.get_mut::<Sockets>(c).unwrap().0.extend(sockets);
            (w, models)
        };

        // on the body: the body part's polygon, beside the character's own of the same number
        let (mut w, [_, _, scabbard, _]) =
            with_sockets(vec![socket(2, "dummy 69", -0.1), socket(3, "dummy 2069", -0.3)]);
        scabbard_at(&mut w, 2069, Shown::Body);
        let report = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap();
        assert_eq!(w.get::<Attachment>(scabbard).unwrap().socket, SocketId(3));
        assert_eq!(w.get::<Transform>(scabbard).unwrap().cols[3], [10.0, 0.9, -0.3]);
        assert_eq!(
            report.steps[0].log.omitted.len(),
            1,
            "{:?}",
            report.steps[0].log.omitted
        );

        // in a hand: the character's own polygon of that number
        let (mut w, [_, _, scabbard, _]) =
            with_sockets(vec![socket(2, "dummy 117", -0.1), socket(3, "dummy 2117", -0.3)]);
        scabbard_at(&mut w, 2117, Shown::LeftHand);
        let report = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap();
        assert_eq!(w.get::<Attachment>(scabbard).unwrap().socket, SocketId(2));
        assert_eq!(w.get::<Transform>(scabbard).unwrap().cols[3], [10.0, 0.9, -0.1]);
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["held weapon models attached at their dummy polygon"],
            2
        );
        assert_eq!(
            log.counts["held weapon models shown in a hand at a body part's polygon number (2000 + n), held at the character's own polygon n"],
            1
        );

        // A base-character socket is not enough to establish a model's visibility. Omit by name, without claiming
        // that the native game hides it. The refusal survives despawning the model and its referring placements.
        let (mut w, [_, _, scabbard, _]) = with_sockets(vec![socket(2, "dummy 69", -0.1)]);
        scabbard_at(&mut w, 2069, Shown::Body);
        let root = scabbard_root(&w);
        let skeleton = w.get::<RigInstance>(scabbard).unwrap().skeleton;
        // something that refers to the model alone (here, attached to its rig)
        let placement = w.spawn();
        w.insert(
            placement,
            Attachment {
                rig: scabbard,
                socket: SocketId(0),
                offset: Transform::IDENTITY,
            },
        );
        let report = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap();
        for e in [root, skeleton, scabbard, placement] {
            assert!(!w.is_alive(e), "{e:?}");
        }
        assert!(w.iter::<HeldWeapon>().all(|(r, _)| r != root));
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["held weapon models with unresolved placement at a missing body socket, omitted by name"],
            1
        );
        assert!(log.omitted.keys().any(|reason| reason.contains("wp_a_0300_1_l")
            && reason.contains("body dummy 2069")
            && reason.contains("visibility is unresolved")));
        assert!(
            log.drops
                .keys()
                .all(|(_, reason)| !reason.contains("game does not draw"))
        );
        assert_eq!(
            log.counts["held weapon models attached at their dummy polygon"],
            1
        );

        // a held weapon's model has that polygon (as the hand slot names it): refused
        let (mut w, [_, katana, _, _]) = world();
        let blade = w.get::<RigInstance>(katana).unwrap().skeleton;
        let slot = HandSlot {
            left: false,
            index: 0,
        };
        w.insert(
            blade,
            Sockets(vec![socket(0, &format!("dummy {}", slot.global_id(69)), 0.0)]),
        );
        scabbard_at(&mut w, 2069, Shown::Body);
        let err = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap_err();
        assert!(err.contains("a held weapon's model has"), "{err}");

        // a way of showing no stock slot uses: refused
        let (mut w, _) = with_sockets(vec![socket(3, "dummy 2069", -0.3)]);
        scabbard_at(&mut w, 2069, Shown::Other(2));
        let err = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap_err();
        assert!(err.contains("shown as 2"), "{err}");
    }

    #[test]
    fn a_weapon_whose_rig_is_not_placed_is_refused() {
        let (mut w, [_, katana, ..]) = world();
        w.despawn(katana);
        assert!(
            run(&mut w, "hold-fromsoft-weapons{grip=one-handed}")
                .unwrap_err()
                .contains("not placed")
        );
    }
}
