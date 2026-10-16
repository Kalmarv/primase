//! Event-flag rules from a synthetic script: matching by entity ID and by group, both flag polarities, several rules
//! on one part, calls that cannot be decided, other functions and direct instructions; and the conditions they give,
//! resolved by core's resolve-state.
use primase_core::state::{Literal, ResolveState, StateAxis};
use primase_core::{Instance, Transform, World, pass::Pass, pass::PassLog};
use primase_fromsoft::emevd::{Event, Instruction, Parameter, Script};
use primase_fromsoft::msb::{Part, PartKind};
use primase_fromsoft::sekiro::states::States;

fn call(args: &[i32]) -> Instruction {
    Instruction {
        bank: 2000,
        id: 6,
        args: args.iter().flat_map(|x| x.to_le_bytes()).collect(),
        layer: None,
    }
}
fn direct(bank: i32, id: i32, entity: i32) -> Instruction {
    Instruction {
        bank,
        id,
        args: [entity, 1].iter().flat_map(|x| x.to_le_bytes()).collect(),
        layer: None,
    }
}
/// The four state bytes, packed little-endian (first pair in the low byte).
fn states(s: [u8; 4]) -> i32 {
    i32::from_le_bytes(s)
}
fn part(name: &str, kind: PartKind, entity_id: i32, groups: &[i32]) -> Part {
    let mut g = [-1; 8];
    g[..groups.len()].copy_from_slice(groups);
    Part {
        index: 0,
        name: name.into(),
        kind,
        model: None,
        hit_filter: None,
        position: [0.; 3],
        rotation_degrees: [0.; 3],
        scale: [1.; 3],
        entity_id,
        entity_group_ids: g,
    }
}

/// The castle's event 50 in small: intact walls (group 1117201) disabled while 8306 or 8302 is on, charcoal walls
/// (1117204) while 8302 is off, objects 1116204 while 8302 is off; a call with six arguments, one whose arguments are
/// substituted, another common function naming 1104680, and a story event switching map piece 1114550 directly.
fn script() -> Script {
    Script {
        events: vec![
            Event {
                id: 50,
                rest: 0,
                instructions: vec![
                    call(&[20004110, 1117201, 8306, 8302, 0, 0, states([1, 1, 1, 1])]),
                    call(&[20004110, 1117204, 8302, 0, 0, 0, states([0, 1, 1, 1])]),
                    call(&[20004112, 1116204, 8302, 0, 0, 0, states([0, 1, 1, 1])]),
                    call(&[20004110, 1117203, 8301, 0, 0, states([0, 1, 1, 1])]),
                    call(&[20004115, 1104680, 1104681, 8302]),
                    // a known function whose arguments end in a partial word
                    Instruction {
                        bank: 2000,
                        id: 6,
                        args: [20004110, 1117205, 8302, 0, 0, 0, states([1, 1, 1, 1])]
                            .iter()
                            .flat_map(|x| x.to_le_bytes())
                            .chain([0, 0])
                            .collect(),
                        layer: None,
                    },
                ],
                parameters: vec![],
            },
            Event {
                id: 11115943,
                rest: 1,
                instructions: vec![
                    direct(2012, 1, 1114550),
                    call(&[20004110, 0, 8301, 0, 0, 0, states([1, 1, 1, 1])]),
                ],
                parameters: vec![Parameter {
                    instruction: 1,
                    target: 4,
                    source: 0,
                    bytes: 4,
                    unk: 0,
                }],
            },
        ],
        linked_files: vec![],
        strings: vec![],
    }
}

