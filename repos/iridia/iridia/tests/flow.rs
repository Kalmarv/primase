//! The dataflow query ([`iridia::flow`]) on `fixtures/flow.hlsl`, each entry point compiled to both bytecodes
//! and lifted by both lifters: the query sees each way a vertex colour can reach the target or the discard (by its
//! value, as a coordinate, through a condition, not at all), sees a blend's structure, and says the same of either
//! bytecode.
use std::collections::{BTreeMap, BTreeSet};

use iridia::flow::{Flow, Node, NodeId, Op, Read, Values, Via};
use iridia::{Interface, Label, Program};

/// Each entry point of the fixture, as fxc and dxc compile it.
const FIXTURES: [(&str, &[u8], &[u8]); 6] = [
    (
        "value",
        include_bytes!("fixtures/flow_value.dxbc"),
        include_bytes!("fixtures/flow_value.dxil"),
    ),
    (
        "coordinate",
        include_bytes!("fixtures/flow_coordinate.dxbc"),
        include_bytes!("fixtures/flow_coordinate.dxil"),
    ),
    (
        "control",
        include_bytes!("fixtures/flow_control.dxbc"),
        include_bytes!("fixtures/flow_control.dxil"),
    ),
    (
        "never",
        include_bytes!("fixtures/flow_never.dxbc"),
        include_bytes!("fixtures/flow_never.dxil"),
    ),
    (
        "blend",
        include_bytes!("fixtures/flow_blend.dxbc"),
        include_bytes!("fixtures/flow_blend.dxil"),
    ),
    (
        "carried",
        include_bytes!("fixtures/flow_carried.dxbc"),
        include_bytes!("fixtures/flow_carried.dxil"),
    ),
];

/// The entry point `entry` lifted from each bytecode.
fn lifted(entry: &str) -> Vec<(&'static str, Program)> {
    let (_, dxbc, dxil) = FIXTURES.iter().find(|f| f.0 == entry).expect("a fixture");
    vec![
        ("dxbc", iridia_dxbc::lift(dxbc).expect("fxc's program lifts")),
        ("dxil", iridia_dxil::lift(dxil).expect("dxc's program lifts")),
    ]
}

fn target() -> Label {
    Label {
        name: "SV_Target".into(),
        index: 0,
    }
}

/// A source by its interface's names: `COLOR0.w`, `base_map.x`.
fn input_name(interface: &Interface, input: usize, lane: u8) -> String {
    let l = &interface.inputs[input].label;
    format!("{}{}.{}", l.name, l.index, b"xyzw"[lane as usize] as char)
}

fn texel_name(interface: &Interface, resource: iridia::ResourceId, lane: u8) -> String {
    let r = interface
        .resources
        .iter()
        .find(|r| r.id == resource)
        .expect("a resource");
    format!("{}.{}", r.provenance.name, b"xyzw"[lane as usize] as char)
}

/// What reaches the target's lane `lane`, by name, with the via.
fn reaching(program: &Program, flow: &Flow, lane: u8) -> BTreeMap<String, Via> {
    let i = &program.interface;
    let mut out: BTreeMap<String, Via> = flow
        .reaching(i, &target(), lane)
        .into_iter()
        .map(|(l, v)| (input_name(i, l.input, l.lane), v))
        .collect();
    out.extend(
        flow.texels_reaching(i, &target(), lane)
            .into_iter()
            .map(|(t, v)| (texel_name(i, t.resource, t.lane), v)),
    );
    out
}

/// What reaches the discard, by name.
fn discarding(program: &Program, flow: &Flow) -> BTreeMap<String, Via> {
    let i = &program.interface;
    let reach = flow.discard_reach();
    let mut out: BTreeMap<String, Via> = reach
        .inputs
        .iter()
        .map(|(l, v)| (input_name(i, l.input, l.lane), *v))
        .collect();
    out.extend(
        reach
            .texels
            .iter()
            .map(|(t, v)| (texel_name(i, t.resource, t.lane), *v)),
    );
    out
}

fn named(pairs: &[(&str, Via)]) -> BTreeMap<String, Via> {
    pairs.iter().map(|(n, v)| ((*n).to_owned(), *v)).collect()
}

#[test]
fn a_colour_that_multiplies_the_texel_reaches_the_target_by_value() {
    for (bytecode, program) in lifted("value") {
        let flow = Flow::of_program(&program).unwrap();
        assert_eq!(
            reaching(&program, &flow, 3),
            named(&[
                ("COLOR0.w", Via::Value),
                ("TEXCOORD0.x", Via::Coordinate),
                ("TEXCOORD0.y", Via::Coordinate),
                ("base_map.w", Via::Value),
            ]),
            "{bytecode}"
        );
        for (lane, c) in [(0, "x"), (1, "y"), (2, "z")] {
            assert_eq!(
                reaching(&program, &flow, lane),
                named(&[
                    ("TEXCOORD0.x", Via::Coordinate),
                    ("TEXCOORD0.y", Via::Coordinate),
                    (&format!("base_map.{c}"), Via::Value),
                ]),
                "{bytecode} lane {lane}"
            );
        }
        assert!(flow.discard().is_empty(), "{bytecode}");
    }
}

