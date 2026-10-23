//! Compiled-program gate against an independent reader (`oracles.unitypy.sledding/shader-programs.json`, JSON:
//! `shaders`, each `file`, `path_id`, `name` and `passes[subshader][pass]`: `variants` (each `group`, `blob_index`,
//! `keywords`, `gpu_program_type`, `requirements` and `code`, the sha256 of its D3D11 code or null) and `parameters`
//! (`constant_buffers` each `name`, `slot`, `size`, `fields` by offset; `textures`; `samplers`)), for the player
//! `installs.sledding/Sledding Game_Data` read with `tools.unity_tpk`: every shader's programs exactly, every variant's
//! code bytes.

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use primase_unity::TypeTreePackage;
use primase_unity::player::{ObjRef, Player};
use primase_unity::program::{D3D11_PROGRAM_TYPES, ShaderPrograms, Stage};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

#[test]
fn shader_programs_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        install_file(Game::Sledding, "Sledding Game_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Sledding, "shader-programs.json"),
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
                            let p = &p.fragment;
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

/// The programs of every stage of every pass against the same independent reader's dump of all five stages
/// (`shader_programs_stages.json`, beside the file `oracles.unitypy.sledding/shader-programs.json` names: per pass
/// `vertex`, `fragment`, `geometry`, `hull` and `domain`, each as above). The oracle could not parse the code of two
/// entries (one `Hidden/FrameDebuggerRenderTargetDisplay` program of an unknown GPU program type, one it ran off the
/// end of): their code is not compared.
#[test]
fn programs_of_every_stage_match_the_oracle() {
    let (Some(data), Some(tpk), Some(programs)) = (
        install_file(Game::Sledding, "Sledding Game_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Sledding, "shader-programs.json"),
    ) else {
        return;
    };
    all_stages(
        "Sledding Game",
        &data,
        &tpk,
        &programs.with_file_name("shader_programs_stages.json"),
    );
}

/// SkateBIRD (Unity 2019.4: the older `m_SubPrograms` layout, one list of variants with their tiers, keywords by name
/// index and parameters of their own) against the same reader's dump, `shader_programs_stages_skatebird.json`.
#[test]
fn skatebird_programs_of_every_stage_match_the_oracle() {
    let (Some(data), Some(tpk), Some(programs)) = (
        install_file(Game::Skatebird, "SkateBIRD_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Sledding, "shader-programs.json"),
    ) else {
        return;
    };
    all_stages(
        "SkateBIRD",
        &data,
        &tpk,
        &programs.with_file_name("shader_programs_stages_skatebird.json"),
    );
}

fn all_stages(label: &str, data: &std::path::Path, tpk: &std::path::Path, oracle: &std::path::Path) {
    let Ok(oracle) = std::fs::read(oracle) else {
        return;
    };
    let player = Player::open(
        data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let oracle: Json = serde_json::from_slice(&oracle).unwrap();
    let mut gate = GateReport::new(
        format!("{label}: shader programs of all stages"),
        Tolerance::Exact,
    );
    let mut variants = [0usize; 5];
    let mut uncompared = 0;
    for t in oracle["shaders"].as_array().unwrap() {
        let r = ObjRef {
            file: t["file"].as_str().unwrap().to_owned(),
            path_id: t["path_id"].as_i64().unwrap(),
        };
        let item = format!("{}/{} {}", r.file, r.path_id, t["name"].as_str().unwrap());
        let ours = ShaderPrograms::read(&player, &r).unwrap();
        let mut same = ours.name == t["name"].as_str().unwrap();
        let subs = t["passes"].as_array().unwrap();
        same &= ours.passes.len() == subs.len();
        for (sub, expected_sub) in ours.passes.iter().zip(subs) {
            let expected_sub = expected_sub.as_array().unwrap();
            same &= sub.len() == expected_sub.len();
            for (pass, expected) in sub.iter().zip(expected_sub) {
                for (i, (stage, key)) in [
                    (Stage::Vertex, "vertex"),
                    (Stage::Fragment, "fragment"),
                    (Stage::Geometry, "geometry"),
                    (Stage::Hull, "hull"),
                    (Stage::Domain, "domain"),
                ]
                .into_iter()
                .enumerate()
                {
                    let p = pass.stage(stage);
                    let vs: Vec<Json> = p
                        .variants
                        .iter()
                        .map(|v| {
                            variants[i] += 1;
                            let mut j = serde_json::to_value(v).unwrap();
                            let hash = (ours.platforms.contains(&4)
                                && D3D11_PROGRAM_TYPES.contains(&v.gpu_program_type))
                            .then(|| match ours.code(4, v.blob_index) {
                                Ok(code) => Sha256::digest(code)
                                    .iter()
                                    .map(|b| format!("{b:02x}"))
                                    .collect::<String>(),
                                Err(e) => {
                                    println!("unreadable code: {item} {stage:?} blob {}: {e}", v.blob_index);
                                    String::new()
                                }
                            });
                            j["code"] = json!(hash);
                            j
                        })
                        .collect();
                    let mut theirs = expected[key].clone();
                    for v in theirs["variants"].as_array_mut().unwrap() {
                        if !D3D11_PROGRAM_TYPES.contains(&v["gpu_program_type"].as_i64().unwrap()) {
                            v["code"] = Json::Null;
                        } else if v["code"].is_null() {
                            // the oracle could not read this entry
                            uncompared += 1;
                            v["code"] = json!(ours.platforms.contains(&4).then(|| {
                                ours.code(4, v["blob_index"].as_u64().unwrap() as u32)
                                    .map(|c| {
                                        Sha256::digest(c)
                                            .iter()
                                            .map(|b| format!("{b:02x}"))
                                            .collect::<String>()
                                    })
                                    .unwrap_or_default()
                            }));
                        }
                    }
                    same &= json!(vs) == theirs["variants"]
                        && serde_json::to_value(&p.parameters).unwrap() == theirs["parameters"];
                }
            }
        }
        if same {
            gate.record(&item, 0.0);
        } else {
            gate.fail(&item, "programs differ".to_string());
        }
    }
    println!(
        "{gate}; variants by stage (vertex, fragment, geometry, hull, domain) {variants:?}, {uncompared} codes not compared"
    );
    gate.assert_passed();
}
