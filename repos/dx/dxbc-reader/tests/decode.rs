#![allow(clippy::unwrap_used)]
//! The token-level decoder on hand-assembled streams (token layouts from Microsoft's tokenized program format): a
//! vertex program, typed declarations, an instruction's controls and extended tokens, and what it refuses by name.
use dxbc_reader::decode::{Declared, ProgramType, Register, Selection};

fn instruction(token: u32, operands: &[u32]) -> Vec<u32> {
    let mut v = vec![token | (((operands.len() + 1) as u32) << 24)];
    v.extend_from_slice(operands);
    v
}
/// An operand of register `kind` with a 4-lane mask and one index.
fn masked(kind: u32, index: u32, mask: u32) -> [u32; 2] {
    [2 | mask << 4 | kind << 12 | 1 << 20, index]
}
fn swizzled(kind: u32, index: u32) -> [u32; 2] {
    [2 | 1 << 2 | 0b11_10_01_00 << 4 | kind << 12 | 1 << 20, index]
}
fn stream(version: u32, body: &[Vec<u32>]) -> Vec<u32> {
    let mut v = vec![version, 0];
    for b in body {
        v.extend(b);
    }
    v[1] = v.len() as u32;
    v
}

/// vs_5_0: dcl_input v0.xyzw; dcl_output_siv o0.xyzw, position; dcl_temps 1; mov_sat r0.xyzw, v0.xyzw;
/// mov o0.xyzw, r0.xyzw; ret
fn vertex() -> Vec<u32> {
    let mut siv = masked(2, 0, 15).to_vec();
    siv.push(1);
    stream(
        0x10050,
        &[
            instruction(95, &masked(1, 0, 15)),
            instruction(103, &siv),
            instruction(104, &[1]),
            instruction(
                54 | 1 << 13,
                &[masked(0, 0, 15).to_vec(), swizzled(1, 0).to_vec()].concat(),
            ),
            instruction(54, &[masked(2, 0, 15).to_vec(), swizzled(0, 0).to_vec()].concat()),
            instruction(62, &[]),
        ],
    )
}

#[test]
fn a_vertex_program_decodes_to_typed_declarations_and_instructions() {
    let d = dxbc_reader::decode(&vertex()).unwrap();
    assert_eq!((d.kind, d.major, d.minor), (ProgramType::Vertex, 5, 0));
    assert_eq!(d.target(), "vs_5_0");
    assert_eq!(d.declarations.len(), 3);
    let Declared::Input {
        operand,
        interpolation,
        name,
    } = &d.declarations[0].kind
    else {
        panic!("{:?}", d.declarations[0]);
    };
    assert_eq!(operand.register, Register::Input);
    assert_eq!(operand.selection, Selection::Mask(15));
    assert_eq!((*interpolation, *name), (None, None));
    let Declared::Output { name, .. } = &d.declarations[1].kind else {
        panic!("{:?}", d.declarations[1]);
    };
    assert_eq!(*name, Some(1));
    assert!(matches!(d.declarations[2].kind, Declared::Temps(1)));
    let names: Vec<_> = d.instructions.iter().map(|i| i.name()).collect();
    assert_eq!(names, ["mov", "mov", "ret"]);
    assert!(d.instructions[0].saturate && !d.instructions[1].saturate);
    assert_eq!(
        d.instructions[0].operands[1].selection,
        Selection::Swizzle([0, 1, 2, 3])
    );
}

#[test]
fn declarations_carry_their_fields() {
    // dcl_constantbuffer cb3[12], dynamicIndexed; dcl_sampler s1, mode_comparison;
    // dcl_resource_texture2dms(4) t5, (float,float,float,float); dcl_resource_structured t2, 64
    let cb = [8 << 12 | 2 << 20, 3, 12];
    let sampler = [6 << 12 | 1 << 20, 1];
    let texture = [7 << 12 | 1 << 20, 5, 0x5555];
    let structured = [7 << 12 | 1 << 20, 2, 64];
    let d = dxbc_reader::decode(&stream(
        0x50,
        &[
            instruction(89 | 1 << 11, &cb),
            instruction(90 | 1 << 11, &sampler),
            instruction(88 | 3 << 11 | 4 << 16, &texture),
            instruction(162, &structured),
            instruction(62, &[]),
        ],
    ))
    .unwrap();
    let kinds: Vec<_> = d.declarations.iter().map(|d| &d.kind).collect();
    assert!(matches!(
        kinds[0],
        Declared::ConstantBuffer {
            slot: 3,
            rows: 12,
            dynamic: true,
            ..
        }
    ));
    assert!(matches!(kinds[1], Declared::Sampler { slot: 1, mode: 1 }));
    assert!(matches!(
        kinds[2],
        Declared::Resource {
            slot: 5,
            dimension: 3,
            samples: 4,
            returns: [5, 5, 5, 5],
            ..
        }
    ));
    assert!(matches!(
        kinds[3],
        Declared::ResourceStructured { slot: 2, stride: 64 }
    ));
}

#[test]
fn unreadable_tokens_are_errors_naming_the_word() {
    // a stage the format does not have, a length that disagrees, an operand register type with no name, a truncated
    // instruction
    assert!(
        dxbc_reader::decode(&[0x70050, 2])
            .unwrap_err()
            .detail
            .contains("program type 7")
    );
    assert!(
        dxbc_reader::decode(&[0x50, 5])
            .unwrap_err()
            .detail
            .contains("declared length")
    );
    let bad = stream(
        0x50,
        &[instruction(54, &[2 | 15 << 4 | 99 << 12 | 1 << 20, 0, 1])],
    );
    let e = dxbc_reader::decode(&bad).unwrap_err();
    assert!(e.detail.contains("operand register type 99"), "{e}");
    let mut short = vertex();
    short.truncate(short.len() - 2);
    short[1] = short.len() as u32;
    assert!(dxbc_reader::decode(&short).is_err());
    for n in 0..vertex().len() {
        let mut cut = vertex()[..n].to_vec();
        if n >= 2 {
            cut[1] = n as u32;
        }
        let _ = dxbc_reader::decode(&cut);
    }
}
