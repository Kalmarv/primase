//! The pair boundary uses the existing pass and leaves both source inputs immutable.
use primase_animation::{Fit, Placement, Reference, RetargetClip, RetargetSkin, Translation, UnmappedJoints,
    joint_table::JointTable, pair};
use primase_core::pass::{Pass, PassLog};
use primase_core::{Channel, Clip, ClipTarget, Extrapolation, Interpolation, Joint, JointId,
    Binding, BindingId, Deformation, Influences, Mesh, Primitive, Property, Rest, RootMotion, RootSpace,
    Semantic, Skeleton, Skin, Slot, Stream, Topology, Transform, WeightRule, World};

fn skeleton(root: &str, child: &str, height: f32) -> Skeleton {
    Skeleton { joints: vec![
        Joint { id: JointId(0), name: root.into(), parent: None, rest: Rest::Trs {
            translation: [0.0, height, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] } },
        Joint { id: JointId(1), name: child.into(), parent: Some(JointId(0)), rest: Rest::Trs {
            translation: [0.0, height, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] } },
    ] }
}

#[test]
fn explicit_pair_moves_both_directions_and_carries_separate_root_motion() -> Result<(), Box<dyn std::error::Error>> {
    let a = skeleton("Root", "Head", 1.0);
    let b = skeleton("Hips", "Top", 2.0);
    let table = JointTable::parse(r#"{"version":1,"from":"a","to":"b","pairs":[["Root","Hips"],["Head","Top"]]}"#)?;
    let mut identities = World::new();
    let source = identities.spawn(); let target = identities.spawn();
    let clip = Clip { name: "walk".into(), duration: 1.0, extrapolation: Extrapolation::Loop,
        channels: vec![Channel { target: ClipTarget::Joint { skeleton: source, joint: JointId(0) },
            property: Property::Translation, interpolation: Interpolation::Linear, times: vec![0.0, 1.0],
            values: vec![0.0, 1.0, 0.0, 1.0, 1.0, 0.0] }],
        root_motion: Some(RootMotion { space: RootSpace::Rig,
            translation: Some((Interpolation::Linear, vec![0.0, 1.0], vec![0.0, 0.0, 0.0, 0.0, 0.0, 3.0])), rotation: None }),
        additive: None };
    let original = clip.clone();
    let policy = RetargetClip { reference: Reference::Rest, translation: Translation::Root };
    let (forward, log) = pair::retarget_one(&clip, &a, &b, target, &table, policy)?;
    assert_eq!(clip, original);
    assert_eq!(forward.root_motion, original.root_motion);
    assert_eq!(forward.extrapolation, original.extrapolation);
    assert!(!log.approximations.is_empty());
    assert!(forward.channels.iter().all(|c| matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton == target)));
    let movement = forward.channels.iter().find(|c| c.property == Property::Translation).ok_or("root translation")?;
    assert_eq!(movement.values, [0.0, 2.0, 0.0, 2.0, 2.0, 0.0]);
    let (back, _) = pair::retarget_one(&forward, &b, &a, source, &pair::reversed(&table)?, policy)?;
    let movement = back.channels.iter().find(|c| c.property == Property::Translation).ok_or("returned root translation")?;
    assert_eq!(movement.values, original.channels[0].values);
    assert_eq!(back.root_motion, original.root_motion);
    assert_eq!(back.extrapolation, original.extrapolation);
    Ok(())
}

