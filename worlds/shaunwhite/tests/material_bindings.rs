//! Hand-worked constant-bank packing, plus the REF proof's independent controls in albedo.rs.
#![allow(clippy::unwrap_used, reason = "hand-worked fixtures")]
use primase_core::{World, shader::*};
use primase_shaunwhite::{
    components::{LayerState, MaterialRef, MaterialState, SlotRef},
    shaders::{
        Key,
        albedo::{Derivation, DerivedAlbedo, Influence, NAME},
        bind_material,
    },
};
use std::collections::BTreeMap;

fn fixture(count: u32) -> (DerivedAlbedo, MaterialRef) {
    // mov oC0,c59; a bank through c59. Reflection is laid out here, without stored game bytes.
    let bytes = [0xffff0300u32, 0x02000001, 0x800f0800, 0xa0e4003b, 0xffff]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let mut program = iridia_d3d9::lift(&bytes).unwrap();
    let resource = &mut program.interface.resources[0];
    let ResourceKind::ConstantBuffer { layout } = &mut resource.kind else {
        panic!("bank")
    };
    let vector = ConstantType::Vector(Value {
        scalar: ScalarType::Float,
        width: 4,
    });
    layout.constants = vec![
        Constant {
            name: "g_FlagsUseTexAlpha".into(),
            offset: 16,
            ty: vector.clone(),
            default: None,
        },
        Constant {
            name: "g_Layer1_ModulationColor".into(),
            offset: 32,
            ty: vector.clone(),
            default: None,
        },
        Constant {
            name: "g_Layer3_ModulationColor".into(),
            offset: 48,
            ty: vector.clone(),
            default: None,
        },
        Constant {
            name: "g_VectorArray".into(),
            offset: 39 * 16,
            ty: ConstantType::Array {
                element: Box::new(vector),
                count,
                stride: 16,
            },
            default: None,
        },
    ];
    let resource = resource.id;
    let size = layout.size;
    let mut world = World::new();
    let entity = world.spawn();
    let binding = StageBinding {
        stage: Stage::Pixel,
        variant: None,
        shader: entity,
        inputs: vec![],
        constants: vec![ConstantBinding {
            resource,
            offset: 0,
            size,
            name: Some("source bank".into()),
            role: ConstantRole::Unknown {
                why: "frame and material not resolved".into(),
            },
            lines: None,
        }],
        textures: vec![],
        buffers: vec![],
        constant_buffers: vec![],
        outputs: vec![],
    };
    let derived = DerivedAlbedo {
        reencode: program.clone(),
        program,
        binding,
        derivation: Derivation {
            name: NAME,
            record: 0,
            key: Key {
                kind: 4,
                permutation: 3 | (2 << 20),
                dependencies: [0; 4],
            },
            pixel_key: 0,
            source_hash: iridia::source_hash(&bytes),
            projection: iridia_d3d9::Projection {
                after_word: 1,
                temporary: 0,
                lanes: [0, 1, 2],
            },
            influence: Influence::Restored,
            cuts: vec![],
            cut_inputs: vec![],
        },
    };
    let mut slots = [
        "diffuse",
        "diffuse_b",
        "diffuse_c",
        "mask",
        "normal",
        "specular",
        "specular_b",
    ]
    .into_iter()
    .map(|name| {
        (
            name.into(),
            SlotRef {
                texture: entity,
                scale: [1.; 2],
                offset: [0.; 2],
                rotation: 0.,
                texcoord: 0,
                address: None,
            },
        )
    })
    .collect::<BTreeMap<String, _>>();
    let b = slots.get_mut("diffuse_b").unwrap();
    b.scale = [2., 3.];
    b.offset = [0.25, -0.5];
    b.texcoord = 1;
    slots.get_mut("mask").unwrap().texcoord = 1;
    let material = MaterialRef {
        id: 1,
        name: "fixture".into(),
        state: Some(MaterialState {
            blend: 0,
            blended: false,
            alpha_test: false,
            alpha_ref: 128,
            water: false,
            two_sided: false,
            invert_culling: false,
            color: [0.8, 0.9, 0.7, 0.9],
            illumination: 0.75,
            specular_level: 1.,
            specular_power: 10.,
            normal_scale: 2.,
            layering: LayerState {
                colors: [[0.7, 0.6, 0.9, 0.5], [0.9, 0.8, 0.7, 0.6]],
                use_alpha: [true, true, false],
            },
        }),
        slots,
        unread: vec![],
    };
    (derived, material)
}
fn word(b: &StageBinding, row: u32, lane: u32) -> Option<f32> {
    let at = row * 16 + lane * 4;
    b.constants.iter().find_map(|c| {
        if !(c.offset..c.offset + c.size).contains(&at) {
            return None;
        }
        let ConstantRole::Material(words) = &c.role else {
            return None;
        };
        words
            .get(((at - c.offset) / 4) as usize)
            .copied()
            .map(f32::from_bits)
    })
}
fn row(b: &StageBinding, n: u32) -> [Option<f32>; 4] {
    std::array::from_fn(|lane| word(b, n, lane as u32))
}
#[test]
fn material_words_match_the_proof_and_native_zero_rotation_uv_packing() {
    let (derived, material) = fixture(21);
    let old = derived.binding.clone();
    let bound = bind_material(&derived, &material).unwrap();
    assert_eq!(derived.binding, old);
    assert_eq!(row(&bound, 1), [Some(0.), Some(1.), Some(0.), Some(0.)]);
    assert_eq!(row(&bound, 2), material.state.unwrap().color.map(Some));
    assert_eq!(row(&bound, 47), [0.7, 0.6, 0.9, 0.5].map(Some));
    assert_eq!(row(&bound, 48), [Some(2.), Some(0.), Some(0.25), None]);
    assert_eq!(row(&bound, 49), [Some(0.), Some(3.), Some(-0.5), None]);
    assert_eq!(row(&bound, 52), [Some(1.), Some(0.), Some(0.), None]);
    assert_eq!(row(&bound, 53), [Some(0.), Some(1.), Some(0.), None]);
    assert_eq!(row(&bound, 56), [Some(2.), Some(1.), Some(1.), Some(1.)]);
    assert_eq!(row(&bound, 58), [Some(0.75), None, None, None]);
    assert_eq!(row(&bound, 59), [Some(0.), Some(1.), Some(0.), Some(1.)]);
    assert_eq!(row(&bound, 27), [None; 4], "exposure is not a material default");
    assert_eq!(row(&bound, 29), [None; 4], "viewer is not a material default");
    assert!(
        bound
            .constants
            .windows(2)
            .all(|p| p[0].offset + p[0].size <= p[1].offset)
    );
    let mut changed = material.clone();
    changed.state.as_mut().unwrap().blended = true;
    assert_eq!(word(&bind_material(&derived, &changed).unwrap(), 1, 0), Some(1.));
    changed.state.as_mut().unwrap().blended = false;
    changed.state.as_mut().unwrap().alpha_test = true;
    assert_eq!(word(&bind_material(&derived, &changed).unwrap(), 1, 0), Some(1.));
}
#[test]
fn partial_allocations_and_unknown_transforms_never_become_identity_or_zero() {
    let (derived, mut material) = fixture(10);
    let partial = bind_material(&derived, &material).unwrap();
    assert_eq!(word(&partial, 48, 0), Some(2.));
    assert_eq!(word(&partial, 49, 0), None, "outside optimized allocation");
    assert_eq!(word(&partial, 59, 0), None, "outside optimized allocation");
    let (derived, _) = fixture(21);
    material.slots.remove("diffuse_b");
    material.slots.remove("mask");
    let bound = bind_material(&derived, &material).unwrap();
    assert_eq!(row(&bound, 48), [None; 4]);
    assert_eq!(row(&bound, 54), [None; 4]);
    assert_eq!(word(&bound, 59, 3), None);
    assert!(
        bound
            .constants
            .iter()
            .any(|c| matches!(&c.role,ConstantRole::Unknown{why} if why.contains("diffuse_b UV fields")))
    );
    assert!(
        bound
            .constants
            .iter()
            .any(|c| matches!(&c.role,ConstantRole::Unknown{why} if why.contains("mask UV fields")))
    );
    material.slots.insert(
        "diffuse_b".into(),
        fixture(21).1.slots.remove("diffuse_b").unwrap(),
    );
    material.slots.get_mut("diffuse_b").unwrap().texcoord = 2;
    assert!(bind_material(&derived, &material).unwrap_err().contains("UV set"));
    material.slots.get_mut("diffuse_b").unwrap().texcoord = 0;
    material.slots.get_mut("diffuse_b").unwrap().rotation = 0.;
    material.slots.get_mut("diffuse_b").unwrap().offset[0] = f32::NAN;
    assert!(
        bind_material(&derived, &material)
            .unwrap_err()
            .contains("non-finite")
    );
}

