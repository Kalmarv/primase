//! The archives of each game read against an independent census of the same install (SoulsFormats for Sekiro,
//! SoulsFormatsNEXT for Elden Ring, each with UXM's keys and path hash): the decrypted indexes, every entry's decoded
//! bytes and every top-level binder member. Private corpus paths are caller-supplied; no game data is bundled.
use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::{
    Game, Keys, NameDictionary,
    archive::Archive,
    bhd::{Header, unwrap_rsa},
};

use std::path::PathBuf;

/// One game's install, its keys and the independent census of it, when the configuration names them.
struct Install {
    game: Game,
    root: PathBuf,
    keys: Keys,
    oracle: PathBuf,
    names: Option<PathBuf>,
}

fn install(game: Game) -> Result<Option<Install>, Box<dyn std::error::Error>> {
    let (root, oracle, names) = match game {
        Game::Sekiro => (
            primase_testdata::install(primase_testdata::Game::Sekiro),
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
        Game::EldenRing => (
            primase_testdata::install(primase_testdata::Game::Elden),
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
    };
    let (Some(root), Some(oracle)) = (root, oracle) else {
        return Ok(None);
    };
    let keys = match Keys::published(game) {
        Some(keys) => keys,
        None => {
            let Some(folder) = primase_testdata::keys(primase_testdata::Game::Elden) else {
                return Ok(None);
            };
            Keys::from_folder(folder, game)?
        }
    };
    Ok(Some(Install {
        game,
        root,
        keys,
        oracle,
        names,
    }))
}

/// Each archive's index unwrapped as the reference unwraps it, the archive's entries, and the dictionary naming
/// exactly the entries the reference names.
#[test]
fn headers() -> Result<(), Box<dyn std::error::Error>> {
    for game in Game::ALL {
        if let Some(install) = install(game)? {
            headers_of(&install)?;
        }
    }
    Ok(())
}

fn headers_of(install: &Install) -> Result<(), Box<dyn std::error::Error>> {
    let set = install.game.archives();
    let archive = Archive::open_game(&install.root, install.game, &install.keys)?;
    let mut report = GateReport::new(
        format!("{} RSA BHD oracle", install.game.name()),
        Tolerance::Exact,
    );
    for (pair, header) in set
        .pairs
        .iter()
        .filter(|p| !archive.absent.contains(&p.stem))
        .zip(&archive.headers)
    {
        let enc = std::fs::read(install.root.join(format!("{}.bhd", pair.stem)))?;
        let (n, e) = install.keys.get(pair.stem).ok_or("no key")?;
        let plain = unwrap_rsa(&enc, n, e)?;
        let want = std::fs::read(install.oracle.join(format!("{}.plain", pair.name())))?;
        report.compare_bytes(pair.stem, &plain, &want);
        assert_eq!(
            Header::read(&plain, &set.record)?.entries.len(),
            header.entries.len()
        );
        println!("{}: {} entries", pair.stem, header.entries.len());
    }
    println!("absent: {:?}", archive.absent);
    println!("{report}");
    report.assert_passed();
    if install.game == Game::Sekiro {
        assert_eq!(archive.entries.len(), 8765);
        assert_eq!(archive.headers[0].entries.len(), 1183);
        assert_eq!(archive.headers[4].entries.len(), 5449);
    }
    if let Some(names) = &install.names {
        let dict = NameDictionary::read_for(names, install.game)?;
        let named = archive
            .entries
            .keys()
            .filter(|h| dict.names.contains_key(h))
            .count();
        println!("named {named} of {}", archive.entries.len());
        if install.game == Game::Sekiro {
            assert_eq!(named, 8752);
        }
        // the reference's names: every entry it decoded, named by its own hash of the same dictionary
        let mut disagree = Vec::new();
        for line in std::fs::read_to_string(install.oracle.join("entries.tsv"))?.lines() {
            let f: Vec<_> = line.split('\t').collect();
            let hash = u64::from_str_radix(f[1], 16)?;
            let ours = dict.names.get(&hash);
            let agrees = match (f[2], ours) {
                ("", None) => true,
                (path, Some(candidates)) if !path.is_empty() => {
                    let path = path.replace('\\', "/").to_ascii_lowercase();
                    candidates.contains(&format!("/{}", path.trim_start_matches('/')))
                }
                _ => false,
            };
            if !agrees {
                disagree.push(format!("{hash:x}: reference {:?}, ours {ours:?}", f[2]));
            }
        }
        for d in disagree.iter().take(20) {
            println!("NAME {d}");
        }
        assert!(
            disagree.is_empty(),
            "{} entries named differently",
            disagree.len()
        );
    }
    Ok(())
}
#[allow(unsafe_code)]
mod native {
    use primase_compress::oodle::{Oodle, Version};
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<Oodle> {
        // SAFETY: the gate requires an explicitly supplied, trusted version-6 DLL.
        unsafe { Oodle::load(path, Version::V6) }
    }
}
/// Whole-install differential against the reference archive/binder decoder, for each game configured.
/// Every indexed entry is attempted; corrupt entries remain explicit failures.
#[test]
fn whole_install() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dll) = primase_testdata::tool(primase_testdata::Tool::Oodle6) else {
        return Ok(());
    };
    let oodle = native::load(dll)?;
    let decoder = primase_fromsoft::dcx::Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    for game in Game::ALL {
        if let Some(install) = install(game)? {
            whole_install_of(&install, &decoder)?;
        }
    }
    Ok(())
}

