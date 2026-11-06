//! Every cube and volume texture of the two installs reads with primase-dds as DirectXTex's own library reads it.
//!
//! Sekiro's are the cube and volume textures of the packages the SoulsFormats census finds them in
//! (`oracles.soulsformats.sekiro/tpf`), and every distinct one goes to DirectXTex. Elden Ring's are every texture of the
//! environment binders (`_envmap_`) its census lists (`oracles.soulsformats.elden/census`): each reads here, and one
//! file per distinct DDS header goes to DirectXTex (the header fixes the layout the payload is cut by, so files that
//! share it differ only in their bytes). A DX10 cube whose arraySize counts faces (6 per cube) DirectXTex reads only
//! with its permissive load, and refuses with its default one; every other file it reads with its default load.
//!
//! Sekiro's BC6H cubes are also real HDR images for primase-texcodec's BC6H encoder: a spread of them, each face
//! decoded and encoded again here and by texconv (`tools.texconv`, DirectXTex may2026's CPU encoder), the encode here
//! within [`BC6H_MARGIN`] of texconv's in the RMS difference of half-float bit patterns, and decoded by DirectXTex's
//! library as it is decoded here.
mod common;
mod inputs;

use fromsoft_formats::{
    Binder, Game,
    dcx::Decoder,
    tpf::{Package, TextureKind},
};
use inputs::{Game as Data, OracleSet, Program, Tool, oodle, oracle, own_scratch, program};
use primase_core::{ColorSpace, TexFormat};
use primase_dds::{AlphaMode, Dds, Shape, WriteHeader, parse, to_dxgi, write};
use primase_testdata::gate::{GateReport, Tolerance};
use primase_texcodec::{Encoder, TexDecoder, TexEncoder};
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

