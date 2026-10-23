//! skate.'s animation codec read from the game's data (`installs.skate/Data`, `tools.oodle9`): the ECLP resources of
//! three clips of `dingolevel_mpr` decode with the track counts, frame ranges and unit quaternions their
//! `EclipseAnimationAsset`s state, and, with `oracles.scripts.skate/eclipse`, to the values our earlier Python
//! decoder (written from the executable's encoder) gives for the same bytes: frames exactly, values within 1e-6. That
//! reference is a port check, not an independent oracle.

#![allow(clippy::unwrap_used)]

use primase_compress::oodle::{Oodle, Version};
use primase_frostbite::bundle;
use primase_frostbite::ebx::Value;
use primase_frostbite::folder::{Folder, Source};
use primase_reskate::game::eclipse::{Track, decode};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

type R = Result<(), Box<dyn std::error::Error>>;

const TOC: &str = "Win32/levels/game/dingolevel_mpr/dingolevel_mpr.toc";
const CLIPS: [&str; 3] = [
    "c_proto_ofbnb_idle_lfoot",
    "c_proto_onb_inair_bs_grab_method_loop",
    "c_proto_onb_trick_o_varialheel_high",
];
const TOLERANCE: f64 = 1e-6;

fn uint(o: &primase_frostbite::ebx::Object, field: &str) -> Result<usize, String> {
    match o.field(field) {
        Some(Value::UInt(u)) => usize::try_from(*u).map_err(|e| e.to_string()),
        other => Err(format!("{field} is {other:?}")),
    }
}

/// Largest difference from the reference's tracks of one kind, with frames compared exactly.
fn compare<T, const N: usize>(
    name: &str,
    ours: &[Track<T>],
    reference: &Json,
    flatten: impl Fn(&T) -> [f32; N],
) -> f64 {
    let reference = reference.as_array().unwrap();
    assert_eq!(ours.len(), reference.len(), "{name}: track count");
    let mut worst = 0f64;
    for (i, (t, r)) in ours.iter().zip(reference).enumerate() {
        let frames: Vec<u64> = r["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_u64().unwrap())
            .collect();
        assert_eq!(
            t.frames.iter().map(|&f| u64::from(f)).collect::<Vec<_>>(),
            frames,
            "{name} {i} frames"
        );
        let values = r["values"].as_array().unwrap();
        assert_eq!(t.values.len(), values.len(), "{name} {i} key count");
        for (k, (v, rv)) in t.values.iter().zip(values).enumerate() {
            let rv: Vec<f64> = match rv.as_array() {
                Some(a) => a.iter().map(|x| x.as_f64().unwrap()).collect(),
                None => vec![rv.as_f64().unwrap()],
            };
            assert_eq!(rv.len(), N, "{name} {i} key {k} width");
            for (a, b) in flatten(v).iter().zip(&rv) {
                let d = (f64::from(*a) - b).abs();
                assert!(d <= TOLERANCE, "{name} {i} key {k}: {a} against {b}");
                worst = worst.max(d);
            }
        }
    }
    worst
}

#[test]
fn the_clips_decode_as_their_assets_state() -> R {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Ok(());
    };
    let reference = oracle(OracleSet::Scripts, Game::Skate, "eclipse");
    let folder = Folder::open(data)?;
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let toc = folder.toc(TOC)?;
    for clip in CLIPS {
        let (bundle_name, doc) = src.ebx(TOC, &format!("animation/dingo/{clip}"))?;
        // the clip's EclipseAnimationAsset: the instance with a Resource (instance 1 in two of the three)
        let asset = doc
            .instances
            .iter()
            .filter_map(|i| i.object.as_ref())
            .find(|o| o.field("Resource").is_some())
            .ok_or("no instance has a Resource")?;
        let Some(Value::Resource(rid)) = asset.field("Resource") else {
            return Err("Resource is not a resource id".into());
        };
        let region = &toc
            .bundles
            .iter()
            .find(|(b, _)| *b == bundle_name)
            .ok_or("bundle")?
            .1;
        let (files, inline) = bundle::read_region(region)?;
        let (m, first, _) = src.manifest(&files, &inline)?.ok_or("manifest")?;
        let (i, a) = m
            .res
            .iter()
            .enumerate()
            .find(|(_, a)| a.rid == *rid)
            .ok_or("no resource has that id")?;
        assert_eq!(a.rtype, 0x4088_bf7e, "{clip}: resource type");
        let bytes = src.payload(files[first + m.ebx.len() + i], a.size as usize)?;

        let e = decode(&bytes)?;
        let (end, q, v, f) = (
            uint(asset, "EndFrame")?,
            uint(asset, "QuaternionChannelSize")?,
            uint(asset, "VectorChannelSize")?,
            uint(asset, "FloatChannelSize")?,
        );
        assert_eq!(
            (e.rotations.len(), e.translations.len(), e.scalars.len()),
            (q, v, f),
            "{clip}: track counts"
        );
        let frames = e
            .rotations
            .iter()
            .map(|t| &t.frames)
            .chain(e.translations.iter().map(|t| &t.frames))
            .chain(e.scalars.iter().map(|t| &t.frames));
        for t in frames {
            assert!(
                t.iter().all(|&k| k as usize <= end),
                "{clip}: a key past EndFrame {end}"
            );
        }
        for t in &e.rotations {
            for q in &t.values {
                let norm: f64 = q.iter().map(|&c| f64::from(c) * f64::from(c)).sum();
                assert!((norm - 1.0).abs() < 1e-4, "{clip}: squared norm {norm}");
            }
        }
        println!(
            "{clip}: {q} rotations, {v} translations, {f} scalars, formats {:?}, EndFrame {end}",
            e.formats
        );

        let Some(dir) = &reference else { continue };
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let hashes = std::fs::read_to_string(dir.join("sha256.txt"))?;
        assert!(
            hashes
                .lines()
                .any(|l| l.starts_with(&digest) && l.ends_with(&format!("{clip}.eclp"))),
            "{clip}: the bytes are not the reference's ({digest})"
        );
        let r: Json = serde_json::from_slice(&std::fs::read(dir.join(format!("{clip}.tracks.json")))?)?;
        let formats = &r["formats"];
        assert_eq!(
            [
                e.formats.quaternion,
                e.formats.vector,
                e.formats.scalar,
                e.formats.time
            ]
            .map(u64::from),
            ["quaternion", "vector", "scalar", "time"].map(|k| formats[k].as_u64().unwrap()),
            "{clip}: formats"
        );
        let worst = [
            compare("rotations", &e.rotations, &r["rotations"], |q| *q),
            compare("translations", &e.translations, &r["translations"], |t| *t),
            compare("scalars", &e.scalars, &r["scalars"], |s| [*s]),
        ];
        println!(
            "{clip}: largest difference from the reference: rotations {:e}, translations {:e}, scalars {:e}",
            worst[0], worst[1], worst[2]
        );
    }
    Ok(())
}
