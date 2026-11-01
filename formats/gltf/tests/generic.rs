//! The writer writes standard glTF and the Khronos extensions it names, nothing of primase's own: no component in
//! extras, no vendor extension. What glTF has no form for (a dialect's components, a source, shadows, the environment
//! and the atmosphere, a layered material's blend, a light's shadows) is counted by name, never written. The writer
//! chooses no layer of a layered material: one without `pbr.material` is not written, counted, for a pass before the
//! writer to state. An extension a caller passes ([`primase_gltf::extend`]) is what adds to it.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    Ambient, AmbientLight, Atmosphere, ColorSpace, Environment, Instance, Layer, LayerWeights,
    LayeredMaterial, Light, LightKind, Mesh, Name, Opaque, OpaqueData, Semantic, ShadowCast, Shadows, Source,
    TexFormat, TexRole, Texture, Transform, WeightMapping, World,
};
use primase_gltf::{WriteOptions, WriteReport, write};
use serde_json::{Value, json};

mod common;

/// A world stating everything glTF has no form for: a material with a dialect's component sampling a data texture, a
/// layered material over two layers, a prototype with its source and shadows drawn with both, placed twice (once
/// with shadows and a dialect component of its own), a light that casts shadows, the environment's sun and ambient,
/// and an atmosphere.
fn world() -> World {
    let mut w = World::new();
    let opaque = |v: Value| Opaque {
        version: 1,
        data: OpaqueData::Json(v),
    };
    let data = w.spawn();
    w.insert(
        data,
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![(0..16).collect()],
            color_space: ColorSpace::Linear,
            role: TexRole::Data,
            normal: None,
        },
    );
    let stone = w.spawn();
    w.insert(stone, Name("stone".into()));
    w.insert(
        stone,
        PbrMaterial {
            occlusion_texture: Some(TextureRef::new(data)),
            ..PbrMaterial::default()
        },
    );
    w.insert_opaque(stone, "skate.material", opaque(json!({"class": "opaque"})))
        .unwrap();
    let layer = |w: &mut World, name: &str| {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(e, PbrMaterial::default());
        e
    };
    let (rock, grass) = (layer(&mut w, "rock"), layer(&mut w, "grass"));
    let terrain = w.spawn();
    w.insert(terrain, Name("terrain".into()));
    w.insert(
        terrain,
        LayeredMaterial {
            layers: vec![
                None,
                Some(Layer {
                    material: rock,
                    texcoord: None,
                    transform: None,
                }),
                Some(Layer {
                    material: grass,
                    texcoord: Some(1),
                    transform: None,
                }),
            ],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let proto = w.spawn();
    w.insert(proto, Name("crate".into()));
    w.insert(
        proto,
        Mesh::new(vec![
            common::rich_primitive(Some(stone), 0.0),
            common::rich_primitive(Some(terrain), 2.0),
        ]),
    );
    w.insert(proto, Source("models/crate".into()));
    w.insert(
        proto,
        Shadows {
            cast: Some(ShadowCast::Off),
            receive: None,
        },
    );
    for x in [0.0, 4.0] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3][0] = x;
        w.insert(e, t);
        if x == 0.0 {
            w.insert(
                e,
                Shadows {
                    cast: Some(ShadowCast::Only),
                    receive: Some(false),
                },
            );
            w.insert_opaque(e, "skate.placement", opaque(json!({"spawn": true})))
                .unwrap();
        }
    }
    let sun = w.spawn();
    w.insert(
        sun,
        Light {
            kind: LightKind::Directional,
            color: [1.0; 3],
            intensity: 3.0,
            range: None,
            cast_shadows: Some(true),
            radius: None,
            specular: None,
        },
    );
    w.insert(sun, Transform::IDENTITY);
    w.insert_resource(Environment {
        sun: Some(sun),
        ambient: Some(Ambient {
            intensity: 0.5,
            light: AmbientLight::Flat([0.2, 0.3, 0.4]),
        }),
    });
    w.insert_resource(Atmosphere::default());
    w
}

