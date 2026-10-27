#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! Every animation of every clip dictionary (`.ycd`) of the install decoded to its channel values
//! ([`Animation::decode`]), and against an independent reader (CodeWalker, `oracles.codewalker.gta/ycd`, a folder
//! holding `dictionaries.jsonl`: per dictionary the JSON of the harness's `DumpYcd`). Per animation: its frame count,
//! bone count and the frame count of each of its sequences, then three SHA-256 over all its tracks in order:
//! - the channel streams: per track (a single `0xFF` when it has no channels) per channel its type byte then its values
//!   as little-endian floats (a static float, vector or quaternion's own components, otherwise a value a frame; a cached
//!   quaternion's frame values the root of what its track's first three channels leave of 1);
//! - the evaluated vectors of the tracks without a cached quaternion (four floats a frame);
//! - the same of the tracks with one, which CodeWalker normalises as it evaluates: reported, not required to match bit
//!   for bit (a dictionary dumped in full, `YCD_FULL=1`, also gives every vector, compared within 1e-6).
//!
//! The decoder is held to the file (see `rage::ycd_channels`): an animation that fails is counted by the error's
//! message, and the test fails on any. The counts are printed.
//!
//! Needs the GTA install and its key material; the comparison also the oracle.
mod inputs;

use std::collections::{BTreeMap, HashMap};

use inputs::{Game, OracleSet, install, keys, oracle};
use rage_engine_formats::archive::archive_set::Install;
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::ycd::{Animation, ClipDictionary};
use rage_engine_formats::ycd_channels::{Block, Channel, ChannelKind, Track, root_of_rest};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

fn push(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A track's channel stream (see the module documentation) over `frames` frames.
fn channel_stream(track: &Track, frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for c in &track.channels {
        out.push(c.kind().code());
        match c {
            Channel::StaticFloat(v) => push(&mut out, *v),
            Channel::StaticVector3(v) => v.iter().for_each(|x| push(&mut out, *x)),
            Channel::StaticQuaternion(v) => {
                v.iter().for_each(|x| push(&mut out, *x));
                push(&mut out, root_of_rest(*v));
            }
            Channel::Frames { values, .. } => values.iter().for_each(|x| push(&mut out, *x)),
            Channel::Cached { .. } => {
                for f in 0..frames {
                    let s = |i: usize| match &track.channels[i] {
                        Channel::StaticFloat(v) => *v,
                        Channel::Frames { values, .. } => values[f],
                        other => panic!("a cached quaternion over {other:?}"),
                    };
                    push(&mut out, root_of_rest([s(0), s(1), s(2)]));
                }
            }
        }
    }
    out
}

fn vectors(track: &Track, frames: usize) -> Vec<f32> {
    (0..frames).flat_map(|f| track.sample(f).unwrap()).collect()
}

fn has_cached(track: &Track) -> bool {
    track.channels.iter().any(|c| {
        matches!(
            c,
            Channel::Cached {
                kind: ChannelKind::CachedQuaternion1,
                ..
            }
        )
    })
}

#[derive(Default)]
struct Counts {
    dictionaries: usize,
    animations: usize,
    blocks: usize,
    tracks: usize,
    channels: BTreeMap<u8, usize>,
    compared: usize,
    oracle_failed: BTreeMap<String, usize>,
    problems: Vec<String>,
    channel_streams: usize,
    plain_vectors: usize,
    cached: (usize, usize),
    cached_within_tolerance: (usize, usize),
}

/// Compares one animation with the oracle's; what differs goes into `n.problems`.
fn compare(path: &str, hash: u32, blocks: &[Block], a: &Animation, o: &Value, n: &mut Counts) {
    let who = format!("{path} animation {hash:#010x}");
    let ob = o["blocks"].as_array().unwrap();
    if o["frames"] != a.frames || o["bones"] != a.bone_ids.len() || ob.len() != blocks.len() {
        n.problems
            .push(format!("{who}: frames, bones or sequences differ"));
        return;
    }
    let (mut channel, mut plain, mut cached) = (Sha256::new(), Sha256::new(), Sha256::new());
    let mut any_cached = false;
    for (b, (block, ob)) in blocks.iter().zip(ob).enumerate() {
        let tracks = ob["tracks"].as_u64().unwrap() as usize;
        if ob["frames"] != block.frames || tracks > block.tracks.len() {
            n.problems
                .push(format!("{who}: sequence {b}'s frames or tracks differ"));
            return;
        }
        // the oracle sizes its tracks by the last one that holds a channel
        if block.tracks.iter().skip(tracks).any(Option::is_some) {
            n.problems.push(format!(
                "{who}: sequence {b} has channels past the oracle's {tracks} tracks"
            ));
            return;
        }
        for track in block.tracks.iter().take(tracks) {
            let Some(track) = track else {
                channel.update([0xFFu8]);
                continue;
            };
            channel.update(channel_stream(track, block.frames));
            let v: Vec<u8> = vectors(track, block.frames)
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect();
            if has_cached(track) {
                any_cached = true;
                cached.update(v);
            } else {
                plain.update(v);
            }
            n.tracks += 1;
        }
    }
    let same = |mine: Sha256, key: &str| hex(&mine.finalize()) == o[key].as_str().unwrap();
    if same(channel, "sha") {
        n.channel_streams += 1;
    } else {
        n.problems.push(format!("{who}: channel streams differ"));
        localize(&who, blocks, ob, n);
    }
    if same(plain, "vec_sha") {
        n.plain_vectors += 1;
    } else {
        n.problems
            .push(format!("{who}: evaluated vectors of its plain tracks differ"));
    }
    if any_cached {
        n.cached.0 += 1;
        if same(cached, "vec_cached_sha") {
            n.cached.1 += 1;
        }
        // within a tolerance, where the dictionary was dumped in full
        for (block, ob) in blocks.iter().zip(ob) {
            let Some(detail) = ob["detail"].as_array() else {
                continue;
            };
            for (track, o) in block.tracks.iter().zip(detail) {
                let Some(track) = track.as_ref().filter(|t| has_cached(t)) else {
                    continue;
                };
                let theirs: Vec<f32> = o["vec"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap() as f32)
                    .collect();
                let worst = vectors(track, block.frames)
                    .iter()
                    .zip(&theirs)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0f32, f32::max);
                n.cached_within_tolerance.0 += 1;
                if worst <= 1e-6 {
                    n.cached_within_tolerance.1 += 1;
                } else {
                    n.problems
                        .push(format!("{who}: a cached-quaternion track differs by {worst}"));
                }
            }
        }
    }
}

