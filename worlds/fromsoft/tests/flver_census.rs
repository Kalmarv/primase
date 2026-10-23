//! Whole-install FLVER attributes and strip ordering against an independent decoder, for each game configured:
//! SoulsFormats' census of Sekiro (`oracles.soulsformats.sekiro/census`), SoulsFormatsNEXT's of Elden Ring
//! (`oracles.soulsformats.elden/census`). A FLVER the reference refuses (a record with `error`) must be refused by ours
//! too.
use primase_fromsoft::{Binder, Game, NameDictionary, bhd::PathHash, dcx::Decoder, flver::Model};

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;
#[allow(unsafe_code)]
mod native {
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
    }
}
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02X}")).collect()
}
#[derive(Default)]
struct Counts {
    models: usize,
    meshes: usize,
    normal_components: u64,
    normal_changed: u64,
    normal_max_ulp: u32,
    agreed_rejections: usize,
}
/// The independent decoder divides by 127; the game multiplies by its rounded
/// reciprocal. Recover the unique encoded integer only after checking the
/// consumer result bit-for-bit. Project to the reference's value convention for
/// its digest, and separately count every one-ULP deviation. This never changes
/// production decoding or accepts a different packed attribute.
fn reference_normal(value: f32, counts: &mut Counts) -> Result<f32> {
    let integer = (value * 127.).round();
    if !(-127. ..=128.).contains(&integer)
        || (integer * f32::from_bits(0x3c01_0204)).to_bits() != value.to_bits()
    {
        return Err("normal is not a consumer-decoded packed byte".into());
    }
    let reference = integer / 127.;
    let ulp = value.to_bits().abs_diff(reference.to_bits());
    if ulp > 1 {
        return Err("packed normal differs from reference by more than one ULP".into());
    }
    counts.normal_components += 1;
    counts.normal_changed += u64::from(ulp != 0);
    counts.normal_max_ulp = counts.normal_max_ulp.max(ulp);
    Ok(reference)
}
fn visit(
    bytes: &[u8],
    id: &str,
    decoder: &Decoder<'_>,
    expected: &mut BTreeMap<String, serde_json::Value>,
    counts: &mut Counts,
    depth: usize,
) -> Result<()> {
    if depth > 8 {
        return Err("nested binder limit".into());
    }
    if bytes.starts_with(b"FLVER\0") {
        let want = expected
            .remove(id)
            .ok_or_else(|| format!("unexpected FLVER {id}"))?;
        assert_eq!(sha(bytes), want["sha256"].as_str().ok_or("sha")?, "{id}");
        if let Some(theirs) = want.get("error") {
            return match Model::read(bytes) {
                Ok(_) => Err(format!("{id}: read, the reference refuses it: {theirs}").into()),
                Err(e) => {
                    counts.agreed_rejections += 1;
                    println!("Agreed FLVER rejection {id}: ours {e}; reference {theirs}");
                    Ok(())
                }
            };
        }
        let model = Model::read(bytes)?;
        assert_eq!(
            u64::from(model.version),
            want["version"].as_u64().ok_or("version")?
        );
        let meshes = want["meshes"].as_array().ok_or("meshes")?;
        assert_eq!(model.meshes.len(), meshes.len(), "{id}");
        for (m, w) in model.meshes.iter().zip(meshes) {
            for &layout in &m.layouts {
                for member in &model.layouts[layout] {
                    if matches!(member.semantic, 3 | 6) && member.kind != 0x11 {
                        return Err("reference normal projection only covers uint4 attributes".into());
                    }
                }
            }
            assert_eq!(m.material as u64, w["material"].as_u64().ok_or("material")?);
            assert_eq!(m.vertices.len() as u64, w["vertices"].as_u64().ok_or("vertices")?);
            let mut stream = Vec::new();
            for v in &m.vertices {
                for x in v.position {
                    stream.extend(x.to_le_bytes());
                }
                for x in v.normal {
                    stream.extend(reference_normal(x, counts)?.to_le_bytes());
                }
                stream.extend(v.normal_w.to_le_bytes());
                stream.extend((v.uvs.len() as u32).to_le_bytes());
                for x in v.uvs.iter().flatten() {
                    stream.extend(x.to_le_bytes());
                }
                stream.extend((v.tangents.len() as u32).to_le_bytes());
                for x in v.tangents.iter().flatten() {
                    stream.extend(reference_normal(*x, counts)?.to_le_bytes());
                }
                stream.extend((v.colors.len() as u32).to_le_bytes());
                for x in v.colors.iter().flatten() {
                    stream.extend(x.to_le_bytes());
                }
            }
            assert_eq!(
                sha(&stream),
                w["vertex_hash"].as_str().ok_or("vertex hash")?,
                "{id}"
            );
            let faces = w["faces"].as_array().ok_or("faces")?;
            assert_eq!(m.faces.len(), faces.len());
            for (f, expected) in m.faces.iter().zip(faces) {
                assert_eq!(u64::from(f.flags), expected["flags"].as_u64().ok_or("flags")?);
                assert_eq!(f.strip, expected["strip"].as_bool().ok_or("strip")?);
                assert_eq!(f.cull, expected["cull"].as_bool().ok_or("cull")?);
                assert_eq!(
                    sha(&f.indices.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>()),
                    expected["hash"].as_str().ok_or("index hash")?,
                    "{id}"
                );
                assert_eq!(
                    sha(&f
                        .triangulate()
                        .iter()
                        .flat_map(|i| i.to_le_bytes())
                        .collect::<Vec<_>>()),
                    expected["triangulated"].as_str().ok_or("triangle hash")?,
                    "{id}"
                );
            }
            counts.meshes += 1;
        }
        counts.models += 1;
    } else if bytes.starts_with(b"BND4") {
        let b = Binder::read(bytes, decoder)?;
        for (i, f) in b.files.iter().enumerate() {
            visit(
                &f.bytes,
                &format!("{id}/b{i}"),
                decoder,
                expected,
                counts,
                depth + 1,
            )?;
        }
    }
    Ok(())
}
#[test]
fn all_flver_models() -> Result<()> {
    let Some(dll) = primase_testdata::tool(primase_testdata::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, oracle, names) in [
        (
            Game::Sekiro,
            primase_testdata::oracle(
                primase_testdata::OracleSet::SoulsFormats,
                primase_testdata::Game::Sekiro,
                "census",
            ),
            primase_testdata::oracle(
                primase_testdata::OracleSet::Uxm,
                primase_testdata::Game::Sekiro,
                "dictionary.txt",
            ),
        ),
        (
            Game::EldenRing,
            primase_testdata::oracle(
                primase_testdata::OracleSet::SoulsFormats,
                primase_testdata::Game::Elden,
                "census",
            ),
            primase_testdata::oracle(
                primase_testdata::OracleSet::Uxm,
                primase_testdata::Game::Elden,
                "dictionary.txt",
            ),
        ),
    ] {
        let (Some(archive), Some(oracle), Some(names)) = (common::archive(game)?, oracle, names) else {
            continue;
        };
        flver_models(
            game,
            &archive,
            &oracle,
            &NameDictionary::read_for(names, game)?,
            &decoder,
        )?;
    }
    Ok(())
}