#[test]
fn rules_match_by_id_and_group_and_count_what_they_leave_open() {
    let mut s = States::read(&script());
    assert_eq!(s.report.rules.get(&20004110), Some(&2));
    assert_eq!(s.report.rules.get(&20004112), Some(&1));
    assert_eq!(
        s.report.undecidable_calls, 3,
        "six arguments, and substituted arguments"
    );
    assert_eq!(s.report.other_calls.get(&20004115), Some(&1));
    assert_eq!(s.report.direct_instructions, 1);

    let mut w = World::new();
    s.spawn_axes(&mut w);
    let axes: Vec<String> = w.iter::<StateAxis>().map(|(_, a)| a.key.clone()).collect();
    assert_eq!(
        axes,
        ["fromsoft.flag.8302", "fromsoft.flag.8306"],
        "flags of decided rules only"
    );
    let axis = |key: &str| w.iter::<StateAxis>().find(|(_, a)| a.key == key).unwrap().0;
    let (f8302, f8306) = (axis("fromsoft.flag.8302"), axis("fromsoft.flag.8306"));

    // by group, both polarities
    let intact = s
        .clause(&part("m003312_3802", PartKind::MapPiece, -1, &[1117201]))
        .unwrap();
    assert_eq!(
        intact.0,
        [
            Literal {
                axis: f8302,
                values: 0b01
            },
            Literal {
                axis: f8306,
                values: 0b01
            }
        ],
        "present while both are off"
    );
    let burnt = s
        .clause(&part("m003412_3902", PartKind::MapPiece, -1, &[1117204]))
        .unwrap();
    assert_eq!(
        burnt.0,
        [Literal {
            axis: f8302,
            values: 0b10
        }],
        "present while 8302 is on"
    );
    // by entity ID; the same ID under another kind is another rule
    assert!(s.clause(&part("x", PartKind::MapPiece, 1117204, &[])).is_some());
    assert!(s.clause(&part("x", PartKind::Collision, 1117204, &[])).is_none());
    // several rules on one part: their AND
    let both = s
        .clause(&part("x", PartKind::MapPiece, -1, &[1117201, 1117204]))
        .unwrap();
    assert_eq!(both.0.len(), 3);
    // sentinels never match
    assert!(s.clause(&part("x", PartKind::MapPiece, -1, &[0, -1])).is_none());
    // left at their default and counted: a part another function names, one a story event switches, one under an
    // undecidable call
    for p in [
        part("h", PartKind::Collision, 1104680, &[]),
        part("m015070_4000", PartKind::MapPiece, 1114550, &[]),
        part("m300102_3700", PartKind::MapPiece, -1, &[1117203]),
    ] {
        assert!(s.clause(&p).is_none(), "{}", p.name);
    }
    // a part of a decided group that a story event also switches directly keeps its default too
    assert!(
        s.clause(&part("both", PartKind::MapPiece, 1114550, &[1117201]))
            .is_none()
    );
    // the misaligned call's entity is left open
    assert!(s.clause(&part("x", PartKind::MapPiece, -1, &[1117205])).is_none());
    assert_eq!(s.report.undecided_parts.values().sum::<usize>(), 5);
}

#[test]
fn conditions_resolve_to_the_flags_state() {
    let mut s = States::read(&script());
    let mut w = World::new();
    s.spawn_axes(&mut w);
    let proto = w.spawn();
    let mut place = |w: &mut World, p: Part| {
        let e = w.spawn();
        w.insert(
            e,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
            },
        );
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(e, primase_core::Name(p.name.clone()));
        if let Some(c) = s.clause(&p) {
            w.insert(e, primase_core::state::Condition(vec![c]));
        }
    };
    place(&mut w, part("intact", PartKind::MapPiece, -1, &[1117201]));
    place(&mut w, part("burnt", PartKind::MapPiece, -1, &[1117204]));
    place(&mut w, part("plain", PartKind::MapPiece, -1, &[]));
    assert_eq!(w.verify_components(), vec![]);
    let names =
        |w: &World| -> Vec<String> { w.iter::<primase_core::Name>().map(|(_, n)| n.0.clone()).collect() };
    let resolve = |w: &mut World, assign: &str| {
        let mut r = ResolveState::default();
        if !assign.is_empty() {
            let (k, v) = assign.split_once(':').unwrap();
            r.assign.insert(k.into(), v.into());
        }
        r.run(w, &mut PassLog::default()).unwrap();
    };
    let mut a = w.clone();
    resolve(&mut a, "");
    assert_eq!(names(&a), ["intact", "plain"], "the game's start: every flag off");
    resolve(&mut w, "fromsoft.flag.8302:on");
    assert_eq!(names(&w), ["burnt", "plain"]);
}
