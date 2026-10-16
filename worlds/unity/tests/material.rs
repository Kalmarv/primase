//! Material gate against an independent reader (`PRIMASE_UNITY_MATERIALS`, JSON: `materials` each `file`, `path_id`,
//! `name`, `shader` (its name), `textures` as [name, [file, path id] or null, scale, offset], `floats`, `ints`,
//! `colors` as [name, value], `keywords`, `tags`, `render_queue`), for the player `PRIMASE_UNITY_DATA` read with
//! `PRIMASE_UNITY_TPK`: every material's properties exactly (the floats are the same f32s).

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::real_data;
use primase_unity::TypeTreePackage;
use primase_unity::material::{UnityMaterialData, shader_name};
use primase_unity::player::{ObjRef, Player};
use serde_json::{Value as Json, json};

fn obj(r: &Option<ObjRef>) -> Json {
    r.as_ref().map_or(Json::Null, |r| json!([r.file, r.path_id]))
}

#[test]
fn materials_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_MATERIALS"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let oracle: Json = serde_json::from_slice(&std::fs::read(&oracle).unwrap()).unwrap();
    let mut gate = GateReport::new("materials: shader and every property", Tolerance::Exact);
    for t in oracle["materials"].as_array().unwrap() {
        let r = ObjRef {
            file: t["file"].as_str().unwrap().to_owned(),
            path_id: t["path_id"].as_i64().unwrap(),
        };
        let item = format!("{}/{} {}", r.file, r.path_id, t["name"].as_str().unwrap());
        let m = UnityMaterialData::read(&player, &r).unwrap();
        let shader = m.shader.as_ref().map(|s| shader_name(&player, s).unwrap());
        let f = |x: f32| json!(f64::from(x));
        let ours = json!({
            "file": r.file, "path_id": r.path_id, "name": m.name, "shader": shader,
            "textures": m.textures.iter().map(|(n, e)| json!([n, obj(&e.texture), [f(e.scale[0]), f(e.scale[1])],
                [f(e.offset[0]), f(e.offset[1])]])).collect::<Vec<_>>(),
            "floats": m.floats.iter().map(|(n, v)| json!([n, f(*v)])).collect::<Vec<_>>(),
            "ints": m.ints.iter().map(|(n, v)| json!([n, v])).collect::<Vec<_>>(),
            "colors": m.colors.iter().map(|(n, c)| json!([n, c.map(f)])).collect::<Vec<_>>(),
            "keywords": m.keywords, "tags": m.tags.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "render_queue": m.render_queue,
        });
        // the oracle writes Python floats; ours are the same f32s widened, so equal as numbers
        if ours == *t {
            gate.record(&item, 0.0);
        } else {
            let keys: Vec<&str> = ours
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, v)| t.get(k.as_str()) != Some(v))
                .map(|(k, _)| k.as_str())
                .collect();
            gate.fail(&item, format!("differ in {keys:?}"));
        }
    }
    println!("{gate}");
    gate.assert_passed();
}
