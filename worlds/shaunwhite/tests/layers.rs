//! The shipped restored-state RGB equation, checked with hand-worked colours and real layered wall materials.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers assert their explicit material and pixel fixtures"
)]
use primase_core::layered::{Channel, Composition, Source, mask_value};
use primase_core::pass::{Conservation, PassManager, PassRegistry, RunReport};
use primase_core::pbr::{PbrMaterial, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, LayeredMaterial, MaterialVariants, Mesh, Primitive, Semantic, Stream, TexFormat,
    TexRole, Texture, Topology, VertexColor, World,
};
use primase_shaunwhite::components::{LayerState, MaterialRef, MaterialState, SlotRef};
use std::collections::BTreeMap;

fn run(w: &mut World, text: &str) -> RunReport {
    let mut registry = PassRegistry::new();
    primase_shaunwhite::register(&mut registry);
    let pipeline = registry.parse(text).unwrap();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(w, &pipeline, &mut report)
        .unwrap();
    report
}
fn pixel(w: &World, source: &Source, vertex: [f32; 4]) -> f32 {
    match source {
        Source::Vertex { channel, .. } => vertex[usize::from(*channel)],
        Source::Mask {
            texture,
            channels,
            bias,
        } => {
            let bytes = &w.get::<Texture>(texture.texture).unwrap().levels[0];
            let rgba = std::array::from_fn(|i| f32::from(bytes[i]) / 255.);
            mask_value(*channels, *bias, rgba)
        }
    }
}
fn colour(w: &World, host: Entity, vertex: [f32; 4]) -> [f32; 3] {
    let l = w.get::<LayeredMaterial>(host).unwrap();
    assert_eq!(l.weights.composition, Composition::Over);
    let mut rgb = [0.; 3];
    for (i, layer) in l.layers.iter().enumerate() {
        let p = w.get::<PbrMaterial>(layer.as_ref().unwrap().material).unwrap();
        let texture = w.get::<Texture>(p.base_color_texture.unwrap().texture).unwrap();
        let value =
            std::array::from_fn::<_, 3, _>(|k| p.base_color[k] * f32::from(texture.levels[0][k]) / 255.);
        let c = &l.weights.channels.as_ref().unwrap()[i];
        let values: Vec<_> = c.sources().into_iter().map(|s| pixel(w, s, vertex)).collect();
        let a = c.evaluate(&values);
        rgb = if i == 0 {
            value
        } else {
            std::array::from_fn(|k| rgb[k] + (value[k] - rgb[k]) * a)
        };
    }
    rgb
}
fn fixture() -> (World, Entity, Entity, Entity) {
    let mut w = World::new();
    let mut image = |rgba| {
        let e = w.spawn();
        w.insert(
            e,
            Texture {
                width: 1,
                height: 1,
                format: TexFormat::Rgba8Unorm,
                levels: vec![Vec::from(rgba)],
                color_space: ColorSpace::Linear,
                role: TexRole::Color,
                normal: None,
            },
        );
        e
    };
    let base = image([255, 0, 0, 255]);
    let second = image([0, 255, 0, 255]);
    let third = image([0, 0, 255, 255]);
    let mask = image([255, 255, 255, 255]);
    let host = w.spawn();
    let slot = |texture| SlotRef {
        texture,
        scale: [1.; 2],
        offset: [0.; 2],
        rotation: 0.,
        texcoord: 0,
        address: Some([0; 2]),
    };
    w.insert(
        host,
        MaterialRef {
            id: 1,
            name: "fixture".into(),
            state: Some(MaterialState {
                blend: 0,
                blended: false,
                alpha_test: false,
                alpha_ref: 128,
                water: false,
                color: [0.5, 1., 1., 1.],
                illumination: 0.,
                specular_level: 1.,
                specular_power: 10.,
                normal_scale: 1.,
                layering: LayerState {
                    colors: [[1., 1., 1., 0.5], [1., 1., 0.5, 0.5]],
                    use_alpha: [true; 3],
                },
            }),
            slots: BTreeMap::from([
                ("diffuse".into(), slot(base)),
                (
                    "diffuse_b".into(),
                    SlotRef {
                        texcoord: 1,
                        scale: [2., 3.],
                        offset: [0.25, 0.],
                        ..slot(second)
                    },
                ),
                ("diffuse_c".into(), slot(third)),
                ("mask".into(), slot(mask)),
            ]),
            unread: Vec::new(),
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
                    Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
                ),
                (Semantic::TexCoord(0), Stream::F32x2(vec![[0.; 2]; 3])),
                (Semantic::TexCoord(1), Stream::F32x2(vec![[1.; 2]; 3])),
                (Semantic::Color(0), Stream::F32x4(vec![[0.5, 0.5, 0.5, 1.]; 3])),
            ]),
            indices: Some(vec![0, 1, 2]),
            material: Some(host),
        }]),
    );
    (w, host, proto, second)
}

