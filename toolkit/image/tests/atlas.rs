//! `pack-texture-atlas` through the pass manager: layouts worked by hand, coordinates mapped exactly into rectangles,
//! factors baked, neutral fills, and every refusal.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::Conservation;
use primase_core::pass::{Options, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, MaterialVariants, Mesh, MeshFile, Name, NormalConvention, Primitive, Semantic,
    Stream, TexFormat, TexRole, Texture, TextureFile, Topology, World,
};
use primase_foundations::color::linear_to_srgb8;
use primase_image::{GUTTER, PACK_TEXTURE_ATLAS, PackTextureAtlas};

fn run(w: &mut World, size: u32) -> Result<RunReport, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(PackTextureAtlas::new(size).unwrap())],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

/// A `w` x `h` texture of one colour.
fn solid(w: &mut World, size: (u32, u32), rgba: [u8; 4], role: TexRole) -> Entity {
    let t = w.spawn();
    let (color_space, normal) = match role {
        TexRole::Color => (ColorSpace::Srgb, None),
        TexRole::Normal => (ColorSpace::Linear, Some(NormalConvention::GLTF)),
        _ => (ColorSpace::Linear, None),
    };
    w.insert(
        t,
        Texture {
            width: size.0,
            height: size.1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![rgba.repeat(size.0 as usize * size.1 as usize)],
            color_space,
            role,
            normal,
        },
    );
    t
}

/// A material drawing `base` as its colour texture.
fn coloured(w: &mut World, base: Option<Entity>) -> Entity {
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: base.map(TextureRef::new),
            ..PbrMaterial::default()
        },
    );
    m
}

/// A triangle covering the texture's corners: (0, 0), (1, 0), (0, 1).
fn corner_triangle(material: Entity) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            ),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
            ),
        ]),
        indices: None,
        material: Some(material),
    }
}

fn mesh_with(w: &mut World, materials: &[Entity]) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Mesh::new(materials.iter().map(|&m| corner_triangle(m)).collect()),
    );
    e
}

fn uvs(w: &World, e: Entity, i: usize) -> Vec<[f32; 2]> {
    let Stream::F32x2(uv) = &w.get::<Mesh>(e).unwrap().primitives()[i].streams[&Semantic::TexCoord(0)] else {
        panic!("coordinates are float2");
    };
    uv.clone()
}

fn pixel(t: &Texture, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * t.width as usize + x as usize) * 4;
    t.levels[0][at..at + 4].try_into().unwrap()
}

/// The material mesh `e`'s primitive `i` is drawn with.
fn material_of(w: &World, e: Entity, i: usize) -> (Entity, PbrMaterial) {
    let m = w.get::<Mesh>(e).unwrap().primitives()[i].material.unwrap();
    (m, w.get::<PbrMaterial>(m).unwrap().clone())
}

