use morpheme_formats::{Joint, Rig};
use primase_core::{
    Transform, World,
    frame::{Axis, SourceFrame},
};
use primase_shred::{
    components::ReferenceRig,
    read::reference_rig::{Report, append_bundle},
};
use primase_testdata::gate::{GateReport, Tolerance};

fn source(frame: SourceFrame) -> ReferenceRig {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    ReferenceRig {
        pak: "fixture".into(),
        entry: b"anim\\fixture\\fixture".to_vec(),
        record_offset: 0,
        asset_id: 91,
        guid: [4; 16],
        header_word: 0,
        frame,
        source_payload: Vec::new(),
        rig: Rig {
            header_rotation: [0., 0., 0., 1.],
            header_vector_bits: [7f32.to_bits(), 8f32.to_bits(), 9f32.to_bits(), 0],
            joints: vec![
                Joint {
                    id: 0,
                    name: b"same".to_vec(),
                    parent: None,
                    rotation: [0., 0., s, s],
                    translation: [3., 4., 5.],
                    position_w_bits: 0x7fc12345,
                },
                Joint {
                    id: 1,
                    name: b"same".to_vec(),
                    parent: Some(0),
                    rotation: [s, 0., 0., s],
                    translation: [2., 0., 0.],
                    position_w_bits: 0xdeadbeef,
                },
            ],
        },
    }
}
fn close(a: Transform, b: Transform) {
    let mut gate = GateReport::new("hand-worked reference matrix", Tolerance::Abs(2e-6));
    let difference = a
        .cols
        .iter()
        .flatten()
        .zip(b.cols.iter().flatten())
        .map(|(a, b)| f64::from((a - b).abs()))
        .fold(0., f64::max);
    gate.record("literal Rz * Rx with translated child", difference);
    eprintln!("{gate}");
    gate.assert_passed();
}

#[test]
fn hand_worked_noncommuting_parent_and_frame() {
    // Root Rz(90), T(3,4,5); child Rx(90), T(2,0,0).
    // Parent * child sends the child origin to (3,6,5), x to +Y, y to +Z, z to +X.
    let expected = Transform {
        cols: [[0., 1., 0.], [0., 0., 1.], [1., 0., 0.], [3., 6., 5.]],
    };
    let source = source(SourceFrame::WORLD);
    close(source.source_matrices().expect("references")[1], expected);
    let reflected = source_with_frame(SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 0.01));
    close(
        reflected.matrices().expect("converted")[1],
        Transform {
            cols: [[0., 1., 0.], [0., 0., -1.], [-1., 0., 0.], [0.03, 0.06, -0.05]],
        },
    );
    // The separate header vector is retained without applying it as another root-joint transform.
    assert_eq!(
        source.rig.header_vector_bits,
        [7f32.to_bits(), 8f32.to_bits(), 9f32.to_bits(), 0]
    );
    let mut reordered = source.clone();
    reordered.rig.joints.swap(0, 1);
    reordered.rig.joints[0].id = 0;
    reordered.rig.joints[0].parent = Some(1);
    reordered.rig.joints[1].id = 1;
    reordered.rig.joints[1].parent = None;
    close(
        reordered.source_matrices().expect("parent after child")[0],
        expected,
    );
    reordered.rig.joints[1].parent = Some(0);
    assert!(reordered.source_matrices().expect_err("cycle").contains("cyclic"));
}
fn source_with_frame(frame: SourceFrame) -> ReferenceRig {
    source(frame)
}

#[test]
fn unsupported_siblings_reported_and_malformed_bundle_is_atomic() {
    let mut world = World::new();
    let mut report = Report::default();
    let mut bundle = vec![0; 32];
    bundle[..4].copy_from_slice(&99u32.to_be_bytes());
    bundle[4..8].copy_from_slice(&7u32.to_be_bytes());
    append_bundle(
        &mut world,
        &mut report,
        "fixture",
        b"anim",
        &bundle,
        SourceFrame::WORLD,
    )
    .expect("bounded unknown");
    assert_eq!((report.bundles, report.records, report.rigs), (1, 1, 0));
    assert_eq!(report.bundles_without_rigs, 1);
    assert_eq!(report.uninterpreted_assets[0].asset_type, 99);
    assert_eq!(report.uninterpreted_assets[0].asset_id, 7);
    let before = report.clone();
    bundle.extend_from_slice(&1u32.to_be_bytes());
    bundle.resize(64, 0);
    let error = append_bundle(
        &mut world,
        &mut report,
        "fixture",
        b"anim",
        &bundle,
        SourceFrame::WORLD,
    )
    .expect_err("empty rig");
    assert!(error.to_string().contains("0x40"));
    assert_eq!(report, before);
    assert!(world.column::<ReferenceRig>().is_none());
}