#[test]
fn a_colour_that_only_says_where_the_texel_is_reaches_it_as_a_coordinate() {
    for (bytecode, program) in lifted("coordinate") {
        let flow = Flow::of_program(&program).unwrap();
        for (lane, c) in [(0, "x"), (1, "y"), (2, "z"), (3, "w")] {
            assert_eq!(
                reaching(&program, &flow, lane),
                named(&[
                    ("COLOR0.x", Via::Coordinate),
                    ("COLOR0.y", Via::Coordinate),
                    (&format!("base_map.{c}"), Via::Value),
                ]),
                "{bytecode} lane {lane}"
            );
        }
    }
}

#[test]
fn a_colour_that_decides_a_discard_and_a_branch_reaches_them_through_control() {
    for (bytecode, program) in lifted("control") {
        let flow = Flow::of_program(&program).unwrap();
        assert_eq!(
            discarding(&program, &flow),
            named(&[("COLOR0.w", Via::Control)]),
            "{bytecode}"
        );
        for (lane, c) in [(0, "x"), (1, "y"), (2, "z"), (3, "w")] {
            assert_eq!(
                reaching(&program, &flow, lane),
                named(&[
                    ("COLOR0.x", Via::Control),
                    ("TEXCOORD0.x", Via::Coordinate),
                    ("TEXCOORD0.y", Via::Coordinate),
                    (&format!("base_map.{c}"), Via::Value),
                    (&format!("detail_map.{c}"), Via::Value),
                ]),
                "{bytecode} lane {lane}"
            );
        }
    }
}

#[test]
fn a_colour_never_read_reaches_nothing() {
    for (bytecode, program) in lifted("never") {
        let flow = Flow::of_program(&program).unwrap();
        for lane in 0..4 {
            assert!(
                reaching(&program, &flow, lane)
                    .keys()
                    .all(|k| !k.starts_with("COLOR")),
                "{bytecode} lane {lane}"
            );
        }
        assert!(flow.discard().is_empty(), "{bytecode}");
    }
}

/// The texels a node is made of, by name.
fn texels_below(program: &Program, values: &Values, root: NodeId) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut out = BTreeSet::new();
    let mut work = vec![root];
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Node::Texel {
            resource: Some(r),
            lane,
            ..
        } = values.node(id)
        {
            out.insert(texel_name(&program.interface, *r, *lane));
        }
        work.extend(values.operands(id));
    }
    out
}

#[test]
fn a_blend_times_a_tint_is_a_product_whose_factors_are_the_blend_and_the_tint() {
    for (bytecode, program) in lifted("blend") {
        let values = Values::of_program(&program).unwrap();
        let red = values
            .output_labelled(&program.interface, &target(), 0)
            .expect("the target's red");
        let Node::Op {
            op: Op::Binary(naga::BinaryOperator::Multiply),
            operands,
        } = values.node(red)
        else {
            panic!("{bytecode}: the red is {:?}, not a product", values.node(red));
        };
        let factors: Vec<BTreeSet<String>> = operands
            .iter()
            .map(|&o| texels_below(&program, &values, o))
            .collect();
        let tint = operands
            .iter()
            .position(|&o| {
                matches!(
                    values.node(o),
                    Node::Texel {
                        read: Read::Sample,
                        lane: 0,
                        ..
                    }
                ) && texels_below(&program, &values, o) == BTreeSet::from(["tint_map.x".to_owned()])
            })
            .unwrap_or_else(|| panic!("{bytecode}: no factor is the tint's red: {factors:?}"));
        assert_eq!(
            factors[1 - tint],
            BTreeSet::from([
                "base_map.x".to_owned(),
                "detail_map.x".to_owned(),
                "mask_map.w".to_owned()
            ]),
            "{bytecode}"
        );
        // the tint is read on the second pair, the base and the mask on the first, the detail on the first scaled
        let flow = Flow::of_values(&values);
        assert_eq!(
            reaching(&program, &flow, 0),
            named(&[
                ("TEXCOORD0.x", Via::Coordinate),
                ("TEXCOORD0.y", Via::Coordinate),
                ("TEXCOORD0.z", Via::Coordinate),
                ("TEXCOORD0.w", Via::Coordinate),
                ("base_map.x", Via::Value),
                ("detail_map.x", Via::Value),
                ("mask_map.w", Via::Value),
                ("tint_map.x", Via::Value),
            ]),
            "{bytecode}"
        );
    }
}

#[test]
fn a_loop_and_a_switch_the_colour_decides_reach_the_target_through_control() {
    for (bytecode, program) in lifted("carried") {
        let flow = Flow::of_program(&program).unwrap();
        for (lane, c) in [(0, "x"), (1, "y"), (2, "z"), (3, "w")] {
            assert_eq!(
                reaching(&program, &flow, lane),
                named(&[
                    ("COLOR0.y", Via::Control),
                    ("COLOR0.z", Via::Control),
                    ("TEXCOORD0.x", Via::Coordinate),
                    ("TEXCOORD0.y", Via::Coordinate),
                    (&format!("base_map.{c}"), Via::Value),
                    (&format!("detail_map.{c}"), Via::Value),
                ]),
                "{bytecode} lane {lane}"
            );
        }
    }
}

