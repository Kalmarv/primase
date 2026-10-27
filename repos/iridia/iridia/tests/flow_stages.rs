//! The value graph ([`iridia::flow::Values`]) of the programs naga has no entry point for, walked through their
//! stage functions, on the tessellated triangle of the lifters' fixtures (`tessellation.hlsl`, each stage compiled to
//! both bytecodes): the vertex program feeding the tessellator, the hull program's control-point phase and its
//! patch-constant phase, and the domain program. What each output and patch constant is made of is followed to the
//! inputs, the constants and the values the pipeline supplies.
use iridia::flow::{At, Node, NodeId, Supplied, Values};
use iridia::{Label, Program, ResourceKind, StageFacts, WordsRead};

/// Each stage of the fixture, as fxc and dxc compile it.
const FIXTURES: [(&str, &[u8], &[u8]); 3] = [
    (
        "vs",
        include_bytes!("fixtures/tessellation.vs.dxbc"),
        include_bytes!("fixtures/tessellation.vs.dxil"),
    ),
    (
        "hs",
        include_bytes!("fixtures/tessellation.hs.dxbc"),
        include_bytes!("fixtures/tessellation.hs.dxil"),
    ),
    (
        "ds",
        include_bytes!("fixtures/tessellation.ds.dxbc"),
        include_bytes!("fixtures/tessellation.ds.dxil"),
    ),
];

/// The stage `stage` lifted from each bytecode, with its value graph.
fn walked(stage: &str) -> Vec<(&'static str, Program, Values)> {
    let (_, dxbc, dxil) = FIXTURES.iter().find(|f| f.0 == stage).expect("a fixture");
    [
        ("dxbc", iridia_dxbc::lift(dxbc).expect("fxc's program lifts")),
        ("dxil", iridia_dxil::lift(dxil).expect("dxc's program lifts")),
    ]
    .into_iter()
    .map(|(bytecode, program)| {
        assert!(
            program.module.entry_points.is_empty(),
            "{bytecode} {stage}: a function, not an entry point"
        );
        let values = Values::of_program(&program).unwrap_or_else(|e| panic!("{bytecode} {stage}: {e}"));
        (bytecode, program, values)
    })
    .collect()
}

fn label(name: &str) -> Label {
    Label {
        name: name.into(),
        index: 0,
    }
}

/// Whether what `from` is made of holds a node `is` says.
fn reaches(values: &Values, from: NodeId, is: impl Fn(&Node) -> bool) -> bool {
    values
        .reached_from([from])
        .into_iter()
        .any(|id| is(values.node(id)))
}

/// The input labelled `name`'s lane `lane`.
fn input(program: &Program, name: &str, lane: u8) -> impl Fn(&Node) -> bool {
    let index = program
        .interface
        .inputs
        .iter()
        .position(|v| v.label == label(name))
        .expect("the input");
    move |n| matches!(n, Node::Input { input, lane: l } if *input == index && *l == lane)
}

fn supplied(value: Supplied) -> impl Fn(&Node) -> bool {
    move |n| matches!(n, Node::Supplied { value: v, .. } if *v == value)
}

/// Word `word` of the program's one constant buffer (`View`: the view-projection's 16 words, then `scale`).
fn constant(program: &Program, word: u32) -> impl Fn(&Node) -> bool {
    let buffer = program
        .interface
        .resources
        .iter()
        .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
        .expect("the constant buffer")
        .id;
    move |n| matches!(n, Node::Global { resource: Some(r), at: At::Component(w) } if *r == buffer && *w == word)
}

/// The vertex program feeding the tessellator is walked as its `main`: the control point's position is the input's
/// times `scale`, which is the one word it reads.
#[test]
fn a_vertex_program_feeding_the_tessellator_is_walked_as_its_function() {
    for (bytecode, program, values) in walked("vs") {
        let x = values
            .output_labelled(&program.interface, &label("CTRL_POSITION"), 0)
            .expect("the position's x");
        assert!(reaches(&values, x, input(&program, "POSITION", 0)), "{bytecode}");
        assert!(reaches(&values, x, constant(&program, 16)), "{bytecode}: scale");
        let reads = WordsRead::of_values(&values, &program.interface);
        let buffer = program
            .interface
            .resources
            .iter()
            .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }));
        assert_eq!(
            reads.words(buffer.unwrap().id).collect::<Vec<_>>(),
            [16],
            "{bytecode}"
        );
    }
}

/// A hull program is its control-point phase (the outputs: the point's position moved by its id) and its
/// patch-constant phase (the patch constants: an edge factor of the patch's id, the centre of the input points).
#[test]
fn a_hull_programs_phases_give_its_outputs_and_patch_constants() {
    for (bytecode, program, values) in walked("hs") {
        let z = values
            .output_labelled(&program.interface, &label("CTRL_POSITION"), 2)
            .expect("the position's z");
        assert!(
            reaches(&values, z, supplied(Supplied::OutputControlPoint)),
            "{bytecode}"
        );
        assert!(
            reaches(&values, z, input(&program, "CTRL_POSITION", 2)),
            "{bytecode}"
        );
        let StageFacts::Hull(hull) = &program.interface.facts else {
            panic!("{bytecode}: no hull facts");
        };
        let patch = |name: &str, index: u32, lane: u8| {
            let at = hull
                .patch_constants
                .iter()
                .position(|v| {
                    v.label
                        == Label {
                            name: name.into(),
                            index,
                        }
                })
                .expect("the patch constant");
            *values.patch_constants().get(&(at, lane)).expect("its value")
        };
        assert!(
            reaches(
                &values,
                patch("SV_TessFactor", 2, 0),
                supplied(Supplied::Primitive)
            ),
            "{bytecode}"
        );
        assert!(
            reaches(
                &values,
                patch("CENTRE", 0, 0),
                input(&program, "CTRL_POSITION", 0)
            ),
            "{bytecode}"
        );
        assert!(
            !reaches(&values, patch("CENTRE", 0, 0), supplied(Supplied::Primitive)),
            "{bytecode}"
        );
        assert_eq!(
            values.patch_constants().len(),
            7,
            "{bytecode}: three edges, the centre's xyz, the inside"
        );
    }
}

/// A domain program places its vertex from the domain location, the patch constants and the control points, through
/// the view-projection: every one of its 16 words read.
#[test]
fn a_domain_programs_position_is_made_of_the_location_the_patch_and_the_points() {
    for (bytecode, program, values) in walked("ds") {
        let StageFacts::Domain(domain) = &program.interface.facts else {
            panic!("{bytecode}: no domain facts");
        };
        let centre = domain
            .patch_constants
            .iter()
            .position(|v| v.label == label("CENTRE"))
            .expect("the centre");
        let x = values
            .output_labelled(&program.interface, &label("SV_Position"), 0)
            .expect("the position's x");
        assert!(
            reaches(&values, x, supplied(Supplied::DomainLocation)),
            "{bytecode}"
        );
        assert!(
            reaches(&values, x, supplied(Supplied::PatchConstant(centre))),
            "{bytecode}"
        );
        assert!(
            reaches(&values, x, input(&program, "CTRL_POSITION", 0)),
            "{bytecode}"
        );
        let reads = WordsRead::of_values(&values, &program.interface);
        let buffer = program
            .interface
            .resources
            .iter()
            .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }));
        assert_eq!(
            reads.words(buffer.unwrap().id).collect::<Vec<_>>(),
            (0..16).collect::<Vec<_>>(),
            "{bytecode}"
        );
    }
}