/// A 4 x 4 red and an 8 x 8 green material. The rectangles, with a gutter of 2: the taller green outer 12 x 12 at
/// (0, 0), its image at (2, 2); the red outer 8 x 8 beside it at x = 12, its image at (14, 2). The extent 20 x 12
/// rounds up to the atlas of 32 x 16.
#[test]
fn two_materials_are_packed_and_their_coordinates_land_exactly_in_their_rectangles() {
    assert_eq!(GUTTER, 2);
    let mut w = World::new();
    let red_t = solid(&mut w, (4, 4), [255, 0, 0, 255], TexRole::Color);
    let green_t = solid(&mut w, (8, 8), [0, 255, 0, 255], TexRole::Color);
    let (red, green) = (coloured(&mut w, Some(red_t)), coloured(&mut w, Some(green_t)));
    let e = mesh_with(&mut w, &[red, green]);
    w.insert(
        e,
        MeshFile {
            path: "m.glb".into(),
            mesh: 0,
            materials: vec![Some(red), Some(green)],
        },
    );
    let report = run(&mut w, 64).unwrap();

    let (atlas_material, m) = material_of(&w, e, 0);
    assert_eq!(material_of(&w, e, 1).0, atlas_material, "one material for both");
    assert!(atlas_material != red && atlas_material != green);
    assert!(!w.has::<MeshFile>(e));
    let r = m.base_color_texture.unwrap();
    assert_eq!((r.wrap, r.texcoord, r.transform), ([Wrap::Clamp; 2], 0, None));
    assert!(m.normal_texture.is_none() && m.metallic_roughness_texture.is_none());
    assert_eq!(m.base_color, [1.0; 4]);
    let t = w.get::<Texture>(r.texture).unwrap();
    assert_eq!((t.width, t.height, t.levels.len()), (32, 16, 1));
    assert_eq!(
        (t.format, t.color_space, t.role),
        (TexFormat::Rgba8Unorm, ColorSpace::Srgb, TexRole::Color)
    );
    // exact coordinates: (x + u w) / 32 and (y + v h) / 16
    assert_eq!(
        uvs(&w, e, 0),
        [
            [14.0 / 32.0, 2.0 / 16.0],
            [18.0 / 32.0, 2.0 / 16.0],
            [14.0 / 32.0, 6.0 / 16.0]
        ]
    );
    assert_eq!(
        uvs(&w, e, 1),
        [
            [2.0 / 32.0, 2.0 / 16.0],
            [10.0 / 32.0, 2.0 / 16.0],
            [2.0 / 32.0, 10.0 / 16.0]
        ]
    );
    // the images, their gutters repeating the edge, the rest of the atlas the neutral white
    assert_eq!(pixel(t, 14, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(t, 17, 5), [255, 0, 0, 255]);
    assert_eq!(
        pixel(t, 12, 0),
        [255, 0, 0, 255],
        "gutter above and left of the red image"
    );
    assert_eq!(pixel(t, 19, 7), [255, 0, 0, 255], "gutter below and right");
    assert_eq!(pixel(t, 2, 2), [0, 255, 0, 255]);
    assert_eq!(pixel(t, 9, 9), [0, 255, 0, 255]);
    assert_eq!(pixel(t, 0, 0), [0, 255, 0, 255]);
    assert_eq!(pixel(t, 31, 15), [255; 4]);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["atlas materials made"], 1);
    assert_eq!(log.counts["materials packed"], 2);
    assert_eq!(log.counts["atlas textures made"], 1);
    assert_eq!(log.counts["atlas of 32 x 16 texels packing 2 materials"], 1);
    assert!(log.approximations.is_empty(), "{:?}", log.approximations);
}

#[test]
fn factors_that_differ_are_baked_and_those_that_agree_stay() {
    let mut w = World::new();
    let white = solid(&mut w, (2, 2), [255; 4], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(white)), coloured(&mut w, Some(white)));
    // `a` halves the colour in linear light; `b` has none and a colour of its own
    w.get_mut::<PbrMaterial>(a).unwrap().base_color = [0.5, 0.5, 0.5, 1.0];
    let c = coloured(&mut w, None);
    w.get_mut::<PbrMaterial>(c).unwrap().base_color = [1.0, 0.0, 0.0, 0.5];
    // metallic and roughness agree everywhere: they stay factors
    for m in [a, b, c] {
        let m = w.get_mut::<PbrMaterial>(m).unwrap();
        (m.metallic, m.roughness) = (0.25, 0.75);
    }
    let e = mesh_with(&mut w, &[a, b, c]);
    let report = run(&mut w, 64).unwrap();
    let (_, m) = material_of(&w, e, 0);
    assert_eq!(
        m.base_color, [1.0; 4],
        "the differing colour factor is in the pixels"
    );
    assert_eq!((m.metallic, m.roughness), (0.25, 0.75));
    assert!(m.metallic_roughness_texture.is_none(), "nothing to atlas there");
    let t = w.get::<Texture>(m.base_color_texture.unwrap().texture).unwrap();
    // rectangles: a and b 2 x 2 (outer 6), c 1 x 1 (outer 5); tallest first: a at (2, 2), b at (8, 2), c at (14, 2)
    let half = linear_to_srgb8(0.5);
    assert_eq!(pixel(t, 2, 2), [half, half, half, 255]);
    assert_eq!(pixel(t, 3, 3), [half, half, half, 255]);
    assert_eq!(
        pixel(t, 8, 2),
        [255; 4],
        "the material with factors of one is unchanged"
    );
    assert_eq!(
        pixel(t, 14, 2),
        [255, 0, 0, 128],
        "a material with no texture is its colour"
    );
    // c's three vertices all sit in the middle of its texel
    let (uw, uh) = (f64::from(t.width), f64::from(t.height));
    let want = [((14.0 + 0.5) / uw) as f32, ((2.0 + 0.5) / uh) as f32];
    assert_eq!(uvs(&w, e, 2), [want; 3]);
    assert_eq!(
        report.steps[0].log.approximations["slots whose differing factors are multiplied into the atlas' 8-bit pixels"],
        1
    );

    // factors the same everywhere stay factors, the texture untouched
    let mut w = World::new();
    let white = solid(&mut w, (2, 2), [10, 20, 30, 255], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(white)), coloured(&mut w, Some(white)));
    for m in [a, b] {
        w.get_mut::<PbrMaterial>(m).unwrap().base_color = [0.8, 0.8, 0.8, 1.0];
    }
    let e = mesh_with(&mut w, &[a, b]);
    run(&mut w, 64).unwrap();
    let (_, m) = material_of(&w, e, 0);
    assert_eq!(m.base_color, [0.8, 0.8, 0.8, 1.0]);
    let t = w.get::<Texture>(m.base_color_texture.unwrap().texture).unwrap();
    assert_eq!(pixel(t, 2, 2), [10, 20, 30, 255]);
}

