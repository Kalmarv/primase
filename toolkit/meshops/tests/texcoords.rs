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
/// `T` applied by hand: `uv * [2, 4] + [0.5, -0.25]`.
const T_UV: [[f32; 2]; 3] = [[0.5, -0.25], [2.5, -0.25], [1.0, 3.75]];

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
    w.insert(e, Mesh::new(prims));
    e
}

fn uv(w: &World, e: Entity, i: usize) -> Vec<[f32; 2]> {
    match &w.get::<Mesh>(e).unwrap().primitives()[i].streams[&Semantic::TexCoord(0)] {
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
    for (e, i) in [(a, 0), (b, 0), (b, 1)] {
        assert_eq!(uv(&w, e, i), T_UV);
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

/// Every slot the material has is the material's sampling of the set, the specular extension's two included: baking
/// clears them all, so none is applied a second time.
#[test]
fn a_specular_transform_is_baked_and_cleared_with_the_others() {
    use primase_core::pbr::Specular;
    let mut w = World::new();
    let m = material(&mut w, Some(T), None);
    let tex = w.spawn();
    let transformed = TextureRef {
        transform: Some(T),
        ..TextureRef::new(tex)
    };
    w.get_mut::<PbrMaterial>(m).unwrap().specular = Some(Specular {
        factor: 1.0,
        texture: Some(transformed),
        color: [1.0; 3],
        color_texture: Some(transformed),
    });
    let a = prototype(&mut w, vec![primitive(m, true)]);
    let log = bake_texture_transforms(&mut w);
    assert_eq!(uv(&w, a, 0), T_UV);
    let p = w.get::<PbrMaterial>(m).unwrap();
    assert!(p.textures().all(|r| r.transform.is_none()), "{p:?}");
    assert_eq!(log.counts["slot transforms cleared"], 3);
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
            // T applied by hand to UV[1] = [1, 0]: [1*2 + 0.5, 0*4 - 0.25].
            assert_eq!(uv(&w, a, 0), T_UV);
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

    // a material loaded geometry uses, also a variant of geometry not loaded (its base material other or none)
    for base in [true, false] {
        let mut w = World::new();
        let m = material(&mut w, Some(T), None);
        let n = material(&mut w, None, None);
        let a = prototype(&mut w, vec![primitive(m, true)]);
        let f = w.spawn();
        w.insert(
            f,
            MeshFile {
                path: "f.glb".into(),
                mesh: 0,
                materials: vec![base.then_some(n)],
            },
        );
        w.insert(f, MaterialVariants(vec![BTreeMap::from([(1, m)])]));
        let log = bake_texture_transforms(&mut w);
        assert_eq!(base_transform(&w, m), Some(T), "base {base}");
        assert_eq!(uv(&w, a, 0), UV.to_vec(), "base {base}");
        // with a base material its untransformed slot already disagrees with the variant's; without one, the file
        // geometry is what stops it (counted in slots: one each)
        let (why, slots) = if base {
            ("not baked: materials sample a set with different transforms", 2)
        } else {
            ("not baked: geometry still in a file", 1)
        };
        assert_eq!(log.counts.get(why), Some(&slots), "base {base}: {:?}", log.counts);
    }

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

/// A primitive with coordinate sets 0 and 2 (`UV` and `T_UV`), drawn with `material`.
fn two_sets(material: Entity) -> Primitive {
    let mut p = primitive(material, true);
    p.streams
        .insert(Semantic::TexCoord(2), Stream::F32x2(T_UV.to_vec()));
    p
}

/// A material whose base colour and normal map sample `base` and `normal`.
fn sampling(w: &mut World, base: u8, normal: u8) -> Entity {
    let m = material(w, None, Some(None));
    let p = w.get_mut::<PbrMaterial>(m).unwrap();
    p.base_color_texture.as_mut().unwrap().texcoord = base;
    p.normal_texture.as_mut().unwrap().texcoord = normal;
    m
}

fn sets_of(w: &World, m: Entity) -> Vec<u8> {
    w.get::<PbrMaterial>(m)
        .unwrap()
        .textures()
        .map(|r| r.texcoord)
        .collect()
}

fn set(w: &World, e: Entity, i: usize, n: u8) -> Vec<[f32; 2]> {
    match &w.get::<Mesh>(e).unwrap().primitives()[i].streams[&Semantic::TexCoord(n)] {
        Stream::F32x2(v) => v.clone(),
        _ => panic!("not F32x2"),
    }
}

/// A material and its variant sample set 2 only: in the primitive they share, sets 0 and 2 swap, and both sample
/// set 0; nothing is lost.
#[test]
fn the_one_set_a_group_samples_becomes_its_first() {
    let mut w = World::new();
    let m = sampling(&mut w, 2, 2);
    let v = sampling(&mut w, 2, 2);
    let a = prototype(&mut w, vec![two_sets(m)]);
    w.insert(a, MaterialVariants(vec![BTreeMap::from([(1, v)])]));
    w.insert(
        a,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![Some(m)],
        },
    );
    let log = primase_meshops::first_texcoord_set(&mut w);
    assert_eq!(set(&w, a, 0, 0), T_UV);
    assert_eq!(
        set(&w, a, 0, 2),
        UV.to_vec(),
        "the old first set is kept as set 2"
    );
    assert_eq!(sets_of(&w, m), [0, 0]);
    assert_eq!(sets_of(&w, v), [0, 0]);
    assert!(!w.has::<MeshFile>(a), "changed geometry is no longer the file's");
    assert_eq!(log.counts["primitives with a set moved first"], 1);
    assert_eq!(log.counts["slots sampling the first set"], 4);

    // a primitive with only the sampled set: it becomes set 0
    let mut w = World::new();
    let m = sampling(&mut w, 1, 1);
    let mut p = primitive(m, false);
    p.streams
        .insert(Semantic::TexCoord(1), Stream::F32x2(T_UV.to_vec()));
    let a = prototype(&mut w, vec![p]);
    primase_meshops::first_texcoord_set(&mut w);
    assert_eq!(set(&w, a, 0, 0), T_UV);
    assert!(
        !w.get::<Mesh>(a).unwrap().primitives()[0]
            .streams
            .contains_key(&Semantic::TexCoord(1))
    );
}

#[test]
fn groups_whose_first_set_cannot_change_are_left_and_counted() {
    // already the first set: nothing to do, nothing counted
    let mut w = World::new();
    let m = sampling(&mut w, 0, 0);
    prototype(&mut w, vec![two_sets(m)]);
    let log = primase_meshops::first_texcoord_set(&mut w);
    assert!(log.counts.is_empty(), "{:?}", log.counts);

    // two sets in one material, and two sets across a material and its variant
    for (m_sets, v_sets) in [((0, 2), (0, 2)), ((2, 2), (1, 1))] {
        let mut w = World::new();
        let m = sampling(&mut w, m_sets.0, m_sets.1);
        let v = sampling(&mut w, v_sets.0, v_sets.1);
        let a = prototype(&mut w, vec![two_sets(m)]);
        w.insert(a, MaterialVariants(vec![BTreeMap::from([(1, v)])]));
        let log = primase_meshops::first_texcoord_set(&mut w);
        assert_eq!(set(&w, a, 0, 0), UV.to_vec());
        assert_eq!(sets_of(&w, m), [m_sets.0, m_sets.1]);
        assert_eq!(log.counts["not moved: materials sample several sets"], 4);
    }

    // a primitive without the set
    let mut w = World::new();
    let m = sampling(&mut w, 2, 2);
    prototype(&mut w, vec![two_sets(m), primitive(m, true)]);
    let log = primase_meshops::first_texcoord_set(&mut w);
    assert_eq!(sets_of(&w, m), [2, 2]);
    assert_eq!(log.counts["not moved: a primitive lacks the coordinate set"], 2);

    // a primitive drawn with a material that is not a pbr.material as a variant
    let mut w = World::new();
    let m = sampling(&mut w, 2, 2);
    let blend = w.spawn();
    let a = prototype(&mut w, vec![two_sets(m)]);
    w.insert(a, MaterialVariants(vec![BTreeMap::from([(1, blend)])]));
    let log = primase_meshops::first_texcoord_set(&mut w);
    assert_eq!(sets_of(&w, m), [2, 2]);
    assert_eq!(log.counts["not moved: a material is not a pbr.material"], 2);

    // geometry still in a file
    let mut w = World::new();
    let m = sampling(&mut w, 2, 2);
    let f = w.spawn();
    w.insert(
        f,
        MeshFile {
            path: "f.glb".into(),
            mesh: 0,
            materials: vec![Some(m)],
        },
    );
    let log = primase_meshops::first_texcoord_set(&mut w);
    assert_eq!(sets_of(&w, m), [2, 2]);
    assert_eq!(log.counts["not moved: geometry still in a file"], 2);
}
