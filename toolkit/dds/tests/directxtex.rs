//! The DirectXTex layout fixtures (`fixtures/directxtex`, made by its `generate.py` with texconv and texassemble from
//! Microsoft's DirectXTex may2026 release): every layout and header texconv writes reads here as DirectXTex's own
//! library read it (`directxtex.json`, recorded with directxtex-dump), and writing what was read with the header texconv
//! chose gives the file back byte for byte. With directxtex-dump configured (`tools.directxtex_dump`), DirectXTex's
//! library also reads every file this writer writes, in every shape, format, header and alpha mode, as this reader reads
//! it.
#![allow(clippy::unwrap_used)]
mod inputs;

use inputs::{Tool, own_scratch, tool};
use primase_core::{ColorSpace, TexFormat};
use primase_dds::{AlphaMode, Dds, Shape, WriteHeader, from_dxgi, parse, to_dxgi, write};
use primase_testdata::gate::{GateReport, Tolerance};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

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
        u64::from(to_dxgi(d.format, d.color_space).unwrap()),
        dimension,
    ]
}

/// The images in DirectXTex's order (file order): item by item with their mips, or mip by mip with their slices.
fn images(d: &Dds) -> Vec<&[u8]> {
    match d.shape {
        Shape::D3 { .. } => d.levels.iter().flatten().map(Vec::as_slice).collect(),
        _ => (0..d.levels[0].len())
            .flat_map(|item| d.levels.iter().map(move |level| level[item].as_slice()))
            .collect(),
    }
}

#[test]
fn texconv_layouts_read_as_directxtex_reads_them_and_write_back_exactly() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/directxtex");
    let record: Value =
        serde_json::from_slice(&std::fs::read(root.join("directxtex.json")).unwrap()).unwrap();
    let mut reads = GateReport::new(
        "DirectXTex metadata and images of texconv files",
        Tolerance::Exact,
    );
    let mut writes = GateReport::new(
        "texconv files rewritten with texconv's header choice",
        Tolerance::Exact,
    );
    let mut shapes = std::collections::BTreeSet::new();
    for case in record["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let bytes = std::fs::read(root.join(format!("{name}.dds"))).unwrap();
        let parsed = parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let d = &parsed.dds;
        assert!(!parsed.faces_counted, "{name}");
        let x = &case["directxtex"];
        let expected = [
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
        .map(|k| x[k].as_u64().unwrap());
        assert_eq!(
            metadata(d),
            expected,
            "{name}: width height depth arraySize mips misc misc2 format dimension"
        );
        let ours = images(d);
        let theirs = x["images"].as_array().unwrap();
        assert_eq!(ours.len(), theirs.len(), "{name}: images");
        for (i, (image, t)) in ours.iter().zip(theirs).enumerate() {
            let mip = t["mip"].as_u64().unwrap() as u32;
            let (w, h) = ((d.width >> mip).max(1), (d.height >> mip).max(1));
            assert_eq!(
                (u64::from(w), u64::from(h), image.len() as u64),
                (
                    t["width"].as_u64().unwrap(),
                    t["height"].as_u64().unwrap(),
                    t["slicePitch"].as_u64().unwrap()
                ),
                "{name} image {i}"
            );
            reads.record(
                format!("{name}/{i}"),
                f64::from(u8::from(hex(image) != t["sha256"].as_str().unwrap())),
            );
        }
        let header = match case["header"].as_str().unwrap() {
            "auto" => WriteHeader::Auto,
            "legacy" => WriteHeader::Legacy,
            "dx10" => WriteHeader::Dx10,
            h => panic!("{name}: header {h}"),
        };
        writes.compare_bytes(name, &write(d, header).unwrap(), &bytes);
        shapes.insert(format!("{:?}", std::mem::discriminant(&d.shape)));
    }
    eprintln!("{reads}\n{writes}");
    assert_eq!(shapes.len(), 4, "1D, 2D, cube and volume fixtures");
    reads.assert_passed();
    writes.assert_passed();
}

/// One answer of directxtex-dump: its `meta.json` and its `images.bin`.
struct Answer {
    meta: Value,
    images: Vec<u8>,
}

/// DirectXTex's answers for `files` (each a command, `read` or `read-permissive`, and a file's bytes), from one batch run.
fn directxtex(dump: &std::path::Path, files: &[(&str, Vec<u8>)]) -> Vec<Answer> {
    let dir = own_scratch("directxtex-dds");
    let mut list = String::new();
    for (i, (command, bytes)) in files.iter().enumerate() {
        let file = dir.join(format!("{i}.dds"));
        std::fs::write(&file, bytes).unwrap();
        list += &format!(
            "{command}\t{}\t{}\n",
            file.display(),
            dir.join(i.to_string()).display()
        );
    }
    let tsv = dir.join("list.tsv");
    std::fs::write(&tsv, list).unwrap();
    let out = std::process::Command::new(dump)
        .arg("batch")
        .arg(&tsv)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "directxtex-dump batch: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    (0..files.len())
        .map(|i| {
            let folder = dir.join(i.to_string());
            Answer {
                meta: serde_json::from_slice(&std::fs::read(folder.join("meta.json")).unwrap()).unwrap(),
                images: std::fs::read(folder.join("images.bin")).unwrap_or_default(),
            }
        })
        .collect()
}