/// Hand-worked: red base1 tinted by red weight1/2 becomes3/4 red. Green layer's weight is1/2*1*1/2=1/4,
/// then blue's is1/4 and its tint is1/2: final RGB=(27/64,3/16,1/8). Zero sampled alpha disables only that layer.
#[test]
fn the_shader_equation_keeps_tint_mask_alpha_and_order() {
    let (mut w, host, _, _) = fixture();
    run(&mut w, "lower-shaunwhite-layers{influence=restored}");
    let l = w.get::<LayeredMaterial>(host).unwrap();
    assert_eq!(l.layers.len(), 4);
    // the layer selects the slot's coordinate set and tiling; its material samples TexCoord(0) untiled
    let b = l.layers[2].as_ref().unwrap();
    assert_eq!(b.texcoord, Some(1));
    assert_eq!(
        b.transform,
        Some(UvTransform {
            offset: [0.25, 0.],
            scale: [2., 3.],
            rotation: 0.
        })
    );
    assert_eq!(l.layers[3].as_ref().unwrap().transform, None);
    assert!(
        matches!(&l.weights.channels.as_ref().unwrap()[2],Channel::Product { sources } if sources.len()==3)
    );
    run(&mut w, "lower-shaunwhite-material");
    assert_eq!(colour(&w, host, [0.5, 0.5, 0.5, 1.]), [0.421875, 0.1875, 0.125]);
    // This is a texture alpha change, not a weight hard-coded in the fixture material.
    let second = w.get::<LayeredMaterial>(host).unwrap().layers[2]
        .as_ref()
        .unwrap()
        .material;
    let texture = w
        .get::<PbrMaterial>(second)
        .unwrap()
        .base_color_texture
        .unwrap()
        .texture;
    let sampled = w.get::<PbrMaterial>(second).unwrap().base_color_texture.unwrap();
    assert_eq!((sampled.texcoord, sampled.transform), (0, None));
    w.get_mut::<Texture>(texture).unwrap().levels[0][3] = 0;
    assert_eq!(colour(&w, host, [0.5, 0.5, 0.5, 1.]), [0.5625, 0., 0.125]);
}

#[test]
fn colourless_drawings_get_the_shader_one_and_variants_follow_their_primitive() {
    let (mut w, host, proto, _) = fixture();
    let mut p = w.get::<Mesh>(proto).unwrap().primitives()[0].clone();
    p.streams.remove(&Semantic::Color(0));
    let bare = w.spawn();
    w.insert(bare, Mesh::new(vec![p]));
    w.insert(bare, MaterialVariants(vec![BTreeMap::from([(1, host)])]));
    run(&mut w, "lower-shaunwhite-layers{influence=restored}");
    let other = w.get::<Mesh>(bare).unwrap().primitives()[0].material.unwrap();
    assert_ne!(other, host);
    assert_eq!(w.get::<MaterialVariants>(bare).unwrap().0[0][&1], other);
    assert!(!w.get::<LayeredMaterial>(other).unwrap().reads(Semantic::Color(0)));
    run(&mut w, "lower-shaunwhite-material");
    assert_eq!(colour(&w, other, [0.; 4]), [0.125, 0.25, 0.25]);
}

#[test]
fn unsupported_equations_are_named_and_counted() {
    let (mut w, host, _, _) = fixture();
    for (name, kind) in [("alpha", 0), ("rotation", 1), ("border", 2)] {
        let mut m = w.get::<MaterialRef>(host).unwrap().clone();
        m.name = name.into();
        match kind {
            0 => m.state.as_mut().unwrap().alpha_test = true,
            1 => m.slots.get_mut("mask").unwrap().rotation = 90.,
            _ => m.slots.get_mut("diffuse_b").unwrap().address = Some([2, 0]),
        }
        let e = w.spawn();
        w.insert(e, m);
    }
    let report = run(&mut w, "lower-shaunwhite-layers{influence=restored}");
    let omitted = &report.steps[0].log.omitted;
    for label in ["alpha:", "rotation:", "border:"] {
        assert!(omitted.iter().any(|(name, n)| name.starts_with(label) && *n == 1));
    }
}

