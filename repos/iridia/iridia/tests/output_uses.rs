//! The uses a program's discard and its target's blend show for its unroled output lanes
//! ([`iridia::output_uses`]), on `fixtures/output_uses.hlsl`, each entry point compiled to both bytecodes and
//! lifted by both lifters: an alpha-tested program's tested alpha is Coverage where it writes it, a discard on a value
//! written nowhere binds nothing, and a target blended by the source alpha has its alpha lane as Opacity.
use iridia::output_uses::{OutputUses, bind_output_uses};
use iridia::{Label, Program};
use iridia_interface::{
    Blend, BlendEquation, BlendFactor, BlendOp, OutputBinding, OutputRole, RenderState, TargetState,
};

/// Each entry point of the fixture, as fxc and dxc compile it.
const FIXTURES: [(&str, &[u8], &[u8]); 3] = [
    (
        "tested",
        include_bytes!("fixtures/output_uses_tested.dxbc"),
        include_bytes!("fixtures/output_uses_tested.dxil"),
    ),
    (
        "unrelated",
        include_bytes!("fixtures/output_uses_unrelated.dxbc"),
        include_bytes!("fixtures/output_uses_unrelated.dxil"),
    ),
    (
        "opaque",
        include_bytes!("fixtures/output_uses_opaque.dxbc"),
        include_bytes!("fixtures/output_uses_opaque.dxil"),
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

fn target(n: u32) -> Label {
    Label {
        name: "SV_Target".into(),
        index: n,
    }
}

/// The fixture's targets as a world binds them: target 0's xyz, 1's x and 2's xyz with a role, the other lanes Unknown.
fn bindings() -> Vec<OutputBinding> {
    let unknown = |n: u32, lanes: u8| OutputBinding {
        label: target(n),
        lanes,
        role: OutputRole::unknown(format!("target {n} lanes {lanes:#06b}: no role")),
    };
    let known = |n: u32, lanes: u8, role: OutputRole| OutputBinding {
        label: target(n),
        lanes,
        role,
    };
    vec![
        known(0, 0b0111, OutputRole::Fresnel),
        unknown(0, 0b1000),
        known(1, 0b0001, OutputRole::Roughness),
        unknown(1, 0b1110),
        known(2, 0b0111, OutputRole::Albedo),
        unknown(2, 0b1000),
    ]
}

/// The bindings, each lane's role by target and lane mask, an Unknown as `None`.
fn roles(outputs: &[OutputBinding]) -> Vec<(u32, u8, Option<OutputRole>)> {
    let mut out: Vec<_> = outputs
        .iter()
        .map(|o| {
            let role = match &o.role {
                OutputRole::Unknown { why } => {
                    assert!(why.starts_with(&format!("target {}", o.label.index)), "{why}");
                    None
                }
                role => Some(role.clone()),
            };
            (o.label.index, o.lanes, role)
        })
        .collect();
    out.sort_by_key(|(n, lanes, _)| (*n, *lanes));
    out
}

/// The roles `bindings` take on `program` under `state`, with the counts.
fn bound(program: &Program, state: &RenderState) -> (Vec<(u32, u8, Option<OutputRole>)>, OutputUses) {
    let graph = program.graph().expect("the value graph");
    let mut outputs = bindings();
    let uses = bind_output_uses(&mut outputs, &program.interface, Some(&graph), state);
    (roles(&outputs), uses)
}

/// Target 2 blended by `source` over `destination`, colour and alpha alike.
fn blended(source: BlendFactor, destination: BlendFactor) -> RenderState {
    let equation = BlendEquation {
        source,
        destination,
        op: BlendOp::Add,
    };
    let mut state = RenderState::default();
    state.targets[2] = Some(TargetState {
        blend: Some(Blend {
            colour: equation,
            alpha: equation,
        }),
        write_mask: 0xf,
    });
    state
}

fn unbound() -> Vec<(u32, u8, Option<OutputRole>)> {
    roles(&bindings())
}

#[test]
fn an_alpha_tested_programs_tested_alpha_is_coverage_where_it_writes_it() {
    for (bytecode, program) in lifted("tested") {
        let (outputs, uses) = bound(&program, &RenderState::default());
        let mut expected = unbound();
        expected.retain(|(n, lanes, _)| (*n, *lanes) != (2, 0b1000));
        expected.push((2, 0b1000, Some(OutputRole::Coverage)));
        expected.sort_by_key(|(n, lanes, _)| (*n, *lanes));
        // the lanes it writes as the literal 0 are no evidence, nor the lanes made of what it does not test
        assert_eq!(outputs, expected, "{bytecode}");
        assert_eq!(
            uses,
            OutputUses {
                coverage: 1,
                opacity: 0
            },
            "{bytecode}"
        );
    }
}

#[test]
fn a_discard_on_a_value_written_nowhere_binds_nothing() {
    for (bytecode, program) in lifted("unrelated") {
        assert!(
            !program.graph().unwrap().values.discards().is_empty(),
            "{bytecode}"
        );
        let (outputs, uses) = bound(&program, &RenderState::default());
        assert_eq!(outputs, unbound(), "{bytecode}");
        assert_eq!(uses, OutputUses::default(), "{bytecode}");
    }
}

#[test]
fn an_opaque_target_binds_nothing_and_one_blended_by_the_source_alpha_binds_opacity() {
    for (bytecode, program) in lifted("opaque") {
        assert!(
            program.graph().unwrap().values.discards().is_empty(),
            "{bytecode}"
        );
        let (outputs, uses) = bound(&program, &RenderState::default());
        assert_eq!(outputs, unbound(), "{bytecode}");
        assert_eq!(uses, OutputUses::default(), "{bytecode}");
        // a blend that does not weigh by the source alpha is no evidence
        let (outputs, _) = bound(&program, &blended(BlendFactor::One, BlendFactor::One));
        assert_eq!(outputs, unbound(), "{bytecode}");
        for (source, destination) in [
            (BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha),
            (BlendFactor::One, BlendFactor::OneMinusSrcAlpha),
        ] {
            let (outputs, uses) = bound(&program, &blended(source, destination));
            let mut expected = unbound();
            expected.retain(|(n, lanes, _)| (*n, *lanes) != (2, 0b1000));
            expected.push((2, 0b1000, Some(OutputRole::Opacity)));
            expected.sort_by_key(|(n, lanes, _)| (*n, *lanes));
            assert_eq!(outputs, expected, "{bytecode} {source:?} {destination:?}");
            assert_eq!(
                uses,
                OutputUses {
                    coverage: 0,
                    opacity: 1
                },
                "{bytecode}"
            );
        }
    }
}

/// A lane both tested and blended by the source alpha is Opacity: the blend takes it as a fraction.
#[test]
fn a_tested_alpha_the_target_blends_by_is_opacity() {
    for (bytecode, program) in lifted("tested") {
        let state = blended(BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha);
        let (outputs, uses) = bound(&program, &state);
        assert!(
            outputs.contains(&(2, 0b1000, Some(OutputRole::Opacity))),
            "{bytecode}: {outputs:?}"
        );
        assert_eq!(
            uses,
            OutputUses {
                coverage: 0,
                opacity: 1
            },
            "{bytecode}"
        );
    }
}

/// An Unknown binding of several lanes keeps its why on the lanes not bound; with no value graph there is no Coverage.
#[test]
fn an_unknown_binding_is_split_around_the_lanes_bound() {
    for (bytecode, program) in lifted("tested") {
        let graph = program.graph().unwrap();
        let mut outputs = vec![OutputBinding {
            label: target(2),
            lanes: 0b1111,
            role: OutputRole::unknown("target 2: no role"),
        }];
        bind_output_uses(
            &mut outputs,
            &program.interface,
            Some(&graph),
            &RenderState::default(),
        );
        assert_eq!(
            roles(&outputs),
            [(2, 0b0111, None), (2, 0b1000, Some(OutputRole::Coverage))],
            "{bytecode}"
        );
        let mut outputs = bindings();
        let uses = bind_output_uses(&mut outputs, &program.interface, None, &RenderState::default());
        assert_eq!(roles(&outputs), unbound(), "{bytecode}");
        assert_eq!(uses, OutputUses::default(), "{bytecode}");
    }
}
