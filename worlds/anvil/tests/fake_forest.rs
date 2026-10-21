//! MAD_FakeForest lowered as the notes' executed recipe states (docs/material-spec.md §26.6): base colour 1, roughness
//! 1, metalness 0, opaque, single-sided, and each primitive drawn with it alone given COLOR_0 = pow(c, 2.2) (worked by
//! hand); a primitive also drawn with another Material keeps its colour, counted.
use primase_anvil::components::{AnvilMaterial, MaterialFlags};
use primase_anvil::passes::{AlphaChoice, LowerAnvilMaterial, ShaderUv};
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Entity, MaterialVariants, Mesh, NormalGreen, Primitive, Semantic, Stream, Topology, VertexColor, World,
};
use std::collections::BTreeMap;

fn material(w: &mut World, template: &str) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        AnvilMaterial {
            index_id: 1,
            template: Some(2),
            template_name: Some(template.to_owned()),
            bindings: Vec::new(),
            values: Vec::new(),
            texture_set: Vec::new(),
            flags: MaterialFlags {
                blend_mode: 0,
                alpha_display_mode: 1,
                alpha_test_value: 128,
                booleans: [0; 35],
            },
            shaders: None,
        },
    );
    e
}

fn triangle(material: Entity, colour: [[u8; 4]; 3]) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            ),
            (Semantic::Color(0), Stream::U8x4(colour.to_vec())),
        ]),
        indices: None,
        material: Some(material),
    }
}

#[test]
fn fake_forest_takes_its_vertex_colour_decoded() {
    let mut w = World::new();
    let forest = material(&mut w, "MAD_FakeForest");
    let other = material(&mut w, "MAD_Unknown");
    let colour = [[255, 128, 0, 200], [51, 51, 51, 255], [0, 0, 0, 0]];
    let alone = w.spawn();
    w.insert(alone, Mesh::new(vec![triangle(forest, colour)]));
    let shared = w.spawn();
    w.insert(shared, Mesh::new(vec![triangle(forest, colour)]));
    w.insert(shared, MaterialVariants(vec![BTreeMap::from([(1, other)])]));
    let pass = LowerAnvilMaterial {
        normal_green: NormalGreen::Up,
        alpha: AlphaChoice::Prelude,
        shader_uv: ShaderUv::TexCoord,
        emissive_exposure: 1.0,
    };
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("lower");
    let pbr = w.get::<PbrMaterial>(forest).expect("pbr");
    assert_eq!(
        (
            pbr.base_color,
            pbr.roughness,
            pbr.metallic,
            pbr.alpha_mode,
            pbr.double_sided
        ),
        ([1.0; 4], 1.0, 0.0, AlphaMode::Opaque, false)
    );
    assert!(pbr.base_color_texture.is_none() && !w.has::<VertexColor>(forest));
    // the UNORM8 value as the GPU reads it (c / 255 in f32), then pow 2.2 in f64 rounded once to f32; alpha c / 255
    let decoded = |c: u8| f64::from(f32::from(c) / 255.0).powf(2.2) as f32;
    let want: Vec<[f32; 4]> = colour
        .iter()
        .map(|&[r, g, b, a]| [decoded(r), decoded(g), decoded(b), f32::from(a) / 255.0])
        .collect();
    assert_eq!(
        w.get::<Mesh>(alone).unwrap().primitives()[0].streams[&Semantic::Color(0)],
        Stream::F32x4(want)
    );
    assert_eq!(
        w.get::<Mesh>(shared).unwrap().primitives()[0].streams[&Semantic::Color(0)],
        Stream::U8x4(colour.to_vec())
    );
    assert_eq!(log.counts["FakeForest primitives' COLOR_0 decoded (pow 2.2)"], 1);
    assert_eq!(
        log.omitted["primitives drawn with a FakeForest Material and another: COLOR_0 as stored"],
        1
    );
}
