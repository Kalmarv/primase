//! Each summary the crate registers merges commutatively and associatively (the contract the conversion's forward and
//! reverse check only samples): four parts' summaries, two of them sharing an entity (a dependency both reach), give the
//! same merge in every order (all 24) and in either grouping ((a b)(c d) and a(b(c d))).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fmt::Debug;

use primase_core::summary::Summary;
use primase_exactsum::ExactSum;
use primase_reskate::components::{
    CellsCounted, ChainCandidate, OverlayGrouping, RankedLight, SharedContent, SkateContentBounds,
    SkateFloorSummary, SkateLightsSummary, SkateMapSummary, SkateSpawnSummary, SkateTextureWeights,
};

/// Every order of `parts` merged left to right, and two groupings, equal.
fn merges_alike<S: Summary + Debug>(parts: [S; 4]) {
    let fold = |order: &[usize]| {
        let mut m = parts[order[0]].clone();
        for &i in &order[1..] {
            m.merge(parts[i].clone()).unwrap();
        }
        m
    };
    let first = fold(&[0, 1, 2, 3]);
    let mut orders = 0;
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                for d in 0..4 {
                    let o = [a, b, c, d];
                    if BTreeSet::from(o).len() == 4 {
                        assert_eq!(fold(&o), first, "{o:?}");
                        orders += 1;
                    }
                }
            }
        }
    }
    assert_eq!(orders, 24);
    // (a b)(c d)
    let (mut ab, mut cd) = (parts[0].clone(), parts[2].clone());
    ab.merge(parts[1].clone()).unwrap();
    cd.merge(parts[3].clone()).unwrap();
    ab.merge(cd).unwrap();
    assert_eq!(ab, first);
    // a(b(c d))
    let mut c_d = parts[2].clone();
    c_d.merge(parts[3].clone()).unwrap();
    let mut b_cd = parts[1].clone();
    b_cd.merge(c_d).unwrap();
    let mut a_bcd = parts[0].clone();
    a_bcd.merge(b_cd).unwrap();
    assert_eq!(a_bcd, first);
}

#[test]
fn the_floor_summary() {
    let s = |lows: &[(u64, &[f32])], soups: &[(u64, &[f32])]| SkateFloorSummary {
        lows: lows.iter().map(|(k, v)| (*k, v.to_vec())).collect(),
        soup_lows: soups.iter().map(|(k, v)| (*k, v.to_vec())).collect(),
    };
    // entity 2 reached by two parts; one part has only soups
    merges_alike([
        s(&[(1, &[0.5, -1.0]), (2, &[3.0])], &[]),
        s(&[(2, &[3.0])], &[]),
        s(&[], &[(9, &[-7.0])]),
        s(&[(4, &[1.0])], &[]),
    ]);
    // only soups anywhere: they are kept
    merges_alike([
        s(&[], &[(9, &[-7.0])]),
        s(&[], &[(8, &[2.0])]),
        s(&[], &[(9, &[-7.0])]),
        s(&[], &[]),
    ]);
}

fn light(id: u64, energy: f32) -> RankedLight {
    let mut fields = [0.0; 18];
    fields[0] = id as f32;
    fields[12] = energy;
    RankedLight {
        id,
        default_group: true,
        priority: energy,
        fields,
        spot: false,
        time_of_day: 127,
        cast_shadow: false,
        group: String::new(),
        source: String::new(),
    }
}

#[test]
fn the_lights_summary() {
    let s =
        |ls: &[(u64, f32)]| SkateLightsSummary::of(2, 200.0, ls.iter().map(|&(i, e)| light(i, e)).collect());
    // light 4 in two parts; ties of priority
    merges_alike([
        s(&[(1, 5.0), (4, 6.0)]),
        s(&[(4, 6.0), (3, 4.0)]),
        s(&[(2, 5.0)]),
        s(&[(5, 1.0), (6, 6.0)]),
    ]);
}

#[test]
fn the_texture_weights() {
    let s = |c: &[(&str, u64, f64)]| SkateTextureWeights {
        max: 2,
        aux_weight: 0.25,
        contributions: c
            .iter()
            .map(|&(t, k, a)| {
                let mut x = ExactSum::default();
                x.add(a);
                ((t.to_owned(), k), x)
            })
            .collect(),
    };
    // placement 2 in two parts
    merges_alike([
        s(&[("a", 1, 0.1), ("b", 2, 1e16)]),
        s(&[("b", 2, 1e16), ("a", 3, 0.2)]),
        s(&[("b", 4, -1e16), ("c", 5, 3.0)]),
        s(&[("a", 6, 0.3)]),
    ]);
}

