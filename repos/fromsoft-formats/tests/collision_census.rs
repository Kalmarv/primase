//! Collision scope: every named map collision binder and every HKX in object
//! binders, including broken states for census coverage. Character/ragdoll
//! binders are counted as outside the map/object reader's scope.
use fromsoft_formats::{Archive, Binder, NameDictionary, dcx::Decoder};
use havok_formats::{Compendium, Tagfile};
use primase_testdata::{Game, OracleSet, Tool, install, oodle, oracle};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
fn native(path: &primase_testdata::OodleLibrary) -> primase_foundations::compress::Result<primase_foundations::compress::oodle::Oodle> {
    // SAFETY: caller explicitly supplies a trusted native version-6 decoder.
    unsafe { primase_foundations::compress::oodle::Oodle::open(path.path(), primase_foundations::compress::oodle::Version::V6) }
}
#[test]
fn installed_collision_types() -> Result<()> {
    let Some(dir) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(names) = oracle(OracleSet::Uxm, Game::Sekiro, "dictionary.txt") else {
        return Ok(());
    };
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let oodle = native(&dll)?;
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1024 * 1024 * 1024,
    };
    let Some(sekiro_keys) = primase_testdata::keys(primase_testdata::Game::Sekiro) else {
        return Ok(());
    };
    let archive = Archive::open_game(
        dir,
        fromsoft_formats::Game::Sekiro,
        &fromsoft_formats::Keys::from_folder(sekiro_keys, fromsoft_formats::Game::Sekiro)?,
    )?;
    let names = NameDictionary::read(names)?;
    let unnamed = archive
        .entries
        .keys()
        .filter(|h| !names.names.contains_key(h))
        .count();
    eprintln!(
        "dictionary-indexed scope: {} archive entries, {unnamed} unnamed entries not classified",
        archive.entries.len()
    );
    let mut other_variants = BTreeMap::<String, usize>::new();
    let mut shapes = BTreeMap::<String, usize>::new();
    let mut files = 0;
    let mut map_binders = 0;
    let mut object_binders = 0;
    let mut failures = Vec::new();
    let mut excluded = 0;
    for (&hash, paths) in &names.names {
        if !archive.entries.contains_key(&hash) {
            continue;
        }
        for path in paths {
            if path.starts_with("/chr/") && (path.ends_with(".chrbnd.dcx") || path.ends_with(".anibnd.dcx")) {
                excluded += 1;
            }
            let map = path.starts_with("/map/") && path.ends_with(".hkxbhd");
            let object = path.starts_with("/obj/") && path.ends_with(".objbnd.dcx");
            if !map && !object {
                continue;
            }
            let result = (|| -> Result<()> {
                let bytes = archive.read(path, &decoder)?;
                let binder = if map {
                    map_binders += 1;
                    let payload = archive.read(&path.replace(".hkxbhd", ".hkxbdt"), &decoder)?;
                    Binder::read_split(&bytes, &payload, &decoder)?
                } else {
                    object_binders += 1;
                    Binder::read(&bytes, &decoder)?
                };
                let mut compendia = Vec::new();
                for f in &binder.files {
                    let bytes = decoder.decode_if_dcx(&f.bytes)?;
                    if bytes.starts_with(b"\0") && bytes.get(4..8) == Some(b"TCM0") {
                        compendia.push(Compendium::read(&bytes)?);
                    }
                }
                for f in &binder.files {
                    let name = f.name.as_deref().unwrap_or("");
                    if !name.ends_with(".hkx") && !name.ends_with(".hkx.dcx") {
                        continue;
                    }
                    let bytes = decoder.decode_if_dcx(&f.bytes)?;
                    let mut result = Tagfile::read(&bytes, None);
                    for comp in &compendia {
                        if result.is_ok() {
                            break;
                        }
                        result = Tagfile::read(&bytes, Some(comp));
                    }
                    let tag = result.map_err(|e| format!("{name}: {e}"))?;
                    let variants = tag.root()?.field("namedVariants")?.array()?;
                    for i in 0..variants.len() {
                        let physics = variants.get(i)?.field("variant")?.dereference()?;
                        if physics.type_name() != "hknpPhysicsSceneData" {
                            *other_variants.entry(physics.type_name().into()).or_default() += 1;
                            continue;
                        }
                        let systems = physics.field("systemDatas")?.array()?;
                        for j in 0..systems.len() {
                            let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
                            for k in 0..bodies.len() {
                                let shape = bodies.get(k)?.field("shape")?.dereference()?;
                                let leaf = name
                                    .rsplit(['/', '\\'])
                                    .next()
                                    .unwrap_or(name)
                                    .trim_end_matches(".dcx");
                                let stem = path
                                    .rsplit('/')
                                    .next()
                                    .unwrap_or(path)
                                    .trim_end_matches(".objbnd.dcx");
                                let group = if map {
                                    path.rsplit('/').next().and_then(|n| n.get(..1)).unwrap_or("map")
                                } else if leaf == format!("{stem}.hkx") {
                                    "object-main"
                                } else {
                                    "object-other"
                                };
                                *shapes
                                    .entry(format!("{group}/{}", shape.type_name()))
                                    .or_default() += 1;
                            }
                        }
                    }
                    files += 1;
                }
                Ok(())
            })();
            if let Err(e) = result {
                failures.push(format!("{path}: {e}"));
            }
        }
    }
    eprintln!(
        "collision files {files}, map binders {map_binders}, object binders {object_binders}, out-of-scope named character/ragdoll binders {excluded}"
    );
    eprintln!("shape bodies {shapes:?}; nonphysics variants {other_variants:?}");
    for f in &failures {
        eprintln!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} failed binders", failures.len());
    Ok(())
}
