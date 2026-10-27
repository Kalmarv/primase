//! `Stored<T>` shows a record in a world's JSON exactly as the record serialises: the wrapper adds nothing.
use primase_core::{Component, Resource};
use primase_shred::components::{
    LevelParams, MaterialResolution, PixelProgram, SessionGraph, ShredMaterial, Stored,
};
use rme_formats::{
    level_params::{LevelParams as Params, ScriptCall, ScriptValue},
    library::{BankEvidence, MaterialResolution as Resolution},
    material::{MaterialKey, ShredMaterial as Material},
    pixel_program::PixelProgram as Class,
    session::SessionGraph as Graph,
};
use serde_json::json;

#[test]
fn a_stored_component_serialises_as_its_record() {
    let class = Stored(Class::ScaledTexture {
        unit: 2,
        uniforms: vec!["GlobalBrightness".into()],
    });
    assert_eq!(
        Component::to_json(&class).unwrap(),
        Some(json!({"ScaledTexture": {"unit": 2, "uniforms": ["GlobalBrightness"]}}))
    );
    let unclassified: PixelProgram = Stored(Class::Unclassified);
    assert_eq!(
        Component::to_json(&unclassified).unwrap(),
        Some(json!("Unclassified"))
    );

    let key = MaterialKey {
        bank: "actors".into(),
        id: 7,
    };
    let material: ShredMaterial = Stored(Material {
        key: key.clone(),
        unknown_header: [1, 2, 3, 4],
        vertex_shader: "v".into(),
        fragment_shader: "p".into(),
        parameters: vec![],
        samplers: vec![],
    });
    assert_eq!(
        Component::to_json(&material).unwrap(),
        Some(json!({
            "key": {"bank": "actors", "id": 7},
            "unknown_header": [1, 2, 3, 4],
            "vertex_shader": "v",
            "fragment_shader": "p",
            "parameters": [],
            "samplers": [],
        }))
    );

    let resolution: MaterialResolution = Stored(Resolution {
        candidates: vec![key.clone()],
        selected: Some(key),
        evidence: Some(BankEvidence::UniqueId),
    });
    assert_eq!(
        Component::to_json(&resolution).unwrap(),
        Some(json!({
            "candidates": [{"bank": "actors", "id": 7}],
            "selected": {"bank": "actors", "id": 7},
            "evidence": "UniqueId",
        }))
    );
}

#[test]
fn a_stored_resource_serialises_as_its_record() {
    let params: LevelParams = Stored(Params {
        level: "zulu".into(),
        script: "script".into(),
        calls: vec![ScriptCall {
            function: "change_fog".into(),
            args: vec![ScriptValue::Number(400.0), ScriptValue::Text("x".into())],
        }],
    });
    assert_eq!(
        Resource::to_json(&params).unwrap(),
        Some(json!({
            "level": "zulu",
            "script": "script",
            "calls": [{"function": "change_fog", "args": [{"Number": 400.0}, {"Text": "x"}]}],
        }))
    );
    let db = include_bytes!("fixtures/sessions.db");
    let graph: SessionGraph = Stored(Graph::from_main_db(db, 7).unwrap());
    let value = Resource::to_json(&graph).unwrap().unwrap();
    assert_eq!(value, serde_json::to_value(&graph.0).unwrap());
    assert_eq!(value["level_id"], json!(7));
}
