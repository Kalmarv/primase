//! A program's WGSL for a pipeline ([`primase_shader::emit::wgsl_for`]): each lowering read back from the text by
//! naga's WGSL front end, so what is checked is what a pipeline is given.
use std::collections::BTreeMap;

use naga::{Binding, Interpolation, TypeInner};
use primase_shader::emit::{ClipVaryings, Pipeline, wgsl_for};
use primase_shader::{InterpMode, Interpolation as Interp, Program, ProgramError, ResourceKind, emit};

/// dxc's vertex program: SV_Position, TEXCOORD0, COLOR0 and one clip distance.
fn vertex() -> Program {
    primase_dxil::lift(include_bytes!("../../dxil/tests/fixtures/vertex.dxil")).expect("dxc's program lifts")
}

/// dxc's pixel program reading TEXCOORD0 and COLOR0.
fn pixel() -> Program {
    primase_dxil::lift(include_bytes!("fixtures/flow_value.dxil")).expect("dxc's program lifts")
}

fn parsed(text: &str) -> naga::Module {
    naga::front::wgsl::parse_str(text).unwrap_or_else(|e| panic!("the text parses: {e}\n{text}"))
}

/// The entry point's result members: name and binding.
fn results(module: &naga::Module) -> Vec<(String, Option<Binding>)> {
    let result = module.entry_points[0].function.result.as_ref().expect("a result");
    let TypeInner::Struct { members, .. } = &module.types[result.ty].inner else {
        panic!("a result structure")
    };
    members
        .iter()
        .map(|m| (m.name.clone().unwrap_or_default(), m.binding.clone()))
        .collect()
}

fn location(binding: &Option<Binding>) -> Option<(u32, Option<Interpolation>)> {
    match binding {
        Some(Binding::Location {
            location,
            interpolation,
            ..
        }) => Some((*location, *interpolation)),
        _ => None,
    }
}

/// The member `name`, as the writer spells it (an identifier ending in a digit gains an underscore).
fn member<'a>(members: &'a [(String, Option<Binding>)], name: &str) -> &'a Option<Binding> {
    &members
        .iter()
        .find(|(n, _)| n.trim_end_matches('_') == name)
        .unwrap_or_else(|| panic!("{name} in {members:?}"))
        .1
}

#[test]
fn the_default_pipeline_is_the_program_as_written() {
    for program in [vertex(), pixel()] {
        let written = wgsl_for(&program, &Pipeline::default()).expect("written");
        assert_eq!(written.text, emit::wgsl(&program).expect("written"));
        assert_eq!(written.clip, None);
        assert!(written.notes.is_empty());
    }
}

#[test]
fn outputs_take_the_locations_and_interpolation_their_consumer_reads_them_at() {
    // the consumer reads COLOR0 first (location 0) and TEXCOORD0 second, linear: the vertex program's outputs follow
    let mut consumer = pixel().interface;
    consumer.inputs.reverse();
    let uv = consumer
        .inputs
        .iter_mut()
        .find(|v| v.label.name.eq_ignore_ascii_case("TEXCOORD"))
        .expect("TEXCOORD0");
    uv.interp = Some(Interp {
        mode: InterpMode::Linear,
        ..uv.interp.unwrap_or(Interp {
            mode: InterpMode::Perspective,
            sampling: primase_shader::InterpSampling::Center,
        })
    });
    let program = vertex();
    let written = wgsl_for(
        &program,
        &Pipeline {
            feeds: Some(&consumer),
            ..Pipeline::default()
        },
    )
    .expect("written");
    let members = results(&parsed(&written.text));
    assert_eq!(
        location(member(&members, "COLOR0")),
        Some((0, Some(Interpolation::Perspective)))
    );
    assert_eq!(
        location(member(&members, "TEXCOORD0")),
        Some((1, Some(Interpolation::Linear)))
    );
    assert_eq!(written.next_location, 2);
}

#[test]
fn a_consumer_input_no_output_carries_is_refused_by_name() {
    let mut consumer = pixel().interface;
    let colour = consumer
        .inputs
        .iter_mut()
        .find(|v| v.label.name.eq_ignore_ascii_case("COLOR"))
        .expect("COLOR0");
    colour.label.index = 3;
    let error = wgsl_for(
        &vertex(),
        &Pipeline {
            feeds: Some(&consumer),
            ..Pipeline::default()
        },
    )
    .expect_err("COLOR3 is not written");
    assert!(
        matches!(&error, ProgramError::Pipeline(why) if why.contains("COLOR3") && why.contains("COLOR0")),
        "{error}"
    );
}

