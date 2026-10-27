//! Binding sets and pass bindings: a hand-laid set read to its groups, parameters and entries, the register rule over
//! it, the refusals, and the shared data that names the sets; and, on the game's data (the skate. install and its
//! Oodle 9 library), every binding set and pass binding of the game configuration, level root and customization TOCs
//! read, and the shaders of the game configuration's permutations compared with the rule: each constant buffer's size
//! with the group the rule names, each resource with the entry the rule names.

#![allow(clippy::unwrap_used)]

use frostbite_formats::binding::{self, BINDING_SET, Bindings};
use frostbite_formats::permutation::{self, SHARED_RASTER};

fn meta(size: usize, reloc: usize, id: u64) -> [u8; 16] {
    let mut m = [0u8; 16];
    m[0..4].copy_from_slice(&(size as u32).to_le_bytes());
    m[4..8].copy_from_slice(&(reloc as u32).to_le_bytes());
    m[8..16].copy_from_slice(&id.to_le_bytes());
    m
}

/// A binding set of two groups laid as the game's are: a constant buffer of `[float3 at 0, float at 12]` (16 bytes) and
/// two resources at indices 0 and 1. Returns the payload (with its relocations) and the metadata.
fn set(id: u64) -> (Vec<u8>, [u8; 16]) {
    let mut d = vec![0u8; 0x20 + 0x40 * 2];
    d[0..4].copy_from_slice(&2u32.to_le_bytes());
    d[4..8].copy_from_slice(&0x20u32.to_le_bytes());
    d[0x10..0x18].copy_from_slice(&id.to_le_bytes());
    d[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
    d[0x1c..0x20].copy_from_slice(&4u32.to_le_bytes());
    // three parameters' records: (name, type, count, extra)
    let params = [
        (0xA1u64, 0x885e_ff59u32),
        (0xA2, 0x2bb6_5c2d),
        (0xB1, 0x14c5_fc30),
        (0xB2, 0x14c5_fc30),
    ];
    let params_at = d.len();
    for (name, ty) in params {
        d.extend(name.to_le_bytes());
        d.extend(ty.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(0u16.to_le_bytes());
    }
    let (t0, t1) = (d.len(), d.len() + 16);
    for (at, kind) in [(0u32, 0x30u16), (12, 0x10)] {
        d.extend(at.to_le_bytes());
        d.extend(0xFFFFu16.to_le_bytes());
        d.extend(kind.to_le_bytes());
    }
    for (at, kind) in [(0u32, 0x0du16), (1, 0x0d)] {
        d.extend(at.to_le_bytes());
        d.extend(0xFFFFu16.to_le_bytes());
        d.extend(kind.to_le_bytes());
    }
    let end = d.len();
    for (g, (name, size, flags, n, p0, t)) in [
        (0x6701u64, 16u32, [2u8, 2, 2, 0], 2u16, params_at, t0),
        (0x6702, 2, [2, 0, 2, 1], 2, params_at + 32, t1),
    ]
    .into_iter()
    .enumerate()
    {
        let at = 0x20 + 0x40 * g;
        d[at..at + 8].copy_from_slice(&name.to_le_bytes());
        d[at + 8..at + 12].copy_from_slice(&size.to_le_bytes());
        d[at + 12..at + 16].copy_from_slice(&flags);
        d[at + 0x10..at + 0x12].copy_from_slice(&n.to_le_bytes());
        for (k, v) in [params_at, p0, t, end].into_iter().enumerate() {
            d[at + 0x20 + 8 * k..at + 0x28 + 8 * k].copy_from_slice(&(v as u64).to_le_bytes());
        }
    }
    let payload = d.len();
    for g in 0..2usize {
        for k in 0..4usize {
            d.extend(((0x20 + 0x40 * g + 0x20 + 8 * k) as u32).to_le_bytes());
        }
    }
    d.extend(4u32.to_le_bytes());
    let reloc = d.len() - payload;
    (d, meta(payload, reloc, id))
}

#[test]
fn a_binding_set_reads_to_its_groups_and_entries() {
    let (d, m) = set(0xABCD);
    let s = binding::read(BINDING_SET, &d, &m).unwrap();
    assert_eq!((s.id, s.pass, s.head), (0xABCD, false, [1, 4]));
    assert_eq!(s.groups.len(), 2);
    let (cb, res) = (&s.groups[0], &s.groups[1]);
    assert_eq!((cb.name, cb.size, cb.flags), (0x6701, 16, [2, 2, 2, 0]));
    assert_eq!(cb.params.iter().map(|p| p.name).collect::<Vec<_>>(), [0xA1, 0xA2]);
    assert_eq!(cb.params[0].ty, 0x885e_ff59);
    // a constant buffer's size is the end of its last parameter: float3 at 0 and a float at 12
    assert_eq!(cb.buffer_end(), 16);
    assert_eq!((res.entries[1].at, res.entries[1].kind), (1, 0x0d));
    assert_eq!(s.relocations.last(), Some(&4));
}

#[test]
fn a_malformed_binding_set_is_refused() {
    let (d, m) = set(0xABCD);
    let err = |d: &[u8], m: &[u8; 16], t: u32| binding::read(t, d, m).unwrap_err().to_string();
    // another resource type, the metadata's id, its size
    assert!(err(&d, &m, 0x1234).contains("not a binding set"));
    assert!(err(&d, &meta(d.len() - 36, 36, 0xABCE), BINDING_SET).contains("id"));
    assert!(err(&d[..d.len() - 4], &m, BINDING_SET).contains("relocations"));
    // a group whose entries lie outside the payload
    let mut bad = d.clone();
    bad[0x20 + 0x30..0x20 + 0x38].copy_from_slice(&0x9000u64.to_le_bytes());
    assert!(err(&bad, &m, BINDING_SET).contains("do not fit"));
    // a pointer the relocation list does not name
    let mut bad = d.clone();
    let n = bad.len();
    bad[n - 8..n - 4].copy_from_slice(&0x7fu32.to_le_bytes());
    assert!(err(&bad, &m, BINDING_SET).contains("relocation list lacks"));
}

/// The register rule over a pass binding and two sets: b4 is the pass binding's group 0, b0 set 1's, b1 set 0's; a
/// resource reads the group 1 of its range's set at its index.
#[test]
fn registers_resolve_through_the_slots() {
    let (a, ma) = set(1);
    let (b, mb) = set(2);
    let (p, mp) = set(3);
    let (s0, s1, pass) = (
        binding::read(BINDING_SET, &a, &ma).unwrap(),
        binding::read(BINDING_SET, &b, &mb).unwrap(),
        binding::read(BINDING_SET, &p, &mp).unwrap(),
    );
    let bindings = Bindings {
        pass: Some(&pass),
        sets: [Some(&s0), Some(&s1), None],
    };
    assert_eq!(bindings.constant_buffer(4).unwrap().name, 0x6701);
    assert_eq!(bindings.constant_buffer(1).map(|g| g.size), Some(16));
    assert!(bindings.constant_buffer(2).is_none());
    // t1 reads set 1's resources at index 1, t33 set 0's at index 1, t65 the pass binding's at 1
    for (register, set_has) in [
        (1u32, true),
        (33, true),
        (65, true),
        (2, false),
        (34, false),
        (66, false),
    ] {
        let found = bindings.resource(register);
        assert_eq!(found.is_some(), set_has, "t{register}");
        if let Some((g, i)) = found {
            assert_eq!(g.params[i].name, 0xB2);
        }
    }
    // a parameter by name: the first set that has it (set 0 and set 1 here name the same four)
    let slot = bindings.parameter(0xA2).unwrap();
    assert_eq!((slot.set, slot.group, slot.index, slot.entry.at), (0, 0, 1, 12));
    let slot = bindings.parameter(0xB1).unwrap();
    assert_eq!((slot.set, slot.group, slot.index, slot.entry.at), (0, 1, 0, 0));
    assert!(bindings.parameter(0xFF).is_none());
    // no set in a slot: nothing resolves there
    let none = Bindings {
        pass: None,
        sets: [None, None, None],
    };
    assert!(
        none.constant_buffer(0).is_none() && none.resource(40).is_none() && none.parameter(0xA1).is_none()
    );
}

#[test]
fn shared_data_names_the_sets() {
    let mut d = vec![0u8; 0x48];
    for (at, v) in [(0usize, 11u64), (8, 22), (0x18, 33), (0x28, 44), (0x38, 55)] {
        d[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }
    d[0x40] = 9;
    let s = permutation::read_shared(SHARED_RASTER, &d, &meta(d.len(), 0, 7)).unwrap();
    assert_eq!(
        (s.id, s.pass_binding, s.signature, s.sets, s.root_slots),
        (7, Some(11), 22, [Some(33), None, Some(44)], 55)
    );
    assert_eq!(s.rest[0], 9);
    // no signature, another type
    d[8] = 0;
    d[9..16].fill(0);
    assert!(permutation::read_shared(SHARED_RASTER, &d, &meta(d.len(), 0, 7)).is_err());
    assert!(permutation::read_shared(0x1, &d, &meta(d.len(), 0, 7)).is_err());
}
