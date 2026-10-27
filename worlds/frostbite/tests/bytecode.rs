//! CompiledBytecode payloads: containers found by their own size fields (a committed DXIL container, the one the
//! DXIL reader's fixtures use, in a hand-laid payload with a header, a gap and a tail), what a magic that heads no
//! container does, the bytes around each container kept as stored, and the resource type names against their ids.

#![allow(clippy::unwrap_used)]

use iridia_interface::ShaderFormat;
use primase_frostbite::bytecode::{self, Span};
use primase_frostbite::listing::{TYPE_NAMES, type_id};

const BLEND: &[u8] = include_bytes!("../../../toolkit/dxil/tests/fixtures/blend.dxil");

/// A payload: a `header` of 0x40 bytes with `tag` at 0x20, then each container with the `gap` after it.
fn payload(tag: &[u8; 4], containers: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut p = vec![0u8; 0x40];
    p[0] = 6;
    p[bytecode::TAG_AT..bytecode::TAG_AT + 4].copy_from_slice(tag);
    for (container, gap) in containers {
        p.extend_from_slice(container);
        p.extend_from_slice(gap);
    }
    p
}

#[test]
fn containers_are_found_by_their_size_fields() {
    let p = payload(b"RC2S", &[(BLEND, &[1, 2, 3]), (BLEND, &[9; 8])]);
    let n = BLEND.len();
    assert_eq!(
        bytecode::containers(&p).unwrap(),
        [
            Span {
                offset: 0x40,
                size: n
            },
            Span {
                offset: 0x40 + n + 3,
                size: n
            }
        ]
    );
    assert_eq!(bytecode::tag(&p), Some(*b"RC2S"));
    assert_eq!(bytecode::tag(&p[..0x20]), None);
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

/// A magic that heads no container is an error naming where; a payload with no container and a container that does
/// not fit are errors too; a magic inside a container's bytes is not another container.
#[test]
fn a_magic_that_heads_no_container_is_an_error() {
    // version 2
    let mut bad = BLEND.to_vec();
    bad[20] = 2;
    let err = bytecode::containers(&payload(b"RC2S", &[(&bad, &[])]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("0x40") && err.contains("version"), "{err}");
    // a size past the payload
    let p = payload(b"RC2S", &[(BLEND, &[])]);
    let err = bytecode::containers(&p[..p.len() - 1]).unwrap_err().to_string();
    assert!(err.contains("size"), "{err}");
    // none at all
    let err = bytecode::containers(&[0u8; 64]).unwrap_err().to_string();
    assert!(err.contains("no DXBC container"), "{err}");
    // a magic after the container that heads nothing
    let mut tail = b"DXBC".to_vec();
    tail.extend([0u8; 40]);
    let err = bytecode::containers(&payload(b"RC2S", &[(BLEND, &tail)]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("version"), "{err}");
    // a magic inside the container is part of it
    let mut inside = BLEND.to_vec();
    let k = inside.len() - 8;
    inside[k..k + 4].copy_from_slice(b"DXBC");
    assert_eq!(
        bytecode::containers(&payload(b"RC2S", &[(&inside, &[])])).unwrap(),
        [Span {
            offset: 0x40,
            size: inside.len()
        }]
    );
}

/// The names the game's executable holds for the resource types, against the ids the manifests carry: the id is the
/// djb2-xor hash of the lowercased name, so a name and an id that disagree is a wrong name.
#[test]
fn resource_type_names_hash_to_their_ids() {
    for (id, name) in TYPE_NAMES {
        assert_eq!(type_id(name), *id, "{name}");
    }
    let mut ids: Vec<u32> = TYPE_NAMES.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), TYPE_NAMES.len());
}
