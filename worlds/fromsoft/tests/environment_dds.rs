//! Every cube and volume texture of the two installs reads with primase-dds as DirectXTex's own library reads it.
//!
//! Sekiro's are the cube and volume textures of the packages the SoulsFormats census finds them in
//! (`oracles.soulsformats.sekiro/tpf`), and every distinct one goes to DirectXTex. Elden Ring's are every texture of the
//! environment binders (`_envmap_`) its census lists (`oracles.soulsformats.elden/census`): each reads here, and one
//! file per distinct DDS header goes to DirectXTex (the header fixes the layout the payload is cut by, so files that
//! share it differ only in their bytes). A DX10 cube whose arraySize counts faces (6 per cube) DirectXTex reads only
//! with its permissive load, and refuses with its default one; every other file it reads with its default load.
mod common;
mod inputs;

use fromsoft_formats::{
    Binder, Game,
    dcx::Decoder,
    tpf::{Package, TextureKind},
};
use inputs::{Game as Data, OracleSet, Tool, oodle, oracle, own_scratch, tool};
use primase_dds::{Dds, Shape, parse, to_dxgi};
use primase_testdata::gate::{GateReport, Tolerance};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// DirectXTex's `TexMetadata` for what was read: arraySize counts cube faces, depth is 1 but for volumes.
fn metadata(d: &Dds) -> [u64; 9] {
    let (dimension, array, depth, misc) = match d.shape {
        Shape::D1 { layers } => (2, u64::from(layers), 1, 0),
        Shape::D2 { layers } => (3, u64::from(layers), 1, 0),
        Shape::Cube { cubes } => (3, u64::from(cubes) * 6, 1, 4),
        Shape::D3 { depth } => (4, 1, u64::from(depth), 0),
    };
    [
        u64::from(d.width),
        u64::from(d.height),
        depth,
        array,
        d.levels.len() as u64,
        misc,
        u64::from(d.alpha_mode.misc2()),
        u64::from(to_dxgi(d.format, d.color_space).unwrap_or(0)),
        dimension,
    ]
}

/// The images in DirectXTex's order (file order): item by item with their mips, or mip by mip with their slices.
fn images(d: &Dds) -> Vec<u8> {
    match d.shape {
        Shape::D3 { .. } => d.levels.iter().flatten().flatten().copied().collect(),
        _ => (0..d.levels[0].len())
            .flat_map(|item| d.levels.iter().flat_map(move |level| level[item].iter().copied()))
            .collect(),
    }
}