/// Which tracks of a dictionary dumped in full differ in their channel streams.
fn localize(who: &str, blocks: &[Block], ob: &[Value], n: &mut Counts) {
    for (b, (block, ob)) in blocks.iter().zip(ob).enumerate() {
        let Some(detail) = ob["detail"].as_array() else {
            return;
        };
        for (t, (track, o)) in block.tracks.iter().zip(detail).enumerate() {
            match track {
                None if !o.is_null() => n.problems.push(format!(
                    "{who}: sequence {b} track {t} has no channels, the oracle's has"
                )),
                Some(_) if o.is_null() => n.problems.push(format!(
                    "{who}: sequence {b} track {t} has channels, the oracle's has none"
                )),
                Some(track) => {
                    let types: Vec<u64> = track
                        .channels
                        .iter()
                        .map(|c| u64::from(c.kind().code()))
                        .collect();
                    let theirs: Vec<u64> = o["types"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_u64().unwrap_or(99))
                        .collect();
                    if types != theirs {
                        n.problems.push(format!(
                            "{who}: sequence {b} track {t} channels {types:?}, the oracle's {theirs:?}"
                        ));
                    } else if hex(&Sha256::digest(channel_stream(track, block.frames)))
                        != o["sha"].as_str().unwrap()
                    {
                        n.problems
                            .push(format!("{who}: sequence {b} track {t} ({types:?}) differs"));
                    }
                }
                None => {}
            }
        }
    }
}

#[test]
fn every_animation_decodes() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let keys = Keys::from_dir(&keys, None).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let mut oracle_lines: HashMap<String, Value> = HashMap::new();
    if let Some(folder) = oracle(OracleSet::CodeWalker, Game::Gta, "ycd") {
        for line in std::fs::read_to_string(folder.join("dictionaries.jsonl"))
            .unwrap()
            .lines()
        {
            let v: Value = serde_json::from_str(line).unwrap();
            oracle_lines.insert(v["path"].as_str().unwrap().to_lowercase(), v);
        }
    }
    let mut n = Counts::default();
    let mut failures: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in install.listing().filter(|f| f.extension == "ycd") {
        let dictionary = install
            .resource(f.file, &keys)
            .map_err(|e| e.to_string())
            .and_then(|r| ClipDictionary::read_resource(&r).map_err(|e| e.to_string()))
            .unwrap();
        n.dictionaries += 1;
        let o = oracle_lines.get(f.path);
        for (hash, a) in &dictionary.animations {
            n.animations += 1;
            let blocks = match a.decode() {
                Ok(b) => b,
                Err(e) => {
                    failures
                        .entry(e.to_string())
                        .or_default()
                        .push(format!("{} {hash:#010x}", f.path));
                    continue;
                }
            };
            n.blocks += blocks.len();
            for block in &blocks {
                for c in block.tracks.iter().flatten().flat_map(|t| &t.channels) {
                    *n.channels.entry(c.kind().code()).or_default() += 1;
                }
            }
            let Some(o) = o else { continue };
            let oa = o["value"]["animations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["hash"].as_u64() == Some(u64::from(*hash)))
                .unwrap();
            if let Some(error) = oa.get("error") {
                *n.oracle_failed
                    .entry(error.as_str().unwrap().to_owned())
                    .or_default() += 1;
                continue;
            }
            n.compared += 1;
            compare(f.path, *hash, &blocks, a, oa, &mut n);
        }
    }
    println!(
        "{} dictionaries, {} animations, {} sequences; channels by type {:?}",
        n.dictionaries, n.animations, n.blocks, n.channels
    );
    println!(
        "{} animations compared with the oracle, {} tracks: channel streams equal {}, plain vectors equal {}; with a cached quaternion {} animations, {} equal bit for bit, {}/{} tracks equal within 1e-6 of those dumped in full; {} problems",
        n.compared,
        n.tracks,
        n.channel_streams,
        n.plain_vectors,
        n.cached.0,
        n.cached.1,
        n.cached_within_tolerance.1,
        n.cached_within_tolerance.0,
        n.problems.len()
    );
    for (message, count) in &n.oracle_failed {
        println!("the oracle fails on {count} animations: {message}");
    }
    for m in n.problems.iter().take(20) {
        println!("  {m}");
    }
    for (message, files) in &failures {
        println!("{} animations: {message}, e.g. {}", files.len(), files[0]);
    }
    assert!(failures.is_empty(), "{} kinds of failure", failures.len());
    assert!(n.problems.is_empty());
}
