//! Gates of the PhysicsResource reader, writer and mesh encoder (`primase_frostbite::physics`).
//!
//! - Synthetic: a cube's encoding (welding, edges, faces) worked by hand; every face of a bumpy grid inside the box
//!   its leaf of the tree decodes to; a refused corrupt resource.
//! - The encoding reproduces ReSkate Studio's own output byte for byte: `worlds/reskate/tests/physics.rs`.
//! - `the_games_own_resources_round_trip` (`installs.skate/Data`, `tools.oodle9`): every PhysicsResource of every TOC
//!   reads and writes back byte-identical (the game's layouts: with and without the body, polygon faces, convex shapes
//!   and boxes with face planes), but those with the tables not read yet, refused by name.

#![allow(clippy::unwrap_used)]

use primase_compress::oodle::{Oodle, Version};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::physics::{
    self, Edge, KIND_BOX, KIND_CONVEX, KIND_MESH, Part, PhysicsResource, mesh_shape, weld,
};
use primase_testdata::{Game, Tool, install_file, tool};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A 1 x 2 x 3 box centred on (20, 30, 0), its six quads as two triangles each.
fn cube() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (x, y, z) = (0.5, 1.0, 1.5);
    let p: Vec<[f32; 3]> = [
        [-x, -y, -z],
        [x, -y, -z],
        [x, y, -z],
        [-x, y, -z],
        [-x, -y, z],
        [x, -y, z],
        [x, y, z],
        [-x, y, z],
    ]
    .iter()
    .map(|v| [v[0] + 20.0, v[1] + 30.0, v[2]])
    .collect();
    let quads = [
        [4, 5, 6, 7],
        [1, 0, 3, 2],
        [5, 1, 2, 6],
        [0, 4, 7, 3],
        [7, 6, 2, 3],
        [0, 1, 5, 4],
    ];
    let t = quads
        .iter()
        .flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]])
        .collect();
    (p, t)
}

/// The box worked by hand: welding keeps its 8 vertices in first-use order and starts each triangle at its smallest
/// vertex; the encoding recentres it on (20, 30, 0); each triangle's three edges are its quad's two outer edges (90
/// degrees to the next face: convex, cos 45 deg) and the quad's diagonal (coplanar: flat). The tree has 11 nodes and
/// holds every face once.
#[test]
fn a_box_encodes_as_worked_by_hand() -> R {
    let (p, t) = cube();
    let w = weld(&p, &t)?;
    assert_eq!(w.positions.len(), 8);
    assert_eq!((w.degenerate, w.repeated), (0, 0));
    // first use: 4 5 6 7 1 0 3 2 -> welded 0..7; [1, 0, 3] is welded [4, 5, 6]
    assert_eq!(w.triangles[0], [0, 1, 2]);
    assert_eq!(w.triangles[2], [4, 5, 6]);
    // [0, 1, 5] = welded [5, 4, 1] starts at its smallest: [1, 5, 4]; [0, 5, 4] = welded [5, 1, 0]: [0, 5, 1]
    assert_eq!(w.triangles[10], [1, 5, 4]);
    assert_eq!(w.triangles[11], [0, 5, 1]);
    let (s, centre) = mesh_shape(&w.positions, &w.triangles)?.unwrap();
    assert_eq!(centre, [20.0, 30.0, 0.0]);
    assert_eq!((s.min, s.max), ([-0.5, -1.0, -1.5], [0.5, 1.0, 1.5]));
    assert_eq!(
        (s.vertices.len(), s.corners.len(), s.face_count(), s.tree.len()),
        (8, 36, 12, 11)
    );
    let convex = Edge {
        value: std::f32::consts::FRAC_1_SQRT_2,
        concave: false,
        flat: false,
    };
    let flat = Edge {
        value: 1.0,
        concave: false,
        flat: true,
    };
    let mut kinds = [0usize; 2];
    for e in &s.edges {
        if *e == flat {
            kinds[0] += 1;
        } else {
            assert!(
                (e.value - convex.value).abs() < 1e-6 && !e.concave && !e.flat,
                "{e:?}"
            );
            kinds[1] += 1;
        }
    }
    // 6 quads: 2 diagonal corners each flat, 4 outer corners each convex
    assert_eq!(kinds, [12, 24]);
    let mut faces: Vec<Vec<u16>> = (0..12)
        .map(|f| {
            let mut c = s.corners[s.faces[f] as usize..s.faces[f + 1] as usize].to_vec();
            c.sort_unstable();
            c
        })
        .collect();
    faces.sort();
    faces.dedup();
    assert_eq!(faces.len(), 12, "every face once");
    Ok(())
}

