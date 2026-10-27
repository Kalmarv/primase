//! Hand-worked prefix and replacement-tail evidence; no source-specific matching.
#![allow(clippy::unwrap_used, reason = "hand-worked assertions")]
use iridia_d3d9::{Projection, project, reencode};
use iridia_exec::{PixelShader, ProgramShader, Sample};
fn bytes() -> Vec<u8> {
    // mov r0,c0; mul r0,r0,c1; add oC0,r0,c2
    [
        0xffff0300u32,
        0x02000001,
        0x800f0000,
        0xa0e40000,
        0x03000005,
        0x800f0000,
        0x80e40000,
        0xa0e40001,
        0x03000002,
        0x800f0800,
        0x80e40000,
        0xa0e40002,
        0xffff,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect()
}
#[test]
fn projection_keeps_original_hash_and_runs_original_tail_with_replaced_lanes() {
    let b = bytes();
    let cut = Projection {
        after_word: 4,
        temporary: 0,
        lanes: [2, 0, 1],
    };
    let prefix = project(&b, cut).unwrap();
    assert_eq!(
        prefix.provenance.source_hash,
        Some(iridia::source_hash(&b))
    );
    assert!(prefix.provenance.lifter.contains("projection:word=4"));
    let constants = [[2., 3., 4., 5.], [3.; 4], [1.; 4]].map(|v| v.map(f32::to_bits));
    let run = |p, inputs: &[[u32; 4]]| {
        ProgramShader::new(p)
            .unwrap()
            .execute(inputs, &[(0, &constants)], &mut |_: Sample| {
                Err("unexpected sample".into())
            })
            .unwrap()
            .outputs[0]
            .map(f32::from_bits)
    };
    assert_eq!(run(prefix, &[]), [12., 6., 9., 1.]);
    let mut inputs = vec![[0; 4]; 32];
    inputs[31] = [10., 20., 30., 0.].map(f32::to_bits);
    assert_eq!(run(reencode(&b, cut).unwrap(), &inputs), [21., 31., 11., 16.]);
    for bad in [
        Projection { after_word: 5, ..cut },
        Projection {
            lanes: [0, 0, 1],
            ..cut
        },
        Projection { temporary: 1, ..cut },
    ] {
        assert!(project(&b, bad).is_err());
    }
}
