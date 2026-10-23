//! Texture gate against an independent reader's decoded images (`oracles.unitypy.sledding/textures`: a folder with
//! `index.json`, `textures` each `file`, `path_id`, `name`, `format`, `width`, `height`, `mips`, `color_space`, `usage`
//! and `png`, its level 0 as an RGBA PNG with rows top first), for the player `installs.sledding/Sledding Game_Data`
//! read with `tools.unity_tpk`.
//!
//! Every texture's size, level count and colour space; level 0 decoded to RGBA8 against the image, per TextureFormat:
//! uncompressed and BC7 exactly; DXT1 and DXT5 within 1, named "oracle decoder's BC1-3 palette rounding": the
//! interpolated palette entries are rounded otherwise by the oracle's decoder, while ours and bcdec (decoded as well)
//! agree bit for bit, which is asserted.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::TexFormat;
use primase_core::gate::{GateReport, Tolerance};
use primase_core::{ColorSpace, Texture};
use primase_image::{Samples, read_png};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use primase_texcodec::{Bcdec, Decoder, TexDecoder};
use primase_unity::TypeTreePackage;
use primase_unity::player::{ObjRef, Player};
use primase_unity::texture::UnityTexture;
use serde_json::Value as Json;

/// Per TextureFormat: textures, textures equal, channels compared and differing, the largest difference and where.
#[derive(Default)]
struct Pixels {
    textures: u64,
    equal: u64,
    channels: u64,
    differ: u64,
    max: u8,
    at: String,
    /// BC1-3 only: channels where bcdec's decode differs from the oracle's, and from ours.
    bcdec_differ: u64,
    bcdec_vs_ours: u64,
}

/// The largest channel difference allowed for a TextureFormat.
fn tolerance(format: i64) -> u8 {
    match format {
        10 | 12 => 1,
        _ => 0,
    }
}

fn rgba8(t: &Texture) -> Vec<u8> {
    Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
        .unwrap()
}

/// What the texture reader leaves undecoded on purpose (its `Unsupported` reasons): textures without pixels, and
/// crunched formats (a second compression the reader does not unpack). Any other decode failure fails the gate.
const NOT_DECODED: [&str; 2] = ["an empty texture", "crunched texture format"];

#[test]
fn textures_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        install_file(Game::Sledding, "Sledding Game_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Sledding, "textures"),
    ) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let index: Json =
        serde_json::from_slice(&std::fs::read(Path::new(&oracle).join("index.json")).unwrap()).unwrap();
    let mut header = GateReport::new("textures: size, levels, colour space", Tolerance::Exact);
    let mut not_read: BTreeMap<String, u64> = BTreeMap::new();
    let mut pixels: BTreeMap<i64, Pixels> = BTreeMap::new();
    for t in index["textures"].as_array().unwrap() {
        let r = ObjRef {
            file: t["file"].as_str().unwrap().to_lowercase(),
            path_id: t["path_id"].as_i64().unwrap(),
        };
        let item = format!("{}/{} {}", r.file, r.path_id, t["name"].as_str().unwrap());
        let u = match UnityTexture::read(&player, &r) {
            Ok(u) => u,
            Err(e) => {
                header.fail(&item, e.to_string());
                continue;
            }
        };
        let want = (
            t["width"].as_u64().unwrap(),
            t["height"].as_u64().unwrap(),
            t["mips"].as_u64().unwrap(),
            t["format"].as_i64().unwrap(),
            t["color_space"].as_i64().unwrap() == 1,
        );
        let got = (
            u64::from(u.width),
            u64::from(u.height),
            u64::from(u.mips),
            u.format,
            u.srgb,
        );
        if got != want {
            header.fail(&item, format!("{got:?} vs {want:?}"));
            continue;
        }
        let ours = match u.texture() {
            Ok(x) => x,
            // the header is right; pixels the reader does not decode on purpose are counted, any other failure fails
            Err(primase_unity::Error::Unsupported(why))
                if NOT_DECODED
                    .iter()
                    .any(|p| why.rsplit(": ").next().unwrap().starts_with(p)) =>
            {
                *not_read
                    .entry(why.rsplit(": ").next().unwrap().to_owned())
                    .or_default() += 1;
                header.record(&item, 0.0);
                continue;
            }
            Err(e) => {
                header.fail(&item, format!("not decoded: {e}"));
                continue;
            }
        };
        if ours.levels.len() as u32 != u.mips
            || (ours.color_space == ColorSpace::Srgb) != (u.srgb && u.normal().is_none())
        {
            header.fail(&item, "levels or colour space".to_owned());
            continue;
        }
        header.record(&item, 0.0);
        let Some(png) = t["png"].as_str() else {
            header.fail(&item, "decoded, but the oracle has no image for it".to_owned());
            continue;
        };
        let image = read_png(&std::fs::read(Path::new(&oracle).join(png)).unwrap())
            .unwrap()
            .image;
        let Samples::U8(theirs) = image.samples else {
            panic!("{png}: not 8-bit");
        };
        let mine = rgba8(&ours);
        let p = pixels.entry(u.format).or_default();
        p.textures += 1;
        assert_eq!(mine.len(), theirs.len(), "{item}: pixel count");
        let mut equal = true;
        for (i, (&a, &b)) in mine.iter().zip(&theirs).enumerate() {
            p.channels += 1;
            let d = a.abs_diff(b);
            if d != 0 {
                p.differ += 1;
                equal = false;
            }
            if d > p.max {
                p.max = d;
                p.at = format!("{item} pixel {} channel {}: {a} vs {b}", i / 4, i % 4);
            }
        }
        if equal {
            p.equal += 1;
        }
        if matches!(ours.format, TexFormat::Bc1 | TexFormat::Bc3) {
            let b = Bcdec
                .decode_rgba8(ours.format, ours.width, ours.height, &ours.levels[0])
                .unwrap();
            p.bcdec_differ += b.iter().zip(&theirs).filter(|(a, b)| a != b).count() as u64;
            p.bcdec_vs_ours += b.iter().zip(&mine).filter(|(a, b)| a != b).count() as u64;
        }
    }
    println!("{header}");
    println!("not decoded: {not_read:?}");
    for (f, p) in &pixels {
        println!(
            "format {f}: {} textures, {} equal; {} channels, {} differ (bcdec from the oracle: {}, from ours: {}), \
             largest difference {} ({})",
            p.textures, p.equal, p.channels, p.differ, p.bcdec_differ, p.bcdec_vs_ours, p.max, p.at
        );
    }
    header.assert_passed();
    // pixels were compared, and only the named cases were left undecoded
    let compared: u64 = pixels.values().map(|p| p.textures).sum();
    assert!(compared > 0, "no texture's pixels compared");
    assert!(
        not_read
            .keys()
            .all(|k| NOT_DECODED.iter().any(|p| k.starts_with(p))),
        "not decoded for another reason: {not_read:?}"
    );
    for (f, p) in &pixels {
        assert!(
            p.max <= tolerance(*f),
            "format {f}: a difference of {} ({})",
            p.max,
            p.at
        );
        assert_eq!(p.bcdec_vs_ours, 0, "format {f}: bcdec and ours disagree");
    }
}