#[test]
fn both_bytecodes_say_the_same_of_every_fixture() {
    for (entry, ..) in FIXTURES {
        let flows: Vec<BTreeMap<u8, BTreeMap<String, Via>>> = lifted(entry)
            .into_iter()
            .map(|(_, program)| {
                let flow = Flow::of_program(&program).unwrap();
                (0..4)
                    .map(|lane| (lane, reaching(&program, &flow, lane)))
                    .collect()
            })
            .collect();
        assert_eq!(flows[0], flows[1], "{entry}");
    }
}

#[test]
fn a_call_is_refused_not_dropped() {
    let (_, mut program) = lifted("value").remove(0);
    let helper = program.module.functions.append(
        naga::Function {
            name: Some("helper".into()),
            ..Default::default()
        },
        naga::Span::UNDEFINED,
    );
    let body = &mut program.module.entry_points[0].function.body;
    let mut called = naga::Block::new();
    called.push(
        naga::Statement::Call {
            function: helper,
            arguments: Vec::new(),
            result: None,
        },
        naga::Span::UNDEFINED,
    );
    called.append(body);
    *body = called;
    let err = Flow::of_program(&program).unwrap_err();
    assert!(err.contains("call to helper"), "{err}");
}

/// An atomic on thread-group memory is followed: dxc's compute program adds its group index to `counter` with
/// `InterlockedAdd(counter, index)` and later writes `tile[63 - index] + counter` to a buffer, so what that write is made
/// of holds the add of the counter's earlier value and the index.
#[test]
fn an_atomic_on_thread_group_memory_is_followed() {
    let program =
        iridia_dxil::lift(include_bytes!("fixtures/compute_basic.dxil")).expect("dxc's program lifts");
    let values = Values::of_program(&program).expect("the value graph");
    let index = program
        .interface
        .inputs
        .iter()
        .position(|v| v.system == Some(iridia::SystemValue::GroupIndex))
        .expect("SV_GroupIndex");
    let reached = values.reached_from(values.writes().iter().flatten().copied());
    let added = reached.iter().any(|&id| match values.node(id) {
        Node::Op {
            op: Op::Binary(iridia::flow::BinaryOperator::Add),
            operands,
        } => {
            operands
                .iter()
                .any(|&o| matches!(values.node(o), Node::Merge { .. }))
                && operands
                    .iter()
                    .any(|&o| matches!(values.node(o), Node::Input { input, .. } if *input == index))
        }
        _ => false,
    });
    assert!(
        added,
        "no add of the counter and the index in what the writes are made of"
    );
}

/// A texture picked from an array by an index the program computes (`resource_arrays.hlsl`, dxc's `-T ps_6_0` and
/// `-T ps_6_6`): the value graph builds (the element of an array of resources is a handle, which is read where the
/// resource is named, not a value), and each read of the array is a texel of the array's resource with the index among
/// its controls.
#[test]
fn a_texture_picked_from_an_array_by_a_run_time_index_is_a_texel_of_the_array_with_the_index_as_a_control() {
    for (name, bytes) in [
        ("ps_6_0", &include_bytes!("fixtures/resource_arrays.dxil")[..]),
        ("ps_6_6", &include_bytes!("fixtures/resource_arrays.66.dxil")[..]),
    ] {
        let program = iridia_dxil::lift(bytes).expect("dxc's program lifts");
        let values = Values::of_program(&program).unwrap_or_else(|e| panic!("{name}: {e}"));
        let tex = program
            .interface
            .resources
            .iter()
            .find(|r| r.count == 4 && matches!(r.kind, iridia::ResourceKind::Texture { .. }))
            .unwrap_or_else(|| panic!("{name}: the array of four textures"));
        // each read of the array: how, and whether its first control (the element) is a number or something computed
        let reads: Vec<(Read, bool)> = values
            .reached_from(values.roots())
            .into_iter()
            .filter_map(|id| match values.node(id) {
                Node::Texel {
                    resource,
                    read,
                    controls,
                    ..
                } if *resource == Some(tex.id) => {
                    let element = controls.first().map(|&c| values.node(c));
                    Some((*read, element.is_some_and(|n| !matches!(n, Node::Literal(_)))))
                }
                _ => None,
            })
            .collect();
        // `tex[which.x].SampleLevel(..)` and `tex[which.x + 1].Load(..)` name their element by a computed value,
        // `tex[2].SampleLevel(..)` by a number
        assert!(reads.contains(&(Read::Sample, true)), "{name}: {reads:?}");
        assert!(reads.contains(&(Read::Sample, false)), "{name}: {reads:?}");
        assert!(reads.contains(&(Read::Load, true)), "{name}: {reads:?}");
    }
}
