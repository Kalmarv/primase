//! Hand-worked offsets and values, independent of the reader; no source game bytes.
use primase_core::World;
use primase_shred::components::ShredMaterial;
use primase_shred::read::material::{self, UniformNames};

fn put(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_be_bytes());
}
fn fixture() -> Vec<u8> {
    // 96 header + 4 strings + 16 parameter + 16 value + 28 sampler + 4 pointer
    // + 4 texture string + 8 alignment = 176 bytes.
    let mut b = vec![0; 176];
    for (at, v) in [
        (0, 0xaaaa0003),
        (4, 64),
        (8, 0x00400000),
        (12, 0xbeefface),
        (16, 7),
        (20, 0xffff000e),
        (24, u32::MAX),
        (28, 42),
        (32, u32::MAX),
        (36, u32::MAX),
        (40, 3),
        (44, 5),
        (48, u32::MAX),
        (52, u32::MAX),
        (60, 9),
        (64, u32::MAX),
        (68, 32),
        (72, 34),
        (76, 1),
        (80, 36),
        (84, 1),
        (88, 68),
        (92, 96),
        (100, 120),
        (104, 0x100ad),
        (108, u32::MAX),
        (112, 52),
        (116, 0x3f800000),
        (120, 0xc0000000),
        (124, 0x3f000000),
        (128, 0),
        (132, 0x01010100),
        (136, 2),
        (140, 0x20c02),
        (144, 0),
        (148, 999),
        (152, 0x201),
        (156, 0xffff000e),
        (160, 100),
    ] {
        put(&mut b, at, v);
    }
    b[96..100].copy_from_slice(b"v\0p\0");
    b[164..168].copy_from_slice(b"tex\0");
    b
}
fn index() -> Vec<u8> {
    let mut b = vec![0; 36];
    for (at, v) in [
        (0, 0xfadd0001),
        (4, 36),
        (8, 0x00180000),
        (12, 0xbeefface),
        (16, 1),
        (20, 24),
        (24, 7),
        (28, 0),
        (32, 0x004000b0),
    ] {
        put(&mut b, at, v);
    }
    b
}
#[test]
fn hand_worked_record_and_banked_world() {
    let names = UniformNames::from([(120, "x".into())]);
    let b = fixture();
    let m = material::record("actors", &b, &names).unwrap();
    assert_eq!((&*m.vertex_shader, &*m.fragment_shader), ("v", "p"));
    assert_eq!(m.unknown_header, [42, 3, 5, 9]);
    assert_eq!(m.parameters[0].value, [1., -2., 0.5, 0.]);
    assert_eq!(m.parameters[0].name.as_deref(), Some("x"));
    assert_eq!(m.samplers[0].unknown_state, [0x01010100, 2, 0x20c02, 0, 0x201]);
    assert_eq!(m.samplers[0].texture, "tex");
    assert_eq!(m.samplers[0].hash, 999);
    let a = material::bank("actors", &index(), &b, |s| {
        assert_eq!(s, "p");
        Ok(names.clone())
    })
    .unwrap();
    let c = material::bank("characters", &index(), &b, |_| Ok(names.clone())).unwrap();
    let mut world = World::new();
    assert_eq!(material::attach(&mut world, a.clone()).unwrap().len(), 1);
    assert_eq!(material::attach(&mut world, c).unwrap().len(), 1);
    assert!(material::attach(&mut world, a).is_err());
    assert_eq!(world.iter::<ShredMaterial>().count(), 2);
}
#[test]
fn refuses_unexplained_structure_and_non_finite_values() {
    let original = fixture();
    for n in 0..original.len() {
        assert!(
            material::record("a", &original[..n], &UniformNames::new()).is_err(),
            "prefix {n}"
        );
    }
    for (at, v) in [
        (0, 0),
        (4, 65),
        (8, 0),
        (20, 0),
        (64, 0),
        (68, 33),
        (72, 35),
        (76, u32::MAX),
        (80, 37),
        (88, 69),
        (92, 97),
        (104, 0x100ae),
        (108, 0),
        (112, 53),
        (116, 0x7fc00000),
        (156, 0),
        (160, 101),
        (172, 1),
    ] {
        let mut b = original.clone();
        put(&mut b, at, v);
        assert!(
            material::record("a", &b, &UniformNames::new()).is_err(),
            "at {at}"
        );
    }
    let mut b = original.clone();
    b.push(0);
    assert!(material::record("a", &b, &UniformNames::new()).is_err());
    for (at, v) in [
        (0, 0),
        (4, 35),
        (8, 0),
        (16, u32::MAX),
        (20, 28),
        (24, 8),
        (28, 1),
        (32, 176),
    ] {
        let mut i = index();
        put(&mut i, at, v);
        assert!(material::bank("a", &i, &original, |_| Ok(UniformNames::new())).is_err());
    }
}
#[test]
fn fragment_reflection_is_local_and_checked() {
    let mut b = vec![0; 128];
    for (at, v) in [
        (0, 0x1b5c),
        (4, 6),
        (8, 82),
        (12, 1),
        (16, 32),
        (40, 4102),
        (48, 80),
    ] {
        put(&mut b, at, v);
    }
    b[80] = b'x';
    assert_eq!(
        material::fragment_uniform_names(&b).unwrap(),
        UniformNames::from([(120, "x".into())])
    );
    for (at, v) in [
        (0, 0x1b5b),
        (4, 7),
        (8, 0),
        (12, u32::MAX),
        (16, 100),
        (48, 128),
        (124, 1),
    ] {
        let mut corrupt = b.clone();
        put(&mut corrupt, at, v);
        assert!(material::fragment_uniform_names(&corrupt).is_err(), "at {at}");
    }
}
