// Test code: helpers outside #[test] functions unwrap too.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{Entity, MaterialVariants, Mesh, MeshFile, Primitive, Semantic, Stream, Topology, World};
use primase_meshops::bake_texture_transforms;

const T: UvTransform = UvTransform {
    offset: [0.5, -0.25],
    scale: [2.0, 4.0],
    rotation: 0.0,
};
const U: UvTransform = UvTransform {
    offset: [0.0, 0.0],
    scale: [3.0, 3.0],
    rotation: 0.0,
};
const UV: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.25, 1.0]];

fn primitive(material: Entity, uv: bool) -> Primitive {
    let mut streams = BTreeMap::from([(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; 3]))]);
    if uv {
        streams.insert(Semantic::TexCoord(0), Stream::F32x2(UV.to_vec()));
    }
    Primitive {
        topology: Topology::Triangles,
        streams,
        indices: None,
        material: Some(material),
    }
}

/// A material whose base colour (and, with `normal`, normal map) sample set 0 with these transforms.
fn material(w: &mut World, base: Option<UvTransform>, normal: Option<Option<UvTransform>>) -> Entity {
    let tex = w.spawn();
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(TextureRef {
                transform: base,
                ..TextureRef::new(tex)
            }),
            normal_texture: normal.map(|t| TextureRef {
                transform: t,
                ..TextureRef::new(tex)
            }),
            ..PbrMaterial::default()
        },
    );
    m
}

fn prototype(w: &mut World, prims: Vec<Primitive>) -> Entity {
    let e = w.spawn();
    w.insert(e, Mesh { primitives: prims });
    e
}

fn uv(w: &World, e: Entity, i: usize) -> Vec<[f32; 2]> {
    match &w.get::<Mesh>(e).unwrap().primitives[i].streams[&Semantic::TexCoord(0)] {
        Stream::F32x2(v) => v.clone(),
        _ => panic!("not F32x2"),
    }
}

fn base_transform(w: &World, m: Entity) -> Option<UvTransform> {
    w.get::<PbrMaterial>(m)
        .unwrap()
        .base_color_texture
        .unwrap()
        .transform
}

#[test]
fn a_shared_material_bakes_into_every_primitive_using_it() {
    let mut w = World::new();
    let m = material(&mut w, Some(T), Some(Some(T)));
    let a = prototype(&mut w, vec![primitive(m, true)]);
    let b = prototype(&mut w, vec![primitive(m, true), primitive(m, true)]);
    w.insert(
        b,
        MeshFile {
            path: "b.glb".into(),
            mesh: 0,
            materials: vec![Some(m), Some(m)],
        },
    );
    let log = bake_texture_transforms(&mut w);
    let want: Vec<[f32; 2]> = UV.iter().map(|&x| T.apply(x)).collect();
    assert_eq!(want[2], [1.0, 3.75]);
    for (e, i) in [(a, 0), (b, 0), (b, 1)] {
        assert_eq!(uv(&w, e, i), want);
    }
    assert_eq!(base_transform(&w, m), None);
    assert_eq!(
        w.get::<PbrMaterial>(m).unwrap().normal_texture.unwrap().transform,
        None
    );
    assert!(!w.has::<MeshFile>(b), "changed geometry is no longer the file's");
    assert_eq!(log.counts["coordinate sets baked"], 3);
    assert_eq!(log.counts["slot transforms cleared"], 2);
}

#[test]
fn variants_must_agree() {
    for (variant, baked) in [(T, true), (U, false)] {
        let mut w = World::new();
        let m = material(&mut w, Some(T), None);
        let v = material(&mut w, Some(variant), None);
        let a = prototype(&mut w, vec![primitive(m, true)]);
        w.insert(a, MaterialVariants(vec![BTreeMap::from([(1, v)])]));
        let log = bake_texture_transforms(&mut w);
        assert_eq!(base_transform(&w, m), (!baked).then_some(T), "{variant:?}");
        if baked {
            assert_eq!(uv(&w, a, 0)[1], T.apply(UV[1]));
            assert_eq!(base_transform(&w, v), None);
        } else {
            assert_eq!(uv(&w, a, 0), UV.to_vec());
            assert_eq!(base_transform(&w, v), Some(U));
            assert_eq!(
                log.counts["not baked: materials sample a set with different transforms"],
                2
            );
        }
    }
}

#[test]
fn groups_that_cannot_bake_are_left_and_counted() {
    // slots of one material disagree
    let mut w = World::new();
    let m = material(&mut w, Some(T), Some(None));
    let a = prototype(&mut w, vec![primitive(m, true)]);
    let log = bake_texture_transforms(&mut w);
    assert_eq!(uv(&w, a, 0), UV.to_vec());
    assert_eq!(base_transform(&w, m), Some(T));
    assert_eq!(
        log.counts["not baked: materials sample a set with different transforms"],
        2
    );

    // a primitive without the coordinate set
    let mut w = World::new();
    let m = material(&mut w, Some(T), None);
    prototype(&mut w, vec![primitive(m, true), primitive(m, false)]);
    let log = bake_texture_transforms(&mut w);
    assert_eq!(base_transform(&w, m), Some(T));
    assert_eq!(log.counts["not baked: a primitive lacks the coordinate set"], 1);

    // geometry not loaded
    let mut w = World::new();
    let m = material(&mut w, Some(T), None);
    let f = w.spawn();
    w.insert(
        f,
        MeshFile {
            path: "f.glb".into(),
            mesh: 0,
            materials: vec![Some(m)],
        },
    );
    let log = bake_texture_transforms(&mut w);
    assert_eq!(base_transform(&w, m), Some(T));
    assert_eq!(log.counts["not baked: geometry still in a file"], 1);

    // nothing uses it; and untransformed materials are not touched
    let mut w = World::new();
    let m = material(&mut w, Some(T), None);
    let plain = material(&mut w, None, None);
    let a = prototype(&mut w, vec![primitive(plain, true)]);
    let log = bake_texture_transforms(&mut w);
    assert_eq!(base_transform(&w, m), Some(T));
    assert_eq!(uv(&w, a, 0), UV.to_vec());
    assert_eq!(log.counts["not baked: no primitive uses the material"], 1);
    assert!(!log.counts.contains_key("coordinate sets baked"));
}