#[test]
fn a_slot_a_material_lacks_is_filled_with_its_neutral_value() {
    let mut w = World::new();
    let base = solid(&mut w, (4, 4), [10, 20, 30, 255], TexRole::Color);
    let normal = solid(&mut w, (4, 4), [200, 100, 200, 255], TexRole::Normal);
    let with = w.spawn();
    w.insert(
        with,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(base)),
            normal_texture: Some(TextureRef::new(normal)),
            ..PbrMaterial::default()
        },
    );
    let without = coloured(&mut w, None);
    let e = mesh_with(&mut w, &[with, without]);
    let report = run(&mut w, 64).unwrap();
    let (_, m) = material_of(&w, e, 0);
    let n = w.get::<Texture>(m.normal_texture.unwrap().texture).unwrap();
    assert_eq!(
        (n.role, n.color_space, n.normal),
        (TexRole::Normal, ColorSpace::Linear, Some(NormalConvention::GLTF))
    );
    // `with` (4 x 4, outer 8) at x = 0 then `without` (1 x 1, outer 5) at x = 8: its image at (10, 2)
    assert_eq!(pixel(n, 2, 2), [200, 100, 200, 255]);
    assert_eq!(pixel(n, 10, 2), [128, 128, 255, 255], "the flat normal");
    let b = w.get::<Texture>(m.base_color_texture.unwrap().texture).unwrap();
    assert_eq!(pixel(b, 10, 2), [255; 4], "white");
    assert_eq!(
        report.steps[0].log.counts["rectangles filled with the slot's neutral value"],
        2
    );
    // `without` has no texture at all: its rectangle is one texel and its vertices sit in its middle
    let (uw, uh) = (f64::from(n.width), f64::from(n.height));
    assert_eq!(
        uvs(&w, e, 1)[0],
        [((10.0 + 0.5) / uw) as f32, ((2.0 + 0.5) / uh) as f32]
    );
}

#[test]
fn a_set_of_materials_too_large_is_scaled_down_and_each_scale_reported() {
    let mut w = World::new();
    let big = solid(&mut w, (16, 16), [255, 0, 0, 255], TexRole::Color);
    let small = solid(&mut w, (8, 8), [0, 0, 255, 255], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(big)), coloured(&mut w, Some(small)));
    w.insert(a, Name("big".into()));
    w.insert(b, Name("small".into()));
    let e = mesh_with(&mut w, &[a, b]);
    // 16 x 16 does not fit 16 (outer 20); halved to 8 x 8 it and the other still do not; the larger is halved first
    // (the earlier on a tie), then the other: 4 x 4 each, outer 8, side by side
    let report = run(&mut w, 16).unwrap();
    let log = &report.steps[0].log;
    let scaled: Vec<&String> = log.counts.keys().filter(|k| k.contains("scaled from")).collect();
    assert_eq!(scaled.len(), 2, "{scaled:?}");
    assert!(
        log.counts
            .keys()
            .any(|k| k.contains("(big)") && k.contains("from 16 x 16 to 4 x 4"))
    );
    assert!(
        log.counts
            .keys()
            .any(|k| k.contains("(small)") && k.contains("from 8 x 8 to 4 x 4"))
    );
    assert_eq!(
        log.approximations["materials whose textures are scaled down to fit their atlas"],
        2
    );
    let (_, m) = material_of(&w, e, 0);
    let t = w.get::<Texture>(m.base_color_texture.unwrap().texture).unwrap();
    assert_eq!((t.width, t.height), (16, 8));
    // the box average of a solid colour is the colour
    assert_eq!(pixel(t, 2, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(t, 10, 2), [0, 0, 255, 255]);
    assert_eq!(uvs(&w, e, 1)[1], [(10.0 + 4.0) / 16.0, 2.0 / 8.0]);
}

