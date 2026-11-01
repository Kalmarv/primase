//! `rebase-texcoord-set` through the pass manager: a quarter-turned parameterisation worked by hand (every texel of every
//! slot, the normal map turned between the two frames), a constant emission folded, overlapping coordinates split into
//! groups, and the refusals.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::Conservation;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, MaterialVariants, Mesh, NormalConvention, Primitive, Semantic, Stream, TexFormat,
    TexRole, Texture, Topology, World,
};
use primase_foundations::color::srgb8_to_linear;
use primase_image::{MAX_GROUPS, RebaseTexcoordSet};

fn run(w: &mut World) -> Result<RunReport, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(RebaseTexcoordSet::new(4, 256).unwrap())],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn count(report: &RunReport, what: &str) -> u64 {
    report.steps.iter().filter_map(|s| s.log.counts.get(what)).sum()
}

fn texture(w: &mut World, size: (u32, u32), pixels: &[[u8; 4]], role: TexRole) -> Entity {
    assert_eq!(pixels.len(), (size.0 * size.1) as usize);
    let (color_space, normal) = match role {
        TexRole::Color => (ColorSpace::Srgb, None),
        TexRole::Normal => (ColorSpace::Linear, Some(NormalConvention::GLTF)),
        _ => (ColorSpace::Linear, None),
    };
    let t = w.spawn();
    w.insert(
        t,
        Texture {
            width: size.0,
            height: size.1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![pixels.concat()],
            color_space,
            role,
            normal,
        },
    );
    t
}

fn pixel(t: &Texture, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * t.width as usize + x as usize) * 4;
    t.levels[0][at..at + 4].try_into().unwrap()
}

/// A 4 x 4 colour map: red grows with x, green with y.
fn gradient(w: &mut World) -> Entity {
    let px: Vec<[u8; 4]> = (0..16u8).map(|i| [(i % 4) * 16, (i / 4) * 16, 0, 255]).collect();
    texture(w, (4, 4), &px, TexRole::Color)
}

/// A 2 x 2 emission map of four different colours.
fn glow(w: &mut World) -> Entity {
    texture(
        w,
        (2, 2),
        &[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ],
        TexRole::Color,
    )
}

/// `r` sampling `set`, clamped (so the edge texels of a small map are what a read past its centres gives).
fn set_of(r: TextureRef, set: u8) -> TextureRef {
    TextureRef {
        texcoord: set,
        wrap: [primase_core::pbr::Wrap::Clamp; 2],
        ..r
    }
}

/// The unit square on the ground (x, z), two triangles; set 0 is (x, z), set 4 the same turned a quarter: (z, 1 - x).
fn square(material: Entity, tangents: bool) -> Primitive {
    let positions = vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.], [1., 0., 1.]];
    let uv0: Vec<[f32; 2]> = positions.iter().map(|p| [p[0], p[2]]).collect();
    let uv4: Vec<[f32; 2]> = positions.iter().map(|p| [p[2], 1. - p[0]]).collect();
    let mut streams = BTreeMap::from([
        (Semantic::Position, Stream::F32x3(positions)),
        (Semantic::TexCoord(0), Stream::F32x2(uv0)),
        (Semantic::TexCoord(4), Stream::F32x2(uv4)),
    ]);
    if tangents {
        streams.insert(Semantic::Tangent(0), Stream::F32x4(vec![[1., 0., 0., 1.]; 4]));
    }
    Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(vec![0, 1, 2, 1, 3, 2]),
        material: Some(material),
    }
}

fn mesh_of(w: &mut World, primitives: Vec<Primitive>) -> Entity {
    let e = w.spawn();
    w.insert(e, Mesh::new(primitives));
    e
}

fn coordinates(w: &World, e: Entity, i: usize, set: u8) -> Vec<[f32; 2]> {
    let Stream::F32x2(uv) = &w.get::<Mesh>(e).unwrap().primitives()[i].streams[&Semantic::TexCoord(set)]
    else {
        panic!("float2 coordinates");
    };
    uv.clone()
}

fn material_of(w: &World, e: Entity, i: usize) -> (Entity, PbrMaterial) {
    let m = w.get::<Mesh>(e).unwrap().primitives()[i].material.unwrap();
    (m, w.get::<PbrMaterial>(m).unwrap().clone())
}

