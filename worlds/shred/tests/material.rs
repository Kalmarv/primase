//! Source materials attached to a world as entities. The bank records are tested in `rme-formats`.
use primase_core::World;
use primase_shred::components::{ShredMaterial, Stored};
use primase_shred::read::material as attach_material;
use rme_formats::material::{self, UniformNames};

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
fn a_bank_attaches_one_entity_per_key_and_refuses_a_repeat() {
    let names = UniformNames::from([(120, "x".into())]);
    let b = fixture();
    let a: Vec<ShredMaterial> = material::bank("actors", &index(), &b, |_| Ok(names.clone()))
        .unwrap()
        .into_iter()
        .map(Stored)
        .collect();
    let c: Vec<ShredMaterial> = material::bank("characters", &index(), &b, |_| Ok(names.clone()))
        .unwrap()
        .into_iter()
        .map(Stored)
        .collect();
    let mut world = World::new();
    assert_eq!(attach_material::attach(&mut world, a.clone()).unwrap().len(), 1);
    assert_eq!(attach_material::attach(&mut world, c).unwrap().len(), 1);
    assert!(attach_material::attach(&mut world, a).is_err());
    assert_eq!(world.iter::<ShredMaterial>().count(), 2);
}
