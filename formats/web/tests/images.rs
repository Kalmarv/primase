//! Format 2.0: images are files beside the tiles, named by the SHA256 of their bytes and shared by every tile that
//! uses them; tile GLBs reference them by URI and carry geometry and materials only.

use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology,
    Transform, World,
};
use primase_web::{IMAGE_DIR, Options, write};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn texture(rgba: [u8; 4]) -> Texture {
    Texture {
        width: 2,
        height: 2,
        format: TexFormat::Rgba8Unorm,
        levels: vec![rgba.repeat(4)],
        color_space: ColorSpace::Srgb,
        role: TexRole::Color,
        normal: None,
    }
}

/// Two materials (two images) on two prototypes, both placed in cells (0, 0) and (1, 0): every tile uses both images.
fn world() -> World {
    let mut w = World::new();
    let mut protos = Vec::new();
    for rgba in [[200, 100, 50, 255], [10, 20, 30, 255]] {
        let tex = w.spawn();
        w.insert(tex, texture(rgba));
        let mat = w.spawn();
        w.insert(
            mat,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(tex)),
                ..PbrMaterial::default()
            },
        );
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., -1.]]),
                    ),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]),
                    ),
                ]),
                indices: Some(vec![0, 1, 2]),
                material: Some(mat),
            }]),
        );
        protos.push(proto);
    }
    for x in [10., 250.] {
        for (k, &proto) in protos.iter().enumerate() {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3][0] = x + 50. * k as f32;
            w.insert(e, t);
        }
    }
    w
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn images_are_shared_files_and_tiles_reference_them() {
    for options in [
        Options::new(primase_web::TileSize::new(200.).unwrap()),
        Options {
            compress: false,
            gpu_instancing: false,
            ..Options::new(primase_web::TileSize::new(200.).unwrap())
        },
    ] {
        let set = write(&world(), &options).unwrap();
        let json = serde_json::to_value(&set.index).unwrap();
        assert_eq!(json["format_version"], "2.0");
        assert_eq!(set.index.tiles.len(), 2);
        // two distinct images, each one file, named by its bytes
        assert_eq!(set.images.len(), 2);
        assert_eq!(set.index.images.len(), 2);
        for i in &set.index.images {
            let bytes = &set.images[&i.file];
            assert_eq!(i.bytes, bytes.len() as u64);
            assert_eq!(i.file, format!("{IMAGE_DIR}/{}.png", hex(bytes)));
            assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
        for t in &set.index.tiles {
            // both tiles use both images
            assert_eq!(
                t.images.iter().collect::<BTreeSet<_>>(),
                set.index.images.iter().map(|i| &i.file).collect::<BTreeSet<_>>()
            );
            let glb = &set.files[&t.file];
            let chunks = primase_gltf::glb::parse(glb).unwrap();
            let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
            let uris: Vec<&str> = doc["images"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| {
                    assert!(i.get("bufferView").is_none());
                    i["uri"].as_str().unwrap()
                })
                .collect();
            assert_eq!(uris, t.images.iter().map(String::as_str).collect::<Vec<_>>());
            // no image bytes left in the GLB: every view is an accessor's
            let views = doc["bufferViews"].as_array().unwrap().len();
            let used: BTreeSet<u64> = doc["accessors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["bufferView"].as_u64().unwrap())
                .collect();
            assert_eq!(used.len(), views, "every view belongs to an accessor");
            for (name, bytes) in &set.images {
                assert!(
                    !glb.windows(bytes.len().min(64))
                        .any(|w| w == &bytes[..bytes.len().min(64)]),
                    "{name} embedded"
                );
            }
            // the plain set's geometry still reads back (primase-gltf does not decode meshopt)
            if !options.compress {
                let g = primase_gltf::Gltf::from_glb(glb).unwrap();
                assert!(g.mesh_count() >= 1);
            }
        }
        // images first, the index last, and write_to checks names against bytes
        let dir = std::env::temp_dir().join(format!("primase-web-images-{}", options.compress));
        let _ = std::fs::remove_dir_all(&dir);
        set.write_to(&dir).unwrap();
        for i in &set.index.images {
            assert_eq!(std::fs::read(dir.join(&i.file)).unwrap(), set.images[&i.file]);
        }
        let mut stale = set.clone();
        let first = stale.index.images[0].file.clone();
        stale.images.get_mut(&first).unwrap()[10] ^= 1;
        assert!(stale.write_to(&dir.join("stale")).is_err());
    }
}

#[test]
fn subcells_share_their_cells_images() {
    let whole = write(&world(), &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    let split = write(
        &world(),
        &Options {
            // every GLB is over this: each cell splits until one placement is left
            tile_byte_budget: Some(64),
            ..Options::new(primase_web::TileSize::new(200.).unwrap())
        },
    )
    .unwrap();
    assert!(split.index.tiles.len() > whole.index.tiles.len());
    assert!(split.index.tiles.iter().all(|t| t.cell.is_some()));
    // the same two image files, the same bytes, however many tiles use them
    assert_eq!(split.images, whole.images);
    assert_eq!(split.index.images, whole.index.images);
}
