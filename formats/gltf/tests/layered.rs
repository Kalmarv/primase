//! `core.layered` written as `PRIMASE_layered_material`: layer indices, mapping, per-layer UV set and transform (with
//! rotation), null slots, the host copy of the first non-null layer, extensionsUsed, counts and determinism.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pbr::{PbrMaterial, UvTransform};
use primase_core::{
    Instance, Layer, LayerWeights, LayeredMaterial, Mesh, Name, Primitive, Semantic, Stream, Topology,
    Transform, WeightMapping, World,
};
use primase_gltf::{WriteOptions, write};
use serde_json::Value;

fn document(bytes: &[u8]) -> Value {
    let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + n]).unwrap()
}

fn world(weights: Option<Semantic>, mapping: WeightMapping, layers: usize) -> (World, primase_core::Entity) {
    let mut w = World::new();
    let slot = |name: &str, colour: f32, w: &mut World| {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            PbrMaterial {
                base_color: [colour, 0.5, 0.25, 1.0],
                roughness: 0.3,
                ..PbrMaterial::default()
            },
        );
        e
    };
    let a = slot("rock", 0.1, &mut w);
    let b = slot("grass", 0.2, &mut w);
    let c = slot("sand", 0.3, &mut w);
    let d = slot("snow", 0.4, &mut w);
    let materials = [a, b, c, d];
    let host = w.spawn();
    w.insert(host, Name("terrain".into()));
    let mut stack = vec![
        None,
        Some(Layer {
            material: b,
            texcoord: Some(1),
            transform: Some(UvTransform {
                offset: [0.25, 0.5],
                rotation: 0.75,
                scale: [4.0, 8.0],
            }),
        }),
        Some(Layer {
            material: c,
            texcoord: None,
            transform: Some(UvTransform::IDENTITY),
        }),
        Some(Layer {
            material: d,
            texcoord: Some(0),
            transform: None,
        }),
    ];
    stack.truncate(layers);
    let _ = materials;
    w.insert(
        host,
        LayeredMaterial {
            layers: stack,
            weights: LayerWeights::new(weights.unwrap_or(Semantic::Color(1)), mapping),
        },
    );
    // every set the layers are sampled with (layer 0 with set 1, the others with set 0)
    let uv = || Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]);
    let mut streams = BTreeMap::from([
        (
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
        ),
        (Semantic::TexCoord(0), uv()),
        (Semantic::TexCoord(1), uv()),
    ]);
    if weights != Some(Semantic::Color(9)) {
        streams.insert(
            Semantic::Color(1),
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0.5, 1., 1.]]),
        );
    }
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2]),
            material: Some(host),
        }]),
    );
    let place = w.spawn();
    w.insert(
        place,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(place, Transform::IDENTITY);
    (w, host)
}

fn material<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc["materials"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap()
}

