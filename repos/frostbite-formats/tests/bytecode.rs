//! CompiledBytecode payloads: containers found by their own size fields (in a hand-laid payload with a header, a gap
//! and a tail), what a magic that heads no container does, and the resource type names against their ids.

#![allow(clippy::unwrap_used)]

use frostbite_formats::bytecode::{self, Span};
use frostbite_formats::listing::{TYPE_NAMES, type_id};

/// A container of one empty `DXIL` part: the `DXBC` magic, a 16-byte digest, version 1.0, the container's size, one
/// part offset, and the part's fourcc and size.
fn container() -> Vec<u8> {
    let mut c = b"DXBC".to_vec();
    c.extend([0u8; 16]);
    c.extend(1u32.to_le_bytes());
    c.extend(44u32.to_le_bytes());
    c.extend(1u32.to_le_bytes());
    c.extend(36u32.to_le_bytes());
    c.extend(b"DXIL");
    c.extend(0u32.to_le_bytes());
    c
}

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
    let blend = container();
    let p = payload(b"RC2S", &[(&blend, &[1, 2, 3]), (&blend, &[9; 8])]);
    let n = blend.len();
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

/// A magic that heads no container is an error naming where; a payload with no container and a container that does
/// not fit are errors too; a magic inside a container's bytes is not another container.
#[test]
fn a_magic_that_heads_no_container_is_an_error() {
    // version 2
    let blend = container();
    let mut bad = blend.clone();
    bad[20] = 2;
    let err = bytecode::containers(&payload(b"RC2S", &[(&bad, &[])]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("0x40") && err.contains("version"), "{err}");
    // a size past the payload
    let p = payload(b"RC2S", &[(&blend, &[])]);
    let err = bytecode::containers(&p[..p.len() - 1]).unwrap_err().to_string();
    assert!(err.contains("size"), "{err}");
    // none at all
    let err = bytecode::containers(&[0u8; 64]).unwrap_err().to_string();
    assert!(err.contains("no DXBC container"), "{err}");
    // a magic after the container that heads nothing
    let mut tail = b"DXBC".to_vec();
    tail.extend([0u8; 40]);
    let err = bytecode::containers(&payload(b"RC2S", &[(&blend, &tail)]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("version"), "{err}");
    // a magic inside the container is part of it
    let mut inside = blend.clone();
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
