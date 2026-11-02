//! Oracle: Blender 5.2.3's Cycles, rendering a water body one metre deep over an emitting floor, against the light
//! worked by hand: seen straight down, the floor's emission (1) arrives through the surface weighted by its Fresnel
//! transmission at normal incidence (`1 - ((ior - 1) / (ior + 1))^2`, 0.96 for an ior of 1.5) and absorbed over the metre
//! of water (`exp(-extinction)`). The extinction the image shows is held to the body's per channel. The wave's
//! coordinates move with the frame: its drivers are read back from the saved `.blend` at two frames.
//!
//! The camera is orthographic, the pixel filter a hundredth of a pixel wide; the water surface is smooth, so the only
//! noise is Cycles' choice between reflecting and transmitting at the surface (a pixel's 256 samples): the extinction is
//! measured over the middle 64 pixels' mean. Needs `tools.blender`; skips without it.

#![allow(clippy::unwrap_used)]

mod inputs;

use std::collections::BTreeMap;

use primase_blender::render::{Camera, Device, Projection, RenderOptions, render};
use primase_blender::save::{Blender, save};
use primase_blender::{WriteOptions, write_file};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Mesh, Name, NormalConvention, Primitive, Semantic, Stream, TexFormat, TexRole, Texture,
    Topology, Transform, WaterBody, WaveLayer, World,
};

const EXTINCTION: [f32; 3] = [0.215, 0.115, 0.09];
const IOR: f32 = 1.5;
/// How far the wave's coordinates move a second, in core's frame (v down).
const VELOCITY: [f32; 2] = [0.25, -0.5];
const SIZE: u32 = 16;

/// A two-metre square at height `z`, facing up.
fn square(z: f32, material: primase_core::Entity) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[-1., 1., z], [1., 1., z], [1., -1., z], [-1., -1., z]]),
            ),
            (Semantic::Normal, Stream::F32x3(vec![[0., 0., 1.]; 4])),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]]),
            ),
        ]),
        indices: Some(vec![0, 3, 2, 0, 2, 1]),
        material: Some(material),
    }
}

fn world() -> World {
    let mut w = World::new();
    let floor = w.spawn();
    w.insert(floor, Name("floor".into()));
    w.insert(
        floor,
        PbrMaterial {
            base_color: [0., 0., 0., 1.],
            metallic: 0.,
            emissive: [1.; 3],
            ..PbrMaterial::default()
        },
    );
    let normal = w.spawn();
    w.insert(normal, Name("flat".into()));
    w.insert(
        normal,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![128, 128, 255, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Normal,
            normal: Some(NormalConvention::GLTF),
        },
    );
    let water = w.spawn();
    w.insert(water, Name("water".into()));
    w.insert(
        water,
        PbrMaterial {
            metallic: 0.,
            roughness: 0.,
            ..PbrMaterial::default()
        },
    );
    w.insert(
        water,
        WaterBody {
            extinction: EXTINCTION,
            scatter: [0.; 3],
            ior: IOR,
            // a flat map at strength 0: the surface stays smooth, the coordinates still move
            waves: vec![WaveLayer {
                normal: TextureRef::new(normal),
                velocity: VELOCITY,
                strength: 0.,
            }],
            shore_fade: 0.,
        },
    );
    for (name, z, m) in [("floor", 0., floor), ("water", 1., water)] {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(e, Mesh::new(vec![square(z, m)]));
        w.insert(e, Transform::IDENTITY);
    }
    w
}

#[test]
fn a_water_body_absorbs_what_is_behind_it_by_its_extinction() {
    let Some(executable) = inputs::tool(inputs::Tool::Blender) else {
        return;
    };
    let blender = Blender {
        executable: executable.clone(),
    };
    let scratch = inputs::own_scratch("blender-water");
    let folder = scratch.join("scene");
    let report = write_file(&world(), &WriteOptions::default(), &folder).unwrap();
    println!("{report}");
    assert_eq!(report.water_bodies, 1);
    assert!(report.water_unwritten.is_empty(), "{:?}", report.water_unwritten);
    let blend = scratch.join("scene.blend");
    save(&blender, &folder, &blend).unwrap();
    let options = RenderOptions {
        camera: Camera {
            eye: [0., 0., 10.],
            target: [0., 0., 0.],
            up: [0., 1., 0.],
            projection: Projection::Orthographic { height: 2. },
            near: 0.1,
            far: 100.,
        },
        size: [SIZE, SIZE],
        samples: 256,
        seed: 0,
        filter_width: 0.01,
        device: Device::Cpu,
    };
    let r = render(&blender, &blend, &options, &scratch.join("render")).unwrap();
    let combined = r.pass("Combined").unwrap();
    let mut mean = [0f64; 3];
    let mut n = 0;
    for j in 4..12 {
        for i in 4..12 {
            let p = (j * SIZE as usize + i) * 4;
            for k in 0..3 {
                mean[k] += f64::from(combined[p + k]);
            }
            n += 1;
        }
    }
    let f0 = ((IOR - 1.) / (IOR + 1.)).powi(2);
    for k in 0..3 {
        let seen = mean[k] / f64::from(n);
        let extinction = -(seen / f64::from(1. - f0)).ln();
        assert!(
            (extinction - f64::from(EXTINCTION[k])).abs() < 0.01,
            "channel {k}: {seen} seen, extinction {extinction}, the body's {}",
            EXTINCTION[k]
        );
    }
    // the wave's coordinates are moved by the frame: X by u, Y by -v (Blender's v is up), velocity times frame / fps
    let script = "import bpy\n\
        tree = bpy.data.materials['water'].node_tree\n\
        moved = [n for n in tree.nodes if n.bl_idname == 'ShaderNodeCombineXYZ']\n\
        assert len(moved) == 1, len(moved)\n\
        for frame in (0, 48):\n\
        \x20   bpy.context.scene.frame_set(frame)\n\
        \x20   print('primase-water-frame', frame, moved[0].inputs['X'].default_value, moved[0].inputs['Y'].default_value)\n";
    let run = std::process::Command::new(&executable)
        .args(["--background", "--factory-startup"])
        .arg(&blend)
        .args(["--python-exit-code", "1", "--python-expr", script])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let frames: BTreeMap<u32, [f32; 2]> = text
        .lines()
        .filter_map(|l| l.strip_prefix("primase-water-frame "))
        .map(|l| {
            let v: Vec<f32> = l.split(' ').map(|x| x.parse().unwrap()).collect();
            (v[0] as u32, [v[1], v[2]])
        })
        .collect();
    let seconds = 48. / primase_blender::graph::FPS as f32;
    assert_eq!(frames.get(&0), Some(&[0., 0.]), "{text}");
    let moved = frames[&48];
    for (got, want) in moved.iter().zip([VELOCITY[0] * seconds, -VELOCITY[1] * seconds]) {
        assert!((got - want).abs() < 1e-6, "frame 48: {moved:?}");
    }
}
