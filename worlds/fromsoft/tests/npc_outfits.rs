//! FromSoftware NPC outfits against other sources, for both games.
//!
//! - The NPC param as SoulsFormats' PARAM reads it with Paramdex's definition (`oracles.soulsformats.sekiro/npc`'s
//!   `npcparam.json`; `oracles.soulsformats.elden/npc.jsonl`, the regulation decrypted by SoulsFormats'
//!   `RegulationDecryptor` and its definition applied at the regulation's version): the file's header facts, every
//!   row's id and all 32 `modelDispMask` cells of every row, against ours.
//! - The rows' link to characters: every Enemy and DummyEnemy part of the maps (SoulsFormats' MSBS and MSBE, in the
//!   same oracle sets) against [`fromsoft_formats::game_params::npc_placements`]'s model name and NPC row.
//! - Three Sekiro characters' outfits through the passes (`raise-fromsoft-display-masks`, `select-fromsoft-outfit`,
//!   `resolve-state`) against the manifests of the old C# exporter (`oracles.baseline.sekiro/outfits`, our own earlier
//!   output: it detects a change of the selection and checks no correctness): each mesh's material and mark, the masks
//!   the chosen row shows, and which meshes were kept. The material names are also checked against SoulsFormats' FLVER,
//!   and every other row of one character is chosen from SoulsFormats' mask cells and the baseline's marks.
//! - An Elden Ring character's outfits (`c3000`, whose weapons are mask groups) through the passes for every row its map
//!   parts name: the meshes drawn are those SoulsFormats' cells of the row show, and each mask is traced to the row.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use primase_fromsoft::game::ParamKey;
use std::collections::{BTreeMap, BTreeSet};

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::param::NpcParam;
use fromsoft_formats::{Game, Install, Keys, NameDictionary};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Entity, World};
use primase_fromsoft::rigid::SourceModel;
use primase_fromsoft::sekiro::lowered::SourceMeshes;
use primase_fromsoft::sekiro::source::{MeshMasks, NpcRows};
use primase_fromsoft::sekiro::{self, CharacterOptions};
use primase_testdata::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use serde_json::Value;

mod common;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn json(path: impl AsRef<std::path::Path>) -> Result<Value> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(text.trim_start_matches('\u{feff}'))?)
}

/// The oracle's `modelDispMask0..31` cells of a row as one word.
fn oracle_mask(fields: &Value) -> Result<u32> {
    let mut mask = 0u32;
    for k in 0..32 {
        let cell = fields[format!("modelDispMask{k}")]
            .as_u64()
            .ok_or_else(|| format!("no modelDispMask{k} cell"))?;
        assert!(cell <= 1, "modelDispMask{k} is {cell}");
        mask |= (cell as u32) << k;
    }
    Ok(mask)
}

/// A configured install of `game` with the decoder's library, or `None` when the configuration lacks one.
fn configured(game: Game) -> Result<Option<(Install, primase_testdata::OodleLibrary)>> {
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(None);
    };
    let data = match game {
        Game::Sekiro => Data::Sekiro,
        Game::EldenRing => Data::Elden,
    };
    Ok(match (install(data), keys(data)) {
        (Some(dir), Some(k)) => Some((
            Install::new(dir, game, Keys::from_folder(k, game, game.param_key_sha256())?),
            dll,
        )),
        _ => None,
    })
}

fn dictionary(game: Game) -> Result<Option<NameDictionary>> {
    let data = match game {
        Game::Sekiro => Data::Sekiro,
        Game::EldenRing => Data::Elden,
    };
    oracle(OracleSet::Uxm, data, "dictionary.txt")
        .map(|p| NameDictionary::read_for(p, game))
        .transpose()
        .map_err(Into::into)
}