fn flver_models(
    game: Game,
    archive: &primase_fromsoft::Archive,
    oracle: &std::path::Path,
    names: &NameDictionary,
    decoder: &Decoder<'_>,
) -> Result<()> {
    let decoder = *decoder;
    // the reference writes a hash at its game's width
    let width = match game.archives().hash {
        PathHash::Base37 => 8,
        PathHash::Base133 => 16,
    };
    let mut expected = BTreeMap::new();
    for line in std::fs::read_to_string(oracle.join("flver/models.jsonl"))?.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        expected.insert(value["id"].as_str().ok_or("id")?.to_owned(), value);
    }
    // shard, hash, SoulsFormats' message: the entries the reference cannot decode
    let mut rejected = BTreeMap::new();
    for line in std::fs::read_to_string(oracle.join("errors.tsv"))?.lines() {
        let f: Vec<_> = line.splitn(3, '\t').collect();
        let [shard, hash, message] = f[..] else {
            return Err(format!("errors.tsv row: {line}").into());
        };
        rejected.insert(format!("{shard}/{hash}"), message.to_owned());
    }
    let mut counts = Counts::default();
    let mut failures = Vec::new();
    let mut split = 0;
    for (&hash, entry) in &archive.entries {
        let name = names.names.get(&hash).and_then(|x| x.first());
        if name.is_some_and(|n| n.ends_with("bdt")) {
            continue;
        }
        let id = format!("{}/{hash:0width$x}", entry.shard);
        let result = (|| -> Result<()> {
            let bytes = archive.read_hash(hash, &decoder)?;
            if bytes.starts_with(b"BHF4") {
                let name = name.ok_or("unnamed split binder")?;
                let payload = archive.read(&format!("{}bdt", &name[..name.len() - 3]), &decoder)?;
                let binder = Binder::read_split(&bytes, &payload, &decoder)?;
                for (i, f) in binder.files.iter().enumerate() {
                    visit(
                        &f.bytes,
                        &format!("{id}/s{i}"),
                        &decoder,
                        &mut expected,
                        &mut counts,
                        0,
                    )?;
                }
                split += 1;
            } else {
                visit(&bytes, &id, &decoder, &mut expected, &mut counts, 0)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            // both decoders reject it: agreement on the entry, the reasons reported side by side
            if let Some(theirs) = rejected.remove(&id) {
                println!("Agreed rejection {id}: ours {e}; reference {theirs}");
            } else {
                failures.push(format!("{id}: {e}"));
            }
        }
    }
    println!(
        "{}: FLVER models {}, agreed rejections {}, meshes {}, split binders {}, remaining oracle {}, failures {}",
        game.name(),
        counts.models,
        counts.agreed_rejections,
        counts.meshes,
        split,
        expected.len(),
        failures.len()
    );
    println!(
        "Packed normal/tangent components {}, reference-convention differences {}, maximum ULP {}",
        counts.normal_components, counts.normal_changed, counts.normal_max_ulp
    );
    for f in &failures {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty());
    assert!(expected.is_empty());
    assert!(rejected.is_empty(), "reference rejection not reproduced");
    Ok(())
}