fn written(w: &World) -> (Value, WriteReport) {
    let (bytes, report) = write(w, &WriteOptions::default()).unwrap();
    let doc = serde_json::from_slice(primase_glb::parse(&bytes).unwrap().json).unwrap();
    (doc, report)
}

/// Every `extras` object in `v`, by its JSON path, and every extension name in `v` (keys of `extensions` objects and
/// entries of `extensionsUsed`).
fn walk(v: &Value, at: &str, extras: &mut Vec<String>, extensions: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                let path = format!("{at}/{k}");
                match k.as_str() {
                    "extras" => extras.push(format!("{path} = {x}")),
                    "extensions" => extensions.extend(x.as_object().unwrap().keys().cloned()),
                    "extensionsUsed" | "extensionsRequired" => {
                        extensions.extend(
                            x.as_array()
                                .unwrap()
                                .iter()
                                .map(|n| n.as_str().unwrap().to_owned()),
                        );
                    }
                    _ => {}
                }
                walk(x, &path, extras, extensions);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                walk(x, &format!("{at}/{i}"), extras, extensions);
            }
        }
        _ => {}
    }
}

#[test]
fn the_glb_holds_standard_gltf_alone_and_counts_the_rest() {
    let (doc, report) = written(&world());
    let (mut extras, mut extensions) = (Vec::new(), Vec::new());
    walk(&doc, "", &mut extras, &mut extensions);
    assert_eq!(extras, Vec::<String>::new());
    assert!(extensions.iter().all(|n| n.starts_with("KHR_")), "{extensions:?}");
    assert_eq!(doc["extensionsUsed"], json!(["KHR_lights_punctual"]));
    // per entity written, each component its objects do not express
    assert_eq!(
        report.components_without_form,
        BTreeMap::from(
            [
                ("core.shadows", 2),
                ("core.source", 1),
                ("skate.material", 1),
                ("skate.placement", 1),
            ]
            .map(|(k, n)| (k.to_owned(), n))
        )
    );
    assert_eq!(
        report.resources_without_form,
        BTreeMap::from([
            ("core.atmosphere".to_owned(), 1),
            ("core.environment".to_owned(), 1)
        ])
    );
    assert_eq!((report.lights_shadows_unwritten, report.ambient_unlit), (1, 1));
    // the layered material is not written, nor its layers, which nothing else draws: no layer is chosen for it
    assert_eq!(materials(&doc), ["stone"]);
    assert!(doc["meshes"][0]["primitives"][1].get("material").is_none());
    assert_eq!(
        (report.layered_materials_unwritten, report.materials_without_pbr),
        (1, 0)
    );
    assert_eq!(
        report.components_unwritten,
        BTreeMap::from([("core.name".to_owned(), 2), ("pbr.material".to_owned(), 2)])
    );
    assert_eq!(report.primitives_without_material_attribute, 0);
    let text = report.to_string();
    assert!(
        text.contains("components without a glTF form (not written)"),
        "{text}"
    );
}

fn materials(doc: &Value) -> Vec<&str> {
    doc["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect()
}

/// A layered material that a pass also stated as a `pbr.material` is written from that, its blend counted.
#[test]
fn a_layered_material_with_its_pbr_material_is_written_from_it() {
    let mut w = world();
    let terrain = w.iter::<LayeredMaterial>().map(|(e, _)| e).next().unwrap();
    w.insert(
        terrain,
        PbrMaterial {
            base_color: [0.5, 0.25, 0.125, 1.0],
            ..PbrMaterial::default()
        },
    );
    let (doc, report) = written(&w);
    assert_eq!(materials(&doc), ["stone", "terrain"]);
    assert_eq!(doc["meshes"][0]["primitives"][1]["material"], 1);
    assert_eq!(
        doc["materials"][1]["pbrMetallicRoughness"]["baseColorFactor"],
        json!([0.5, 0.25, 0.125, 1.0])
    );
    assert_eq!(report.layered_materials_unwritten, 0);
    assert_eq!(report.components_without_form.get("core.layered"), Some(&1));
}