#[test]
fn the_content_bounds_and_the_spawn_summary() {
    let b = |lo: [f64; 2], hi: [f64; 2]| SkateContentBounds { lo, hi };
    merges_alike([
        b([0.0, 0.0], [1.0, 1.0]),
        SkateContentBounds::EMPTY,
        b([-5.0, 2.0], [0.0, 3.0]),
        b([0.0, 0.0], [1.0, 1.0]),
    ]);
    // each part by its smallest root, with its area (or none); a part listed in two summaries alike
    let s = |spawns: &[u64], parts: &[(u32, Option<[f64; 4]>)]| SkateSpawnSummary {
        spawns: spawns.iter().copied().collect(),
        lo: [0.0; 2],
        hi: [1.0; 2],
        parts: parts.iter().copied().collect(),
    };
    let area = Some([0.0, 0.0, 400.0, 400.0]);
    merges_alike([
        s(&[7], &[(3, area)]),
        s(&[7], &[(1, None), (3, area)]),
        s(&[], &[]),
        s(&[8], &[(9, Some([400.0, 0.0, 800.0, 400.0]))]),
    ]);
}

#[test]
fn the_map_summary() {
    let g = OverlayGrouping {
        tile: 200.0,
        min_instances: 3,
        min_instance_tris: 150,
        max_render_edge: 80.0,
        limit: 4000.0,
        offset: [0.0, 0.0],
    };
    // each part's content counted with its collision cells apart, and its own tiles with a chain form
    let s = |n: usize, tiles: &[(&str, i32, i32, usize)]| SkateMapSummary {
        grouping: g,
        cull_ids: 4096,
        summed: n,
        shared: Default::default(),
        water: false,
        cells: CellsCounted::Apart,
        chains: tiles
            .iter()
            .map(|&(name, row, column, objects)| (name.to_owned(), ChainCandidate { row, column, objects }))
            .collect(),
    };
    // shared content by key (a light two parts write, a hull soup), and water in some parts
    let with = |mut m: SkateMapSummary, shared: &[(SharedContent, u64, usize)], water: bool| {
        m.shared = shared.iter().map(|&(kind, key, n)| ((kind, key), n)).collect();
        m.water = water;
        m
    };
    let light = (SharedContent::Light, 7, 1);
    merges_alike([
        with(s(3, &[("t_0_0", 0, 0, 2)]), &[light], true),
        with(s(0, &[]), &[light, (SharedContent::HullSoup, 9, 2)], false),
        s(10, &[("t_1_0", 0, 1, 1), ("t_0_1", 1, 0, 3)]),
        with(
            s(3, &[("t_2_0", 0, 2, 1)]),
            &[(SharedContent::GrindPrism, 7, 1)],
            true,
        ),
    ]);
    let mut m = with(s(3, &[("t_0_0", 0, 0, 2)]), &[light], true);
    m.merge(with(s(10, &[("t_1_0", 0, 1, 1)]), &[light], true))
        .unwrap();
    // the parts' own content summed, the light both write once, the map's one water object once
    assert_eq!((m.summed, m.other_cull_ids()), (13, 15));
    assert_eq!(m.chains.len(), 2);

    // refused: other writer settings, another pool, cells counted otherwise, a tile two parts state, two whole maps
    let refused = |a: SkateMapSummary, b: SkateMapSummary, why: &str| {
        let mut a = a;
        let e = a.merge(b).unwrap_err();
        assert!(e.contains(why), "{e}");
    };
    let mut other_grouping = s(1, &[]);
    other_grouping.grouping.tile = 100.0;
    refused(s(1, &[]), other_grouping, "writer settings");
    let mut other_pool = s(1, &[]);
    other_pool.cull_ids = 2048;
    refused(s(1, &[]), other_pool, "cull IDs");
    let mut included = s(1, &[]);
    included.cells = CellsCounted::Included;
    refused(s(1, &[]), included.clone(), "cells");
    refused(included.clone(), s(1, &[]), "cells");
    refused(included.clone(), included, "count their collision cells");
    refused(
        s(1, &[("t_0_0", 0, 0, 2)]),
        s(2, &[("t_0_0", 0, 0, 2)]),
        "t_0_0's blend chains are summarized by two parts",
    );
    refused(
        with(s(1, &[]), &[(SharedContent::HullSoup, 9, 2)], false),
        with(s(1, &[]), &[(SharedContent::HullSoup, 9, 3)], false),
        "HullSoup 9 is counted by two parts of the map as 2 and 3 cull IDs",
    );
}
