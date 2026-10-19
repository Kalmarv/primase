//! `uv-frame-normals` on hand-worked quads: each case's supplied frame and coordinates are chosen so the map from
//! the stored frame to the derived one is known (identity, a mirror, a quarter turn, frames off every map).
#![allow(clippy::unwrap_used)]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, Mesh, Name, NormalConvention, Primitive, Semantic, Stream, TexFormat, TexRole,
    Texture, Topology, World,
};
use std::collections::BTreeMap;

fn normal_map(world: &mut World, convention: NormalConvention) -> Entity {
    let t = world.spawn();
    world.insert(t, Name("n".into()));
    world.insert(
        t,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![10, 20, 30, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Normal,
            normal: Some(convention),
        },
    );
    t
}

fn material(world: &mut World, texture: Entity) -> Entity {
    let m = world.spawn();
    world.insert(
        m,
        PbrMaterial {
            normal_texture: Some(TextureRef::new(texture)),
            ..Default::default()
        },
    );
    m
}

/// A unit quad on the ground (`size` metres), the given UV corners for (0,0), (1,0), (0,1), (1,1) in x and z, and
/// one supplied tangent at every corner.
fn quad(
    world: &mut World,
    material: Entity,
    size: f32,
    uv: [[f32; 2]; 4],
    tangent: Option<[f32; 4]>,
) -> Entity {
    let mut streams = BTreeMap::from([
        (
            Semantic::Position,
            Stream::F32x3(vec![
                [0., 0., 0.],
                [size, 0., 0.],
                [0., 0., size],
                [size, 0., size],
            ]),
        ),
        (Semantic::Normal, Stream::F32x3(vec![[0., 1., 0.]; 4])),
        (Semantic::TexCoord(0), Stream::F32x2(uv.to_vec())),
    ]);
    if let Some(t) = tangent {
        streams.insert(Semantic::Tangent, Stream::F32x4(vec![t; 4]));
    }
    let e = world.spawn();
    world.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 2, 1, 1, 2, 3]),
            material: Some(material),
        }]),
    );
    e
}

const PLAIN: [[f32; 2]; 4] = [[0., 0.], [1., 0.], [0., 1.], [1., 1.]];

fn run(world: &mut World, pipeline: &str) -> Result<RunReport, String> {
    let mut registry = PassRegistry::new();
    primase_image::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager
        .run(
            world,
            &manager.registry.parse(pipeline).map_err(|e| e.to_string())?,
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn tangents(world: &World, mesh: Entity) -> Vec<[f32; 4]> {
    match world.get::<Mesh>(mesh).unwrap().primitives()[0]
        .streams
        .get(&Semantic::Tangent)
    {
        Some(Stream::F32x4(t)) => t.clone(),
        _ => panic!("no tangents"),
    }
}

#[test]
fn a_frame_that_already_is_the_derived_one_changes_nothing_but_is_restated() {
    // u along +x, v along +z; T = +x, cross(+y, +x) * w = -z * w: w = -1 points the bitangent along +v
    let mut world = World::new();
    let t = normal_map(&mut world, NormalConvention::GLTF);
    let m = material(&mut world, t);
    let mesh = quad(&mut world, m, 1., PLAIN, Some([1., 0., 0., -1.]));
    let report = run(&mut world, "uv-frame-normals{bitangent=+v}").unwrap();
    assert_eq!(world.get::<Texture>(t).unwrap().levels[0], [10, 20, 30, 255]);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["normal maps already in the derived frame"], 1);
    assert!(log.approximations.is_empty(), "{log:?}");
    assert_eq!(tangents(&world, mesh)[0], [1., 0., 0., -1.]);
    // the same frame, with the bitangent the other way along v: green flips, w turns
    let report = run(&mut world, "uv-frame-normals{bitangent=-v}").unwrap();
    assert_eq!(world.get::<Texture>(t).unwrap().levels[0], [10, 235, 30, 255]);
    assert_eq!(tangents(&world, mesh)[0], [1., 0., 0., 1.]);
    assert_eq!(report.steps[0].log.counts.len(), 2, "{:?}", report.steps[0].log);
}

#[test]
fn mirrored_coordinates_negate_red() {
    // u decreasing along +x: the derived tangent is -x, so red, written against +x, is negated
    let mut world = World::new();
    let t = normal_map(&mut world, NormalConvention::GLTF);
    let m = material(&mut world, t);
    let mirrored = [[1., 0.], [0., 0.], [1., 1.], [0., 1.]];
    let mesh = quad(&mut world, m, 1., mirrored, Some([1., 0., 0., -1.]));
    run(&mut world, "uv-frame-normals{bitangent=+v}").unwrap();
    assert_eq!(world.get::<Texture>(t).unwrap().levels[0], [245, 20, 30, 255]);
    // derived T = -x; cross(+y, -x) = +z = +v: w = +1
    assert_eq!(tangents(&world, mesh)[0], [-1., 0., 0., 1.]);
}

#[test]
fn a_shared_map_in_two_frames_gets_a_copy_for_the_smaller() {
    let mut world = World::new();
    let t = normal_map(&mut world, NormalConvention::GLTF);
    let a = material(&mut world, t);
    let b = material(&mut world, t);
    // a: 2 m quad already in the derived frame; b: 1 m quad whose supplied tangent runs along +v (a quarter turn)
    quad(&mut world, a, 2., PLAIN, Some([1., 0., 0., -1.]));
    quad(&mut world, b, 1., PLAIN, Some([0., 0., 1., 1.]));
    let report = run(&mut world, "uv-frame-normals{bitangent=+v}").unwrap();
    // the larger keeps the texture as it is; the smaller is rebound to a re-expressed copy
    assert_eq!(world.get::<Texture>(t).unwrap().levels[0], [10, 20, 30, 255]);
    let copy = world
        .get::<PbrMaterial>(b)
        .unwrap()
        .normal_texture
        .unwrap()
        .texture;
    assert_ne!(copy, t);
    assert_eq!(
        world
            .get::<PbrMaterial>(a)
            .unwrap()
            .normal_texture
            .unwrap()
            .texture,
        t
    );
    assert_eq!(world.get::<Name>(copy).unwrap().0, "n~uv1");
    // b's red was along +v (T) and green along cross(+y, +z) * 1 = +x (+u): swapped
    assert_eq!(world.get::<Texture>(copy).unwrap().levels[0], [20, 10, 30, 255]);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["normal-map copies for materials in another frame"], 1);
    assert!(log.approximations.is_empty(), "{log:?}");
}

