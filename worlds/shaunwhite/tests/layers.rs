//! The shipped restored-state RGB equation, checked with hand-worked colours and real layered wall materials.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers assert their explicit material and pixel fixtures"
)]
use primase_core::layered::{Channel, Composition, Source, mask_value};
use primase_core::pass::{Conservation, PassManager, PassRegistry, RunReport};
use primase_core::pbr::{PbrMaterial, Wrap};
use primase_core::{
    ColorSpace, Entity, LayeredMaterial, MaterialVariants, Mesh, Primitive, Semantic, Stream, TexFormat,
    TexRole, Texture, Topology, World,
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
    assert_eq!(l.layers[2].as_ref().unwrap().texcoord, Some(1));
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