#[test]
fn restored_metro_mural_and_level_six_masked_walls_keep_every_weight_factor() {
    use primase_testdata::{Game, install};
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    let shared = vec![folder.join("DataPC_02.forge"), folder.join("DataPC_04.forge")];
    for file in ["DataPC_01.forge", "DataPC_06.forge"] {
        let (mut w, _) = primase_shaunwhite::read::level::read_level(&folder.join(file), &shared).unwrap();
        let mural = w
            .iter::<MaterialRef>()
            .find(|(_, m)| m.name == "01_Wall_Metro01Grafitti_Mat")
            .map(|(e, _)| e);
        let cliff = w
            .iter::<MaterialRef>()
            .find(|(_, m)| m.name == "Gen_Dirt_CliffDry01_Mat")
            .map(|(e, _)| e);
        let report = run(&mut w, "lower-shaunwhite-layers{influence=restored}");
        let layered = w.iter::<LayeredMaterial>().count();
        assert!(layered > 10, "{file}: {layered}");
        if file == "DataPC_01.forge" {
            let mural = mural.expect("independently inspected metro wall material");
            assert_eq!(w.get::<LayeredMaterial>(mural).unwrap().layers.len(), 4);
            assert!(
                w.get::<LayeredMaterial>(cliff.expect("CliffDry material"))
                    .is_some()
            );
            for c in &w
                .get::<LayeredMaterial>(mural)
                .unwrap()
                .weights
                .channels
                .as_ref()
                .unwrap()[2..]
            {
                assert!(matches!(c,Channel::Product { sources } if sources.len()==3));
            }
        }
        run(&mut w, "lower-shaunwhite-material");
        if let Some(mural) = mural {
            let third = w.get::<LayeredMaterial>(mural).unwrap().layers[3]
                .as_ref()
                .unwrap()
                .material;
            assert_eq!(
                w.get::<PbrMaterial>(third)
                    .unwrap()
                    .base_color_texture
                    .unwrap()
                    .wrap[1],
                Wrap::Clamp
            );
        }
        println!(
            "{file}: {layered} restored layer materials; {:?}; {} omission rows",
            report.steps[0].log.counts,
            report.steps[0].log.omitted.len()
        );
        assert!(w.verify_components().is_empty());
    }
}

fn plain(
    w: &mut World,
    name: &str,
    tint: [f32; 4],
    blended: bool,
    vertex: Option<[f32; 4]>,
) -> (Entity, Entity) {
    let texture = w.spawn();
    w.insert(
        texture,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![128, 64, 255, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Color,
            normal: None,
        },
    );
    let material = w.spawn();
    w.insert(
        material,
        MaterialRef {
            id: 7,
            name: name.into(),
            state: Some(MaterialState {
                blend: if blended { 4 } else { 0 },
                blended,
                alpha_test: false,
                alpha_ref: 128,
                water: false,
                color: tint,
                illumination: 0.,
                specular_level: 1.,
                specular_power: 10.,
                normal_scale: 1.,
                layering: LayerState::default(),
            }),
            slots: BTreeMap::from([(
                "diffuse".into(),
                SlotRef {
                    texture,
                    scale: [1.; 2],
                    offset: [0.; 2],
                    rotation: 0.,
                    texcoord: 0,
                    address: Some([0; 2]),
                },
            )]),
            unread: Vec::new(),
        },
    );
    let mut streams = BTreeMap::from([
        (
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
        ),
        (Semantic::TexCoord(0), Stream::F32x2(vec![[0.; 2]; 3])),
    ]);
    if let Some(c) = vertex {
        streams.insert(Semantic::Color(0), Stream::F32x4(vec![c; 3]));
    }
    let mesh = w.spawn();
    w.insert(
        mesh,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2]),
            material: Some(material),
        }]),
    );
    (material, mesh)
}

/// The shader's first step is `lerp(texture, texture * tint, COLOR_0.r)` and its vertex alpha only reaches the alpha:
/// a white tint draws the texture whatever the vertex colour (the level 01 planters, red `COLOR_0` on a grey texture),
/// a tint is weighted by the vertex red, and no vertex colour leaves texture times tint.
#[test]
fn a_single_layer_material_takes_its_tint_by_the_vertex_red_and_never_the_vertex_rgb() {
    let mut w = World::new();
    let (planter, _) = plain(&mut w, "planter", [1.; 4], false, Some([0.93, 0., 0., 1.]));
    let (tinted, _) = plain(
        &mut w,
        "tinted",
        [0.5, 0.25, 1., 1.],
        false,
        Some([0.5, 0.9, 0.1, 1.]),
    );
    let (bare, _) = plain(&mut w, "bare", [0.5, 0.25, 1., 1.], false, None);
    let report = run(&mut w, "lower-shaunwhite-layers{influence=restored}");
    assert_eq!(
        report.steps[0]
            .log
            .counts
            .get("single-layer materials weighted by the vertex red"),
        Some(&1)
    );
    assert!(!w.has::<LayeredMaterial>(planter) && !w.has::<LayeredMaterial>(bare));
    // red 0.5: halfway from the texture (128, 64, 255) to texture times tint (64, 16, 255)
    let l = w.get::<LayeredMaterial>(tinted).unwrap();
    assert_eq!(l.layers.len(), 2);
    assert!(l.reads(Semantic::Color(0)));
    run(&mut w, "lower-shaunwhite-material");
    let c = colour(&w, tinted, [0.5, 0.9, 0.1, 1.]);
    assert!((c[0] - 96. / 255.).abs() < 1e-6 && (c[1] - 40. / 255.).abs() < 1e-6 && (c[2] - 1.).abs() < 1e-6);
    for e in [planter, bare] {
        assert_eq!(w.get::<VertexColor>(e), Some(&VertexColor::Ignored));
    }
    let p = w.get::<PbrMaterial>(planter).unwrap();
    assert_eq!(p.base_color, [1.; 4]);
    assert_eq!(
        w.get::<PbrMaterial>(bare).unwrap().base_color,
        [0.5, 0.25, 1., 1.]
    );
    assert!(w.verify_components().is_empty());
}

