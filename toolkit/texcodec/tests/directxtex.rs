//! The encoders here measured against Microsoft's DirectXTex may2026 (`tools.texconv`, `tools.directxtex_dump`). Each
//! image is encoded here and by texconv's CPU encoders (`-nogpu -m 1`), texconv once with uniform channel weights
//! (`-bc u`, the weighting here) and once with its default (perceptual weights for BC1-BC3), and all are decoded here and
//! measured against the source: per channel the largest difference and the PSNR in 8-bit levels, or for BC6H the RMS
//! difference of half-float bit patterns (the measure both encoders minimize). The encodes of BC1-BC6H (this crate's
//! own encoders) must be within [`LDR_MARGIN_DB`] and [`HDR_MARGIN`] of texconv's uniform ones on every image; BC7's
//! (`block_compression`'s) mean PSNR over a set must be at least texconv's. BC1 is measured on opaque copies of the
//! images (its one bit of alpha is measured apart: a texel is transparent here exactly where it is in texconv's).
//!
//! The images: the synthetic set (tests/images), and a sample of Sekiro's own textures (`oracles.scripts.sekiro/pbr`:
//! each file's largest stored level of at most 256x256, decoded here, encoded again in the formats of its kind).
//!
//! Every block written here is decoded by DirectXTex's library (directxtex-dump `decode`) and by bcdec as it is decoded
//! here.
#![allow(clippy::unwrap_used)]
mod images;
mod inputs;

use images::{Image, hdr, ldr};
use inputs::{Game, OracleSet, Query, Tool, expect_found, own_scratch, resolve, tool};
use primase_core::{ColorSpace, TexFormat};
use primase_dds::{AlphaMode, Dds, Shape, WriteHeader, parse, write};
use primase_foundations::parallel::Threads;
use primase_texcodec::{Bcdec, Decoder, Encoder, TexDecoder, TexEncoder};
use std::{collections::BTreeMap, fmt::Write as _, path::Path};

/// How far below texconv's uniform encode (PSNR, dB) an LDR encode here may be.
const LDR_MARGIN_DB: f64 = 0.1;
/// How far above texconv's (RMS of half patterns, as a fraction) a BC6H encode here may be.
const HDR_MARGIN: f64 = 0.02;

/// texconv's settings: uniform channel weights, and its default.
const SETTINGS: [&[&str]; 2] = [&["-bc", "u"], &[]];

/// texconv's name for a format, and the channels measured (alpha where the format stores it; BC1 apart).
fn dxgi(format: TexFormat) -> (&'static str, usize) {
    match format {
        TexFormat::Bc1 => ("BC1_UNORM", 3),
        TexFormat::Bc2 => ("BC2_UNORM", 4),
        TexFormat::Bc3 => ("BC3_UNORM", 4),
        TexFormat::Bc4Unorm => ("BC4_UNORM", 1),
        TexFormat::Bc4Snorm => ("BC4_SNORM", 1),
        TexFormat::Bc5Unorm => ("BC5_UNORM", 2),
        TexFormat::Bc5Snorm => ("BC5_SNORM", 2),
        TexFormat::Bc6hUfloat => ("BC6H_UF16", 3),
        TexFormat::Bc6hSfloat => ("BC6H_SF16", 3),
        TexFormat::Bc7 => ("BC7_UNORM", 4),
        f => panic!("{f:?} is not a BC format"),
    }
}

fn encoder() -> Encoder {
    Encoder::with_threads(Threads::new(4).unwrap())
}

/// A one-level 2D DDS of `level`.
fn dds(format: TexFormat, width: u32, height: u32, level: Vec<u8>) -> Vec<u8> {
    let d = Dds {
        shape: Shape::D2 { layers: 1 },
        width,
        height,
        format,
        color_space: ColorSpace::Linear,
        alpha_mode: AlphaMode::Unknown,
        levels: vec![vec![level]],
    };
    write(&d, WriteHeader::Dx10).unwrap()
}