/// directxtex-dump's answers for `files`, each a command and a file's bytes, from one batch run: `meta.json`, and
/// `images.bin` (`read`) or `floats.bin` (`decode`).
fn directxtex(dump: &Program, files: &[(&str, &[u8])]) -> Result<Vec<(Value, Vec<u8>)>> {
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
    let out = dump.command().arg("batch").path_arg(&tsv).output()?;
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
            let data = std::fs::read(folder.join("images.bin"))
                .or_else(|_| std::fs::read(folder.join("floats.bin")))
                .unwrap_or_default();
            Ok((meta, data))
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
    dump: &Program,
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

/// Sekiro's cube and volume textures: the number of packages the census finds them in, the census's count of them, and
/// each distinct one (by content) as `<package id>/<texture name>` and the texture.
struct Environment {
    packages: usize,
    census_textures: usize,
    textures: Vec<(String, fromsoft_formats::tpf::Texture)>,
}

/// A DDS header's bytes, up to the payload.
type Header = Vec<u8>;

/// A file's name and bytes.
type Named = (String, Vec<u8>);

/// [`Environment`] from the census at `census`, or `None` when the install or Oodle is not configured.
fn sekiro_environment(census: &Path) -> Result<Option<Environment>> {
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(None);
    };
    let Some(archive) = common::archive(Game::Sekiro).map_err(|e| e.to_string())? else {
        return Ok(None);
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
    for id in &packages {
        let (_, hash) = id.split_once('/').ok_or("id")?;
        if hash.contains('/') {
            return Err(format!(
                "{id}: a binder member; the census put every cube and volume in a package of its own"
            )
            .into());
        }
        let package = Package::read(&archive.read_hash(u64::from_str_radix(hash, 16)?, &decoder)?)?;
        for t in package
            .textures
            .into_iter()
            .filter(|t| t.kind != TextureKind::Image2D)
        {
            if seen.insert(<[u8; 32]>::from(Sha256::digest(&t.dds))) {
                textures.push((format!("{id}/{}", t.name), t));
            }
        }
    }
    Ok(Some(Environment {
        packages: packages.len(),
        census_textures,
        textures,
    }))
}

/// Sekiro's cube and volume textures, each distinct one read by DirectXTex too.
#[test]
fn sekiro_cubes_and_volumes_read_as_directxtex_reads_them() -> Result<()> {
    let (Some(census), Some(dump)) = (
        oracle(OracleSet::SoulsFormats, Data::Sekiro, "tpf"),
        program(Tool::DirectxtexDump),
    ) else {
        return Ok(());
    };
    let Some(Environment {
        packages,
        census_textures,
        textures: found,
    }) = sekiro_environment(&census)?
    else {
        return Ok(());
    };
    let mut textures = Vec::new();
    let mut files = Vec::new();
    for (name, t) in found {
        let texture = read(name, &t);
        if texture.read.is_ok() {
            files.push((texture.id.clone(), t.dds));
        }
        textures.push(texture);
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
        "sekiro: {packages} packages, {census_textures} cube and volume textures, {} distinct\nclasses={classes:#?}\nDirectXTex loads={loads:#?}\n{reads}\n{gate}",
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
        program(Tool::DirectxtexDump),
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
    let firsts: Mutex<BTreeMap<Header, Named>> = Mutex::default();
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

/// How far above texconv's RMS difference of half-float bit patterns (as a fraction) a BC6H encode here may be.
const BC6H_MARGIN: f64 = 0.02;

/// A float's half-float bit pattern as an integer (UF16: negative values are 0), the space BC6H is fitted in.
fn half_pattern(v: f32) -> f64 {
    let h = half::f16::from_f32(v).to_bits();
    if h & 0x8000 != 0 {
        0.
    } else {
        f64::from(h.min(0x7bff))
    }
}

/// The RMS difference of the RGB half patterns of two RGBA images.
fn rms(a: &[f32], b: &[f32]) -> f64 {
    let (sum, n) = a
        .iter()
        .zip(b)
        .enumerate()
        .filter(|(i, _)| i % 4 < 3)
        .fold((0., 0usize), |(s, n), (_, (x, y))| {
            (s + (half_pattern(*x) - half_pattern(*y)).powi(2), n + 1)
        });
    (sum / n as f64).sqrt()
}

/// A one-level 2D DDS of `level`.
fn one_level(format: TexFormat, width: u32, height: u32, level: Vec<u8>) -> Result<Vec<u8>> {
    Ok(write(
        &Dds {
            shape: Shape::D2 { layers: 1 },
            width,
            height,
            format,
            color_space: ColorSpace::Linear,
            alpha_mode: AlphaMode::Unknown,
            levels: vec![vec![level]],
        },
        WriteHeader::Dx10,
    )?)
}

/// A spread of Sekiro's BC6H cubes: each face of the largest level of at most 128x128 decoded, then encoded again here
/// and by texconv; the encode here within [`BC6H_MARGIN`] of texconv's on every face, and decoded by DirectXTex as here.
#[test]
fn sekiro_bc6h_cubes_encode_again_as_well_as_texconv() -> Result<()> {
    let (Some(census), Some(texconv), Some(dump)) = (
        oracle(OracleSet::SoulsFormats, Data::Sekiro, "tpf"),
        program(Tool::Texconv),
        program(Tool::DirectxtexDump),
    ) else {
        return Ok(());
    };
    let Some(environment) = sekiro_environment(&census)? else {
        return Ok(());
    };
    let cubes: Vec<(String, Dds)> = environment
        .textures
        .iter()
        .filter_map(|(name, t)| {
            let d = parse(&t.dds).ok()?.dds;
            (d.format == TexFormat::Bc6hUfloat && d.shape == (Shape::Cube { cubes: 1 }))
                .then(|| (name.clone(), d))
        })
        .collect();
    let decoder = primase_texcodec::Decoder::default();
    // the faces: each sampled cube's six at its largest level of at most 128x128
    let mut faces = Vec::new();
    for (name, d) in cubes.iter().step_by(cubes.len().div_ceil(8).max(1)) {
        let Some(level) = (0..d.levels.len()).find(|&l| (d.width >> l).max(1) <= 128) else {
            continue;
        };
        let size = (d.width >> level).max(1);
        for (face, blocks) in d.levels[level].iter().enumerate() {
            let floats = decoder.decode_rgba32f(TexFormat::Bc6hUfloat, size, size, blocks)?;
            faces.push((format!("{name}/face {face}"), size, floats));
        }
    }
    let dir = own_scratch("texconv-bc6h");
    let (input, output) = (dir.join("in"), dir.join("out"));
    std::fs::create_dir_all(&input)?;
    std::fs::create_dir_all(&output)?;
    let mut command = texconv.command();
    command
        .args(["-nologo", "-nogpu", "-y", "-m", "1", "-f", "BC6H_UF16", "-o"])
        .path_arg(&output);
    for (i, (_, size, floats)) in faces.iter().enumerate() {
        let bytes = floats.iter().flat_map(|v| v.to_le_bytes()).collect();
        let path = input.join(format!("{i}.dds"));
        std::fs::write(&path, one_level(TexFormat::Rgba32Float, *size, *size, bytes)?)?;
        command.path_arg(path);
    }
    let out = command.output()?;
    if !out.status.success() {
        return Err(format!(
            "texconv: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout)
        )
        .into());
    }
    let encoder = Encoder::with_threads(primase_foundations::parallel::Threads(inputs::threads()));
    let mut gate = GateReport::new(
        "sekiro BC6H cube faces encoded again, half-pattern RMS over texconv's",
        Tolerance::Abs(1. + BC6H_MARGIN),
    );
    let mut ours_files = Vec::new();
    let (mut ours_sum, mut theirs_sum, mut worst) = (0., 0., 0f64);
    for (i, (name, size, floats)) in faces.iter().enumerate() {
        let ours = encoder.encode_rgba32f(TexFormat::Bc6hUfloat, *size, *size, floats)?;
        let theirs = parse(&std::fs::read(output.join(format!("{i}.dds")))?)?
            .dds
            .levels
            .swap_remove(0)
            .swap_remove(0);
        let ours_rms = rms(
            floats,
            &decoder.decode_rgba32f(TexFormat::Bc6hUfloat, *size, *size, &ours)?,
        );
        let theirs_rms = rms(
            floats,
            &decoder.decode_rgba32f(TexFormat::Bc6hUfloat, *size, *size, &theirs)?,
        );
        ours_sum += ours_rms;
        theirs_sum += theirs_rms;
        // the ratio, or 1 where both are exact
        let ratio = if theirs_rms > 0. {
            ours_rms / theirs_rms
        } else if ours_rms > 0. {
            f64::INFINITY
        } else {
            1.
        };
        gate.record(name, ratio);
        worst = worst.max(ratio);
        ours_files.push((name.clone(), *size, ours));
    }
    // DirectXTex decodes every block written here to the floats decoded here (bcdec)
    let files: Vec<Vec<u8>> = ours_files
        .iter()
        .map(|(_, size, blocks)| one_level(TexFormat::Bc6hUfloat, *size, *size, blocks.clone()))
        .collect::<Result<_>>()?;
    let answers = directxtex(
        &dump,
        &files.iter().map(|b| ("decode", b.as_slice())).collect::<Vec<_>>(),
    )?;
    let mut decoded = GateReport::new(
        "sekiro BC6H encodes here, decoded by DirectXTex",
        Tolerance::Exact,
    );
    for ((name, size, blocks), (meta, theirs)) in ours_files.iter().zip(&answers) {
        let ours = decoder.decode_rgba32f(TexFormat::Bc6hUfloat, *size, *size, blocks)?;
        let theirs: Vec<f32> = theirs
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        if meta["hr"] != "0x00000000" || theirs.len() != ours.len() {
            decoded.fail(name, format!("DirectXTex: {}", meta["hr"]));
        } else {
            let d = ours
                .iter()
                .zip(&theirs)
                .map(|(a, b)| f64::from((a - b).abs()))
                .fold(0., f64::max);
            decoded.record(name, d);
        }
    }
    let n = faces.len() as f64;
    eprintln!(
        "sekiro: {} BC6H cubes, {} sampled faces; mean half-pattern RMS here {:.2}, texconv {:.2}; largest ratio {worst:.3}\n{gate}\n{decoded}",
        cubes.len(),
        faces.len(),
        ours_sum / n,
        theirs_sum / n
    );
    gate.assert_passed();
    decoded.assert_passed();
    Ok(())
}