fn whole_install_of(
    install: &Install,
    decoder: &primase_fromsoft::dcx::Decoder<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    use primase_fromsoft::Binder;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    let decoder = *decoder;
    let oracle = &install.oracle;
    let archive = Archive::open_game(&install.root, install.game, &install.keys)?;
    let stem = |shard: u8| archive.pair(shard).map_or("?", |p| p.stem);
    let entry_text = std::fs::read_to_string(oracle.join("entries.tsv"))?;
    let member_text = std::fs::read_to_string(oracle.join("binders.tsv"))?;
    let mut expected = BTreeMap::new();
    for line in entry_text.lines() {
        let f: Vec<_> = line.split('\t').collect();
        expected.insert(
            (f[0].parse::<u8>()?, u64::from_str_radix(f[1], 16)?),
            (f[3].parse::<usize>()?, f[4].to_owned()),
        );
    }
    let mut members = BTreeMap::new();
    for line in member_text.lines() {
        let f: Vec<_> = line.split('\t').collect();
        members.insert(
            (
                f[0].parse::<u8>()?,
                u64::from_str_radix(f[1], 16)?,
                f[2].parse::<usize>()?,
            ),
            (
                f[3].parse::<i32>()?,
                f[4].to_owned(),
                f[5].parse::<usize>()?,
                f[6].to_owned(),
            ),
        );
    }
    let error_text = std::fs::read_to_string(oracle.join("errors.tsv"))?;
    let mut rejected = BTreeMap::new();
    // shard, hash, SoulsFormats' message: the entries the reference cannot decode
    for line in error_text.lines() {
        let f: Vec<_> = line.splitn(3, '\t').collect();
        let [shard, hash, message] = f[..] else {
            return Err(format!("errors.tsv row: {line}").into());
        };
        rejected.insert(
            (shard.parse::<u8>()?, u64::from_str_radix(hash, 16)?),
            message.to_owned(),
        );
    }
    let mut agreed_rejections = 0;
    let mut counts = BTreeMap::<(u8, String), usize>::new();
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut checked_members = 0;
    for (&hash, entry) in &archive.entries {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let raw = archive.read_stored(hash)?;
            let kind = if raw.starts_with(b"DCX\0") {
                String::from_utf8_lossy(raw.get(40..44).unwrap_or_default()).into_owned()
            } else {
                String::from_utf8_lossy(raw.get(..4).unwrap_or_default()).into_owned()
            };
            *counts.entry((entry.shard, kind)).or_default() += 1;
            let bytes = decoder.decode_if_dcx(&raw)?;
            let want = expected.get(&(entry.shard, hash)).ok_or("missing oracle entry")?;
            assert_eq!(
                (
                    bytes.len(),
                    Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<String>()
                ),
                *want,
                "{hash:08x}"
            );
            checked += 1;
            if bytes.starts_with(b"BND4") {
                let binder = Binder::read(&bytes, &decoder)?;
                for (i, file) in binder.files.iter().enumerate() {
                    let want = members
                        .get(&(entry.shard, hash, i))
                        .ok_or("missing oracle member")?;
                    assert_eq!(
                        (
                            file.id.unwrap_or(-1),
                            file.name.clone().unwrap_or_default(),
                            file.bytes.len(),
                            Sha256::digest(&file.bytes)
                                .iter()
                                .map(|b| format!("{b:02X}"))
                                .collect::<String>()
                        ),
                        *want,
                        "{hash:08x} member {i}"
                    );
                    checked_members += 1;
                }
            }
            Ok(())
        })();
        match result {
            Err(e) => {
                // both decoders reject it: agreement on the entry; the reasons are an independent decoder's own
                // words, reported side by side, not compared
                if let Some(theirs) = rejected.get(&(entry.shard, hash)) {
                    agreed_rejections += 1;
                    println!(
                        "AGREED REJECTION {} {hash:08x}: ours {e}; reference {theirs}",
                        stem(entry.shard)
                    );
                } else {
                    failures.push(format!("{} {hash:08x}: {e}", stem(entry.shard)));
                }
            }
            Ok(()) => {
                if rejected.contains_key(&(entry.shard, hash)) {
                    failures.push(format!("{hash:08x}: unexpectedly accepted rejected oracle input"));
                }
            }
        }
    }
    let mut samples = GateReport::new(
        format!("{} reference decoded entry bytes", install.game.name()),
        Tolerance::Exact,
    );
    for line in std::fs::read_to_string(oracle.join("samples.tsv"))?.lines() {
        let (hash, path) = line.split_once('\t').ok_or("sample manifest row")?;
        let bytes = archive.read(path, &decoder)?;
        let want = std::fs::read(oracle.join("samples").join(format!("{hash}.bin")))?;
        samples.compare_bytes(path, &bytes, &want);
    }
    println!("{samples}");
    samples.assert_passed();
    println!(
        "Equivalent rejected entries {agreed_rejections}/{}",
        rejected.len()
    );
    assert_eq!(agreed_rejections, rejected.len());
    assert_eq!(checked + agreed_rejections, archive.entries.len());
    for ((shard, kind), count) in counts {
        println!("{} {kind:?}: {count}", stem(shard));
    }
    println!(
        "entries matched {checked}/{}; members matched {checked_members}/{}; failures {}",
        expected.len(),
        members.len(),
        failures.len()
    );
    for failure in &failures {
        println!("FAIL {failure}");
    }
    assert_eq!(checked, expected.len());
    assert_eq!(checked_members, members.len());
    assert!(failures.is_empty(), "{} archive failures", failures.len());
    Ok(())
}