#[test]
fn frames_off_every_map_are_counted_by_area() {
    // 20 degrees off u: still nearest the identity, but not in it; the 4 m2 quad is counted
    let mut world = World::new();
    let t = normal_map(&mut world, NormalConvention::GLTF);
    let m = material(&mut world, t);
    let (s, c) = 20f32.to_radians().sin_cos();
    quad(&mut world, m, 2., PLAIN, Some([c, 0., s, -1.]));
    let report = run(&mut world, "uv-frame-normals{bitangent=+v}").unwrap();
    let log = &report.steps[0].log;
    assert_eq!(
        log.approximations["triangle area (square metres) read with its material's frame map but not in it"],
        4
    );
    assert_eq!(log.counts["normal maps already in the derived frame"], 1);
    // an eighth of a turn is near no map: counted as well, the map left as it is
    let mut world = World::new();
    let t = normal_map(&mut world, NormalConvention::GLTF);
    let m = material(&mut world, t);
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let mesh = quad(&mut world, m, 2., PLAIN, Some([h, 0., h, -1.]));
    let report = run(&mut world, "uv-frame-normals{bitangent=+v}").unwrap();
    // the map stays in the supplied frame, and so do the tangents it is read with
    assert_eq!(tangents(&world, mesh)[0], [h, 0., h, -1.]);
    let log = &report.steps[0].log;
    assert_eq!(
        log.approximations["triangle area (square metres) read with its material's frame map but not in it"],
        4
    );
    assert_eq!(
        log.counts["normal-mapped materials with no triangle in a frame: left as they are"],
        1
    );
    assert_eq!(world.get::<Texture>(t).unwrap().levels[0], [10, 20, 30, 255]);
    assert!(
        !log.counts
            .contains_key("primitives given the derived tangent frame")
    );
}

#[test]
fn needs_the_targets_choice_and_gltf_maps_and_leaves_what_has_no_frame() {
    let mut world = World::new();
    let t = normal_map(&mut world, NormalConvention::GLTF);
    let m = material(&mut world, t);
    let mesh = quad(&mut world, m, 1., PLAIN, None);
    assert!(run(&mut world.clone(), "uv-frame-normals").is_err());
    assert!(run(&mut world.clone(), "uv-frame-normals{bitangent=up}").is_err());
    let report = run(&mut world, "uv-frame-normals{bitangent=-v}").unwrap();
    assert_eq!(
        report.steps[0].log.counts["primitives without a tangent frame left as they are"],
        1
    );
    assert_eq!(world.get::<Texture>(t).unwrap().levels[0], [10, 20, 30, 255]);
    assert!(
        !world.get::<Mesh>(mesh).unwrap().primitives()[0]
            .streams
            .contains_key(&Semantic::Tangent)
    );
    // a map in another convention: normalize-normals first
    let mut world = World::new();
    let mut convention = NormalConvention::GLTF;
    convention.green = primase_core::NormalGreen::Down;
    let t = normal_map(&mut world, convention);
    let m = material(&mut world, t);
    quad(&mut world, m, 1., PLAIN, Some([1., 0., 0., -1.]));
    let err = run(&mut world, "uv-frame-normals{bitangent=+v}").unwrap_err();
    assert!(err.contains("normalize-normals"), "{err}");
}