#[test]
fn a_box_filter_averages_whole_source_texels() {
    let mut w = World::new();
    // 16 x 4, columns alternating black and white; halved to fit 16 it is 8 x 2, each texel the mean of 2 x 2:
    // (0 + 255 + 0 + 255 + 2) / 4 = 128 (half up)
    let stripes = w.spawn();
    let mut texels = Vec::new();
    for _ in 0..4 {
        for x in 0..16 {
            texels.extend([if x % 2 == 0 { 0 } else { 255 }, 0, 0, 255]);
        }
    }
    w.insert(
        stripes,
        Texture {
            width: 16,
            height: 4,
            format: TexFormat::Rgba8Unorm,
            levels: vec![texels],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let a = coloured(&mut w, Some(stripes));
    let b = coloured(&mut w, None);
    let e = mesh_with(&mut w, &[a, b]);
    run(&mut w, 16).unwrap();
    let (_, m) = material_of(&w, e, 0);
    let t = w.get::<Texture>(m.base_color_texture.unwrap().texture).unwrap();
    assert_eq!(pixel(t, 2, 2), [128, 0, 0, 255]);
    assert_eq!(pixel(t, 9, 3), [128, 0, 0, 255]);
}

#[test]
fn tiling_coordinates_are_refused_unless_the_texture_clamps() {
    let build = |wrap: [Wrap; 2]| {
        let mut w = World::new();
        let t = solid(&mut w, (4, 4), [9, 9, 9, 255], TexRole::Color);
        let a = coloured(&mut w, Some(t));
        w.get_mut::<PbrMaterial>(a)
            .unwrap()
            .base_color_texture
            .as_mut()
            .unwrap()
            .wrap = wrap;
        w.insert(a, Name("bricks".into()));
        let t2 = solid(&mut w, (4, 4), [9, 9, 9, 255], TexRole::Color);
        let b = coloured(&mut w, Some(t2));
        let e = mesh_with(&mut w, &[a, b]);
        let mut mesh = w.get_mut::<Mesh>(e).unwrap().primitive_mut(0).unwrap();
        let Some(Stream::F32x2(uv)) = mesh.streams_mut().get_mut(&Semantic::TexCoord(0)) else {
            panic!()
        };
        uv[1] = [2.0, 0.5];
        (w, e)
    };
    let (mut w, _) = build([Wrap::Repeat; 2]);
    let err = run(&mut w, 64).unwrap_err();
    assert!(
        err.contains("bricks") && err.contains("outside 0-1") && err.contains("[2.0, 0.5]"),
        "{err}"
    );
    // clamped on u only: v is fine here, u is out of range and clamps
    let (mut w, e) = build([Wrap::Clamp, Wrap::Repeat]);
    let report = run(&mut w, 64).unwrap();
    assert_eq!(
        report.steps[0].log.approximations["vertex coordinates outside 0-1, clamped as their texture clamps"],
        1
    );
    // clamped to u = 1: the right edge of the first rectangle, at x = 2 + 4 of an atlas 16 texels wide; v untouched
    assert_eq!(uvs(&w, e, 0)[1], [6.0 / 16.0, (2.0 + 0.5 * 4.0) / 8.0]);
    // clamped, a coordinate outside 0-1 on the repeating axis is still refused
    let (mut w, _) = build([Wrap::Repeat, Wrap::Clamp]);
    assert!(run(&mut w, 64).is_err());
}

#[test]
fn what_one_material_cannot_hold_is_refused_naming_it() {
    let two = |tweak: &dyn Fn(&mut World, Entity, Entity)| {
        let mut w = World::new();
        let t = solid(&mut w, (4, 4), [9, 9, 9, 255], TexRole::Color);
        let (a, b) = (coloured(&mut w, Some(t)), coloured(&mut w, Some(t)));
        tweak(&mut w, a, b);
        mesh_with(&mut w, &[a, b]);
        run(&mut w, 64).unwrap_err()
    };
    let err = two(&|w, _, b| w.get_mut::<PbrMaterial>(b).unwrap().alpha_mode = AlphaMode::Blend);
    assert!(
        err.contains("alpha mode") && err.contains("Opaque") && err.contains("Blend"),
        "{err}"
    );
    let err =
        two(&|w, a, _| w.get_mut::<PbrMaterial>(a).unwrap().alpha_mode = AlphaMode::Mask { cutoff: 0.5 });
    assert!(err.contains("alpha mode") && err.contains("Mask"), "{err}");
    let err = two(&|w, _, b| w.get_mut::<PbrMaterial>(b).unwrap().double_sided = true);
    assert!(err.contains("double sidedness"), "{err}");
    let err = two(&|w, _, b| w.get_mut::<PbrMaterial>(b).unwrap().emissive_strength = 2.0);
    assert!(err.contains("emissive strength"), "{err}");
    let err = two(&|w, _, b| {
        let m = w.get_mut::<PbrMaterial>(b).unwrap();
        m.base_color_texture.as_mut().unwrap().texcoord = 1;
    });
    assert!(err.contains("coordinate set 1"), "{err}");
    let err = two(&|w, _, b| {
        let m = w.get_mut::<PbrMaterial>(b).unwrap();
        m.base_color_texture.as_mut().unwrap().transform = Some(UvTransform {
            scale: [2.0, 2.0],
            ..UvTransform::IDENTITY
        });
    });
    assert!(err.contains("bake-texture-transforms"), "{err}");
    let err = two(&|w, _, b| {
        let t = w
            .get::<PbrMaterial>(b)
            .unwrap()
            .base_color_texture
            .unwrap()
            .texture;
        w.get_mut::<Texture>(t).unwrap().color_space = ColorSpace::Linear;
        let t2 = solid(w, (4, 4), [9; 4], TexRole::Color);
        w.get_mut::<PbrMaterial>(b).unwrap().base_color_texture = Some(TextureRef::new(t2));
    });
    assert!(err.contains("colour space"), "{err}");
    // material variants
    let mut w = World::new();
    let t = solid(&mut w, (4, 4), [9, 9, 9, 255], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(t)), coloured(&mut w, Some(t)));
    let e = mesh_with(&mut w, &[a, b]);
    w.insert(
        e,
        MaterialVariants(vec![BTreeMap::from([(1, b)]), BTreeMap::new()]),
    );
    let err = run(&mut w, 64).unwrap_err();
    assert!(err.contains("specialize-material-variants"), "{err}");
}

#[test]
fn textures_not_loaded_or_not_exact_in_rgba8_are_refused_naming_the_pass_to_run() {
    let mut w = World::new();
    let file = w.spawn();
    w.insert(file, TextureFile { path: "a.png".into() });
    let t = solid(&mut w, (4, 4), [9, 9, 9, 255], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(file)), coloured(&mut w, Some(t)));
    mesh_with(&mut w, &[a, b]);
    assert!(run(&mut w, 64).unwrap_err().contains("load-textures"));

    let mut w = World::new();
    let half = w.spawn();
    w.insert(
        half,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba16Float,
            levels: vec![vec![0; 8]],
            color_space: ColorSpace::Linear,
            role: TexRole::Color,
            normal: None,
        },
    );
    let t = solid(&mut w, (4, 4), [9, 9, 9, 255], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(half)), coloured(&mut w, Some(t)));
    mesh_with(&mut w, &[a, b]);
    assert!(run(&mut w, 64).unwrap_err().contains("lower-textures-rgba8"));
}