#[test]
fn the_sekiro_npc_param_reads_as_soulsformats_reads_it() -> Result<()> {
    let (Some((install, dll)), Some(oracle)) = (
        configured(Game::Sekiro)?,
        oracle(OracleSet::SoulsFormats, Data::Sekiro, "npc"),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let archive = install.open()?;
    let param = fromsoft_formats::game_params::read_param(&install, &archive, &decoder, "NpcParam.param")?;
    let o = json(oracle.join("npcparam.json"))?;
    assert_eq!(Some(param.param_type.as_str()), o["paramType"].as_str());
    assert_eq!(Some(u64::from(param.data_version)), o["dataVersion"].as_u64());
    assert_eq!(Some(u64::from(param.unk06)), o["unk06"].as_u64());
    // the layout the mask offsets are for: SoulsFormats applied the 896-byte definition to this file's rows
    assert_eq!(Some(param.row_size as u64), o["detectedSize"].as_u64());
    assert_eq!(Some(param.row_size as u64), o["paramdefRowSize"].as_u64());
    let rows = o["rows"].as_array().ok_or("no rows")?;
    assert_eq!(param.rows.len(), rows.len());
    let npc = NpcParam::from_param(&param, &Game::Sekiro.npc_param())?;
    assert_eq!(npc.rows.len(), rows.len());
    for ((ours, theirs), typed) in param.rows.iter().zip(rows).zip(&npc.rows) {
        let id = theirs["id"].as_i64().ok_or("row id")?;
        assert_eq!(i64::from(ours.id), id);
        assert_eq!(i64::from(typed.id), id);
        assert_eq!(ours.name.as_deref(), theirs["name"].as_str(), "row {id}: name");
        assert_eq!(
            typed.display_mask,
            oracle_mask(&theirs["fields"])?,
            "row {id}: display masks"
        );
    }
    // the masks are not all alike, so the comparison above could not pass by reading nothing
    let distinct: BTreeSet<u32> = npc.rows.iter().map(|r| r.display_mask).collect();
    assert!(distinct.len() > 100, "{} distinct mask words", distinct.len());
    Ok(())
}

/// The material names of a character as SoulsFormats reads its FLVER: by index, and each mesh's material index.
struct OracleMaterials {
    names: Vec<String>,
    mesh_material: Vec<usize>,
}

fn oracle_materials(dir: &std::path::Path, id: &str) -> Result<OracleMaterials> {
    let o = json(dir.join(format!("{id}.materials.json")))?;
    let names = o["materials"]
        .as_array()
        .ok_or("materials")?
        .iter()
        .map(|m| m["name"].as_str().map(str::to_owned).ok_or("material name"))
        .collect::<std::result::Result<_, _>>()?;
    let mesh_material = o["meshes"]
        .as_array()
        .ok_or("meshes")?
        .iter()
        .map(|m| {
            m["materialIndex"]
                .as_u64()
                .map(|i| i as usize)
                .ok_or("material index")
        })
        .collect::<std::result::Result<_, _>>()?;
    Ok(OracleMaterials { names, mesh_material })
}

/// One mesh of the old route's manifest.
struct Old {
    material_index: usize,
    material: String,
    mask: Option<u32>,
    reason: String,
}

struct Baseline {
    masks: BTreeSet<u32>,
    meshes: Vec<Old>,
}

fn baseline(dir: &std::path::Path, id: &str) -> Result<Baseline> {
    let o = json(dir.join(format!("{id}.json")))?;
    assert_eq!(o["character"].as_str(), Some(id));
    let masks = o["options"]["DisplayMasks"]
        .as_array()
        .ok_or("DisplayMasks")?
        .iter()
        .map(|m| m.as_u64().map(|m| m as u32).ok_or("mask"))
        .collect::<std::result::Result<_, _>>()?;
    let meshes = o["meshes"]
        .as_array()
        .ok_or("meshes")?
        .iter()
        .enumerate()
        .map(|(i, m)| {
            assert_eq!(m["sourceMeshIndex"].as_u64(), Some(i as u64));
            Ok(Old {
                material_index: m["materialIndex"].as_u64().ok_or("materialIndex")? as usize,
                material: m["material"].as_str().ok_or("material")?.to_owned(),
                mask: m["displayMask"].as_u64().map(|m| m as u32),
                reason: m["reason"].as_str().ok_or("reason")?.to_owned(),
            })
        })
        .collect::<Result<_>>()?;
    Ok(Baseline { masks, meshes })
}

fn manager() -> PassManager {
    let mut reg = PassRegistry::new();
    primase_core::register(&mut reg);
    sekiro::passes::register(&mut reg);
    PassManager::new(reg).with_pass_checks(true)
}

/// The character's prototype: the entity holding its model.
fn prototype(world: &World) -> Result<Entity> {
    Ok(world.iter::<SourceModel>().next().ok_or("no model")?.0)
}

const LOWER: &str = "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,\
                     blocker-cards=draw,speedtree-frame=program}, raise-fromsoft-display-masks";

/// The FLVER meshes the primitives of the character's finest form were lowered from.
fn lowered_from(world: &World, p: Entity) -> Result<BTreeSet<usize>> {
    Ok(world
        .get::<SourceMeshes>(p)
        .ok_or("no source meshes")?
        .0
        .iter()
        .map(|&m| m as usize)
        .collect())
}

/// Lower the read `world` and raise its masks; then choose row `npc`. The meshes lowered, the meshes still drawn, and
/// the choosing run's report.
fn choose(
    world: &mut World,
    p: Entity,
    id: &str,
    npc: i32,
    extra: &str,
) -> Result<(BTreeSet<usize>, BTreeSet<usize>, RunReport)> {
    let manager = manager();
    let mut report = RunReport::default();
    manager.run(world, &manager.registry.parse(LOWER)?, &mut report)?;
    let lowered = lowered_from(world, p)?;
    let mut report = RunReport::default();
    manager.run(
        world,
        &manager.registry.parse(&format!(
            "select-fromsoft-outfit{{chr={id},npc={npc}{extra}}}, resolve-state"
        ))?,
        &mut report,
    )?;
    let drawn = lowered_from(world, p)?;
    Ok((lowered, drawn, report))
}

/// What a mesh with the mark `mask` shows for a row's `bits`.
fn shown(mask: Option<u32>, bits: u32) -> bool {
    mask.is_none_or(|k| bits >> k & 1 == 1)
}

#[test]
fn sekiro_outfits_agree_with_the_old_route_and_with_soulsformats() -> Result<()> {
    let (Some((install, dll)), Some(oracle), Some(old)) = (
        configured(Game::Sekiro)?,
        oracle(OracleSet::SoulsFormats, Data::Sekiro, "npc"),
        primase_testdata::oracle(OracleSet::Baseline, Data::Sekiro, "outfits"),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let param = json(oracle.join("npcparam.json"))?;
    let oracle_rows: BTreeMap<i64, u32> = param["rows"]
        .as_array()
        .ok_or("rows")?
        .iter()
        .map(|r| Ok((r["id"].as_i64().ok_or("id")?, oracle_mask(&r["fields"])?)))
        .collect::<Result<_>>()?;
    let read = |id: &str| -> Result<(World, sekiro::CharacterReport)> {
        let mut w = World::new();
        let report =
            sekiro::read_character_with(&install, id, &decoder, CharacterOptions::default(), &mut w)?;
        Ok((w, report))
    };
    for (id, npc) in [("c1010", 10100000), ("c1020", 10200000), ("c1050", 10500000)] {
        let (read, report) = read(id)?;
        let p = prototype(&read)?;
        let model = read.get::<SourceModel>(p).ok_or("model")?.0.clone();
        let marks = read.get::<MeshMasks>(p).ok_or("marks")?.0.clone();
        let before = baseline(&old, id)?;
        let theirs = oracle_materials(&oracle, id)?;
        assert_eq!(model.meshes.len(), before.meshes.len(), "{id}: mesh count");
        assert_eq!(model.meshes.len(), theirs.mesh_material.len(), "{id}: mesh count");
        assert_eq!(
            marks.iter().filter(|m| !m.is_empty()).count(),
            report.masked_meshes,
            "{id}"
        );
        // each mesh's material name is SoulsFormats', and its mark is the old route's
        for (i, mesh) in model.meshes.iter().enumerate() {
            let name = &model.materials[mesh.material].name;
            assert_eq!(
                mesh.material, theirs.mesh_material[i],
                "{id} mesh {i}: material index"
            );
            assert_eq!(name, &theirs.names[mesh.material], "{id} mesh {i}: material name");
            assert_eq!(mesh.material, before.meshes[i].material_index, "{id} mesh {i}");
            assert_eq!(name, &before.meshes[i].material, "{id} mesh {i}");
            assert_eq!(
                marks[i],
                before.meshes[i].mask.into_iter().collect::<Vec<_>>(),
                "{id} mesh {i} ({name:?}): mark"
            );
        }
        // the row's masks are the ones the old route showed, and SoulsFormats' cells for the row
        let rows = &read.resource::<NpcRows>().ok_or("no rows")?.0;
        let row = rows.iter().find(|r| r.id == npc).ok_or("row missing")?;
        let bits: BTreeSet<u32> = (0..32).filter(|k| row.display_mask >> k & 1 == 1).collect();
        assert_eq!(bits, before.masks, "{id}: row {npc}'s masks");
        assert_eq!(oracle_rows[&i64::from(npc)], row.display_mask, "{id}: row {npc}");

        // the outfit through the passes against the old route's selection
        let mut world = read.clone();
        let (lowered, drawn, run) = choose(&mut world, p, id, npc, "")?;
        let dropped: BTreeSet<usize> = lowered.difference(&drawn).copied().collect();
        let expected_dropped: BTreeSet<usize> = before
            .meshes
            .iter()
            .enumerate()
            .filter(|(i, m)| {
                lowered.contains(i)
                    && match m.reason.as_str() {
                        "included" => false,
                        "hidden appearance mask" => true,
                        // left out by the old route's bone-subtree rule before its mask was looked at
                        "equipment bone subtree" => !shown(m.mask, row.display_mask),
                        other => panic!("{id}: reason {other}"),
                    }
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(dropped, expected_dropped, "{id}: dropped meshes");
        assert!(
            lowered.len() * 10 >= model.meshes.len() * 9,
            "{id}: only {} of {} meshes lowered",
            lowered.len(),
            model.meshes.len()
        );
        assert_eq!(world.verify_components(), vec![], "{id}");
        let counts = &run.steps.last().ok_or("steps")?.log.counts;
        assert!(
            counts["primitives dropped by their condition"] >= dropped.len() as u64,
            "{id}"
        );
        assert!(
            !dropped.is_empty(),
            "{id}: nothing was hidden, so nothing was tested"
        );
        assert!(!drawn.is_empty(), "{id}");
    }

    // every row of one character: the drawn meshes from SoulsFormats' cells and the old route's marks
    let (read, _) = read("c1050")?;
    let before = baseline(&old, "c1050")?;
    let p = prototype(&read)?;
    let mut outfits = BTreeSet::new();
    for (&npc, &bits) in oracle_rows.range(10500000..10510000) {
        let mut world = read.clone();
        let (lowered, drawn, _) = choose(&mut world, p, "c1050", npc as i32, "")?;
        let want: BTreeSet<usize> = before
            .meshes
            .iter()
            .enumerate()
            .filter(|(i, m)| lowered.contains(i) && shown(m.mask, bits))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(drawn, want, "row {npc}");
        outfits.insert(drawn);
    }
    assert!(outfits.len() >= 3, "{} distinct outfits", outfits.len());

    // equipment named by mask goes, on top of the outfit
    let mut world = read.clone();
    let (_, drawn, _) = choose(&mut world, p, "c1050", 10500000, ",drop-masks=0")?;
    assert!(!drawn.contains(&0), "mesh 0 is marked 0");
    Ok(())
}

/// Every Enemy and DummyEnemy part of Sekiro's maps as [`fromsoft_formats::game_params::npc_placements`] reads them is SoulsFormats'. A row's
/// id is the character number times 10000 plus a variant for most parts; these are the exceptions, which is why
/// `select-fromsoft-outfit` takes the character as an option instead of deriving it.
#[test]
fn sekiro_placements_name_the_rows_soulsformats_reads() -> Result<()> {
    let (Some((install, dll)), Some(oracle), Some(names)) = (
        configured(Game::Sekiro)?,
        oracle(OracleSet::SoulsFormats, Data::Sekiro, "npc"),
        dictionary(Game::Sekiro)?,
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let archive = install.open()?;
    let rows: BTreeSet<i32> = fromsoft_formats::game_params::read_npc_param(&install, &archive, &decoder)?
        .rows
        .iter()
        .map(|r| r.id)
        .collect();
    let ours: Vec<(String, String, String, i32)> =
        fromsoft_formats::game_params::npc_placements(&archive, &decoder, &names)?
            .into_iter()
            .map(|p| {
                (
                    format!("/map/mapstudio/{}.msb.dcx", p.map),
                    p.part,
                    p.model,
                    p.row,
                )
            })
            .collect();
    let mut theirs = Vec::new();
    for line in std::fs::read_to_string(oracle.join("enemies.jsonl"))?.lines() {
        let part: Value = serde_json::from_str(line.trim_start_matches('\u{feff}'))?;
        theirs.push((
            part["map"].as_str().ok_or("map")?.to_owned(),
            part["name"].as_str().ok_or("name")?.to_owned(),
            part["model"].as_str().ok_or("model")?.to_owned(),
            part["npcParamId"].as_i64().ok_or("npcParamId")? as i32,
        ));
    }
    let (mut a, mut b) = (ours.clone(), theirs);
    a.sort();
    b.sort();
    assert_eq!(a, b);
    let (mut characters, mut agree, mut without_row, mut other) = (0, 0, 0, 0);
    let mut exceptions = BTreeSet::new();
    for (_, _, model, id) in &ours {
        let number = model
            .strip_prefix('c')
            .filter(|n| n.len() == 4)
            .and_then(|n| n.parse::<i32>().ok());
        let Some(number) = number else {
            // a dummy placed with an object's model, with no row
            assert_eq!(*id, 0, "{model}");
            other += 1;
            continue;
        };
        characters += 1;
        if !rows.contains(id) {
            without_row += 1;
        }
        if id / 10000 == number {
            agree += 1;
        } else {
            exceptions.insert((model.clone(), *id));
        }
    }
    assert_eq!((ours.len(), characters, other), (1763, 1760, 3));
    assert_eq!(agree, 1738);
    // the one part naming a row the param does not have is a dummy enemy of c1110
    assert_eq!(without_row, 1);
    let known: BTreeSet<(String, i32)> = [
        ("c1000", 12600050),
        ("c1001", 10002000),
        ("c1001", 10002010),
        ("c1001", 10002020),
        ("c1001", 10002220),
        ("c1001", 10002280),
        ("c1001", 10008000),
        ("c7200", 72010000),
        ("c7300", 73010000),
        ("c7300", 73100000),
        ("c7400", 74010000),
    ]
    .into_iter()
    .map(|(m, id)| (m.to_owned(), id))
    .collect();
    assert_eq!(exceptions, known);
    Ok(())
}

/// The lines of the Elden Ring oracle (`npc.jsonl`): its header, its rows' mask words by id, and its parts (map path,
/// part name, model, row).
struct EldenOracle {
    header: Value,
    rows: Vec<(i32, Option<String>, u32)>,
    parts: Vec<(String, String, String, i32)>,
}

fn elden_oracle(path: &std::path::Path) -> Result<EldenOracle> {
    let (mut header, mut rows, mut parts) = (Value::Null, Vec::new(), Vec::new());
    for line in std::fs::read_to_string(path)?.lines() {
        let v: Value = serde_json::from_str(line.trim_start_matches('\u{feff}'))?;
        match v["kind"].as_str().ok_or("kind")? {
            "param" => header = v,
            "row" => {
                let mut mask = 0u32;
                for (k, cell) in v["masks"].as_array().ok_or("masks")?.iter().enumerate() {
                    let cell = cell.as_u64().ok_or("mask cell")?;
                    assert!(cell <= 1, "row {}: modelDispMask{k} is {cell}", v["id"]);
                    mask |= (cell as u32) << k;
                }
                rows.push((
                    v["id"].as_i64().ok_or("id")? as i32,
                    v["name"].as_str().map(str::to_owned),
                    mask,
                ));
            }
            "enemy" | "dummy" => parts.push((
                v["map"].as_str().ok_or("map")?.to_owned(),
                v["name"].as_str().ok_or("name")?.to_owned(),
                // a part naming no model is null there, the empty name in ours
                v["model"].as_str().unwrap_or_default().to_owned(),
                v["npc"].as_i64().ok_or("npc")? as i32,
            )),
            other => return Err(format!("line kind {other}").into()),
        }
    }
    Ok(EldenOracle { header, rows, parts })
}

#[test]
fn the_elden_ring_npc_param_and_placements_read_as_soulsformats_reads_them() -> Result<()> {
    let (Some((install, dll)), Some(oracle), Some(names)) = (
        configured(Game::EldenRing)?,
        oracle(OracleSet::SoulsFormats, Data::Elden, "npc.jsonl"),
        dictionary(Game::EldenRing)?,
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let o = elden_oracle(&oracle)?;
    let archive = install.open()?;
    let param = fromsoft_formats::game_params::read_param(&install, &archive, &decoder, "NpcParam.param")?;
    let h = &o.header;
    assert_eq!(Some(param.param_type.as_str()), h["paramType"].as_str());
    assert_eq!(Some(u64::from(param.data_version)), h["dataVersion"].as_u64());
    assert_eq!(Some(u64::from(param.unk06)), h["unk06"].as_u64());
    // SoulsFormats applied the definition at the regulation's version to rows of the size ours are
    assert_eq!(Some(param.row_size as u64), h["detectedSize"].as_u64());
    assert_eq!(Some(param.row_size as u64), h["paramdefRowSize"].as_u64());
    let npc = NpcParam::from_param(&param, &Game::EldenRing.npc_param())?;
    assert_eq!(npc.rows.len(), o.rows.len());
    for ((ours, typed), (id, name, mask)) in param.rows.iter().zip(&npc.rows).zip(&o.rows) {
        assert_eq!((ours.id, typed.id), (*id, *id));
        assert_eq!(&ours.name, name, "row {id}: name");
        assert_eq!(typed.display_mask, *mask, "row {id}: display masks");
    }
    let distinct: BTreeSet<u32> = npc.rows.iter().map(|r| r.display_mask).collect();
    assert!(distinct.len() > 100, "{} distinct mask words", distinct.len());

    let mut ours: Vec<(String, String, String, i32)> =
        fromsoft_formats::game_params::npc_placements(&archive, &decoder, &names)?
            .into_iter()
            .map(|p| {
                (
                    format!("/map/mapstudio/{}.msb.dcx", p.map),
                    p.part,
                    p.model,
                    p.row,
                )
            })
            .collect();
    let mut theirs = o.parts;
    ours.sort();
    theirs.sort();
    assert_eq!(ours.len(), theirs.len());
    assert_eq!(ours, theirs);
    assert!(ours.len() > 20000, "{} parts", ours.len());
    Ok(())
}

/// `c3000`'s outfits for every row its map parts name: the meshes drawn are those SoulsFormats' cells of the row show
/// (its weapons among them, marked `#00#` to `#12#`), and each of its masks is traced to the row's bit.
#[test]
fn an_elden_ring_character_wears_the_outfit_of_each_row_that_places_it() -> Result<()> {
    let (Some((install, dll)), Some(oracle)) = (
        configured(Game::EldenRing)?,
        oracle(OracleSet::SoulsFormats, Data::Elden, "npc.jsonl"),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let o = elden_oracle(&oracle)?;
    let cells: BTreeMap<i32, u32> = o.rows.iter().map(|(id, _, m)| (*id, *m)).collect();
    let rows: BTreeSet<i32> = o
        .parts
        .iter()
        .filter(|p| p.2 == "c3000" && cells.contains_key(&p.3))
        .map(|p| p.3)
        .collect();
    assert_eq!(rows.len(), 28, "{rows:?}");
    let mut read = World::new();
    sekiro::read_character_with(
        &install,
        "c3000",
        &decoder,
        CharacterOptions::default(),
        &mut read,
    )?;
    let p = prototype(&read)?;
    let marks = read.get::<MeshMasks>(p).ok_or("marks")?.0.clone();
    let model = read.get::<SourceModel>(p).ok_or("model")?.0.clone();
    // its weapons are its marked meshes `#00#` to `#12#`
    for (i, m) in model.meshes.iter().enumerate() {
        let name = &model.materials[m.material].name;
        if marks[i].first().is_some_and(|&k| k <= 12) {
            let mtd = &model.materials[m.material].mtd;
            assert!(
                mtd.contains("_WP_") || mtd.contains("_Thorn"),
                "mesh {i} {name}: {mtd}"
            );
        }
    }
    let mut outfits = BTreeSet::new();
    for &npc in &rows {
        let bits = cells[&npc];
        let mut world = read.clone();
        let (lowered, drawn, run) = choose(&mut world, p, "c3000", npc, "")?;
        let want: BTreeSet<usize> = lowered
            .iter()
            .copied()
            .filter(|&i| shown(marks[i].first().copied(), bits))
            .collect();
        assert_eq!(drawn, want, "row {npc}");
        // every mask of the model traced to the row's bit
        let select = &run.steps[0].log.counts;
        for k in marks.iter().flatten().collect::<BTreeSet<_>>() {
            let bit = bits >> k & 1;
            let trace = format!(
                "NpcParam {npc} modelDispMask{k}={bit} -> #{k:02}# {} (primitives)",
                if bit == 1 { "shown" } else { "hidden" }
            );
            assert!(
                select.contains_key(&trace),
                "row {npc}: no {trace:?} in {select:?}"
            );
        }
        outfits.insert(drawn);
    }
    assert_eq!(outfits.len(), 5, "distinct outfits");
    Ok(())
}