#[test]
fn clip_distances_travel_as_varyings_and_their_consumer_discards_below_zero() {
    let carrying = wgsl_for(
        &vertex(),
        &Pipeline {
            clip_varyings: true,
            ..Pipeline::default()
        },
    )
    .expect("written");
    let clips = ClipVaryings { first: 2, count: 1 };
    assert_eq!(carrying.clip, Some(clips));
    assert_eq!(carrying.next_location, 3);
    let module = parsed(&carrying.text);
    let members = results(&module);
    assert_eq!(
        location(member(&members, "clip_distance0")),
        Some((2, Some(Interpolation::Perspective)))
    );
    // no clip distances anywhere in the text: a pipeline without the feature refuses the built-in and its `enable`
    for refused in ["enable clip_distances", "@builtin(clip_distances)"] {
        assert!(!carrying.text.contains(refused), "{}", carrying.text);
    }
    assert!(module.types.iter().all(|(_, t)| {
        match &t.inner {
            TypeInner::Struct { members, .. } => members
                .iter()
                .all(|m| !matches!(m.binding, Some(Binding::BuiltIn(naga::BuiltIn::ClipDistances)))),
            _ => true,
        }
    }));

    let discarding = wgsl_for(
        &pixel(),
        &Pipeline {
            clip_discard: Some(clips),
            ..Pipeline::default()
        },
    )
    .expect("written");
    assert_eq!(discarding.next_location, 3);
    let module = parsed(&discarding.text);
    let arguments = &module.entry_points[0].function.arguments;
    let last = arguments.last().expect("the clip input");
    assert_eq!(
        location(&last.binding),
        Some((2, Some(Interpolation::Perspective)))
    );
    // a discard under a test of the input, the last statement before the one return but the return value's emits
    let body = &module.entry_points[0].function.body;
    let at = body
        .iter()
        .position(|s| matches!(s, naga::Statement::Return { .. }))
        .expect("a return");
    let before = body[..at]
        .iter()
        .rev()
        .find(|s| !matches!(s, naga::Statement::Emit(_)));
    assert!(
        matches!(before, Some(naga::Statement::If { accept, .. }) if matches!(accept.first(), Some(naga::Statement::Kill))),
        "{}",
        discarding.text
    );

    // the clip varyings may not take a location the program's own inputs use
    let error = wgsl_for(
        &pixel(),
        &Pipeline {
            clip_discard: Some(ClipVaryings { first: 1, count: 1 }),
            ..Pipeline::default()
        },
    )
    .expect_err("location 1 is COLOR0's");
    assert!(matches!(error, ProgramError::Pipeline(_)), "{error}");
}

#[test]
fn options_for_another_stage_are_refused() {
    let error = wgsl_for(
        &pixel(),
        &Pipeline {
            clip_varyings: true,
            ..Pipeline::default()
        },
    )
    .expect_err("a pixel program has no clip distances to carry");
    assert!(matches!(error, ProgramError::Pipeline(_)), "{error}");
    let error = wgsl_for(
        &vertex(),
        &Pipeline {
            clip_discard: Some(ClipVaryings { first: 4, count: 1 }),
            ..Pipeline::default()
        },
    )
    .expect_err("a vertex program does not discard");
    assert!(matches!(error, ProgramError::Pipeline(_)), "{error}");
}

#[test]
fn resources_are_bound_where_the_pipeline_says() {
    let program = pixel();
    let texture = program
        .interface
        .resources
        .iter()
        .find(|r| matches!(r.kind, ResourceKind::Texture { .. }))
        .expect("base_map")
        .id;
    let written = wgsl_for(
        &program,
        &Pipeline {
            bindings: BTreeMap::from([(texture, (3, 7))]),
            ..Pipeline::default()
        },
    )
    .expect("written");
    assert_eq!(written.bindings[&texture], (3, 7));
    let module = parsed(&written.text);
    assert!(
        module
            .global_variables
            .iter()
            .any(|(_, g)| g.binding == Some(naga::ResourceBinding { group: 3, binding: 7 }))
    );
    // every other resource where the program had it
    for (id, (group, binding)) in &written.bindings {
        if *id != texture {
            let g = program.global(*id).expect("its global");
            assert_eq!(
                program.module.global_variables[g].binding,
                Some(naga::ResourceBinding {
                    group: *group,
                    binding: *binding
                })
            );
        }
    }
    // two resources at one place
    let others: Vec<_> = written
        .bindings
        .iter()
        .filter(|(id, _)| **id != texture)
        .collect();
    let (_, &(group, binding)) = others[0];
    let error = wgsl_for(
        &program,
        &Pipeline {
            bindings: BTreeMap::from([(texture, (group, binding))]),
            ..Pipeline::default()
        },
    )
    .expect_err("two resources at one binding");
    assert!(matches!(error, ProgramError::Pipeline(_)), "{error}");
}

#[test]
fn a_sampler_heap_is_one_sampler() {
    // dxc's compute program samples with a sampler from the sampler heap at an index from a constant buffer
    let program = primase_dxil::lift(include_bytes!("../../dxil/tests/fixtures/heap.dxil"))
        .expect("dxc's program lifts");
    let as_is = emit::wgsl(&program).expect("written");
    assert!(as_is.contains("binding_array<sampler>"), "{as_is}");
    let written = wgsl_for(
        &program,
        &Pipeline {
            single_samplers: true,
            ..Pipeline::default()
        },
    )
    .expect("written");
    assert!(
        !written.text.contains("binding_array<sampler>"),
        "{}",
        written.text
    );
    assert_eq!(written.notes.len(), 1, "{:?}", written.notes);
    let module = parsed(&written.text);
    assert!(
        module
            .global_variables
            .iter()
            .any(|(_, g)| matches!(module.types[g.ty].inner, TypeInner::Sampler { .. }))
    );
}
