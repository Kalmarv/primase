use primase_core::{
    Instance, Mesh, Origin, Primitive, Provenance, Semantic, Stream, Topology, Transform, World,
};
use primase_web::{Options, write};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn primitive() -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
        )]),
        indices: Some(vec![0, 1, 2]),
        material: None,
    }
}

fn drawable(file: &str) -> Origin {
    Origin::new(Some("levels/city/block.rpf"), file, None)
}

/// Two prototypes from one archive, each two primitives (the second prototype's both from one geometry); four
/// placements of the first (instanced together) and one of the second (a node of its own).
fn world() -> World {
    let mut w = World::new();
    let mut protos = Vec::new();
    for (file, items) in [
        ("a.ydr", ["model 0 geometry 0", "model 0 geometry 1"]),
        ("b.ydr", ["model 0 geometry 0"; 2]),
    ] {
        let p = w.spawn();
        w.insert(
            p,
            Mesh {
                primitives: vec![primitive(), primitive()],
            },
        );
        let mut prov = Provenance::default();
        for item in items {
            prov.push_part(drawable(file).with_item(item));
        }
        w.insert(p, prov);
        protos.push(p);
    }
    for i in 0..5 {
        let e = w.spawn();
        let prototype = if i == 4 { protos[1] } else { protos[0] };
        w.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [10. + i as f32, 0., 10.];
        w.insert(e, t);
        w.insert(
            e,
            Provenance::new(Origin::new(
                Some("levels/city/block.rpf"),
                "block.ymap",
                Some(&format!("entity {i}")),
            )),
        );
    }
    w
}

fn doc(bytes: &[u8]) -> Value {
    let chunks = primase_gltf::glb::parse(bytes).expect("a GLB");
    serde_json::from_slice(chunks.json).expect("its JSON")
}

/// The origin objects an object's `web.provenance` resolves to: (origins, parts).
fn resolve(table: &[Value], object: &Value) -> (Vec<Value>, Vec<Value>) {
    let v = &object["extras"]["primase"]["web.provenance"]["value"];
    let pick = |key: &str| {
        v[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| table[i.as_u64().expect("an index") as usize].clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    (pick("origins"), pick("parts"))
}

#[test]
fn tiles_share_one_origin_table() {
    for compress in [false, true] {
        let options = Options {
            compress,
            ..Options::default()
        };
        let set = write(&world(), &options).unwrap();
        assert_eq!(set.index.tiles.len(), 1);
        let bytes = &set.files[&set.index.tiles[0].file];
        assert_eq!(
            *bytes,
            write(&world(), &options).unwrap().files[&set.index.tiles[0].file]
        );
        let d = doc(bytes);
        assert!(!serde_json::to_string(&d).unwrap().contains("core.provenance"));
        let table = d["scenes"][0]["extras"]["primase"]["web.origins"]["value"]
            .as_array()
            .unwrap()
            .clone();
        // three drawable parts and five placements, each once
        assert_eq!(table.len(), 8, "{table:#?}");
        let o =
            |file: &str, item: &str| json!({"archive": "levels/city/block.rpf", "file": file, "item": item});
        let meshes = d["meshes"].as_array().unwrap();
        let by_file: BTreeMap<String, Vec<Value>> = meshes
            .iter()
            .map(|m| {
                let (origins, parts) = resolve(&table, m);
                (origins[0]["file"].as_str().unwrap().to_owned(), parts)
            })
            .collect();
        assert_eq!(
            by_file["a.ydr"],
            [o("a.ydr", "model 0 geometry 0"), o("a.ydr", "model 0 geometry 1")]
        );
        assert_eq!(
            by_file["b.ydr"],
            [o("b.ydr", "model 0 geometry 0"), o("b.ydr", "model 0 geometry 0")]
        );

        // every placement keeps its own origin, instanced or not
        let mut placements = Vec::new();
        for node in d["nodes"].as_array().unwrap() {
            match node["extras"]["primase"]["web.instances"]["value"].as_array() {
                Some(instances) => placements.extend(instances.iter().map(|i| resolve(&table, i).0)),
                None => placements.push(resolve(&table, node).0),
            }
        }
        let mut items: Vec<String> = placements
            .iter()
            .map(|p| p[0]["item"].as_str().unwrap().to_owned())
            .collect();
        items.sort();
        assert_eq!(
            items,
            ["entity 0", "entity 1", "entity 2", "entity 3", "entity 4"]
        );
    }
}

#[test]
fn tiles_without_provenance_have_no_table() {
    let mut w = world();
    let held: Vec<_> = w.iter::<Provenance>().map(|(e, _)| e).collect();
    for e in held {
        w.remove::<Provenance>(e);
    }
    let set = write(&w, &Options::default()).unwrap();
    let text = String::from_utf8_lossy(
        primase_gltf::glb::parse(&set.files[&set.index.tiles[0].file])
            .unwrap()
            .json,
    )
    .into_owned();
    assert!(!text.contains("provenance") && !text.contains("web.origins"));
}
