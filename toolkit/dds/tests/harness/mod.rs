//! directxtex-dump (`tools.directxtex_dump`, primase-oracles' dumps/directxtex): what DirectXTex's own library reads from
//! a DDS file, and whether it is what this crate reads.
use crate::inputs::own_scratch;
use primase_dds::{Dds, Shape, to_dxgi};
use serde_json::Value;
use std::path::Path;

/// DirectXTex's `TexMetadata` for what was read: arraySize counts cube faces, depth is 1 but for volumes.
pub fn metadata(d: &Dds) -> [u64; 9] {
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

/// The `TexMetadata` fields of [`metadata`], as `meta.json` names them.
pub const FIELDS: [&str; 9] = [
    "width",
    "height",
    "depth",
    "arraySize",
    "mipLevels",
    "miscFlags",
    "miscFlags2",
    "format",
    "dimension",
];

/// The images in DirectXTex's order (file order): item by item with their mips, or mip by mip with their slices.
pub fn images(d: &Dds) -> Vec<&[u8]> {
    match d.shape {
        Shape::D3 { .. } => d.levels.iter().flatten().map(Vec::as_slice).collect(),
        _ => (0..d.levels[0].len())
            .flat_map(|item| d.levels.iter().map(move |level| level[item].as_slice()))
            .collect(),
    }
}

/// One answer of directxtex-dump: its `meta.json` and its `images.bin`.
pub struct Answer {
    pub meta: Value,
    pub images: Vec<u8>,
}

impl Answer {
    /// Whether DirectXTex read the file (`hr` 0).
    pub fn read(&self) -> bool {
        self.meta["hr"] == "0x00000000"
    }
}

/// DirectXTex's answers for `files` (each a command, `read` or `read-permissive`, and a file's bytes), from one batch
/// run in a scratch folder of this call's own, removed when it returns.
pub fn directxtex(dump: &Path, files: &[(&str, &[u8])]) -> Vec<Answer> {
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
                // a refusal writes no images
                images: std::fs::read(folder.join("images.bin")).unwrap_or_default(),
            }
        })
        .collect()
}

/// Whether DirectXTex read `d` from the file: the same metadata and the same bytes for every image, in its order.
pub fn agrees(d: &Dds, answer: &Answer) -> Result<(), String> {
    if !answer.read() {
        return Err(format!("DirectXTex refused it ({})", answer.meta["hr"]));
    }
    let theirs = FIELDS.map(|k| answer.meta[k].as_u64().unwrap_or(u64::MAX));
    if metadata(d) != theirs {
        return Err(format!("metadata {:?}, DirectXTex {theirs:?}", metadata(d)));
    }
    let ours = images(d).concat();
    if ours != answer.images {
        return Err(format!(
            "images differ ({} and {} bytes)",
            ours.len(),
            answer.images.len()
        ));
    }
    Ok(())
}