/// Core can multiply `COLOR_0` or ignore it, not take its alpha alone: a blended material with a vertex alpha below
/// one keeps the multiplier (counted); one whose alpha is one everywhere, or that is opaque, ignores it.
#[test]
fn vertex_alpha_is_kept_only_where_it_reaches_the_output() {
    let mut w = World::new();
    let (fading, _) = plain(&mut w, "fading", [1.; 4], true, Some([1., 1., 1., 0.5]));
    let (solid, _) = plain(&mut w, "solid", [1.; 4], true, Some([1., 1., 1., 1.]));
    let (opaque, _) = plain(&mut w, "opaque", [1.; 4], false, Some([1., 1., 1., 0.5]));
    let report = run(
        &mut w,
        "lower-shaunwhite-layers{influence=restored},lower-shaunwhite-material",
    );
    assert_eq!(w.get::<VertexColor>(fading), None);
    assert_eq!(w.get::<VertexColor>(solid), Some(&VertexColor::Ignored));
    assert_eq!(w.get::<VertexColor>(opaque), Some(&VertexColor::Ignored));
    assert_eq!(
        report.steps[1]
            .log
            .approximations
            .iter()
            .find(|(name, _)| name.starts_with("blended or alpha-tested materials keep COLOR_0")),
        Some((
            &"blended or alpha-tested materials keep COLOR_0 as a colour for its alpha (the shader takes only the alpha)"
                .to_owned(),
            &1
        ))
    );
}

/// Level 01: the planter material (white tint, `COLOR_0` about (0.93, 0, 0)) draws its grey texture, and the only
/// materials still multiplying `COLOR_0` are blended or alpha-tested ones.
#[test]
fn level_01_materials_no_longer_take_the_vertex_rgb() {
    use primase_testdata::{Game, install};
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    let shared = vec![folder.join("DataPC_02.forge"), folder.join("DataPC_04.forge")];
    let (mut w, _) =
        primase_shaunwhite::read::level::read_level(&folder.join("DataPC_01.forge"), &shared).unwrap();
    let planter = w
        .iter::<MaterialRef>()
        .find(|(_, m)| m.name == "01_Grind_TopRampPlaza01_Mat")
        .map(|(e, _)| e)
        .unwrap();
    let report = run(
        &mut w,
        "lower-shaunwhite-layers{influence=restored},lower-shaunwhite-material",
    );
    assert!(!w.has::<LayeredMaterial>(planter));
    assert_eq!(w.get::<VertexColor>(planter), Some(&VertexColor::Ignored));
    assert_eq!(w.get::<PbrMaterial>(planter).unwrap().base_color[..3], [1.; 3]);
    let mut keep = std::collections::BTreeSet::new();
    let mut drawn = std::collections::BTreeSet::new();
    for (_, mesh) in w.iter::<Mesh>() {
        for p in mesh.primitives() {
            let Some(m) = p.material.filter(|_| p.streams.contains_key(&Semantic::Color(0))) else {
                continue;
            };
            drawn.insert(m);
            if w.get::<VertexColor>(m).is_none() && !w.has::<LayeredMaterial>(m) {
                assert_ne!(
                    w.get::<PbrMaterial>(m).unwrap().alpha_mode,
                    primase_core::pbr::AlphaMode::Opaque
                );
                keep.insert(m);
            }
        }
    }
    println!(
        "{} materials drawn with COLOR_0: {} still multiply it for its alpha; layers {:?}; approximations {:?}",
        drawn.len(),
        keep.len(),
        report.steps[0].log.counts,
        report.steps[1].log.approximations
    );
    assert!(!keep.is_empty() && keep.len() < 100);
}
