//! One name, one image (the scene format's rule): two different images from same-named sources (GTA's texture
//! dictionaries give them) are written under names of their own, the shared source name kept as their stem, so the
//! scene reads back and the map's texture files stay named as before.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Instance, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology,
    Transform, World,
};
use primase_reskate::components::{MarkerKind, Other, SkateMarker};
use primase_reskate::passes::{LightCalibration, RaiseOptions};
use primase_reskate::{EncodingOptions, interchange};

/// A raised map: two slabs, each drawn with a texture named "diffuse", their pixels different; and a spawn.
fn map() -> World {
    let mut w = World::new();
    for (k, grey) in [(0.0f32, 60u8), (10.0, 200)] {
        let tex = w.spawn();
        w.insert(tex, Name("diffuse".into()));
        w.insert(
            tex,
            Texture {
                width: 2,
                height: 2,
                format: TexFormat::Rgba8Unorm,
                levels: vec![[grey, grey, grey, 255].repeat(4)],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        let material = w.spawn();
        w.insert(material, Name(format!("stone_{grey}")));
        w.insert(
            material,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(tex)),
                ..PbrMaterial::default()
            },
        );
        let proto = w.spawn();
        w.insert(proto, Name(format!("slab_{grey}")));
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, -4.0]]),
                    ),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
                    ),
                ]),
                indices: None,
                material: Some(material),
            }]),
        );
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [k, 0.0, 0.0];
        w.insert(p, t);
    }
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("names", LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([1.0, 1.0], Some(0.0))),
            yaw: None,
            m: None,
            blueprint: None,
            snap: None,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    w
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-names-{}", std::process::id()))
        .join(tag);
    std::fs::remove_dir_all(&d).ok();
    d
}

/// The texture files of the map written from `w`.
fn texture_files(w: &World, tag: &str) -> BTreeSet<String> {
    let r = primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root: tmp(tag),
            name: Some("names".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    std::fs::read_dir(r.out_dir.join("textures"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn same_named_images_are_written_under_names_of_their_own_and_read_back() {
    let w = map();
    let out = tmp("scene");
    interchange::write(&w, &out).unwrap();
    // the scene's texture names are one per image; the source name is the stem of both
    let scene: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("scene.json")).unwrap()).unwrap();
    let textures = scene["textures"].as_object().unwrap();
    assert_eq!(textures.len(), 2);
    let names: BTreeSet<&str> = textures.values().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 2, "{textures:?}");
    assert!(names.contains("diffuse"), "{names:?}");
    let stemmed: Vec<&str> = textures.values().filter_map(|t| t["stem"].as_str()).collect();
    assert_eq!(
        stemmed,
        ["diffuse"],
        "the renamed one keeps the source name as its stem"
    );
    // it reads back, loaded, and draws the same texture files as the world it came from
    let mut back = World::new();
    interchange::read(&out, &mut back).unwrap();
    interchange::load(&out, &mut back).unwrap();
    assert_eq!(texture_files(&back, "back"), texture_files(&w, "direct"));
    std::fs::remove_dir_all(out.parent().unwrap()).ok();
}
