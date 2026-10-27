//! Oracle: the game's independently stored prefetch and streamed mip copies.
//! BCn output is also expanded through the codec so every encoded level is exercised.
mod inputs;

use primase_core::{ColorSpace, TexRole, Texture};
use primase_shred::read::texture::{self, Error};
use inputs::{Depth, Game, Query, depth, expect_found, install, seed};
use primase_texcodec::{Decoder, TexDecoder};
use rme_formats::{
    pak::Pak,
    texture::{Payload, Residency, TextureCatalog},
};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = entry.expect("directory entry").path();
        if p.is_dir() {
            archives(&p, out);
        } else if p.extension().is_some_and(|s| s.eq_ignore_ascii_case("pak")) {
            out.push(p);
        }
    }
}
fn expand(images: &[Texture]) {
    for face in images {
        for (mip, bytes) in face.levels.iter().enumerate() {
            let w = (face.width >> mip).max(1);
            let h = (face.height >> mip).max(1);
            let pixels = Decoder::default()
                .decode_rgba8(face.format, w, h, bytes)
                .expect("supported mip expansion");
            assert_eq!(pixels.len(), w as usize * h as usize * 4);
        }
    }
}

#[test]
fn texture_pairs_and_independently_stored_mip_tails() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", paths.len());
    let depth = depth();
    let seed = seed();
    if depth == Depth::Smoke {
        let n = paths.len();
        paths.rotate_left(seed as usize % n);
        paths.truncate(8);
    }
    let (
        mut headers,
        mut decoded,
        mut volumes,
        mut missing,
        mut comparisons,
        mut text_non_cube,
        mut cube_aliases,
    ) = (0, 0, 0, 0, 0, 0, 0);
    let mut raw_non_cube = 0;
    for path in &paths {
        let pak = Pak::open(path).unwrap();
        let label = path.to_str().unwrap();
        let c = TextureCatalog::new(&[(label, &pak)]).unwrap_or_else(|e| panic!("{label} seed={seed}: {e}"));
        for (i, r) in c.records().iter().enumerate() {
            headers += 1;
            // The documented 9,342 non-cube pairs include 408 volumes. Their
            // independently stored bytes can be checked without inventing 3D swizzle.
            if r.residency == Residency::Prefetch && r.header.gcm.cubemap == 0 {
                for full in c
                    .records()
                    .iter()
                    .filter(|other| other.path == r.path && other.residency == Residency::Streamed)
                {
                    if let (Some(pre), Some(streamed)) = (r.pixels, full.pixels) {
                        let pre = pak.payload(pre.entry).unwrap();
                        let streamed = pak.payload(streamed.entry).unwrap();
                        assert!(
                            streamed.ends_with(&pre),
                            "raw mip tail {label} {:?}",
                            String::from_utf8_lossy(&r.path)
                        );
                        if pak.sections()[r.section.unwrap()].name == b"TEXTURES_PREFETCH" {
                            raw_non_cube += 1;
                        }
                    }
                }
            }
            if r.pixels.is_none() {
                missing += 1;
                continue;
            }
            let images = match texture::decode(&c, i, Payload::Primary, ColorSpace::Linear, TexRole::Color) {
                Ok(x) => x,
                Err(Error::Gcm(primase_texcodec::gcm::Error::Dimension { dimension: 3, .. })) => {
                    volumes += 1;
                    continue;
                }
                Err(e) => panic!(
                    "{label} {:?} {:?} seed={seed}: {e}",
                    r.section,
                    String::from_utf8_lossy(&r.path)
                ),
            };
            decoded += 1;
            expand(&images);
            if r.residency == Residency::Prefetch {
                for (full_index, full) in c
                    .records()
                    .iter()
                    .enumerate()
                    .filter(|(_, other)| other.path == r.path && other.residency == Residency::Streamed)
                {
                    let full_images = texture::decode(
                        &c,
                        full_index,
                        Payload::Primary,
                        ColorSpace::Linear,
                        TexRole::Color,
                    )
                    .unwrap();
                    let first = r.header.tail_first_mip() as usize;
                    assert_eq!(images.len(), full_images.len());
                    for (tail, full) in images.iter().zip(&full_images) {
                        assert_eq!(
                            (tail.width, tail.height),
                            ((full.width >> first).max(1), (full.height >> first).max(1))
                        );
                        assert_eq!(tail.format, full.format);
                        assert_eq!(
                            tail.levels,
                            full.levels[first..],
                            "{label} {:?}",
                            String::from_utf8_lossy(&r.path)
                        );
                    }
                    comparisons += 1;
                    let section = &pak.sections()[r.section.unwrap()].name;
                    if section == b"TEXTURES_PREFETCH" && r.header.gcm.cubemap == 0 {
                        text_non_cube += 1;
                    }
                    if full.cube_tail.is_some() {
                        let alias = texture::decode(
                            &c,
                            full_index,
                            Payload::CubeTail,
                            ColorSpace::Linear,
                            TexRole::Color,
                        )
                        .unwrap();
                        assert_eq!(alias, images, "cube alias {label}");
                        cube_aliases += 1;
                    }
                }
            }
        }
    }
    eprintln!(
        "SHRED textures {depth:?} seed={seed}: PAKs={} headers={headers} decoded={decoded} volumes={volumes} header_only={missing} tail_comparisons={comparisons} TEXTURES_non_cube={text_non_cube} cube_aliases={cube_aliases}",
        paths.len()
    );
    eprintln!("TEXTURES non-cube raw tail comparisons (including volumes): {raw_non_cube}");
    if depth == Depth::Full {
        assert_eq!(headers, 27446);
        assert_eq!(volumes, 818);
        assert_eq!(decoded, 26628);
        assert_eq!(missing, 0);
        assert_eq!(comparisons, 12273);
        assert_eq!(decoded + volumes + missing, headers);
        assert_eq!(text_non_cube, 8934);
        assert_eq!(raw_non_cube, 9342);
        assert_eq!(cube_aliases, 600);
    }
}