/// texconv's encode of each file (`name`, DDS bytes) in `format` with `args`: the first level of each.
fn texconv(exe: &Path, dir: &Path, format: &str, args: &[&str], files: &[(String, Vec<u8>)]) -> Vec<Vec<u8>> {
    let input = dir.join("in");
    let output = dir.join(format!("{format}{}", args.concat()));
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&output).unwrap();
    let mut command = std::process::Command::new(exe);
    command
        .args(["-nologo", "-nogpu", "-y", "-m", "1", "-f", format])
        .args(args)
        .arg("-o")
        .arg(&output);
    for (name, bytes) in files {
        let path = input.join(format!("{name}.dds"));
        std::fs::write(&path, bytes).unwrap();
        command.arg(path);
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "texconv {format} {args:?}: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout)
    );
    files
        .iter()
        .map(|(name, _)| {
            let bytes = std::fs::read(output.join(format!("{name}.dds"))).unwrap();
            parse(&bytes).unwrap().dds.levels.swap_remove(0).swap_remove(0)
        })
        .collect()
}

/// DirectXTex's decode of each DDS file to RGBA32F, from one batch run.
fn directxtex_decode(dump: &Path, files: &[Vec<u8>]) -> Vec<Vec<f32>> {
    let dir = own_scratch("directxtex-decode");
    let mut list = String::new();
    for (i, bytes) in files.iter().enumerate() {
        let file = dir.join(format!("{i}.dds"));
        std::fs::write(&file, bytes).unwrap();
        writeln!(
            list,
            "decode\t{}\t{}",
            file.display(),
            dir.join(i.to_string()).display()
        )
        .unwrap();
    }
    let tsv = dir.join("list.tsv");
    std::fs::write(&tsv, list).unwrap();
    let out = std::process::Command::new(dump)
        .arg("batch")
        .arg(&tsv)
        .output()
        .unwrap();
    assert!(out.status.success(), "directxtex-dump batch: {}", out.status);
    (0..files.len())
        .map(|i| {
            let folder = dir.join(i.to_string());
            let meta = std::fs::read_to_string(folder.join("meta.json")).unwrap();
            assert!(meta.contains("0x00000000"), "file {i}: {meta}");
            std::fs::read(folder.join("floats.bin"))
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect()
        })
        .collect()
}

/// Per channel: the largest difference and the mean squared difference.
#[derive(Clone, Copy, Default)]
struct Errors {
    max: [f64; 4],
    mse: [f64; 4],
}

impl Errors {
    fn of(a: impl Iterator<Item = f64>, b: impl Iterator<Item = f64>) -> Errors {
        let mut e = Errors::default();
        let mut n = 0usize;
        for (i, (x, y)) in a.zip(b).enumerate() {
            let d = (x - y).abs();
            e.max[i % 4] = e.max[i % 4].max(d);
            e.mse[i % 4] += d * d;
            n += 1;
        }
        let texels = (n / 4) as f64;
        e.mse = e.mse.map(|s| s / texels);
        e
    }

    fn mse(&self, channels: usize) -> f64 {
        self.mse[..channels].iter().sum::<f64>() / channels as f64
    }

    /// The PSNR over the first `channels` channels, in 8-bit levels.
    fn psnr(&self, channels: usize) -> f64 {
        10. * (255. * 255. / self.mse(channels)).log10()
    }

    /// The RMS difference over the first `channels` channels.
    fn rms(&self, channels: usize) -> f64 {
        self.mse(channels).sqrt()
    }
}

/// A float's half-float bit pattern as an integer, its sign apart (the space BC6H is fitted in).
fn half_pattern(v: f32, signed: bool) -> f64 {
    let h = half::f16::from_f32(v).to_bits();
    let m = f64::from((h & 0x7fff).min(0x7bff));
    match (h & 0x8000 != 0, signed) {
        (false, _) => m,
        (true, true) => -m,
        (true, false) => 0.,
    }
}

/// The texels of an image to encode.
#[derive(Clone)]
enum Texels {
    Bytes(Vec<u8>),
    Floats(Vec<f32>),
}

/// An image to encode: its name, size and texels.
#[derive(Clone)]
struct Source {
    name: String,
    width: u32,
    height: u32,
    texels: Texels,
}

impl Source {
    fn ldr(image: &Image<u8>) -> Source {
        Source {
            name: image.name.to_owned(),
            width: image.width,
            height: image.height,
            texels: Texels::Bytes(image.texels.clone()),
        }
    }

