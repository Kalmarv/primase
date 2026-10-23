//! Whole-install PC texture-package differential against independent C# metadata.
use primase_fromsoft::{
    Archive, Binder, NameDictionary,
    dcx::Decoder,
    tpf::{Package, TextureKind},
};
use primase_testdata::{Game, OracleSet, Tool, install, oracle, tool};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
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
fn visit(
    bytes: &[u8],
    id: &str,
    decoder: &Decoder<'_>,
    expected: &mut BTreeMap<String, serde_json::Value>,
    counts: &mut (usize, usize),
    depth: usize,
) -> Result<()> {
    if depth > 8 {
        return Err("nested binder limit".into());
    }
    if bytes.starts_with(b"TPF\0") {
        let want = expected
            .remove(id)
            .ok_or_else(|| format!("unexpected TPF {id}"))?;
        assert_eq!(sha(bytes), want["sha256"].as_str().ok_or("sha field")?, "{id}");
        let package = Package::read(bytes)?;
        assert_eq!(
            package.flags,
            u8::try_from(want["flags"].as_u64().ok_or("flags")?)?
        );
        assert_eq!(
            package.encoding,
            want["encoding"].as_u64().ok_or("encoding")? as u8
        );
        let textures = want["textures"].as_array().ok_or("textures")?;
        assert_eq!(textures.len(), package.textures.len(), "{id}");
        for (t, w) in package.textures.iter().zip(textures) {
            let kind = match t.kind {
                TextureKind::Image2D => 0,
                TextureKind::Cube => 1,
                TextureKind::Volume => 2,
            };
            assert_eq!(t.name, w["name"].as_str().ok_or("name")?, "{id}");
            assert_eq!(
                (
                    t.format as u64,
                    kind,
                    t.mipmaps as u64,
                    t.flags as u64,
                    t.dds.len() as u64
                ),
                (
                    w["format"].as_u64().ok_or("format")?,
                    w["kind"].as_u64().ok_or("kind")?,
                    w["mipmaps"].as_u64().ok_or("mipmaps")?,
                    w["flags"].as_u64().ok_or("flags")?,
                    w["size"].as_u64().ok_or("size")?
                ),
                "{id}"
            );
            assert_eq!(sha(&t.dds), w["sha256"].as_str().ok_or("dds hash")?, "{id}");
            counts.1 += 1;
        }
        counts.0 += 1;
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
fn all_texture_packages() -> Result<()> {
    let Some(root) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(oracle) = oracle(OracleSet::SoulsFormats, Game::Sekiro, "tpf") else {
        return Ok(());
    };
    let Some(names) = primase_testdata::oracle(OracleSet::Uxm, Game::Sekiro, "dictionary.txt") else {
        return Ok(());
    };
    let Some(dll) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let names = NameDictionary::read(names)?;
    let mut expected = BTreeMap::new();
    for line in std::fs::read_to_string(oracle.join("packages.jsonl"))?.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        expected.insert(value["id"].as_str().ok_or("id")?.to_owned(), value);
    }
    // shard, hash, the reference's message: the entries it cannot decode (the archive census's schema)
    let mut rejected = BTreeMap::new();
    for line in std::fs::read_to_string(oracle.join("errors.tsv"))?.lines() {
        let f: Vec<_> = line.splitn(3, '\t').collect();
        let [shard, hash, message] = f[..] else {
            return Err(format!("errors.tsv row: {line}").into());
        };
        rejected.insert(format!("{shard}/{hash}"), message.to_owned());
    }
    let mut counts = (0, 0);
    let mut failures = Vec::new();
    let mut split = 0;
    for (&hash, entry) in &archive.entries {
        let name = names.names.get(&hash).and_then(|x| x.first());
        if name.is_some_and(|n| n.ends_with("bdt")) {
            continue;
        }
        let id = format!("{}/{hash:08x}", entry.shard);
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
        "TPF packages {}, textures {}, split binders {}, remaining oracle {}, failures {}",
        counts.0,
        counts.1,
        split,
        expected.len(),
        failures.len()
    );
    for f in &failures {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty());
    assert!(expected.is_empty());
    assert!(rejected.is_empty(), "reference rejection not reproduced");
    Ok(())
}