#[test]
fn missing_pairs_stay_in_the_report_and_never_become_automatic_matches() -> Result<(), Box<dyn std::error::Error>> {
    let table = JointTable::parse(r#"{"version":1,"from":"a","to":"b","pairs":[["Root","Hips"],["Head","Absent"]]}"#)?;
    let (selected, report) = pair::available_table(&skeleton("Root", "Head", 1.0), &skeleton("Hips", "Head", 2.0), &table)?;
    assert_eq!(selected.pairs, [("Root".into(), "Hips".into())]);
    assert_eq!(report.pairs.iter().filter(|p| p.refusal.is_some()).count(), 1);
    assert_eq!(report.unmapped_source, ["Head"]);
    assert_eq!(report.unmapped_target, ["Head"]);
    let stored = JointTable::parse(pair::SEKIRO_SKATE_TABLE)?;
    assert_eq!(pair::reversed(&pair::reversed(&stored)?)?, stored);
    Ok(())
}

/// Hand-worked model: source joints at y=1 and y=2, target at y=2 and y=4. Three weighted points must move
/// to y=2,3,4. This exercises the same report/run path as the full installed-character census, without game files.
#[test]
fn self_contained_character_census_measures_ratios_fit_and_rebound_vertices() -> Result<(), Box<dyn std::error::Error>> {
    let a = skeleton("Root", "Head", 1.0);
    let b = skeleton("Hips", "Top", 2.0);
    let table = JointTable::parse(r#"{"version":1,"from":"short","to":"tall","pairs":[["Root","Hips"],["Head","Top"]]}"#)?;
    let observation = pair::inspect(&a, &b, &table)?;
    assert!(observation.pairs.iter().all(|p| p.refusal.is_none() && p.parents_agree == Some(true)));
    assert!(observation.unmapped_source.is_empty()); assert!(observation.unmapped_target.is_empty());
    let head = observation.pairs.iter().find(|p| p.source == "Head").ok_or("head observation")?;
    assert_eq!(head.length_ratio, Some(2.0));
    assert_eq!(head.local_rest_max_delta, Some(1.0));

    let mut world = World::new();
    let source = world.spawn(); world.insert(source, a.clone());
    let target = world.spawn(); world.insert(target, b.clone());
    let map = world.spawn(); world.insert(map, primase_core::JointMap { from: source, to: target, pairs: table.resolve(&a, &b)? });
    let prototype = world.spawn();
    world.insert(prototype, Mesh::new(vec![Primitive { topology: Topology::Points,
        streams: [
            (Semantic::Position, Stream::F32x3(vec![[0.0, 1.0, 0.0], [0.0, 1.5, 0.0], [0.0, 2.0, 0.0]])),
            (Semantic::Joints(0), Stream::U8x4(vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]])),
            (Semantic::Weights(0), Stream::F32x4(vec![[1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]])),
        ].into(), indices: None, material: None }]));
    let palette = [1.0, 2.0].into_iter().enumerate().map(|(i, height)| {
        let mut inverse_bind = Transform::IDENTITY; inverse_bind.cols[3][1] = -height;
        Slot { joint: JointId(i as u32), inverse_bind }
    }).collect();
    world.insert(prototype, Skin { influences: vec![Some(Influences::Streams { sets: 1 })],
        weights: WeightRule::SumsToOne, bindings: vec![Binding { id: BindingId(0), skeleton: source,
            palettes: vec![Some(palette)], deformation: Deformation::Linear }] });
    assert!(world.verify_components().is_empty(), "{:?}", world.verify_components());
    let policy = RetargetSkin { fit: Fit::Positions, unmapped: UnmappedJoints::Nearest, placement: Placement::Automatic };
    let reports = policy.report(&world)?;
    assert_eq!(reports.len(), 1);
    let report = &reports[0].1;
    assert_eq!(report.joints.len(), 2);
    assert!(report.joints.iter().all(|j| j.residual <= 1e-9));
    let segment = report.joints.iter().flat_map(|j| &j.segments).next().ok_or("measured segment")?;
    assert_eq!((segment.source, segment.target, segment.fitted), (1.0, 2.0, 2.0));
    let vertices = report.groups.iter().map(|g| g.vertices).sum::<u64>();
    let displacement = report.groups.iter().map(|g| g.largest_displacement).fold(0.0, f64::max);
    assert_eq!(vertices, 3);
    assert_eq!(displacement, 2.0);
    let mut log = PassLog::default(); policy.run(&mut world, &mut log)?;
    let mesh = world.get::<Mesh>(prototype).ok_or("rebound mesh")?;
    let positions = mesh.primitives()[0].streams.get(&Semantic::Position).ok_or("positions")?;
    assert_eq!(positions, &Stream::F32x3(vec![[0.0, 2.0, 0.0], [0.0, 3.0, 0.0], [0.0, 4.0, 0.0]]));
    assert_eq!(world.get::<Skin>(prototype).ok_or("rebound skin")?.bindings[0].skeleton, target);
    assert!(world.verify_components().is_empty(), "{:?}", world.verify_components());
    println!("PAIR_FIXTURE_CENSUS={}", serde_json::json!({"fixture":"short-to-tall","pairs":observation,
        "max_joint_residual_m":report.joints.iter().map(|j| j.residual).fold(0.0, f64::max),
        "largest_vertex_displacement_m":displacement,"vertices":vertices,"approximations":log.approximations}));
    Ok(())
}