#[test]
fn meshes_with_one_material_are_untouched_and_equal_sets_share_one_atlas() {
    let mut w = World::new();
    let t1 = solid(&mut w, (4, 4), [1, 1, 1, 255], TexRole::Color);
    let t2 = solid(&mut w, (4, 4), [2, 2, 2, 255], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(t1)), coloured(&mut w, Some(t2)));
    let single = mesh_with(&mut w, &[a, a]);
    let first = mesh_with(&mut w, &[a, b]);
    let second = mesh_with(&mut w, &[b, a, b]);
    let before = w.get::<Mesh>(single).unwrap().clone();
    let textures = w.iter::<Texture>().count();
    let report = run(&mut w, 64).unwrap();
    assert_eq!(w.get::<Mesh>(single).unwrap(), &before);
    assert_eq!(material_of(&w, first, 0).0, material_of(&w, second, 0).0);
    assert_eq!(w.iter::<Texture>().count(), textures + 1, "one atlas texture");
    let log = &report.steps[0].log;
    assert_eq!(
        (
            log.counts["atlas materials made"],
            log.counts["meshes drawn with an atlas material"]
        ),
        (1, 2)
    );
    // the second mesh's a is at the same rectangle as the first's
    assert_eq!(uvs(&w, second, 1), uvs(&w, first, 0));
}