    fn hdr(image: &Image<f32>) -> Source {
        Source {
            name: image.name.to_owned(),
            width: image.width,
            height: image.height,
            texels: Texels::Floats(image.texels.clone()),
        }
    }

    /// The image with every alpha 1.
    fn opaque(&self) -> Source {
        let texels = match &self.texels {
            Texels::Bytes(b) => Texels::Bytes(
                b.iter()
                    .enumerate()
                    .map(|(i, &v)| if i % 4 == 3 { 255 } else { v })
                    .collect(),
            ),
            Texels::Floats(f) => Texels::Floats(
                f.iter()
                    .enumerate()
                    .map(|(i, &v)| if i % 4 == 3 { 1. } else { v })
                    .collect(),
            ),
        };
        Source {
            name: self.name.clone(),
            width: self.width,
            height: self.height,
            texels,
        }
    }

    /// The image as an uncompressed DDS file, for texconv.
    fn file(&self) -> Vec<u8> {
        match &self.texels {
            Texels::Bytes(b) => dds(TexFormat::Rgba8Unorm, self.width, self.height, b.clone()),
            Texels::Floats(f) => dds(
                TexFormat::Rgba32Float,
                self.width,
                self.height,
                f.iter().flat_map(|v| v.to_le_bytes()).collect(),
            ),
        }
    }

    fn encode(&self, format: TexFormat) -> Vec<u8> {
        match &self.texels {
            Texels::Bytes(b) => encoder().encode_rgba8(format, self.width, self.height, b),
            Texels::Floats(f) => encoder().encode_rgba32f(format, self.width, self.height, f),
        }
        .unwrap()
    }

    /// The errors of `blocks` (this image in `format`) against the image, in the format's measure.
    fn errors(&self, format: TexFormat, blocks: &[u8]) -> Errors {
        let decoder = Decoder::default();
        match (&self.texels, format) {
            (Texels::Floats(f), TexFormat::Bc6hUfloat | TexFormat::Bc6hSfloat) => {
                let signed = format == TexFormat::Bc6hSfloat;
                let d = decoder
                    .decode_rgba32f(format, self.width, self.height, blocks)
                    .unwrap();
                Errors::of(
                    f.iter().map(|&v| half_pattern(v, signed)),
                    d.iter().map(|&v| half_pattern(v, signed)),
                )
            }
            (Texels::Bytes(b), _) => {
                let d = decoder
                    .decode_rgba8(format, self.width, self.height, blocks)
                    .unwrap();
                Errors::of(b.iter().map(|&v| f64::from(v)), d.iter().map(|&v| f64::from(v)))
            }
            _ => panic!("{}: {format:?} from floats is not measured", self.name),
        }
    }
}

/// One image in one format: the errors here and texconv's (uniform, default), and the blocks written here.
struct Row {
    format: TexFormat,
    name: String,
    ours: Errors,
    theirs: [Errors; 2],
    width: u32,
    height: u32,
    blocks: Vec<u8>,
}

impl Row {
    fn hdr(&self) -> bool {
        matches!(self.format, TexFormat::Bc6hUfloat | TexFormat::Bc6hSfloat)
    }

    /// The measure here and texconv's (uniform, default): PSNR (LDR) or RMS of half patterns (BC6H).
    fn scores(&self) -> (f64, [f64; 2]) {
        let channels = dxgi(self.format).1;
        let score = |e: &Errors| {
            if self.hdr() {
                e.rms(channels)
            } else {
                e.psnr(channels)
            }
        };
        (score(&self.ours), self.theirs.each_ref().map(score))
    }

    /// Whether the encode here is within the margin of texconv's uniform one (BC7 is held to its mean instead).
    fn within_margin(&self) -> bool {
        let (ours, [uniform, _]) = self.scores();
        if self.format == TexFormat::Bc7 {
            true
        } else if self.hdr() {
            ours <= uniform * (1. + HDR_MARGIN)
        } else {
            ours >= uniform - LDR_MARGIN_DB
        }
    }

    fn line(&self) -> String {
        let channels = dxgi(self.format).1;
        let (ours, [uniform, default]) = self.scores();
        let unit = if self.hdr() { "half RMS" } else { "PSNR dB" };
        format!(
            "{:>9} {:<28} {unit}: here {ours:8.2}, texconv {uniform:8.2} uniform {default:8.2} default; largest difference here {:?}, texconv uniform {:?}",
            dxgi(self.format).0,
            self.name,
            &self.ours.max[..channels],
            &self.theirs[0].max[..channels],
        )
    }
}