#[test]
fn layered_material_is_an_extension_over_ordinary_layer_materials() {
    let (w, _) = world(None, WeightMapping::RgbProduct4, 4);
    let (bytes, report) = write(&w, &WriteOptions::default()).unwrap();
    let doc = document(&bytes);
    assert_eq!(
        doc["extensionsUsed"],
        serde_json::json!(["PRIMASE_layered_material"])
    );
    assert!(doc.get("extensionsRequired").is_none());
    assert_eq!(
        (
            report.layered_materials,
            report.primitives_layered_without_weights
        ),
        (1, 0)
    );
    // the three layers' ordinary materials (the null slot's `rock` is unreferenced, so not written) + the host
    assert_eq!(doc["materials"].as_array().unwrap().len(), 4);
    let host = material(&doc, "terrain");
    let ext = &host["extensions"]["PRIMASE_layered_material"];
    assert_eq!(ext["weights"]["attribute"], "COLOR_1");
    assert_eq!(ext["weights"]["mapping"], "rgb_product_4");
    let layers = ext["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 4);
    assert!(
        layers[0].is_null(),
        "a null slot stays null and keeps its position"
    );
    let index = |name: &str| {
        doc["materials"]
            .as_array()
            .unwrap()
            .iter()
            .position(|m| m["name"] == name)
            .unwrap()
    };
    assert_eq!(layers[1]["material"], index("grass"));
    assert_eq!(layers[1]["texCoord"], 1);
    let transform = &layers[1]["transform"];
    assert_eq!(transform["offset"], serde_json::json!([0.25, 0.5]));
    assert_eq!(transform["rotation"], 0.75);
    assert_eq!(transform["scale"], serde_json::json!([4.0, 8.0]));
    // None texcoord and an identity transform write nothing; an explicit zero is kept
    assert_eq!(layers[2], serde_json::json!({"material": index("sand")}));
    assert_eq!(
        layers[3],
        serde_json::json!({"material": index("snow"), "texCoord": 0})
    );
    // the host is a copy of the first non-null layer (grass): same factors, its own name, no layer extras
    let grass = material(&doc, "grass");
    assert_eq!(host["pbrMetallicRoughness"], grass["pbrMetallicRoughness"]);
    // the primitive uses the host
    let prim = &doc["meshes"][0]["primitives"][0];
    assert_eq!(prim["material"], index("terrain"));
    assert!(prim["attributes"].get("COLOR_1").is_some());
    // TEXCOORD_1 is selected by a layer, so the primitive gets the set it lacks (zero filled)
    assert!(prim["attributes"].get("TEXCOORD_1").is_some());
    // deterministic
    assert_eq!(bytes, write(&w, &WriteOptions::default()).unwrap().0);
}

#[test]
fn identity_mapping_needs_no_fourth_slot_and_all_null_hosts_are_black() {
    let (w, _) = world(None, WeightMapping::Identity, 2);
    let doc = document(&write(&w, &WriteOptions::default()).unwrap().0);
    let ext = &material(&doc, "terrain")["extensions"]["PRIMASE_layered_material"];
    assert_eq!(ext["weights"]["mapping"], "identity");
    assert_eq!(ext["layers"].as_array().unwrap().len(), 2);

    let (mut w, host) = world(None, WeightMapping::Identity, 1);
    w.insert(
        host,
        LayeredMaterial {
            layers: vec![None],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let doc = document(&write(&w, &WriteOptions::default()).unwrap().0);
    let m = material(&doc, "terrain");
    assert_eq!(
        m["pbrMetallicRoughness"]["baseColorFactor"],
        serde_json::json!([0.0, 0.0, 0.0, 1.0])
    );
    assert!(m["extensions"]["PRIMASE_layered_material"]["layers"][0].is_null());
}

#[test]
fn missing_weights_are_counted_and_other_semantics_refused() {
    let (w, _) = world(Some(Semantic::Color(9)), WeightMapping::RgbProduct4, 4);
    let (_, report) = write(&w, &WriteOptions::default()).unwrap();
    assert_eq!(report.primitives_layered_without_weights, 1);

    let (w, _) = world(Some(Semantic::Normal), WeightMapping::Identity, 2);
    let error = write(&w, &WriteOptions::default()).unwrap_err().to_string();
    assert!(error.contains("COLOR_n"), "{error}");
}

#[test]
fn per_pixel_channels_and_over_are_version_2() {
    use primase_core::layered::{Channel, Composition, Source};
    use primase_core::pbr::TextureRef;
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    let (mut w, host) = world(None, WeightMapping::Identity, 2);
    let mask = w.spawn();
    w.insert(mask, Name("mask".into()));
    w.insert(
        mask,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![255, 128, 0, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        },
    );
    let v = Source::Vertex {
        semantic: Semantic::Color(1),
        channel: 2,
    };
    let m = Source::Mask {
        texture: TextureRef {
            texcoord: 1,
            ..TextureRef::new(mask)
        },
        channels: [0.5, 0.5, 0.0, 0.0],
        bias: -0.25,
    };
    let mut l = w.get::<LayeredMaterial>(host).unwrap().clone();
    l.weights.channels = Some(vec![
        Channel::Source(v.clone()),
        Channel::Threshold {
            v,
            m,
            k: [0.0, 2.0, -2.0, 2.0],
        },
    ]);
    l.weights.composition = Composition::Over;
    w.insert(host, l);
    let (bytes, _) = write(&w, &WriteOptions::default()).unwrap();
    // the mask encoded up front on several threads: the same bytes
    let threads = WriteOptions {
        threads: primase_core::parallel::Threads::new(4).unwrap(),
        ..WriteOptions::default()
    };
    assert!(write(&w, &threads).unwrap().0 == bytes);
    let doc = document(&bytes);
    let ext = &material(&doc, "terrain")["extensions"]["PRIMASE_layered_material"];
    assert_eq!(ext["version"], 2);
    assert_eq!(ext["composition"], "over");
    let channels = &ext["weights"]["channels"];
    assert_eq!(
        channels[0],
        serde_json::json!({"source": {"vertex": {"attribute": "COLOR_1", "channel": 2}}})
    );
    let t = &channels[1]["threshold"];
    assert_eq!(t["k"], serde_json::json!([0.0, 2.0, -2.0, 2.0]));
    assert_eq!(
        t["m"]["mask"]["channels"],
        serde_json::json!([0.5, 0.5, 0.0, 0.0])
    );
    assert_eq!(t["m"]["mask"]["bias"], -0.25);
    assert_eq!(t["m"]["mask"]["texture"]["texCoord"], 1);
    let index = t["m"]["mask"]["texture"]["index"].as_u64().unwrap() as usize;
    assert!(
        doc["textures"][index].is_object(),
        "the mask is a texture of the file"
    );
    // without channels and summed: version 1 as before (no version key)
    let (w, _) = world(None, WeightMapping::Identity, 2);
    let doc = document(&write(&w, &WriteOptions::default()).unwrap().0);
    let ext = &material(&doc, "terrain")["extensions"]["PRIMASE_layered_material"];
    assert!(
        ext.get("version").is_none()
            && ext.get("composition").is_none()
            && ext["weights"].get("channels").is_none()
    );
}

#[test]
fn the_height_composition_is_version_3_with_a_height_per_slot() {
    use primase_core::layered::{Composition, Source};
    use primase_core::pbr::TextureRef;
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    let (mut w, host) = world(Some(Semantic::Color(0)), WeightMapping::Identity, 2);
    let height = w.spawn();
    w.insert(
        height,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![0, 0, 0, 200]],
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        },
    );
    let h = Source::Mask {
        texture: TextureRef::new(height),
        channels: [0.0, 0.0, 0.0, 1.0],
        bias: 0.0,
    };
    let mut l = w.get::<LayeredMaterial>(host).unwrap().clone();
    l.weights.composition = Composition::Height { contrast: 0.5 };
    l.weights.heights = vec![h.clone(), h];
    w.insert(host, l);
    let doc = document(&write(&w, &WriteOptions::default()).unwrap().0);
    let ext = &material(&doc, "terrain")["extensions"]["PRIMASE_layered_material"];
    assert_eq!(ext["version"], 3);
    assert_eq!(
        ext["composition"],
        serde_json::json!({"height": {"contrast": 0.5}})
    );
    let heights = ext["weights"]["heights"].as_array().unwrap();
    assert_eq!(heights.len(), 2);
    assert_eq!(
        heights[0]["mask"]["channels"],
        serde_json::json!([0.0, 0.0, 0.0, 1.0])
    );
}

#[test]
fn product_channels_write_every_factor_and_require_version_five() {
    use primase_core::layered::{Channel, Source};
    let (mut w, host) = world(Some(Semantic::Color(1)), WeightMapping::Identity, 2);
    let vertex = |channel| Source::Vertex {
        semantic: Semantic::Color(1),
        channel,
    };
    let layered = w.get_mut::<LayeredMaterial>(host).unwrap();
    layered.weights.channels = Some(vec![
        Channel::Source(vertex(0)),
        Channel::Product {
            sources: vec![vertex(0), vertex(1), vertex(2)],
        },
    ]);
    let (bytes, _) = write(&w, &WriteOptions::default()).unwrap();
    let doc = document(&bytes);
    let ext = &material(&doc, "terrain")["extensions"]["PRIMASE_layered_material"];
    assert_eq!(ext["version"], 5);
    let factors = ext["weights"]["channels"][1]["product"].as_array().unwrap();
    assert_eq!(factors.len(), 3);
    for (i, f) in factors.iter().enumerate() {
        assert_eq!(f["vertex"]["attribute"], "COLOR_1");
        assert_eq!(f["vertex"]["channel"], i);
    }
    let (again, _) = write(&w, &WriteOptions::default()).unwrap();
    assert_eq!(bytes, again);
}
