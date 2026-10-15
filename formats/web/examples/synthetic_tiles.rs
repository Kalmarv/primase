//! Generate a tiny public fixture from literal geometry and pixels only.
mod support;
use basisu::{DecodeFlags, TargetFormat, Transcoder};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology,
    Transform, World,
};
use primase_web::{Options, ViewerItem};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(std::env::args().nth(1).ok_or("usage: synthetic_tiles <output>")?);
    let mut world = World::new();
    let tex = world.spawn();
    world.insert(
        tex,
        Texture {
            width: 8,
            height: 8,
            format: TexFormat::Rgba8Unorm,
            levels: vec![
                (0..64)
                    .flat_map(|i| [((i % 8) * 30) as u8, ((i / 8) * 30) as u8, 128, 255])
                    .collect(),
            ],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let material = world.spawn();
    world.insert(
        material,
        PbrMaterial {
            double_sided: true,
            base_color_texture: Some(TextureRef {
                transform: Some(UvTransform {
                    offset: [0.25, 0.5],
                    scale: [2., 1.],
                    rotation: 0.25,
                }),
                ..TextureRef::new(tex)
            }),
            ..PbrMaterial::default()
        },
    );
    let prototype = world.spawn();
    world.insert(
        prototype,
        Mesh {
            primitives: vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]]),
                    ),
                    (Semantic::Normal, Stream::F32x3(vec![[0., 0., 1.]; 3])),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]),
                    ),
                ]),
                indices: Some(vec![0, 1, 2]),
                material: Some(material),
            }],
        },
    );
    for (x, scale) in [(1., 1.), (4., 1.), (7., 1.), (10., -1.)] {
        let e = world.spawn();
        world.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[0][0] = scale;
        t.cols[3] = [x, 0., -1.];
        world.insert(e, t);
    }
    let mut options = Options {
        ktx2: true,
        viewer_items: vec![
            ViewerItem {
                name: "closed curve".into(),
                kind: "grind_curve".into(),
                anchor: [1., 0., -1.],
                data: json!({"points":[[1.,0.,-1.],[2.,1.,-1.],[3.,0.,-1.]],"closed":true}),
            },
            ViewerItem {
                name: "collision triangle".into(),
                kind: "collision".into(),
                anchor: [1., 0., -1.],
                data: json!({"positions":[[1.,0.,-1.],[3.,0.,-1.],[1.,0.,-3.]],"triangles":[[0,1,2]],"surfaces":[7]}),
            },
        ],
        ..Options::default()
    };
    let set = primase_web::write(&world, &options)?;
    set.write_to(&output)?;
    support::write_proof(&set, &output)?;
    let mut reference = Vec::new();
    for tile in &set.index.tiles {
        let chunks = primase_gltf::glb::parse(&set.files[&tile.file])?;
        let doc: Value = serde_json::from_slice(chunks.json)?;
        if let Some(images) = doc["images"].as_array() {
            for (i, image) in images.iter().enumerate() {
                if image["mimeType"] != "image/ktx2" {
                    continue;
                }
                let view = &doc["bufferViews"][image["bufferView"].as_u64().ok_or("image view")? as usize];
                let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
                let length = view["byteLength"].as_u64().ok_or("image length")? as usize;
                let decoder = Transcoder::new(&chunks.bin.ok_or("bin")?[start..start + length])
                    .map_err(|e| format!("{e:?}"))?;
                let mut hashes = Vec::new();
                for mip in 0..decoder.level_count() {
                    let pixels = decoder
                        .transcode(mip, TargetFormat::Rgba32, DecodeFlags::NONE)
                        .map_err(|e| format!("{e:?}"))?;
                    hashes.push(
                        Sha256::digest(&pixels)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>(),
                    );
                }
                reference.push(json!({"file":tile.file,"image":i,"hashes":hashes}));
            }
        }
    }
    std::fs::write(
        output.join("ktx2-reference.json"),
        serde_json::to_vec_pretty(&reference)?,
    )?;
    options.ktx2 = false;
    options.compress = false;
    options.gpu_instancing = false;
    primase_web::write(&world, &options)?.write_to(&output.join("plain"))?;
    Ok(())
}