/// Each source encoded in `format` here and with texconv (both settings), all measured.
fn compare(exe: &Path, dir: &Path, format: TexFormat, sources: &[Source]) -> Vec<Row> {
    let files: Vec<(String, Vec<u8>)> = sources.iter().map(|s| (s.name.clone(), s.file())).collect();
    let theirs: Vec<Vec<Vec<u8>>> = SETTINGS
        .iter()
        .map(|args| texconv(exe, dir, dxgi(format).0, args, &files))
        .collect();
    sources
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let blocks = s.encode(format);
            Row {
                format,
                name: s.name.clone(),
                ours: s.errors(format, &blocks),
                theirs: [0, 1].map(|t| s.errors(format, &theirs[t][k])),
                width: s.width,
                height: s.height,
                blocks,
            }
        })
        .collect()
}

/// BC1's one bit of alpha: each source encoded here and by texconv, the texels transparent in each.
fn cutout(exe: &Path, dir: &Path, sources: &[Source]) {
    let files: Vec<(String, Vec<u8>)> = sources.iter().map(|s| (s.name.clone(), s.file())).collect();
    let theirs = texconv(exe, dir, "BC1_UNORM", &["-bc", "u"], &files);
    for (s, theirs) in sources.iter().zip(theirs) {
        let decode = |blocks: &[u8]| -> Vec<bool> {
            let d = Decoder::default()
                .decode_rgba8(TexFormat::Bc1, s.width, s.height, blocks)
                .unwrap();
            d.chunks(4).map(|t| t[3] == 0).collect()
        };
        let ours = decode(&s.encode(TexFormat::Bc1));
        assert_eq!(
            ours,
            decode(&theirs),
            "{}: BC1 transparency differs from texconv's",
            s.name
        );
        eprintln!(
            "BC1 cutout {}: {} of {} texels transparent here and in texconv's",
            s.name,
            ours.iter().filter(|&&t| t).count(),
            ours.len()
        );
    }
}

/// Every block written here decodes alike here, in bcdec and in DirectXTex: in bcdec (BC1-BC5, the formats decoded here
/// independently of it) bit for bit, SNORM ones to within float rounding (an entry of exactly 0 is 127.5 levels when
/// biased to bytes, a tie the float rounding of bcdec's interpolation can tip); in DirectXTex to the same floats for BC6H, the same 8-bit levels for BC7, and
/// within one level for BC1-BC5, whose palette entries DirectXTex does not round (D3D rounds them, as the decoders here
/// do). Only the channels a format stores are compared (DirectXTex copies BC4's red into green and blue).
fn decoded_alike(dump: &Path, rows: &[Row]) -> String {
    let theirs = directxtex_decode(
        dump,
        &rows
            .iter()
            .map(|r| dds(r.format, r.width, r.height, r.blocks.clone()))
            .collect::<Vec<_>>(),
    );
    let mut worst = BTreeMap::<String, f64>::new();
    for (r, theirs) in rows.iter().zip(theirs) {
        let (format, w, h, blocks) = (r.format, r.width, r.height, &r.blocks);
        let ours = Decoder::default().decode_rgba32f(format, w, h, blocks).unwrap();
        let hdr = matches!(format, TexFormat::Bc6hUfloat | TexFormat::Bc6hSfloat);
        let snorm = matches!(format, TexFormat::Bc4Snorm | TexFormat::Bc5Snorm);
        if snorm {
            let bcdec = Bcdec.decode_rgba32f(format, w, h, blocks).unwrap();
            let d = ours
                .iter()
                .zip(&bcdec)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            assert!(
                d <= 1e-6,
                "{format:?} {}: bcdec decodes the blocks {d} away",
                r.name
            );
        } else if !hdr && format != TexFormat::Bc7 {
            let here = Decoder::default().decode_rgba8(format, w, h, blocks).unwrap();
            let bcdec = Bcdec.decode_rgba8(format, w, h, blocks).unwrap();
            if let Some(i) = (0..here.len()).find(|&i| here[i] != bcdec[i]) {
                let (x, y) = ((i / 4) as u32 % w, (i / 4) as u32 / w);
                let at = ((y / 4) * w.div_ceil(4) + x / 4) as usize * format.block_bytes() as usize;
                panic!(
                    "{format:?} {}: texel ({x}, {y}) channel {} decodes to {} here, {} in bcdec; block {:?}",
                    r.name,
                    i % 4,
                    here[i],
                    bcdec[i],
                    &blocks[at..at + format.block_bytes() as usize]
                );
            }
        }
        let stored = match format {
            TexFormat::Bc4Unorm | TexFormat::Bc4Snorm => 1,
            TexFormat::Bc5Unorm | TexFormat::Bc5Snorm => 2,
            _ => 4,
        };
        let scale = if hdr { 1. } else { 255. };
        let d = ours
            .iter()
            .zip(&theirs)
            .enumerate()
            .filter(|(i, _)| i % 4 < stored)
            .map(|(_, (a, b))| f64::from((a - b).abs()) * scale)
            .fold(0., f64::max);
        let bound = match format {
            _ if hdr => 0.,
            // the same level, through float rounding
            TexFormat::Bc7 => 1e-3,
            _ => 1.,
        };
        assert!(
            d <= bound,
            "{format:?} {}: DirectXTex decodes it {d} away",
            r.name
        );
        let e = worst.entry(dxgi(format).0.to_owned()).or_default();
        *e = e.max(d);
    }
    format!(
        "{} encodes decoded by DirectXTex; largest difference by format (8-bit levels; BC6H in floats): {worst:?}",
        rows.len()
    )
}