fn texture_of(w: &World, r: TextureRef) -> Texture {
    w.get::<Texture>(r.texture).unwrap().clone()
}

/// The square of the ground drawn with a colour map and a normal map on set 0 and an emission map on set 4. Set 4 is
/// set 0 turned a quarter, so: output texel (X, Y) of the atlas, centred at ((X + .5) / 4, (Y + .5) / 4) in set 4, is
/// at (x, z) = (1 - (Y + .5) / 4, (X + .5) / 4) in set 0, which reads the 4 x 4 colour map at texel (3 - Y, X).
/// A normal map's first channel runs along u, its second along -v (green up): on set 0 along +x and -z, on set 4
/// along +z and +x. A vector along the colour map's first channel (+x) is along the atlas' second (+x again), and one
/// along -z is along -(first) = -z.
#[test]
fn a_quarter_turned_set_is_drawn_texel_for_texel_and_the_normal_map_is_turned_with_it() {
    let mut w = World::new();
    let base = gradient(&mut w);
    // x = +1, y = +0.004 (green 128), Z stored
    let normal = texture(&mut w, (2, 2), &[[255, 128, 255, 255]; 4], TexRole::Normal);
    let glow = glow(&mut w);
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(base)),
            normal_texture: Some(TextureRef::new(normal)),
            emissive_texture: Some(set_of(TextureRef::new(glow), 4)),
            emissive: [1.0; 3],
            ..PbrMaterial::default()
        },
    );
    let e = mesh_of(&mut w, vec![square(m, false)]);
    let report = run(&mut w).unwrap();

    let (made, material) = material_of(&w, e, 0);
    assert_ne!(made, m, "the primitive is drawn with a copy of the material");
    assert_eq!(
        w.get::<PbrMaterial>(m)
            .unwrap()
            .base_color_texture
            .unwrap()
            .texture,
        base
    );
    // the coordinates: set 4 over its window (the unit square), as the first set
    let mesh_uv = coordinates(&w, e, 0, 0);
    assert_eq!(mesh_uv, vec![[0., 1.], [0., 0.], [1., 1.], [1., 0.]]);
    assert_eq!(coordinates(&w, e, 0, 4), mesh_uv, "set 4 is as it was");
    // every slot: set 0, no transform, clamped, 4 x 4
    for r in [
        material.base_color_texture.unwrap(),
        material.normal_texture.unwrap(),
        material.emissive_texture.unwrap(),
    ] {
        assert_eq!((r.texcoord, r.transform), (0, None));
        assert_eq!(r.wrap, [primase_core::pbr::Wrap::Clamp; 2]);
        let t = texture_of(&w, r);
        assert_eq!((t.width, t.height), (4, 4));
    }
    let atlas = texture_of(&w, material.base_color_texture.unwrap());
    let source = texture_of(&w, TextureRef::new(base));
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(
                pixel(&atlas, x, y),
                pixel(&source, 3 - y, x),
                "colour texel {x},{y}"
            );
        }
    }
    assert_eq!(
        (atlas.color_space, atlas.role),
        (ColorSpace::Srgb, TexRole::Color)
    );
    // (+1, +0.004) along (+x, -z) is (-0.004, +1) along (+z, +x): bytes 127 and 255
    let turned = texture_of(&w, material.normal_texture.unwrap());
    assert_eq!(turned.role, TexRole::Normal);
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(pixel(&turned, x, y), [127, 255, 255, 255], "normal texel {x},{y}");
        }
    }
    // the emission on set 4 itself: the 2 x 2 map bilinear at the atlas' texel centres, clamped
    let lit = texture_of(&w, material.emissive_texture.unwrap());
    assert_eq!(pixel(&lit, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&lit, 3, 0), [0, 255, 0, 255]);
    assert_eq!(pixel(&lit, 0, 3), [0, 0, 255, 255]);
    assert_eq!(pixel(&lit, 3, 3), [255, 255, 255, 255]);
    assert_eq!(count(&report, "primitives rebased"), 1);
    assert_eq!(count(&report, "atlas textures made"), 3);
    assert_eq!(count(&report, "triangles rebased"), 2);
    assert_eq!(count(&report, "atlas texels"), 3 * 16);
}