/// directxtex-dump's answers (`meta.json` and `images.bin`) for `files`, each a command and a file's bytes: one batch run.
fn directxtex(dump: &Path, files: &[(&str, &[u8])]) -> Result<Vec<(Value, Vec<u8>)>> {
    let dir = own_scratch("directxtex-environment");
    let mut list = String::new();
    for (i, (command, bytes)) in files.iter().enumerate() {
        let file = dir.join(format!("{i}.dds"));
        std::fs::write(&file, bytes)?;
        list += &format!(
            "{command}\t{}\t{}\n",
            file.display(),
            dir.join(i.to_string()).display()
        );
    }
    let tsv = dir.join("list.tsv");
    std::fs::write(&tsv, list)?;
    let out = std::process::Command::new(dump).arg("batch").arg(&tsv).output()?;
    if !out.status.success() {
        return Err(format!(
            "directxtex-dump batch: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    (0..files.len())
        .map(|i| {
            let folder = dir.join(i.to_string());
            let meta: Value = serde_json::from_slice(&std::fs::read(folder.join("meta.json"))?)?;
            Ok((meta, std::fs::read(folder.join("images.bin")).unwrap_or_default()))
        })
        .collect()
}

/// Whether DirectXTex read `d`: the same metadata and the same bytes for every image, in its order.
fn agrees(d: &Dds, (meta, theirs): &(Value, Vec<u8>)) -> std::result::Result<(), String> {
    let hr = meta["hr"].as_str().unwrap_or("?");
    if hr != "0x00000000" {
        return Err(format!("DirectXTex refused it ({hr})"));
    }
    let fields = [
        "width",
        "height",
        "depth",
        "arraySize",
        "mipLevels",
        "miscFlags",
        "miscFlags2",
        "format",
        "dimension",
    ]
    .map(|k| meta[k].as_u64().unwrap_or(u64::MAX));
    if metadata(d) != fields {
        return Err(format!("metadata {:?}, DirectXTex {fields:?}", metadata(d)));
    }
    if images(d) != *theirs {
        return Err("images differ".into());
    }
    Ok(())
}

/// One TPF texture as read here: what it was read as (not its pixels, which the callers do not hold).
struct Texture {
    id: String,
    kind: TextureKind,
    /// The format and colour space read, whether its cube arraySize counted faces, or why it was refused.
    read: std::result::Result<(primase_core::TexFormat, primase_core::ColorSpace, bool), String>,
}

/// `texture` of package `id` read with primase-dds, its shape held against the package's kind and mip count.
fn read(id: String, texture: &fromsoft_formats::tpf::Texture) -> Texture {
    let read = parse(&texture.dds).map_err(|e| e.to_string()).and_then(|p| {
        let shape_agrees = match texture.kind {
            TextureKind::Image2D => p.dds.shape == Shape::D2 { layers: 1 },
            TextureKind::Cube => matches!(p.dds.shape, Shape::Cube { .. }),
            TextureKind::Volume => matches!(p.dds.shape, Shape::D3 { .. }),
        };
        if !shape_agrees {
            return Err(format!("{:?} in a {:?} texture", p.dds.shape, texture.kind));
        }
        if p.dds.levels.len() != usize::from(texture.mipmaps) {
            return Err(format!(
                "{} mips, the package says {}",
                p.dds.levels.len(),
                texture.mipmaps
            ));
        }
        Ok((p.dds.format, p.dds.color_space, p.faces_counted))
    });
    Texture {
        id,
        kind: texture.kind,
        read,
    }
}

/// What a texture is, for the tallies: its kind, format and whether its cube arraySize counted faces.
fn class(t: &Texture) -> String {
    match &t.read {
        Ok((format, color_space, faces)) => format!(
            "{:?} {format:?}/{color_space:?}{}",
            t.kind,
            if *faces { " arraySize counts faces" } else { "" }
        ),
        Err(_) => format!("{:?} refused", t.kind),
    }
}

/// Each file of `files` (a name and its bytes) to DirectXTex: a face-counted cube with the default load (which must
/// refuse it) and the permissive one, every other file with the default load. Each answer is recorded in `gate`, and
/// the agreements counted by how DirectXTex read the file.
fn compare(
    dump: &Path,
    files: &[(String, Vec<u8>)],
    gate: &mut GateReport,
    loads: &mut BTreeMap<&'static str, usize>,
) -> Result<()> {
    for chunk in files.chunks(400) {
        let mut runs = Vec::new();
        let mut expected = Vec::new();
        for (name, bytes) in chunk {
            let p = parse(bytes)?;
            if p.faces_counted {
                runs.push(("read", bytes.as_slice()));
                runs.push(("read-permissive", bytes.as_slice()));
            } else {
                runs.push(("read", bytes.as_slice()));
            }
            expected.push((name, p.dds, p.faces_counted));
        }
        let answers = directxtex(dump, &runs)?;
        let mut next = answers.iter();
        for (name, d, faces) in expected {
            let result = if faces {
                let strict = next.next().ok_or("answers")?;
                let permissive = next.next().ok_or("answers")?;
                if strict.0["hr"] == "0x00000000" {
                    Err("DirectXTex's default load read a face-counted cube".to_owned())
                } else {
                    agrees(&d, permissive)
                        .map(|()| "permissive load (face-counted cube refused by the default load)")
                }
            } else {
                agrees(&d, next.next().ok_or("answers")?).map(|()| "default load")
            };
            match result {
                Ok(load) => {
                    *loads.entry(load).or_default() += 1;
                    gate.record(name, 0.0);
                }
                Err(e) => gate.fail(name, e),
            }
        }
    }
    Ok(())
}

/// Sekiro's cube and volume textures, each distinct one read by DirectXTex too.
#[test]
fn sekiro_cubes_and_volumes_read_as_directxtex_reads_them() -> Result<()> {
    let (Some(census), Some(dump), Some(dll)) = (
        oracle(OracleSet::SoulsFormats, Data::Sekiro, "tpf"),
        tool(Tool::DirectxtexDump),
        oodle(Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let Some(archive) = common::archive(Game::Sekiro).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let dll = common::oodle(dll).map_err(|e| e.to_string())?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    // the packages holding a cube or a volume, by the census's id (`<shard>/<hash>`: each an archive entry of its own)
    let mut packages = BTreeSet::new();
    let mut census_textures = 0;
    for line in std::fs::read_to_string(census.join("packages.jsonl"))?.lines() {
        let p: Value = serde_json::from_str(line)?;
        let kinds = p["textures"].as_array().ok_or("textures")?;
        let n = kinds.iter().filter(|t| t["kind"] != 0).count();
        if n > 0 {
            census_textures += n;
            packages.insert(p["id"].as_str().ok_or("id")?.to_owned());
        }
    }
    let mut seen = BTreeSet::new();
    let mut textures = Vec::new();
    let mut files = Vec::new();
    for id in &packages {
        let (_, hash) = id.split_once('/').ok_or("id")?;
        if hash.contains('/') {
            return Err(format!(
                "{id}: a binder member; the census put every cube and volume in a package of its own"
            )
            .into());
        }
        let package = Package::read(&archive.read_hash(u64::from_str_radix(hash, 16)?, &decoder)?)?;
        for t in package.textures.iter().filter(|t| t.kind != TextureKind::Image2D) {
            let digest: [u8; 32] = Sha256::digest(&t.dds).into();
            if !seen.insert(digest) {
                continue;
            }
            let texture = read(format!("{id}/{}", t.name), t);
            if texture.read.is_ok() {
                files.push((texture.id.clone(), t.dds.clone()));
            }
            textures.push(texture);
        }
    }
    let mut reads = GateReport::new("sekiro cubes and volumes read", Tolerance::Exact);
    let mut classes = BTreeMap::<String, usize>::new();
    for t in &textures {
        *classes.entry(class(t)).or_default() += 1;
        match &t.read {
            Ok(_) => reads.record(&t.id, 0.0),
            Err(e) => reads.fail(&t.id, e),
        }
    }
    let mut gate = GateReport::new("sekiro cubes and volumes, read by DirectXTex", Tolerance::Exact);
    let mut loads = BTreeMap::new();
    compare(&dump, &files, &mut gate, &mut loads)?;
    eprintln!(
        "sekiro: {} packages, {census_textures} cube and volume textures, {} distinct\nclasses={classes:#?}\nDirectXTex loads={loads:#?}\n{reads}\n{gate}",
        packages.len(),
        textures.len()
    );
    reads.assert_passed();
    gate.assert_passed();
    Ok(())
}

/// Elden Ring's environment textures: every one reads here, and one file per distinct header is read by DirectXTex.
#[test]
fn elden_environment_textures_read_as_directxtex_reads_them() -> Result<()> {
    let (Some(census), Some(dump), Some(dll)) = (
        oracle(OracleSet::SoulsFormats, Data::Elden, "census"),
        tool(Tool::DirectxtexDump),
        oodle(Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let Some(archive) = common::archive(Game::EldenRing).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let dll = common::oodle(dll).map_err(|e| e.to_string())?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    // the census's archive entries: shard, hash, path, size, sha256
    let mut paths = Vec::new();
    for line in std::fs::read_to_string(census.join("entries.tsv"))?.lines() {
        let path = line.split('\t').nth(2).ok_or("entries.tsv row")?;
        if path.contains("_envmap_") {
            paths.push(path.to_owned());
        }
    }
    // the first file of each distinct header (the 128 or 148 bytes before the payload), by header
    let firsts: Mutex<BTreeMap<Vec<u8>, (String, Vec<u8>)>> = Mutex::default();
    let per_binder = archive
        .read_each(
            &paths,
            &decoder,
            inputs::threads(),
            |path, bytes| -> Result<Vec<(Texture, [u8; 32])>> {
                let binder = Binder::read(bytes?, &decoder)?;
                let mut out = Vec::new();
                for (i, member) in binder.files.iter().enumerate() {
                    for t in Package::read(&member.bytes)?.textures {
                        let name = format!("{path}/{i}/{}", t.name);
                        let texture = read(name.clone(), &t);
                        if let Ok(p) = parse(&t.dds) {
                            let end = if matches!(p.header, primase_dds::HeaderKind::Dx10) {
                                148
                            } else {
                                128
                            };
                            let header = t.dds[..end].to_vec();
                            let mut firsts = firsts.lock().map_err(|_| "poisoned")?;
                            firsts.entry(header).or_insert_with(|| (name, t.dds.clone()));
                        }
                        out.push((texture, Sha256::digest(&t.dds).into()));
                    }
                }
                Ok(out)
            },
        )
        .map_err(|e| format!("{e:?}"))?;
    let mut reads = GateReport::new("elden ring environment textures read", Tolerance::Exact);
    let mut classes = BTreeMap::<String, usize>::new();
    let mut distinct = BTreeSet::new();
    let mut count = 0;
    for (path, binder) in paths.iter().zip(per_binder) {
        match binder {
            Err(e) => reads.fail(path, e.to_string()),
            Ok(textures) => {
                for (t, digest) in textures {
                    count += 1;
                    if !distinct.insert(digest) {
                        continue;
                    }
                    *classes.entry(class(&t)).or_default() += 1;
                    match &t.read {
                        Ok(_) => reads.record(&t.id, 0.0),
                        Err(e) => reads.fail(&t.id, e),
                    }
                }
            }
        }
    }
    let files: Vec<(String, Vec<u8>)> = firsts
        .into_inner()
        .map_err(|_| "poisoned")?
        .into_values()
        .collect();
    let mut gate = GateReport::new(
        "elden ring environment headers, read by DirectXTex",
        Tolerance::Exact,
    );
    let mut loads = BTreeMap::new();
    compare(&dump, &files, &mut gate, &mut loads)?;
    eprintln!(
        "elden ring: {} binders, {count} textures, {} distinct, {} distinct headers\nclasses={classes:#?}\nDirectXTex loads={loads:#?}\n{reads}\n{gate}",
        paths.len(),
        distinct.len(),
        files.len()
    );
    reads.assert_passed();
    gate.assert_passed();
    Ok(())
}
