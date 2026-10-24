//! `raise-skate`'s derived MSK textures made on the world's stated threads (`core.threads`, a row of texels each) are
//! the serial ones byte for byte, and its log (the quantized factors among it) the serial log, on any count.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::parallel::Threads;
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology,
    World,
};
use primase_reskate::passes::{LightCalibration, RaiseOptions, raise_skate};

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

/// Five placed prototypes, each with its own mask source (sizes not whole blocks, BC1 or RGBA8, two to three levels)
/// and factors that do and do not quantize.
fn world(threads: Option<usize>) -> World {
    let mut w = World::new();
    if let Some(n) = threads {
        w.insert_resource(Threads::new(n).unwrap());
    }
    for k in 0..5u32 {
        let (width, height) = (40 + 12 * k, 33 + 6 * k);
        let bc1 = k % 2 == 0;
        let levels = (0..2 + k as usize % 2)
            .map(|i| {
                let (lw, lh) = ((width >> i).max(1), (height >> i).max(1));
                let n = if bc1 {
                    lw.div_ceil(4) * lh.div_ceil(4) * 8
                } else {
                    lw * lh * 4
                };
                noise(n as usize, 11 + 7 * k + i as u32)
            })
            .collect();
        let source = w.spawn();
        w.insert(
            source,
            Texture {
                width,
                height,
                format: if bc1 {
                    TexFormat::Bc1
                } else {
                    TexFormat::Rgba8Unorm
                },
                levels,
                color_space: ColorSpace::Linear,
                role: TexRole::Mask,
                normal: None,
            },
        );
        let material = w.spawn();
        w.insert(
            material,
            PbrMaterial {
                metallic: [1.0, 0.5, 0.37, 1.0, 0.0][k as usize],
                roughness: [1.0, 0.25, 0.81, 0.5, 1.0][k as usize],
                metallic_roughness_texture: Some(TextureRef::new(source)),
                occlusion_texture: (k % 2 == 1).then(|| TextureRef::new(source)),
                occlusion_strength: [1.0, 0.6, 1.0, 0.33, 1.0][k as usize],
                ..PbrMaterial::default()
            },
        );
        let prototype = w.spawn();
        w.insert(
            prototype,
            Mesh::new(vec![Primitive {
                material: Some(material),
                topology: Topology::Triangles,
                indices: None,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
                    ),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0.0; 2], [1.0, 0.0], [0.0, 1.0]]),
                    ),
                ]),
            }]),
        );
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype,
                variant: 0,
            },
        );
    }
    w
}

#[test]
fn the_masks_on_any_thread_count_are_the_serial_ones() {
    let raised = |threads: Option<usize>| {
        let mut w = world(threads);
        let log = raise_skate(
            &mut w,
            &RaiseOptions::new(
                "threads",
                LightCalibration::Standard,
                primase_reskate::passes::Unclassified::LeaveOut,
            )
            .unwrap(),
        )
        .unwrap();
        let textures: Vec<(Entity, Texture)> = w.iter::<Texture>().map(|(e, t)| (e, t.clone())).collect();
        (textures, log)
    };
    let (serial, log) = raised(None);
    assert!(log.counts["derived MSK textures"] >= 4, "{log:?}");
    for n in [1, 2, 8] {
        let (textures, l) = raised(Some(n));
        assert_eq!(textures, serial, "{n} threads");
        assert_eq!(l, log, "{n} threads: the log");
    }
}
