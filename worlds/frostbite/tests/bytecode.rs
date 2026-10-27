//! CompiledBytecode payloads as programs: each container of a hand-laid payload (a committed DXIL container, the one
//! the DXIL reader's fixtures use, with a header, a gap and a tail) as a DXIL shader, the bytes around each container
//! kept as stored. The container search itself is `frostbite-formats`'s.

#![allow(clippy::unwrap_used)]

use frostbite_formats::bytecode::TAG_AT;
use iridia_interface::ShaderFormat;
use primase_frostbite::bytecode;

const BLEND: &[u8] = include_bytes!("fixtures/blend.dxil");

/// A payload: a `header` of 0x40 bytes with `tag` at 0x20, then each container with the `gap` after it.
fn payload(tag: &[u8; 4], containers: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut p = vec![0u8; 0x40];
    p[0] = 6;
    p[TAG_AT..TAG_AT + 4].copy_from_slice(tag);
    for (container, gap) in containers {
        p.extend_from_slice(container);
        p.extend_from_slice(gap);
    }
    p
}

/// Each container is a DXIL core shader of its own bytes; the header comes with the first and the bytes after a
/// container with it, so nothing of the payload is dropped.
#[test]
fn the_bytes_around_each_container_are_kept() {
    let p = payload(b"RPTR", &[(BLEND, &[1, 2, 3]), (BLEND, &[9; 8])]);
    let programs = bytecode::read(&p).unwrap();
    assert_eq!(programs.len(), 2);
    let ((s0, c0), (s1, c1)) = (&programs[0], &programs[1]);
    assert_eq!((s0.format, s1.format), (ShaderFormat::Dxil, ShaderFormat::Dxil));
    assert_eq!((&s0.bytecode[..], &s1.bytecode[..]), (BLEND, BLEND));
    assert_eq!((c0.index, c0.count, c0.offset), (0, 2, 0x40));
    assert_eq!((c1.index, c1.count, c1.offset), (1, 2, 0x40 + BLEND.len() + 3));
    assert_eq!(c0.tag, Some(*b"RPTR"));
    assert_eq!(c0.before, p[..0x40]);
    assert!(c1.before.is_empty());
    assert_eq!((&c0.after[..], &c1.after[..]), (&[1u8, 2, 3][..], &[9u8; 8][..]));
    // every byte of the payload is in exactly one of them
    let kept: usize = programs
        .iter()
        .map(|(s, c)| c.before.len() + s.bytecode.len() + c.after.len())
        .sum();
    assert_eq!(kept, p.len());
}
