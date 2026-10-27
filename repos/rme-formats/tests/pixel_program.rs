//! `pixel_program::classify` on hand-laid pixel-program containers (header 0..32, one 48-byte parameter record,
//! 24-byte descriptor, strings, embedded-constant table, microcode).
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use rme_formats::pixel_program::{PixelProgram, classify};

const XYZW: u32 = (1 << 11) | (2 << 13) | (3 << 15);

fn word(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

/// One microcode slot: end flag, destination, write mask xyzw, opcode, always-executed predicate, xyzw swizzles.
fn slot(op: u32, end: bool) -> [u32; 4] {
    [
        u32::from(end) | (15 << 9) | ((op & 63) << 24),
        XYZW | (7 << 18),
        XYZW | ((op >> 6) << 31),
        XYZW,
    ]
}
fn encode(words: &[u32]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|w| w.rotate_left(16).to_be_bytes())
        .collect()
}

/// `TEX R0, f[TEX0], TEX<unit>` (op 0x17), then `second` as the program's last instruction.
fn microcode(unit: u32, second: &[[u32; 4]], constant: Option<[u32; 4]>) -> Vec<u8> {
    let mut tex = slot(0x17, false);
    tex[0] |= (unit << 17) | (4 << 13); // texture unit; input register 4 = TEX0
    tex[1] |= 1; // source 0 is an input
    let mut words: Vec<u32> = tex.to_vec();
    for s in second {
        words.extend(s);
    }
    if let Some(c) = constant {
        words.extend(c);
    }
    encode(&words)
}

/// A fragment container: parameter `name` (a uniform whose 16-byte constant is embedded at microcode offset
/// `constant_at`, if given), with `ucode`.
fn container(name: &str, constant_at: Option<u32>, ucode: &[u8]) -> Vec<u8> {
    let name_at = 104;
    let table_at = (name_at + name.len() + 1).next_multiple_of(4);
    let ucode_at = (table_at + 8).next_multiple_of(16);
    let total = ucode_at + ucode.len();
    let mut b = vec![0; total];
    for (i, n) in [
        7004,
        6,
        total as u32,
        1,
        32,
        80,
        ucode.len() as u32,
        ucode_at as u32,
    ]
    .into_iter()
    .enumerate()
    {
        word(&mut b, i * 4, n);
    }
    let embedded = if constant_at.is_some() { table_at as u32 } else { 0 };
    for (i, n) in [
        1046,
        2051,
        4102,
        3,
        name_at as u32,
        0,
        embedded,
        111,
        4097,
        u32::MAX,
        1,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        word(&mut b, 32 + i * 4, n);
    }
    word(&mut b, 80, (ucode.len() / 16) as u32);
    b[92..104].copy_from_slice(&[0, 3, 0, 2, 0, 1, 7, 1, 1, 1, 0, 0]);
    b[name_at..name_at + name.len()].copy_from_slice(name.as_bytes());
    if let Some(at) = constant_at {
        word(&mut b, table_at, 1);
        word(&mut b, table_at + 4, at);
    }
    b[ucode_at..].copy_from_slice(ucode);
    b
}

/// A `.psp` payload: programs each padded with zeros to 64 bytes.
fn psp(programs: &[Vec<u8>]) -> Vec<u8> {
    programs
        .iter()
        .flat_map(|p| {
            let mut p = p.clone();
            p.resize(p.len().next_multiple_of(64), 0);
            p
        })
        .collect()
}

/// `TEX R0, f[TEX0], TEX0; MUL R0, R0, tint` with the tint constant in its own slot (microcode offset 32).
fn scaled(unit: u32) -> Vec<u8> {
    let mut mul = slot(0x02, true);
    mul[2] |= 2; // source 1 is a constant
    container(
        "Tint",
        Some(32),
        &microcode(unit, &[mul], Some([0x3f800000, 0x3f800000, 0x3f800000, 0])),
    )
}

fn scaled_class(uniform: &str, unit: u32) -> PixelProgram {
    PixelProgram::ScaledTexture {
        unit,
        uniforms: vec![uniform.into()],
    }
}

#[test]
fn a_texture_scaled_by_a_named_uniform_is_classified_with_its_unit_and_uniform() {
    assert_eq!(classify(&psp(&[scaled(0)])).unwrap(), scaled_class("Tint", 0));
    assert_eq!(classify(&psp(&[scaled(3)])).unwrap(), scaled_class("Tint", 3));
    // several programs that agree are the class; ones that differ are not
    assert_eq!(
        classify(&psp(&[scaled(0), scaled(0)])).unwrap(),
        scaled_class("Tint", 0)
    );
    assert_eq!(
        classify(&psp(&[scaled(0), scaled(1)])).unwrap(),
        PixelProgram::Unclassified
    );
}

#[test]
fn any_other_interpolant_operation_or_unnamed_constant_is_not_classified() {
    // a second interpolant read: MUL R0, R0, f[COL0]
    let mut by_colour = slot(0x02, true);
    by_colour[0] |= 1 << 13; // input register 1
    by_colour[2] |= 1; // source 1 is an input
    let p = container("Tint", None, &microcode(0, &[by_colour], None));
    assert_eq!(classify(&psp(&[p])).unwrap(), PixelProgram::Unclassified);
    // an operation that is not a copy or a multiply: DP3 R0, R0, tint
    let mut dot = slot(0x05, true);
    dot[2] |= 2;
    let p = container("Tint", Some(32), &microcode(0, &[dot], Some([0; 4])));
    assert_eq!(classify(&psp(&[p])).unwrap(), PixelProgram::Unclassified);
    // a constant that no named parameter holds
    let mut mul = slot(0x02, true);
    mul[2] |= 2;
    let p = container("Tint", Some(16), &microcode(0, &[mul], Some([0; 4])));
    assert_eq!(classify(&psp(&[p])).unwrap(), PixelProgram::Unclassified);
    // no texture sample at all: MOV R0, R0 as the only instruction
    let p = container("Tint", None, &encode(&slot(0x01, true)));
    assert_eq!(classify(&psp(&[p])).unwrap(), PixelProgram::Unclassified);
}

#[test]
fn a_program_that_is_not_a_container_or_has_dirty_padding_is_refused() {
    assert!(classify(&[0; 64]).is_err());
    let mut dirty = psp(&[scaled(0)]);
    let end = scaled(0).len();
    dirty[end] = 1;
    assert!(classify(&dirty).is_err());
}