/// With supplied tangents the frame is the supplied one whatever the coordinates say, so the map's texels are only
/// moved.
#[test]
fn a_primitive_with_tangents_keeps_its_normal_texels() {
    let mut w = World::new();
    let base = gradient(&mut w);
    let normal = texture(&mut w, (2, 2), &[[255, 128, 255, 255]; 4], TexRole::Normal);
    let glow = glow(&mut w);
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(base)),
            normal_texture: Some(TextureRef::new(normal)),
            emissive_texture: Some(set_of(TextureRef::new(glow), 4)),
            ..PbrMaterial::default()
        },
    );
    let e = mesh_of(&mut w, vec![square(m, true)]);
    run(&mut w).unwrap();
    let (_, material) = material_of(&w, e, 0);
    let kept = texture_of(&w, material.normal_texture.unwrap());
    assert_eq!(pixel(&kept, 1, 2), [255, 128, 255, 255]);
}

/// A texture that holds one colour wherever set 4 reads it is a factor: no rebase, the coordinates stay.
#[test]
fn a_constant_emission_is_folded_into_the_factor() {
    let mut w = World::new();
    let base = gradient(&mut w);
    let orange = texture(&mut w, (4, 4), &[[255, 128, 0, 255]; 16], TexRole::Color);
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(base)),
            emissive_texture: Some(set_of(TextureRef::new(orange), 4)),
            emissive: [1.0, 1.0, 0.5],
            ..PbrMaterial::default()
        },
    );
    let e = mesh_of(&mut w, vec![square(m, false)]);
    let before = coordinates(&w, e, 0, 0);
    let report = run(&mut w).unwrap();
    let (made, material) = material_of(&w, e, 0);
    assert_ne!(made, m);
    assert_eq!(material.emissive_texture, None);
    let expected = [
        srgb8_to_linear(255) as f32,
        srgb8_to_linear(128) as f32,
        0.5 * srgb8_to_linear(0) as f32,
    ];
    for (got, want) in material.emissive.iter().zip(expected) {
        assert!(
            (got - want).abs() < 1e-6,
            "{:?} against {expected:?}",
            material.emissive
        );
    }
    assert_eq!(
        material.base_color_texture,
        w.get::<PbrMaterial>(m).unwrap().base_color_texture
    );
    assert_eq!(coordinates(&w, e, 0, 0), before);
    assert_eq!(
        count(&report, "primitives with their emission folded into a factor"),
        1
    );
    assert_eq!(count(&report, "primitives rebased"), 0);
}

/// `n` triangles at different places with the same coordinates on set 4.
fn stacked(material: Entity, n: usize) -> Primitive {
    let mut positions = Vec::new();
    let mut uv0 = Vec::new();
    let mut uv4 = Vec::new();
    for k in 0..n {
        let z = k as f32 * 2.0;
        positions.extend([[0., 0., z], [1., 0., z], [0., 0., z + 1.]]);
        uv0.extend([[0., 0.], [1., 0.], [0., 1.]]);
        uv4.extend([[0., 0.], [1., 0.], [0., 1.]]);
    }
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (Semantic::Position, Stream::F32x3(positions)),
            (Semantic::TexCoord(0), Stream::F32x2(uv0)),
            (Semantic::TexCoord(4), Stream::F32x2(uv4)),
        ]),
        indices: None,
        material: Some(material),
    }
}

fn two_set_material(w: &mut World) -> Entity {
    let base = gradient(w);
    let glow = glow(w);
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(base)),
            emissive_texture: Some(set_of(TextureRef::new(glow), 4)),
            ..PbrMaterial::default()
        },
    );
    m
}