#[test]
fn slot_atlases_with_the_same_pixels_share_one_texture() {
    let mut w = World::new();
    let orm = solid(&mut w, (4, 4), [200, 100, 50, 255], TexRole::Mask);
    let other = solid(&mut w, (4, 4), [10, 20, 30, 255], TexRole::Mask);
    let pack = |w: &mut World, t: Entity| {
        let m = w.spawn();
        w.insert(
            m,
            PbrMaterial {
                metallic_roughness_texture: Some(TextureRef::new(t)),
                occlusion_texture: Some(TextureRef::new(t)),
                ..PbrMaterial::default()
            },
        );
        m
    };
    let (a, b) = (pack(&mut w, orm), pack(&mut w, other));
    let e = mesh_with(&mut w, &[a, b]);
    let report = run(&mut w, 64).unwrap();
    let (_, m) = material_of(&w, e, 0);
    assert_eq!(
        m.metallic_roughness_texture.unwrap().texture,
        m.occlusion_texture.unwrap().texture
    );
    assert_eq!(report.steps[0].log.counts["atlas textures made"], 1);
}

#[test]
fn the_size_is_required_and_checked() {
    let ok = Options::from([("size".to_owned(), "2048".to_owned())]);
    assert_eq!(
        PACK_TEXTURE_ATLAS.instance(&ok).unwrap().options()["size"],
        "2048"
    );
    assert!(
        PACK_TEXTURE_ATLAS
            .instance(&Options::new())
            .err()
            .unwrap()
            .contains("size is required")
    );
    for v in ["0", "8", "x", "99999"] {
        let o = Options::from([("size".to_owned(), v.to_owned())]);
        assert!(PACK_TEXTURE_ATLAS.instance(&o).is_err(), "{v}");
    }
    let o = Options::from([
        ("size".to_owned(), "64".to_owned()),
        ("max".to_owned(), "1".to_owned()),
    ]);
    assert!(
        PACK_TEXTURE_ATLAS
            .instance(&o)
            .err()
            .unwrap()
            .contains("unknown option max")
    );
    let mut registry = PassRegistry::new();
    primase_image::register(&mut registry);
    assert!(registry.requires_options("pack-texture-atlas"));
}