/// Prints the rows and a summary by format, and fails on any row outside the margin.
fn report(rows: &[Row]) {
    let mut by_format = BTreeMap::<&str, (usize, f64, f64, f64, usize)>::new();
    for r in rows {
        eprintln!("{}", r.line());
        let (ours, [uniform, default]) = r.scores();
        let e = by_format.entry(dxgi(r.format).0).or_default();
        // finite scores only in the means (an exact encode's PSNR is infinite)
        if ours.is_finite() && uniform.is_finite() && default.is_finite() {
            e.0 += 1;
            e.1 += ours;
            e.2 += uniform;
            e.3 += default;
        }
        e.4 += usize::from(!r.within_margin());
    }
    for (format, (n, ours, uniform, default, outside)) in &by_format {
        let n = *n as f64;
        eprintln!(
            "{format:>9}: mean over the finite rows here {:.2}, texconv {:.2} uniform {:.2} default; {outside} outside the margin",
            ours / n,
            uniform / n,
            default / n
        );
    }
    let outside: Vec<String> = rows
        .iter()
        .filter(|r| !r.within_margin())
        .map(Row::line)
        .collect();
    assert!(
        outside.is_empty(),
        "outside the margin of texconv's encode:\n{}",
        outside.join("\n")
    );
    if let Some((_, ours, uniform, _, _)) = by_format.get("BC7_UNORM") {
        assert!(ours >= uniform, "BC7's mean PSNR is below texconv's");
    }
}

/// The sources as `format` is measured on them: opaque copies for BC1.
fn for_format(format: TexFormat, sources: &[Source]) -> Vec<Source> {
    if format == TexFormat::Bc1 {
        sources.iter().map(Source::opaque).collect()
    } else {
        sources.to_vec()
    }
}

#[test]
fn synthetic_images_encode_as_well_as_texconv() {
    let (Some(exe), Some(dump)) = (tool(Tool::Texconv), tool(Tool::DirectxtexDump)) else {
        return;
    };
    let dir = own_scratch("texconv-synthetic");
    let images: Vec<Source> = ldr().iter().map(Source::ldr).collect();
    let mut rows = Vec::new();
    for format in [
        TexFormat::Bc1,
        TexFormat::Bc2,
        TexFormat::Bc3,
        TexFormat::Bc4Unorm,
        TexFormat::Bc4Snorm,
        TexFormat::Bc5Unorm,
        TexFormat::Bc5Snorm,
        TexFormat::Bc7,
    ] {
        rows.extend(compare(&exe, &dir, format, &for_format(format, &images)));
    }
    for format in [TexFormat::Bc6hUfloat, TexFormat::Bc6hSfloat] {
        let sources: Vec<Source> = hdr(format == TexFormat::Bc6hSfloat)
            .iter()
            .map(Source::hdr)
            .collect();
        rows.extend(compare(&exe, &dir, format, &sources));
    }
    cutout(&exe, &dir, &images);
    eprintln!("{}", decoded_alike(&dump, &rows));
    report(&rows);
}

