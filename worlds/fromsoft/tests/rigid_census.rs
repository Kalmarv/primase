//! Every model an area's MSB places against a SoulsFormats dump of the same FLVERs in model space (each static
//! mesh's vertices through the bone named by NormalW, from SoulsFormats' bone transforms; the dump lists the models).
//! Position error <=0.2 mm; normal direction error <=2e-5 per component: the dump's normal is the raw inverse transpose
//! (its length follows the bone's scale), ours of unit length under a bone that moves it and as stored (a quantised
//! normal, near unit) under an identity bone, so both are normalised before the comparison, allowing the dump's f32
//! matrices against our f64 evaluation. The census is complete in both directions: the models the
//! dump lists (and the ones it could not dump, with why) are exactly the models its MSB parts place, which are exactly
//! the models our MSB reader's parts place.
use primase_core::pass::PassLog;
use primase_fromsoft::msb::{MapLayout, PartKind};
use primase_fromsoft::{Archive, Binder, dcx::Decoder, flver::Model, rigid::lower_model};
use primase_testdata::{Game, OracleSet, Tool, install, oracle, tool};
use std::collections::BTreeSet;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[test]
#[allow(unsafe_code)]
fn full_castle_rigid_vertices() -> Result<()> {
    let Some(root) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(dll) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    let Some(oracle) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "m11_01") else {
        return Ok(());
    };
    // SAFETY: explicitly supplied trusted native decoder.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let jsonl = |name: &str| -> Result<Vec<serde_json::Value>> {
        std::fs::read_to_string(oracle.join(name))?
            .lines()
            .map(|l| Ok(serde_json::from_str(l)?))
            .collect()
    };
    let keys = |rows: &[serde_json::Value]| -> Result<BTreeSet<String>> {
        rows.iter()
            .map(|r| Ok(r["key"].as_str().ok_or("key")?.to_owned()))
            .collect()
    };
    let area = "m11_01_00_00";
    // the models the dump's parts place, and the ones our MSB reader's parts place
    let placed_by_dump: BTreeSet<String> = jsonl("parts.jsonl")?
        .iter()
        .map(|p| {
            let model = p["model"].as_str().ok_or("model")?;
            Ok(match p["kind"].as_str().ok_or("kind")? {
                "object" => format!("obj/{model}"),
                "piece" => format!("{area}/{model}"),
                k => return Err(format!("part kind {k}").into()),
            })
        })
        .collect::<Result<_>>()?;
    let layout = MapLayout::read(&archive.read(&format!("/map/mapstudio/{area}.msb.dcx"), &decoder)?)?;
    let placed_by_us: BTreeSet<String> = layout
        .parts
        .iter()
        .filter_map(|p| {
            let model = &layout.models[p.model?].name;
            match p.kind {
                PartKind::Object => Some(format!("obj/{model}")),
                PartKind::MapPiece => Some(format!("{area}/{model}")),
                _ => None,
            }
        })
        .collect();
    let dumped = keys(&jsonl("models.jsonl")?)?;
    // models the dump names but could not read as rigid geometry, each with its reason
    let undumped = keys(&jsonl("missing-rigid.jsonl")?)?;
    assert!(dumped.is_disjoint(&undumped));
    let listed: BTreeSet<String> = dumped.union(&undumped).cloned().collect();
    assert_eq!(
        listed, placed_by_dump,
        "the dump's model list against its parts' models"
    );
    assert_eq!(
        placed_by_us, placed_by_dump,
        "our MSB parts' models against the dump's"
    );
    println!(
        "census: {} placed models, {} dumped, {} not dumped {undumped:?}",
        placed_by_dump.len(),
        dumped.len(),
        undumped.len()
    );
    let mut models = 0;
    let mut vertices = 0;
    let mut log = PassLog::default();
    let mut max_position = 0f64;
    let mut max_normal = 0f64;
    let mut exact = 0;
    for line in std::fs::read_to_string(oracle.join("models.jsonl"))?.lines() {
        let row: serde_json::Value = serde_json::from_str(line)?;
        let key = row["key"].as_str().ok_or("key")?;
        let (area, name) = key.split_once('/').ok_or("key")?;
        let name = if area == "obj" {
            name.to_owned()
        } else {
            format!("{area}_{}", &name[1..])
        };
        let path = if area == "obj" {
            format!("/obj/{name}.objbnd.dcx")
        } else {
            format!("/map/{area}/{name}.mapbnd.dcx")
        };
        let binder = Binder::read(&archive.read(&path, &decoder)?, &decoder)?;
        let file = binder
            .files
            .iter()
            .find(|f| {
                f.name.as_deref().unwrap_or("").rsplit(['/', '\\']).next() == Some(&format!("{name}.flver"))
            })
            .ok_or("FLVER")?;
        let raw = Model::read(&file.bytes)?;
        let model = lower_model(&raw, &mut log).map_err(|e| format!("{key}: {e}"))?;
        let bytes = std::fs::read(oracle.join(row["file"].as_str().ok_or("file")?))?;
        let mut words = bytes.as_chunks::<4>().0.iter().map(|v| u32::from_le_bytes(*v));
        assert_eq!(words.next(), Some(model.meshes.len() as u32));
        for mesh in &model.meshes {
            assert_eq!(words.next(), Some(mesh.vertices.len() as u32));
            for v in &mesh.vertices {
                let mut want = [0f64; 6];
                for w in &mut want {
                    *w = f64::from(f32::from_bits(words.next().ok_or("short oracle")?));
                }
                let mut got = [0f64; 6];
                for (g, &x) in got.iter_mut().zip(v.position.iter().chain(&v.normal)) {
                    *g = f64::from(x);
                }
                for side in [&mut want, &mut got] {
                    let length = side[3..].iter().map(|x| x * x).sum::<f64>().sqrt();
                    if length > 0. {
                        for x in &mut side[3..] {
                            *x /= length;
                        }
                    }
                }
                for (i, (got, want)) in got.into_iter().zip(want).enumerate() {
                    let d = (got - want).abs();
                    exact += usize::from(got == want);
                    if i < 3 {
                        max_position = max_position.max(d);
                        assert!(d <= 2e-4, "{key} position {got} vs {want}");
                    } else {
                        max_normal = max_normal.max(d);
                        assert!(d <= 2e-5, "{key} normal {got} vs {want} (normalised)");
                    }
                }
                vertices += 1;
            }
        }
        assert!(words.next().is_none());
        models += 1;
    }
    println!(
        "placed rigid models: {models} models, {vertices} vertices, {exact} exact components; max position {max_position} m, max normal {max_normal}; counts {:?}",
        log.counts
    );
    assert_eq!(models, dumped.len(), "every dumped model compared");
    Ok(())
}