/// The two passes together: the atlas gives a mesh one material, and grouping then joins its primitives.
#[test]
fn grouping_after_the_atlas_joins_a_mesh_drawn_with_several_materials_into_one_primitive() {
    let mut w = World::new();
    let t1 = solid(&mut w, (4, 4), [1, 1, 1, 255], TexRole::Color);
    let t2 = solid(&mut w, (4, 4), [2, 2, 2, 255], TexRole::Color);
    let (a, b) = (coloured(&mut w, Some(t1)), coloured(&mut w, Some(t2)));
    let e = mesh_with(&mut w, &[a, b, a]);
    let registry = || {
        let mut registry = PassRegistry::new();
        primase_image::register(&mut registry);
        primase_meshops::register(&mut registry);
        registry
    };
    let grouping = registry()
        .parse("group-primitives{max-groups=1,max-vertices=100}")
        .unwrap();
    let both = registry()
        .parse("pack-texture-atlas{size=64},group-primitives{max-groups=1,max-vertices=100}")
        .unwrap();
    let manager = PassManager::new(registry())
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce);
    // before the atlas: refused, naming the pass that gives the materials one
    let err = manager
        .run(&mut w.clone(), &grouping, &mut RunReport::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("pack-texture-atlas"), "{err}");
    manager.run(&mut w, &both, &mut RunReport::default()).unwrap();
    let prims = w.get::<Mesh>(e).unwrap().primitives();
    assert_eq!(prims.len(), 1);
    assert_eq!(prims[0].vertex_count(), 9);
    assert_eq!(
        prims[0].indices.as_deref(),
        Some(&[0, 1, 2, 3, 4, 5, 6, 7, 8][..])
    );
}

/// The core facts the materials state alike are carried to the atlas material: two decal materials that ignore the
/// vertex colour give a decal material that ignores it (not an opaque, depth-writing one taking `COLOR_0`); materials
/// that differ in one, or a layered one, are refused naming it.
#[test]
fn the_core_facts_the_materials_share_are_carried_and_those_that_differ_refused() {
    use primase_core::layered::{LayerWeights, LayeredMaterial, WeightMapping};
    use primase_core::{DrawLayer, DrawOrder, MaterialUsage, VertexColor};
    let decal = DrawOrder::conventional(DrawLayer::Decal, 2500);
    let two = |tweak: &dyn Fn(&mut World, Entity, Entity)| {
        let mut w = World::new();
        let (r, g) = (
            solid(&mut w, (4, 4), [255, 0, 0, 255], TexRole::Color),
            solid(&mut w, (4, 4), [0, 255, 0, 255], TexRole::Color),
        );
        let (a, b) = (coloured(&mut w, Some(r)), coloured(&mut w, Some(g)));
        for m in [a, b] {
            w.insert(m, decal);
            w.insert(m, VertexColor::Ignored);
            w.insert(m, MaterialUsage::Foliage);
        }
        tweak(&mut w, a, b);
        let e = mesh_with(&mut w, &[a, b]);
        (run(&mut w, 64), w, e)
    };
    let (report, w, e) = two(&|_, _, _| {});
    report.unwrap();
    let (m, _) = material_of(&w, e, 0);
    assert_eq!(material_of(&w, e, 1).0, m, "one atlas material");
    assert_eq!(w.get::<DrawOrder>(m), Some(&decal));
    assert!(!w.get::<DrawOrder>(m).unwrap().depth_write);
    assert_eq!(w.get::<VertexColor>(m), Some(&VertexColor::Ignored));
    assert_eq!(w.get::<MaterialUsage>(m), Some(&MaterialUsage::Foliage));

    let err = two(&|w, _, b| {
        w.insert(b, DrawOrder::conventional(DrawLayer::Opaque, 2000));
    })
    .0
    .unwrap_err();
    assert!(err.contains("draw order") && err.contains("Decal"), "{err}");
    let err = two(&|w, _, b| {
        w.remove::<VertexColor>(b);
    })
    .0
    .unwrap_err();
    assert!(err.contains("vertex colour"), "{err}");
    let err = two(&|w, _, b| {
        w.insert(b, MaterialUsage::Water);
    })
    .0
    .unwrap_err();
    assert!(err.contains("usage"), "{err}");
    let err = two(&|w, a, _| {
        w.insert(
            a,
            LayeredMaterial {
                layers: Vec::new(),
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
            },
        );
    })
    .0
    .unwrap_err();
    assert!(err.contains("layered"), "{err}");

    // no draw order stated: none is carried (the atlas material draws by its alpha mode, as they did)
    let (report, w, e) = two(&|w, a, b| {
        w.remove::<DrawOrder>(a);
        w.remove::<DrawOrder>(b);
    });
    report.unwrap();
    assert!(!w.has::<DrawOrder>(material_of(&w, e, 0).0));
}