/// Every `.dds` file under `dir`.
fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dds")) {
            out.push(path);
        }
    }
}

/// The largest stored level of at most 256x256 (and at least 16x16) of a 2D DDS file, decoded to RGBA8, and the format
/// the file stores.
fn sample(name: String, bytes: &[u8]) -> Option<(TexFormat, Source)> {
    let d = parse(bytes).ok()?.dds;
    if d.shape != (Shape::D2 { layers: 1 }) {
        return None;
    }
    let (level, w, h) = (0..d.levels.len())
        .map(|l| (l, (d.width >> l).max(1), (d.height >> l).max(1)))
        .find(|&(_, w, h)| w * h <= 256 * 256)?;
    if w < 16 || h < 16 {
        return None;
    }
    let rgba = Decoder::default()
        .decode_rgba8(d.format, w, h, &d.levels[level][0])
        .ok()?;
    Some((
        d.format,
        Source {
            name,
            width: w,
            height: h,
            texels: Texels::Bytes(rgba),
        },
    ))
}

#[test]
fn sekiro_textures_encode_as_well_as_texconv() {
    let (Some(exe), Some(dump)) = (tool(Tool::Texconv), tool(Tool::DirectxtexDump)) else {
        return;
    };
    let query = Query::Oracle(OracleSet::Scripts, Game::Sekiro, "pbr");
    let Some(pbr) = resolve(query) else { return };
    let mut paths = Vec::new();
    collect(&pbr.join("census").join("dds"), &mut paths);
    paths.sort();
    expect_found(query, "DDS files", paths.len());
    // each file's format from its header, then an even spread of up to 12 files of each stored format
    let mut by_format = BTreeMap::<String, Vec<&std::path::PathBuf>>::new();
    for path in &paths {
        let mut header = [0u8; 148];
        let mut file = std::fs::File::open(path).unwrap();
        if std::io::Read::read_exact(&mut file, &mut header).is_err() {
            continue;
        }
        let format = match &header[84..88] {
            b"DX10" => match u32::from_le_bytes(header[128..132].try_into().unwrap()) {
                71 | 72 => "bc1",
                98 | 99 => "bc7",
                80 => "bc4",
                _ => continue,
            },
            b"DXT1" => "bc1",
            b"ATI1" | b"BC4U" => "bc4",
            _ => continue,
        };
        by_format.entry(format.to_owned()).or_default().push(path);
    }
    let dir = own_scratch("texconv-sekiro");
    let mut rows = Vec::new();
    let mut counts = BTreeMap::new();
    for (stored, files) in &by_format {
        let step = files.len().div_ceil(12).max(1);
        let sources: Vec<Source> = files
            .iter()
            .step_by(step)
            .enumerate()
            .filter_map(|(i, path)| {
                sample(format!("{stored}-{i}"), &std::fs::read(path).unwrap()).map(|(_, s)| s)
            })
            .collect();
        counts.insert(stored.clone(), (files.len(), sources.len()));
        let targets: &[TexFormat] = match stored.as_str() {
            "bc7" => &[
                TexFormat::Bc1,
                TexFormat::Bc2,
                TexFormat::Bc3,
                TexFormat::Bc5Unorm,
                TexFormat::Bc5Snorm,
                TexFormat::Bc7,
            ],
            "bc1" => &[TexFormat::Bc1, TexFormat::Bc3, TexFormat::Bc7],
            _ => &[TexFormat::Bc4Unorm, TexFormat::Bc4Snorm, TexFormat::Bc5Unorm],
        };
        for &format in targets {
            rows.extend(compare(&exe, &dir, format, &for_format(format, &sources)));
        }
    }
    eprintln!(
        "{}: files by stored format, and those sampled: {counts:?}",
        query.key()
    );
    eprintln!("{}", decoded_alike(&dump, &rows));
    report(&rows);
}
