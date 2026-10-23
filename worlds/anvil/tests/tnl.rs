//! Every terrain node of Riders Republic (build 25394987, DataPC_MAD_USNAT) decoded, and the decoding measured against
//! what it doesn't use: each B's length against the raw bits read, each raster's values against its palette and (block
//! 2) the node's mask, each node's decoded height range against its stored a and b, and the 4 samples each node shares
//! with its +x and +y neighbours. Ignored: it decodes 611,667 blocks; run by hand.
use std::collections::{BTreeMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::tnl::{StoredNodeLayer, StoredTnlChunk, TERRAIN_NODE_LAYER, node};
use primase_anvil::read::tnl_code::{SIDE, heights, hilbert, raster, tile};
use primase_testdata::{Game, install};

/// A strip's digest: the samples at x in `xs` and y in `ys`.
fn strip<T: Hash + Copy>(samples: &[T], xs: std::ops::Range<usize>, ys: std::ops::Range<usize>) -> u64 {
    let mut h = DefaultHasher::new();
    for y in ys {
        for x in xs.clone() {
            samples[y * SIDE + x].hash(&mut h);
        }
    }
    h.finish()
}

/// A tile's four strips: the first and last 4 columns, the first and last 4 rows.
fn strips<T: Hash + Copy>(samples: &[T]) -> [u64; 4] {
    [
        strip(samples, 0..4, 0..SIDE),
        strip(samples, SIDE - 4..SIDE, 0..SIDE),
        strip(samples, 0..SIDE, 0..4),
        strip(samples, 0..SIDE, SIDE - 4..SIDE),
    ]
}

#[test]
#[ignore = "decodes every terrain node of the game; run by hand"]
fn every_terrain_node_decodes_and_agrees() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&[dir.join("DataPC_MAD_USNAT.forge")]).expect("archive");
    let names = &archive.forge(0).expect("forge").names;
    let entry = names
        .iter()
        .position(|n| n.type_id == TERRAIN_NODE_LAYER)
        .expect("a TerrainNodeLayer");
    let layer = StoredNodeLayer::parse(&archive.resource(At { forge: 0, entry }).expect("layer").body)
        .expect("layer");
    let n = layer.items.len();
    let positions: HashSet<u32> = layer.items.iter().map(|i| i.position).collect();
    assert_eq!(positions.len(), n, "positions distinct");
    assert!(positions.iter().all(|&p| (p as usize) < n), "positions 0..n");
    let order = hilbert();
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut count = |what: &str| *counts.entry(what.to_owned()).or_default() += 1;
    let mut failures: Vec<String> = Vec::new();
    let mut worst_quanta = 0.0f64;
    // per node: for slot 0 and 2..=7, the four strips
    let mut edges: BTreeMap<(u32, u32, u32), [[u64; 4]; 7]> = BTreeMap::new();
    for item in &layer.items {
        let (level, row, col) = node(item.position).expect("node");
        let Some(e) = archive.find(item.chunk) else {
            failures.push(format!(
                "node {}: chunk {:#x} in no forge",
                item.position, item.chunk
            ));
            continue;
        };
        let chunk = match archive
            .resource(e)
            .map_err(|e| e.to_string())
            .and_then(|r| StoredTnlChunk::parse(&r.body).map_err(|e| e.to_string()))
        {
            Ok(c) => c,
            Err(err) => {
                failures.push(format!("node {}: {err}", item.position));
                continue;
            }
        };
        count(&format!(
            "level {level}, tag {:#06x}, tenth block {}",
            chunk.tag,
            chunk.tenth.is_some()
        ));
        let mut digests = [[0u64; 4]; 7];
        // heights
        match heights(&chunk.blocks[0]) {
            Ok(h) => {
                if h.b_bytes as u64 != h.raw_bits.div_ceil(8) + 17 {
                    failures.push(format!(
                        "node {} heights: B {} bytes, {} raw bits",
                        item.position, h.b_bytes, h.raw_bits
                    ));
                }
                let q = 2f64.powi(8 - level as i32 - 20);
                let (lo, hi) = (
                    h.values.iter().min().copied().unwrap_or(0),
                    h.values.iter().max().copied().unwrap_or(0),
                );
                let d = ((f64::from(chunk.a) - lo as f64 * q).abs())
                    .max((f64::from(chunk.b) - hi as f64 * q).abs())
                    / q;
                worst_quanta = worst_quanta.max(d);
                if d > 1.0 {
                    failures.push(format!(
                        "node {} heights: range off its a, b by {d} quanta",
                        item.position
                    ));
                }
                digests[0] = strips(&h.values);
                count("height blocks decoded");
            }
            Err(err) => failures.push(format!("node {} heights: {err}", item.position)),
        }
        // rasters
        for slot in 2..=7 {
            match raster(&chunk.blocks[slot], &order) {
                Ok(r) => {
                    if r.b_bytes as u64 != r.raw_bits.div_ceil(8) + 17 {
                        failures.push(format!(
                            "node {} slot {slot}: B {} bytes, {} raw bits",
                            item.position, r.b_bytes, r.raw_bits
                        ));
                    }
                    let values: HashSet<u8> = r.samples.iter().copied().collect();
                    let palette: HashSet<u8> = r.palette.iter().copied().collect();
                    if values != palette {
                        failures.push(format!(
                            "node {} slot {slot}: values not its palette",
                            item.position
                        ));
                    }
                    if slot == 2 && (0..=255u8).any(|v| chunk.in_mask(v) != values.contains(&v)) {
                        failures.push(format!(
                            "node {} slot 2: values not the node's mask",
                            item.position
                        ));
                    }
                    if r.chunk == 32_768 && r.palette.len() == 1 {
                        count("raster blocks constant in one chunk of 32,768");
                    }
                    digests[slot - 1] = strips(&r.samples);
                    count("raster blocks decoded");
                }
                Err(err) => failures.push(format!("node {} slot {slot}: {err}", item.position)),
            }
        }
        // the LZ4 tiles
        for (slot, bc4) in [(1, false), (8, true)] {
            match tile(&chunk.blocks[slot], bc4) {
                Ok(t) if t.len() == 8_712 => count("LZ4 tiles decoded"),
                Ok(t) => failures.push(format!("node {} slot {slot}: {} bytes", item.position, t.len())),
                Err(err) => failures.push(format!("node {} slot {slot}: {err}", item.position)),
            }
        }
        edges.insert((level, row, col), digests);
    }
    // the shared strips: a node's last 4 columns are its +x neighbour's first, its last 4 rows its +y neighbour's
    let mut pairs = 0u64;
    let mut differing = [0u64; 7];
    for (&(level, row, col), d) in &edges {
        for (other, mine, theirs) in [((level, row, col + 1), 1, 0), ((level, row + 1, col), 3, 2)] {
            if let Some(o) = edges.get(&other) {
                pairs += 1;
                for slot in 0..7 {
                    differing[slot] += u64::from(d[slot][mine] != o[slot][theirs]);
                }
            }
        }
    }
    for (what, n) in &counts {
        println!("{n:>8}  {what}");
    }
    println!(
        "{pairs:>8}  same-level +x / +y neighbour pairs; strips differing (heights, slots 2-7): {differing:?}"
    );
    println!("worst height range off a, b: {worst_quanta} quanta");
    for f in failures.iter().take(20) {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
    assert_eq!(differing, [0; 7]);
}