/// Whether DirectXTex read `d` from the file: the same metadata and the same bytes for every image, in its order.
fn agrees(name: &str, d: &Dds, answer: &Answer) -> Result<(), String> {
    let x = &answer.meta;
    let hr = x["hr"].as_str().unwrap();
    if hr != "0x00000000" {
        return Err(format!("{name}: DirectXTex refused it ({hr})"));
    }
    let theirs = [
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
    .map(|k| x[k].as_u64().unwrap());
    if metadata(d) != theirs {
        return Err(format!(
            "{name}: metadata {:?}, DirectXTex {theirs:?}",
            metadata(d)
        ));
    }
    let ours = images(d).concat();
    if ours != answer.images {
        return Err(format!(
            "{name}: images differ ({} and {} bytes)",
            ours.len(),
            answer.images.len()
        ));
    }
    Ok(())
}

/// A resource whose every image is distinct: byte `j` of image `i` of level `l` is `j + 7 i + 61 l`.
fn resource(shape: Shape, format: TexFormat, color_space: ColorSpace, alpha_mode: AlphaMode) -> Dds {
    let (width, height, mips) = match shape {
        Shape::D1 { .. } => (16, 1, 5),
        Shape::Cube { .. } => (8, 8, 4),
        Shape::D3 { .. } => (8, 4, 4),
        Shape::D2 { .. } => (13, 7, 4),
    };
    Dds {
        shape,
        width,
        height,
        format,
        color_space,
        alpha_mode,
        levels: (0..mips)
            .map(|l| {
                let size = format.level_size((width >> l).max(1), (height >> l).max(1));
                (0..shape.images_at(l))
                    .map(|i| {
                        (0..size)
                            .map(|j| (j as u64 + 7 * i + 61 * u64::from(l)) as u8)
                            .collect()
                    })
                    .collect()
            })
            .collect(),
    }
}

/// Every DXGI format this crate maps, in every shape and every header that can hold it, and in every alpha mode: DirectXTex
/// reads each file this writer writes as this reader reads it.
#[test]
fn directxtex_reads_every_file_written_here_as_it_is_read_here() {
    let Some(dump) = tool(Tool::DirectxtexDump) else {
        return;
    };
    let shapes = [
        Shape::D1 { layers: 1 },
        Shape::D1 { layers: 3 },
        Shape::D2 { layers: 1 },
        Shape::D2 { layers: 4 },
        Shape::Cube { cubes: 1 },
        Shape::Cube { cubes: 3 },
        Shape::D3 { depth: 5 },
        Shape::D3 { depth: 1 },
    ];
    let alphas = [
        AlphaMode::Unknown,
        AlphaMode::Straight,
        AlphaMode::Premultiplied,
        AlphaMode::Opaque,
        AlphaMode::Custom,
    ];
    let mut cases = Vec::new();
    for dxgi in 0..=255 {
        let Ok((format, color_space)) = from_dxgi(dxgi) else {
            continue;
        };
        for shape in shapes {
            for alpha in alphas {
                // the alpha modes on the single 2D image only: the other shapes carry them the same way
                if alpha != AlphaMode::Unknown && shape != (Shape::D2 { layers: 1 }) {
                    continue;
                }
                let d = resource(shape, format, color_space, alpha);
                for header in [WriteHeader::Auto, WriteHeader::Legacy, WriteHeader::Dx10] {
                    // the legacy header cannot hold every texture, and the writer refuses those it cannot
                    let Ok(bytes) = write(&d, header) else { continue };
                    let p = parse(&bytes).unwrap();
                    cases.push((
                        format!("{format:?}/{color_space:?} {shape:?} {alpha:?} {header:?}"),
                        p.dds,
                        bytes,
                    ));
                }
            }
        }
    }
    let answers = directxtex(
        &dump,
        &cases
            .iter()
            .map(|(_, _, b)| ("read", b.clone()))
            .collect::<Vec<_>>(),
    );
    let mut gate = GateReport::new("files written here, read by DirectXTex", Tolerance::Exact);
    for ((name, d, _), answer) in cases.iter().zip(&answers) {
        match agrees(name, d, answer) {
            Ok(()) => gate.record(name, 0.0),
            Err(e) => gate.fail(name, e),
        }
    }
    eprintln!("{gate}");
    gate.assert_passed();
}

/// A DX10 cube whose arraySize counts faces (FromSoftware's environment cubes): DirectXTex refuses it but for its
/// permissive load, which reads it as this reader does.
#[test]
fn face_counted_cubes_read_as_directxtex_reads_them_permissively() {
    let Some(dump) = tool(Tool::DirectxtexDump) else {
        return;
    };
    let mut files = Vec::new();
    let mut expected = Vec::new();
    for (format, cubes) in [(TexFormat::Rgba8Unorm, 1), (TexFormat::Bc1, 2)] {
        let d = resource(
            Shape::Cube { cubes },
            format,
            ColorSpace::Linear,
            AlphaMode::Unknown,
        );
        let mut bytes = write(&d, WriteHeader::Dx10).unwrap();
        bytes[140..144].copy_from_slice(&(cubes * 6).to_le_bytes());
        let p = parse(&bytes).unwrap();
        assert!(p.faces_counted);
        assert_eq!(p.dds, d);
        files.push(("read", bytes.clone()));
        files.push(("read-permissive", bytes));
        expected.push(d);
    }
    let answers = directxtex(&dump, &files);
    for (i, d) in expected.iter().enumerate() {
        let strict = answers[2 * i].meta["hr"].as_str().unwrap();
        assert_ne!(
            strict, "0x00000000",
            "cube {i}: DirectXTex's strict read accepted a face-counted cube"
        );
        agrees(&format!("cube {i} permissive"), d, &answers[2 * i + 1]).unwrap();
    }
}