/// Welding: -0.0 and 0.0 are one vertex (the first use's value kept), a triangle with a repeated vertex or no area
/// is dropped, a repeated triangle (same corners, same winding) is dropped, its reversal kept.
#[test]
fn welding_drops_what_has_no_area_and_repeats() -> R {
    let p = [
        [0.0, 0.0, -0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
    ];
    let t = [[0, 1, 2], [3, 1, 2], [1, 2, 0], [0, 2, 1], [0, 1, 4], [0, 0, 1]];
    let w = weld(&p, &t)?;
    assert_eq!(
        w.positions,
        vec![
            [0.0, 0.0, -0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.0, 0.0, 0.0]
        ]
    );
    assert_eq!(w.positions[0][2].to_bits(), (-0.0f32).to_bits());
    // [3, 1, 2] welds to [0, 1, 2], [1, 2, 0] rotates to it: both repeats; [0, 2, 1] is the reversal; [0, 1, 4] is on
    // a line (no area); [0, 0, 1] repeats a vertex
    assert_eq!(w.triangles, vec![[0, 1, 2], [0, 2, 1]]);
    assert_eq!(w.source, vec![0, 3]);
    assert_eq!((w.degenerate, w.repeated), (2, 2));
    Ok(())
}

/// The child boxes a tree node decodes to hold everything below them: every face of a 32 x 32 bumpy grid (2,048
/// triangles) lies inside the box of the leaf it ends in, the boxes being followed from the root as the game does.
#[test]
fn every_face_lies_inside_its_leaf_box() -> R {
    let n = 32usize;
    let mut p = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, z) = (i as f32 * 0.75 - 12.0, j as f32 * 0.5 - 8.0);
            p.push([x, (x * 0.7).sin() * (z * 0.5).cos() * 1.5 + 0.1 * x, z]);
        }
    }
    let mut t = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u32;
            let s = (n + 1) as u32;
            t.push([a, a + s, a + 1]);
            t.push([a + 1, a + s, a + s + 1]);
        }
    }
    let w = weld(&p, &t)?;
    let (s, _) = mesh_shape(&w.positions, &w.triangles)?.unwrap();
    let nf = s.face_count();
    assert_eq!((nf, s.tree.len()), (2 * n * n, 2 * n * n - 1));
    // the box of every heap node, from the root's
    let mut boxes = vec![[0f32; 6]; 2 * nf - 1];
    boxes[0] = [s.min[0], s.min[1], s.min[2], s.max[0], s.max[1], s.max[2]];
    for (i, &node) in s.tree.iter().enumerate() {
        let (l, r) = physics::decode_node(boxes[i], node);
        boxes[2 * i + 1] = l;
        boxes[2 * i + 2] = r;
    }
    for f in 0..nf {
        let b = boxes[nf - 1 + f];
        for &c in &s.corners[s.faces[f] as usize..s.faces[f + 1] as usize] {
            let v = s.vertices[c as usize];
            for a in 0..3 {
                assert!(
                    b[a] <= v[a] && v[a] <= b[3 + a],
                    "face {f} vertex {v:?} outside its leaf {b:?}"
                );
            }
        }
    }
    Ok(())
}

/// A resource whose shape table runs past its counts is refused, not read.
#[test]
fn a_corrupt_resource_is_refused() {
    let (p, t) = cube();
    let w = weld(&p, &t).unwrap();
    let (s, centre) = mesh_shape(&w.positions, &w.triangles).unwrap().unwrap();
    let res = PhysicsResource {
        version: 2,
        word08: 0,
        body: Vec::new(),
        shapes: vec![s],
        parts: vec![Part::mesh(0, centre, [1, 2, 3], 96)],
    };
    let mut b = physics::write(&res).unwrap();
    assert_eq!(physics::read(&b).unwrap(), res);
    b[0x18] = 3;
    assert!(physics::read(&b).is_err());
}

fn oodle() -> Option<Oodle> {
    let p = tool(Tool::Oodle9)?;
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let o = unsafe { Oodle::load(p, Version::V9) };
    Some(o.expect("tools.oodle9 loads"))
}

/// Every PhysicsResource of every TOC of the game reads and writes back byte for byte, but those holding the two
/// tables not read yet (24- and 44-byte entries; dynamic props), which are refused by that name; the three part kinds
/// (box, convex, mesh) all occur.
#[test]
fn the_games_own_resources_round_trip() -> R {
    let (Some(data), Some(oodle)) = (install_file(Game::Skate, "Data"), oodle()) else {
        return Ok(());
    };
    let folder = Folder::open(&data)?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let (mut read, mut unread) = (0usize, 0usize);
    let mut kinds = std::collections::BTreeMap::<u32, usize>::new();
    let mut seen = std::collections::BTreeSet::new();
    for toc in &folder.tocs {
        let listing = src.listing(toc)?;
        for e in listing
            .entries()
            .filter(|e| e.resource_type == Some(physics::RTYPE))
        {
            let (loc, at, _) = e.file;
            if !seen.insert((loc.patch, loc.chunk, loc.archive, at)) {
                continue;
            }
            let bytes = src.payload(e.file, e.size as usize)?;
            match physics::read(&bytes) {
                Ok(p) => {
                    assert!(physics::write(&p)? == bytes, "{}: round trip", e.name);
                    for q in &p.parts {
                        *kinds.entry(q.kind).or_default() += 1;
                    }
                    read += 1;
                }
                Err(err) => {
                    assert!(
                        err.to_string().contains("entries (none read yet)"),
                        "{}: {err}",
                        e.name
                    );
                    unread += 1;
                }
            }
        }
    }
    eprintln!("{read} PhysicsResources round trip, {unread} hold the unread tables; part kinds {kinds:?}");
    assert!(read > 0);
    for k in [KIND_BOX, KIND_CONVEX, KIND_MESH] {
        assert!(kinds.contains_key(&k), "kind {k} occurs");
    }
    Ok(())
}
