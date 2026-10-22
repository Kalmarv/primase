//! The passes that work textures on the world's stated threads (`core.threads`) make the serial world: the same
//! textures, byte for byte, and the same logs, on any count. normalize-normals over several normal maps of each
//! convention, pack-orm over several material pairs (stored and generated levels, resampled sizes).

#![allow(clippy::unwrap_used)]

use primase_core::parallel::Threads;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat, TexRole,
    Texture, World,
};
use primase_image::{MissingLevels, NormalizeNormals, OrmSize, PackOrm};

fn noise(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 24) as u8
        })
        .collect()
}

/// A BC1 or RGBA8 texture `w` x `h` with `levels` stored levels of noise.
fn texture(w: u32, h: u32, levels: usize, bc1: bool, seed: u32, role: TexRole) -> Texture {
    let level = |i: usize| {
        let (lw, lh) = ((w >> i).max(1), (h >> i).max(1));
        if bc1 {
            noise((lw.div_ceil(4) * lh.div_ceil(4) * 8) as usize, seed + i as u32)
        } else {
            noise((lw * lh * 4) as usize, seed + i as u32)
        }
    };
    Texture {
        width: w,
        height: h,
        format: if bc1 {
            TexFormat::Bc1
        } else {
            TexFormat::Rgba8Unorm
        },
        levels: (0..levels).map(level).collect(),
        color_space: ColorSpace::Linear,
        role,
        normal: None,
    }
}

/// The world after `pass` with `threads` stated (none: absent), and the run's report.
fn after(mut w: World, threads: Option<usize>, pass: Pipeline) -> (Vec<(Entity, Texture)>, RunReport) {
    if let Some(n) = threads {
        w.insert_resource(Threads::new(n).unwrap());
    }
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_pass_checks(true)
        .run(&mut w, &pass, &mut report)
        .unwrap();
    (w.iter::<Texture>().map(|(e, t)| (e, t.clone())).collect(), report)
}

fn normals() -> World {
    let mut w = World::new();
    let conventions = [
        NormalConvention {
            green: NormalGreen::Down,
            z: NormalZ::Rebuild,
            x: NormalX::Red,
            basis: NormalBasis::TangentBitangent,
        },
        NormalConvention {
            green: NormalGreen::Up,
            z: NormalZ::Rebuild,
            x: NormalX::Alpha,
            basis: NormalBasis::BitangentTangent,
        },
        NormalConvention::GLTF,
    ];
    for k in 0..9u32 {
        let e = w.spawn();
        let mut t = texture(36 + 8 * k, 20 + 4 * k, 3, k % 2 == 0, 100 + k, TexRole::Normal);
        t.normal = Some(conventions[k as usize % 3]);
        w.insert(e, t);
    }
    w
}

#[test]
fn normalize_normals_on_any_thread_count_is_the_serial_pass() {
    let pass = || Pipeline {
        passes: vec![Box::new(NormalizeNormals)],
    };
    let (serial, report) = after(normals(), None, pass());
    assert_eq!(
        after(normals(), Some(1), pass()),
        (serial.clone(), report.clone())
    );
    for n in [2, 4, 16] {
        let (textures, r) = after(normals(), Some(n), pass());
        assert_eq!(textures, serial, "{n} threads");
        assert_eq!(r, report, "{n} threads: the log");
    }
}

fn pairs() -> World {
    let mut w = World::new();
    for k in 0..6u32 {
        let mr = w.spawn();
        w.insert(
            mr,
            texture(
                64 + 16 * k,
                48,
                1 + k as usize % 3,
                k % 2 == 0,
                7 + k,
                TexRole::Mask,
            ),
        );
        let ao = w.spawn();
        w.insert(
            ao,
            texture(32 + 8 * k, 24 + 8 * k, 2, k % 3 == 0, 70 + k, TexRole::Mask),
        );
        let m = w.spawn();
        w.insert(
            m,
            PbrMaterial {
                metallic_roughness_texture: Some(TextureRef::new(mr)),
                occlusion_texture: Some(TextureRef::new(ao)),
                ..PbrMaterial::default()
            },
        );
    }
    w
}

#[test]
fn pack_orm_on_any_thread_count_is_the_serial_pass() {
    for size in [OrmSize::MetallicRoughness, OrmSize::Occlusion] {
        let pass = || Pipeline {
            passes: vec![Box::new(PackOrm::new(size, MissingLevels::Generate))],
        };
        let (serial, report) = after(pairs(), None, pass());
        for n in [2, 8] {
            let (textures, r) = after(pairs(), Some(n), pass());
            assert_eq!(textures, serial, "{size:?} on {n} threads");
            assert_eq!(r, report, "{size:?} on {n} threads: the log");
        }
    }
}