/// Triangles that overlap in set 4 are put in groups that do not, each its own primitive with an atlas of its own.
#[test]
fn overlapping_coordinates_are_split_into_groups_of_triangles_that_do_not_overlap() {
    let mut w = World::new();
    let m = two_set_material(&mut w);
    let e = mesh_of(&mut w, vec![stacked(m, 3)]);
    let report = run(&mut w).unwrap();
    let mesh = w.get::<Mesh>(e).unwrap();
    assert_eq!(mesh.primitives().len(), 3, "one primitive a group");
    let mut materials = Vec::new();
    for p in mesh.primitives() {
        assert_eq!(p.indices.as_ref().unwrap().len(), 3);
        assert_eq!(p.vertex_count(), 3);
        materials.push(p.material.unwrap());
    }
    materials.sort();
    materials.dedup();
    assert_eq!(materials.len(), 3, "a material (and atlases) a group");
    assert!(!materials.contains(&m));
    assert_eq!(
        count(
            &report,
            "primitives split into groups of triangles that do not overlap"
        ),
        1
    );
    assert_eq!(count(&report, "primitives made by those splits"), 3);
    assert_eq!(count(&report, "triangles rebased"), 3);
}

#[test]
fn a_primitive_needing_more_than_the_most_groups_is_left_as_it_is() {
    let mut w = World::new();
    let m = two_set_material(&mut w);
    let e = mesh_of(&mut w, vec![stacked(m, MAX_GROUPS + 1)]);
    let before = w.get::<Mesh>(e).unwrap().primitives().to_vec();
    let report = run(&mut w).unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives(), &before[..]);
    assert_eq!(
        count(
            &report,
            &format!(
                "primitives not rebased: the coordinates overlap beyond {MAX_GROUPS} groups of triangles"
            )
        ),
        1
    );
    assert_eq!(
        count(
            &report,
            &format!(
                "triangles not rebased: the coordinates overlap beyond {MAX_GROUPS} groups of triangles"
            )
        ),
        (MAX_GROUPS as u64) + 1
    );
}

#[test]
fn primitives_it_cannot_rebase_are_left_as_they_are_and_counted() {
    // no coordinates on set 4
    let mut w = World::new();
    let m = two_set_material(&mut w);
    let mut bare = square(m, false);
    bare.streams.remove(&Semantic::TexCoord(4));
    let e = mesh_of(&mut w, vec![bare.clone()]);
    let report = run(&mut w).unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives(), &[bare][..]);
    assert_eq!(
        count(
            &report,
            "primitives not rebased: the primitive is not a triangle list with positions and both coordinate sets"
        ),
        1
    );
    // material variants
    let mut w = World::new();
    let m = two_set_material(&mut w);
    let other = two_set_material(&mut w);
    let e = mesh_of(&mut w, vec![square(m, false)]);
    w.insert(e, MaterialVariants(vec![BTreeMap::from([(1, other)])]));
    let report = run(&mut w).unwrap();
    assert_eq!(
        count(
            &report,
            "primitives not rebased: the primitive has material variants"
        ),
        1
    );
    assert_eq!(material_of(&w, e, 0).0, m);
    // a texture that is not loaded as pixels
    let mut w = World::new();
    let m = two_set_material(&mut w);
    let glow = w.get::<PbrMaterial>(m).unwrap().emissive_texture.unwrap().texture;
    w.remove::<Texture>(glow);
    let e = mesh_of(&mut w, vec![square(m, false)]);
    let report = run(&mut w).unwrap();
    assert_eq!(
        count(
            &report,
            "primitives not rebased: a texture is not loaded as pixels"
        ),
        1
    );
    assert_eq!(material_of(&w, e, 0).0, m);
    // a material that samples one set draws as it is, uncounted
    let mut w = World::new();
    let base = gradient(&mut w);
    let one = w.spawn();
    w.insert(
        one,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(base)),
            ..PbrMaterial::default()
        },
    );
    let e = mesh_of(&mut w, vec![square(one, false)]);
    let report = run(&mut w).unwrap();
    assert_eq!(material_of(&w, e, 0).0, one);
    assert!(
        report
            .steps
            .iter()
            .all(|s| s.log.counts.keys().all(|k| !k.contains("not rebased")))
    );
}

#[test]
fn the_options_are_checked() {
    assert!(RebaseTexcoordSet::new(4, 3).is_err());
    assert!(RebaseTexcoordSet::new(4, 8193).is_err());
    assert!(RebaseTexcoordSet::new(4, 256).is_ok());
}