#[test]
fn degrees_rotate_about_origin_before_nonuniform_scale_and_offset() {
    let (derived, mut material) = fixture(21);
    for (angle, expected) in [
        (90., [0., -2., 0.25, 3., 0., -0.5]),
        (-90., [0., 2., 0.25, -3., 0., -0.5]),
        (450., [0., -2., 0.25, 3., 0., -0.5]),
    ] {
        material.slots.get_mut("diffuse_b").unwrap().rotation = angle;
        let bound = bind_material(&derived, &material).unwrap();
        let actual = [
            word(&bound, 48, 0),
            word(&bound, 48, 1),
            word(&bound, 48, 2),
            word(&bound, 49, 0),
            word(&bound, 49, 1),
            word(&bound, 49, 2),
        ];
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual.unwrap() - expected).abs() < 2e-6);
        }
        assert_eq!(word(&bound, 48, 3), None);
        assert_eq!(word(&bound, 49, 3), None);
    }
    material.slots.get_mut("diffuse_b").unwrap().rotation = f32::INFINITY;
    assert!(
        bind_material(&derived, &material)
            .unwrap_err()
            .contains("rotation is non-finite")
    );
}

#[test]
fn native_setter_control_words_match_including_noncardinal_and_multiple_turns() {
    use primase_testdata::{Game, OracleSet, oracle};
    let Some(path) = oracle(OracleSet::GameCode, Game::ShaunWhite, "uv-rotation.tsv") else {
        return;
    };
    let text = std::fs::read_to_string(path).unwrap();
    let mut gate = primase_core::gate::GateReport::new(
        "original native UV setter",
        primase_core::gate::Tolerance::Abs(5e-7),
    );
    let mut count = 0;
    for line in text.lines().filter(|line| !line.starts_with('#')) {
        let mut fields = line.split('\t');
        let name = fields.next().unwrap();
        let values = fields
            .map(|v| f32::from_bits(u32::from_str_radix(v, 16).unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 11);
        let (derived, mut material) = fixture(21);
        let slot = material.slots.get_mut("diffuse_b").unwrap();
        slot.scale = [values[0], values[1]];
        slot.offset = [values[2], values[3]];
        slot.rotation = values[4];
        let bound = bind_material(&derived, &material).unwrap();
        for (n, wanted) in values[5..].iter().enumerate() {
            let actual = word(&bound, 48 + n as u32 / 3, n as u32 % 3).unwrap();
            gate.record(format!("{name} word{n}"), f64::from((actual - wanted).abs()));
        }
        count += 1;
    }
    assert_eq!(count, 11);
    eprintln!("{gate:?}");
    gate.assert_passed();
}
