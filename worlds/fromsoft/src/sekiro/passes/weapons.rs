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
//! polygon `n` as `2000 + n` (its sockets are so named, [`crate::sekiro::effect_data::BODY_PART_DUMMIES`]). A model the
//! grip puts at no dummy polygon, or at one no rig has, stays where it is, counted, and one at a body part's polygon the
//! worn body part does not have is named (the rows name 2116 and 2117, which no body part has; where the game puts such a
//! model is not known, so it is not placed); a socket two rigs have is refused. The rig's
//! `core.transform` is where the attachment puts it with the character's rig in its pose as read (its `core.rig_pose`,
//! else its rest).
//!
//! The model's frame at its socket ([`HOLD_TURN`]) is a reading of the community's tools, not checked against the
//! game: reported as an approximation on every model it places.
use crate::sekiro::effect_data::{BODY_PART_DUMMIES, HeldWeapon, Holds};
use primase_animation::pose::{affine, compose, locals, model_transforms, transform};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Asset, Attachment, Component, Entity, RigInstance, RigPose, Role, Skeleton, Sockets, Transform, World,
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
    /// The dummy polygon a model of a weapon in the left hand's slots (`left`) or the right's stands at in `grip`.
    pub fn in_grip(&self, left: bool, grip: Grip) -> Option<i32> {
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
    fn modifies(&self) -> &[&'static str] {
        &[Transform::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a held model's frame at its dummy polygon turned half about the polygon's upward direction, the community tools' reading, not checked against the game"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // each held model: its asset's rig skeleton and what the read says of it
        let mut held: Vec<(String, HeldWeapon, Entity)> = Vec::new();
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
            held.push((label, *h, skeleton));
        }
        let held_skeletons: BTreeSet<Entity> = held.iter().map(|(_, _, s)| *s).collect();
        let all = rigs(w);
        let mut attached = 0u64;
        for (label, h, skeleton) in held {
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
            let Some(dummy) = h.holds.in_grip(h.slot.left, self.grip) else {
                log.omit(
                    "held weapon models the grip puts at no dummy polygon, left where they are",
                    1,
                );
                continue;
            };
            let name = crate::pose::dummy_socket_name(dummy);
            let holders: Vec<(Entity, Entity, primase_core::Socket)> = all
                .iter()
                .filter(|(_, s)| !held_skeletons.contains(s))
                .filter_map(|(rig, s)| {
                    let socket = w.get::<Sockets>(*s)?.0.iter().find(|k| k.name == name)?.clone();
                    Some((*rig, *s, socket))
                })
                .collect();
            let (holder, holder_skeleton, socket) = match holders.as_slice() {
                [one] => one.clone(),
                [] if (BODY_PART_DUMMIES..BODY_PART_DUMMIES + 1_000).contains(&dummy) => {
                    log.omit(
                        &format!(
                            "held weapon models at a body part's dummy polygon the worn body part does not have, not placed (where the game puts them is not known): {label} at {dummy}"
                        ),
                        1,
                    );
                    continue;
                }
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
            right,
            left: None,
            both,
            left_hang: None,
            right_hang,
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
        w.get_mut::<HeldWeapon>(root).unwrap().holds.right = Some(99);
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
    /// so on the character's skeleton), not at the character's own polygon 69; where the skeleton has no `dummy 2069` it is
    /// not placed, named, even where the character has a polygon 69.
    #[test]
    fn a_body_part_polygon_is_found_by_its_global_id_and_one_the_body_part_lacks_is_named() {
        let socket = |id: u32, name: &str, z: f32| Socket {
            id: SocketId(id),
            name: name.into(),
            joint: JointId(0),
            offset: translate([0.0, 0.9, z]),
        };
        let scabbard_at = |w: &mut World, at: i32| {
            let root = w
                .iter::<HeldWeapon>()
                .find(|(r, _)| w.get::<Name>(*r).is_some_and(|n| n.0 == "wp_a_0300_1_l"))
                .unwrap()
                .0;
            w.get_mut::<HeldWeapon>(root).unwrap().holds.right = Some(at);
        };
        let character = |w: &World| {
            w.iter::<Sockets>()
                .find(|(_, s)| s.0.iter().any(|k| k.name == "dummy 20"))
                .unwrap()
                .0
        };

        // the body part's polygon, beside the character's own of the same number
        let (mut w, [_, _, scabbard, _]) = world();
        let c = character(&w);
        w.get_mut::<Sockets>(c).unwrap().0.extend([socket(2, "dummy 69", -0.1), socket(3, "dummy 2069", -0.3)]);
        scabbard_at(&mut w, 2069);
        let report = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap();
        assert_eq!(w.get::<Attachment>(scabbard).unwrap().socket, SocketId(3));
        assert_eq!(w.get::<Transform>(scabbard).unwrap().cols[3], [10.0, 0.9, -0.3]);
        assert_eq!(report.steps[0].log.omitted.len(), 1, "{:?}", report.steps[0].log.omitted);

        // no body part polygon of that number: not placed, though the character has its own 69
        let (mut w, [_, _, scabbard, _]) = world();
        let c = character(&w);
        w.get_mut::<Sockets>(c).unwrap().0.push(socket(2, "dummy 69", -0.1));
        scabbard_at(&mut w, 2069);
        let report = run(&mut w, "hold-fromsoft-weapons{grip=one-handed}").unwrap();
        assert!(w.get::<Attachment>(scabbard).is_none());
        assert_eq!(w.get::<Transform>(scabbard), Some(&Transform::IDENTITY));
        let log = &report.steps[0].log;
        assert_eq!(
            log.omitted["held weapon models at a body part's dummy polygon the worn body part does not have, not placed (where the game puts them is not known): wp_a_0300_1_l at 2069"],
            1
        );
        assert_eq!(log.counts["held weapon models attached at their dummy polygon"], 1);
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
