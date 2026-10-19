//! A tiny public scene of literal geometry and pixels only, for examples and gates: an 8 x 8 gradient texture bound
//! with a texture transform, one triangle drawn by four placements along +x (the last mirrored), and two viewer
//! records (a closed curve and a collision triangle).
use std::collections::BTreeMap;

use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology,
    Transform, World,
};
use serde_json::json;

use crate::ViewerItem;

/// The scene's world.
pub fn synthetic_world() -> World {
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
        Mesh::new(vec![Primitive {
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
        }]),
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
    world
}

/// The scene's viewer records.
pub fn synthetic_viewer_items() -> Vec<ViewerItem> {
    vec![
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
    ]
}
