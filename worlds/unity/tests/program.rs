//! Compiled-program gate against an independent reader (`PRIMASE_UNITY_PROGRAMS`, JSON: `shaders`, each `file`,
//! `path_id`, `name` and `passes[subshader][pass]`: `variants` (each `group`, `blob_index`, `keywords`,
//! `gpu_program_type`, `requirements` and `code`, the sha256 of its D3D11 code or null) and `parameters`
//! (`constant_buffers` each `name`, `slot`, `size`, `fields` by offset; `textures`; `samplers`)), for the player
//! `PRIMASE_UNITY_DATA` read with `PRIMASE_UNITY_TPK`: every shader's programs exactly, every variant's code bytes.

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::real_data;
use primase_unity::TypeTreePackage;
use primase_unity::player::{ObjRef, Player};
use primase_unity::program::ShaderPrograms;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

#[test]
fn shader_programs_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_PROGRAMS"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let oracle: Json = serde_json::from_slice(&std::fs::read(&oracle).unwrap()).unwrap();
    let mut gate = GateReport::new(
        "shader programs: variants, common parameters, code",
        Tolerance::Exact,
    );
    let (mut variants, mut code) = (0usize, 0usize);
    for t in oracle["shaders"].as_array().unwrap() {
        let r = ObjRef {
            file: t["file"].as_str().unwrap().to_owned(),
            path_id: t["path_id"].as_i64().unwrap(),
        };
        let item = format!("{}/{} {}", r.file, r.path_id, t["name"].as_str().unwrap());
        let ours = ShaderPrograms::read(&player, &r).unwrap();
        let passes: Vec<Json> = ours
            .passes
            .iter()
            .map(|sub| {
                Json::Array(
                    sub.iter()
                        .map(|p| {
                            let vs: Vec<Json> = p
                                .variants
                                .iter()
                                .map(|v| {
                                    variants += 1;
                                    let hash = ours.platforms.contains(&4).then(|| {
                                        code += 1;
                                        let bytes = ours.code(4, v.blob_index).unwrap();
                                        Sha256::digest(&bytes)
                                            .iter()
                                            .map(|b| format!("{b:02x}"))
                                            .collect::<String>()
                                    });
                                    let mut j = serde_json::to_value(v).unwrap();
                                    j["code"] = json!(hash);
                                    j
                                })
                                .collect();
                            json!({"variants": vs, "parameters": serde_json::to_value(&p.parameters).unwrap()})
                        })
                        .collect(),
                )
            })
            .collect();
        let mine = json!(passes);
        if mine == t["passes"] && ours.name == t["name"].as_str().unwrap() {
            gate.record(&item, 0.0);
        } else {
            gate.fail(&item, "programs differ".to_string());
        }
    }
    println!("{gate}; {variants} variants, {code} with code");
    gate.assert_passed();
}
