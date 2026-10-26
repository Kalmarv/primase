//! RealTreeMesh trunks and branches: a body built byte by byte read back, and level 01's three trees read whole
//! (`installs.shaunwhite`) and placed as meshes.
use primase_core::{Mesh, Name, Semantic, Stream, World};
use primase_shaunwhite::read::level::read_level;
use primase_shaunwhite::read::tree::{REAL_TREE_MESH, RING, SUB_REAL_TREE_MESH, Tree};
use primase_testdata::{Game, install};

fn put(b: &mut Vec<u8>, at: usize, v: u32) {
    if b.len() < at + 4 {
        b.resize(at + 4, 0);
    }
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn putf(b: &mut Vec<u8>, at: usize, v: f32) {
    put(b, at, v.to_bits());
}

/// A tree of a trunk of three nodes up the z axis and a branch of two, one leaf; node `k` has radius `0.1 + k / 100`.
fn built() -> Vec<u8> {
    let mut body = vec![1u8];
    for w in [0x1234, REAL_TREE_MESH, 0] {
        body.extend_from_slice(&u32::to_le_bytes(w));
    }
    body.push(1);
    body.extend_from_slice(&2u32.to_le_bytes());
    for (material, kind) in [(0x11u32, 0u32), (0x22, 1)] {
        body.extend_from_slice(&[0, 1]);
        body.extend_from_slice(&u32::to_le_bytes(0x50 + kind));
        body.extend_from_slice(&u32::to_le_bytes(SUB_REAL_TREE_MESH));
        for w in [material, 0, kind, 1] {
            body.extend_from_slice(&w.to_le_bytes());
        }
        body.extend_from_slice(&8f32.to_le_bytes());
    }
    body.extend_from_slice(&[0; 5]);
    let size = 0x2000usize;
    for w in [0x5E5E_7E30u32, size as u32, 154, 12345, 0] {
        body.extend_from_slice(&w.to_le_bytes());
    }
    body.extend_from_slice(&[0; 40]);
    let start = body.len();
    let mut blob = vec![0u8; size];
    let (n, m, l) = (5usize, 2usize, 1usize);
    let base = 0x7230_0000u32;
    let (a, bb, c) = (0x400usize, 0x400 + n * 80, 0x400 + n * 80 + n * 32);
    let (f, g) = (0x1000usize, 0x1000 + n * 72);
    let r = (g + l * 24 + 15) & !15;
    let e = r + n * 32;
    put(&mut blob, 0x104, n as u32);
    put(&mut blob, 0x108, base + a as u32);
    put(&mut blob, 0x10c, base + bb as u32);
    put(&mut blob, 0x110, m as u32);
    put(&mut blob, 0x11c, l as u32);
    put(&mut blob, 0x120, base + c as u32);
    put(&mut blob, 0x354, n as u32);
    put(&mut blob, 0x358, l as u32);
    put(&mut blob, 0x364, base + f as u32);
    put(&mut blob, 0x368, base + g as u32);
    put(&mut blob, 0x380, base + r as u32);
    put(&mut blob, 0x384, base + e as u32);
    // nodes 0..3 the trunk (branch 0), 3..5 branch 1 from the trunk's second node
    let positions = [
        [0., 0., 1.],
        [0., 0., 2.],
        [0., 0., 3.],
        [0., 0., 2.],
        [1., 0., 2.5],
    ];
    let branch = [0u32, 0, 0, 1, 1];
    for k in 0..n {
        for (j, v) in positions[k].iter().enumerate() {
            putf(&mut blob, a + 80 * k + 20 + 4 * j, *v);
        }
        put(&mut blob, a + 80 * k + 72, branch[k]);
        putf(&mut blob, r + 32 * k + 16, 0.1 + k as f32 / 100.);
    }
    body.extend_from_slice(&blob);
    assert!(body.len() > start);
    body
}

#[test]
fn a_built_tree_reads_back_as_branches() {
    let body = built();
    let tree = Tree::parse(&body).unwrap();
    assert_eq!(
        (tree.trunk_material, tree.leaf_material, tree.leaves, tree.dropped),
        (0x11, 0x22, 1, 0)
    );
    assert_eq!(tree.branches.len(), 2);
    assert_eq!(tree.branches[0].len(), 3);
    assert_eq!(tree.branches[1].len(), 2);
    // node k's radius is render record k - 1's; node 0 has record 0's
    let radii: Vec<f32> = tree.branches.iter().flatten().map(|n| n.radius).collect();
    for (got, want) in radii.iter().zip([0.1, 0.1, 0.11, 0.12, 0.13]) {
        assert!((got - want).abs() < 1e-6, "{radii:?}");
    }
    let prim = tree.tubes(None);
    let Some(Stream::F32x3(p)) = prim.streams.get(&Semantic::Position) else {
        panic!("no positions")
    };
    assert_eq!(p.len(), 5 * RING);
    // 2 + 1 segments of RING quads, two triangles each
    assert_eq!(prim.indices.as_ref().map(Vec::len), Some(3 * RING * 6));
    assert!(prim.indices.iter().flatten().all(|&i| (i as usize) < p.len()));
    // every ring vertex is a node's radius away from it
    for (k, node) in tree.branches.iter().flatten().enumerate() {
        for v in &p[k * RING..(k + 1) * RING] {
            let d = (0..3)
                .map(|j| (v[j] - node.position[j]).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!((d - node.radius).abs() < 1e-5, "{d} {}", node.radius);
        }
    }
}

#[test]
fn a_tree_that_does_not_add_up_is_refused() {
    let good = built();
    // the node count disagrees with its second copy
    let mut bad = good.clone();
    let start = good.len() - 0x2000;
    put(&mut bad, start + 0x354, 4);
    assert!(Tree::parse(&bad).is_err());
    // an array pointer outside the blob
    let mut bad = good.clone();
    put(&mut bad, start + 0x108, 0x7230_0000 + 0x1fff);
    assert!(Tree::parse(&bad).is_err());
    // not a RealTreeMesh
    let mut bad = good.clone();
    put(&mut bad, 5, 0x415D_9568);
    assert!(Tree::parse(&bad).is_err());
    // a blob larger than the body
    let mut bad = good;
    let marker = bad
        .windows(4)
        .position(|w| w == 0x5E5E_7E30u32.to_le_bytes())
        .unwrap();
    put(&mut bad, marker + 4, 0x40_0000);
    assert!(Tree::parse(&bad).is_err());
}

#[test]
fn level_01_trees_read_and_place() {
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    let (w, report): (World, _) = read_level(&folder.join("DataPC_01.forge"), &[]).unwrap();
    let count = |key: &str| -> usize {
        report
            .iter()
            .find_map(|(_, t)| t.strip_prefix(&format!("{key}: ")).and_then(|n| n.parse().ok()))
            .unwrap_or(0)
    };
    for (who, what) in report.iter().filter(|(_, t)| t.contains("tree")) {
        println!("{who}: {what}");
    }
    assert!(
        !report.iter().any(|(_, t)| t.starts_with("trees not read")),
        "{report:?}"
    );
    assert_eq!(count("trees"), 3);
    let names: Vec<String> = w
        .iter::<Mesh>()
        .filter_map(|(e, _)| w.get::<Name>(e).map(|n| n.0.clone()))
        .filter(|n| n.contains("UrbanTree"))
        .collect();
    println!("{names:?}");
    for want in [
        "00_Veg_UrbanTree01",
        "00_Veg_UrbanTree01_Thin",
        "00_Veg_UrbanTreeSmall01",
    ] {
        assert!(names.iter().any(|n| n == want), "{want} in {names:?}");
    }
}
