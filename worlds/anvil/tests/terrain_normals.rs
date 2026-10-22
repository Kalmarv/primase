//! `lower-anvil-terrain` gives every ground vertex a smooth normal from the height grid: a sloped plane gives the
//! plane's normal everywhere (the outer edge too), neighbouring per-node pieces carry the same bits at their shared
//! seam vertices, a ridge's crest is a smooth vertical normal rather than a face's, and the normal is up in core's
//! frame (agreeing with the winding) whatever the terrain's frame, for `pieces=whole` and `pieces=per-node` alike.
use primase_anvil::components::{AnvilTerrain, TerrainTile};
use primase_anvil::passes::{
    CellPoint, GroundColour, GroundNormals, LowerAnvilTerrain, Pieces, Splat, SplatUv, SurfaceBy,
};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassLog};
use primase_core::{
    ColorSpace, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Transform, World,
};

/// A level-2 quadtree of 64 m nodes (3 x 3 read), the sample `(gx, gy)` (a node's 128 per axis, 0.5 m apart) at
/// height `height(gx, gy)` metres (`range` 2^26 makes a stored value one metre at level 2).
fn world(frame: SourceFrame, height: impl Fn(i64, i64) -> i64) -> World {
    let mut w = World::new();
    let noise = w.spawn();
    w.insert(
        noise,
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![128; 16]],
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        },
    );
    let mut tiles = Vec::new();
    for row in 0..3i64 {
        for column in 0..3i64 {
            let mut heights = vec![0i32; 132 * 132];
            for i in 0..132i64 {
                for j in 0..132i64 {
                    // the tile's texel (i, j) is the core sample (i - 2, j - 2); the border texels are the
                    // neighbouring node's samples
                    let (gy, gx) = (row * 128 + i - 2, column * 128 + j - 2);
                    heights[(i * 132 + j) as usize] = height(gx, gy) as i32;
                }
            }
            let block = [0u8; 8];
            tiles.push(TerrainTile {
                row: row as u32,
                column: column as u32,
                heights,
                colour: block.repeat(33 * 33),
                material: vec![0; 132 * 132],
                block_8: vec![0; 33 * 33 * 8],
            });
        }
    }
    let e = w.spawn();
    w.insert(
        e,
        AnvilTerrain {
            frame,
            level: 2,
            origin: [0.0, 0.0],
            side: 256.0,
            tiles,
            materials: Vec::new(),
            details: Vec::new(),
            noise: Some(noise),
        },
    );
    w
}

fn lowered(frame: SourceFrame, height: impl Fn(i64, i64) -> i64, pieces: Pieces, step: u32) -> World {
    let mut w = world(frame, height);
    let pass = LowerAnvilTerrain {
        rect: [0.0, 0.0, 128.0, 128.0],
        step,
        range: f64::from(1u32 << 26),
        offset: 0.0,
        sample: CellPoint::Corner,
        colour: ColorSpace::Srgb,
        collision: Some(SurfaceBy::One),
        surface: "ground".to_owned(),
        ground: GroundColour::Splat(Splat {
            k: 8.0,
            pixel: 0.25,
            uv: SplatUv::Factor(1.0),
            noise_srgb: false,
            occlusion: false,
        }),
        pieces,
        normals: GroundNormals::CentralDifferences,
    };
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("lower");
    assert!(
        log.approximations
            .iter()
            .any(|(k, v)| k.starts_with("ground normals") && *v > 0),
        "the pass counts its normals as an approximation: {:?}",
        log.approximations
    );
    w
}

/// Each placed ground piece: its primitive and its origin.
fn placed(w: &World) -> Vec<(Primitive, [f32; 3])> {
    let mut out: Vec<_> = w
        .iter::<Instance>()
        .map(|(e, i)| {
            let t = w.get::<Transform>(e).expect("transform");
            let mesh = w.get::<Mesh>(i.prototype).expect("mesh");
            (mesh.primitives()[0].clone(), t.cols[3])
        })
        .collect();
    out.sort_by(|a, b| a.1.map(f32::to_bits).cmp(&b.1.map(f32::to_bits)));
    out
}

fn streams(p: &Primitive) -> (&[[f32; 3]], &[[f32; 3]]) {
    let (Some(Stream::F32x3(pos)), Some(Stream::F32x3(nrm))) = (
        p.streams.get(&Semantic::Position),
        p.streams.get(&Semantic::Normal),
    ) else {
        panic!("a ground primitive without Position and Normal");
    };
    assert_eq!(pos.len(), nrm.len());
    (pos, nrm)
}

fn unit(n: [f32; 3]) -> bool {
    (n.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt() - 1.0).abs() < 1e-6
}

fn near(a: [f32; 3], b: [f64; 3], tol: f64) -> bool {
    a.iter().zip(b).all(|(x, y)| (f64::from(*x) - y).abs() < tol)
}

/// A plane `h = 2 gx + 3 gy` metres: over 0.5 m samples the slopes are 4 and 6, the normal `(-4, -6, 1)` normalised, at
/// every vertex of every piece, the grid's outer edge and the node seams included, whole or per node, at `step` 1 or 2.
#[test]
fn a_sloped_plane_gives_the_planes_normal_everywhere() {
    let len = (4.0f64 * 4.0 + 6.0 * 6.0 + 1.0).sqrt();
    let want = [-4.0 / len, -6.0 / len, 1.0 / len];
    for pieces in [Pieces::Whole, Pieces::PerNode] {
        for step in [1, 2] {
            let w = lowered(SourceFrame::WORLD, |gx, gy| 2 * gx + 3 * gy, pieces, step);
            let placed = placed(&w);
            assert!(!placed.is_empty());
            for (p, _) in &placed {
                let (_, nrm) = streams(p);
                for n in nrm {
                    assert!(unit(*n), "{n:?}");
                    assert!(
                        near(*n, want, 1e-5),
                        "{n:?} against {want:?} ({pieces:?}, step {step})"
                    );
                }
            }
        }
    }
}

/// Neighbouring per-node pieces draw the vertices on their shared seam with the same normals to the bit, on a surface
/// that is not a plane (so a one-sided normal at a piece edge would differ), and these are the whole ground's.
#[test]
fn neighbouring_pieces_share_their_seam_vertices_normals() {
    let hill = |gx: i64, gy: i64| ((gx * gx) % 97) + ((gy * 3) % 31) + (gx * gy) % 13;
    let whole = lowered(SourceFrame::WORLD, hill, Pieces::Whole, 1);
    let per_node = lowered(SourceFrame::WORLD, hill, Pieces::PerNode, 1);
    let world_normals = |w: &World| {
        let mut by_pos = std::collections::BTreeMap::new();
        for (p, origin) in placed(w) {
            let (pos, nrm) = streams(&p);
            for (q, n) in pos.iter().zip(nrm) {
                let key: [u32; 3] = std::array::from_fn(|k| (q[k] + origin[k]).to_bits());
                let bits = n.map(f32::to_bits);
                if let Some(old) = by_pos.insert(key, bits) {
                    assert_eq!(old, bits, "a shared vertex with two normals");
                }
            }
        }
        by_pos
    };
    let (a, b) = (world_normals(&whole), world_normals(&per_node));
    assert_eq!(placed(&per_node).len(), 4);
    assert_eq!(a.len(), b.len());
    assert_eq!(a, b, "the pieces' normals are the whole's to the bit");
}

/// A ridge `h = -|gx - 64|` metres: a face's normal is `(+-2, 0, 1)` normalised (a 63 deg tilt either way); the crest
/// vertex's normal, the central difference, is exactly vertical, and its neighbours' are the faces': the normal turns
/// through the crest over two vertices (about 63 deg each), not at once (127 deg).
#[test]
fn a_ridge_crest_is_a_smooth_normal() {
    let w = lowered(SourceFrame::WORLD, |gx, _| -(gx - 64).abs(), Pieces::Whole, 1);
    let placed = placed(&w);
    let (pos, nrm) = streams(&placed[0].0);
    let origin = placed[0].1;
    let at = |x: f32, y: f32| {
        let i = pos
            .iter()
            .position(|q| (q[0] + origin[0] - x).abs() < 1e-4 && (q[1] + origin[1] - y).abs() < 1e-4)
            .expect("vertex");
        nrm[i]
    };
    let (left, crest, right) = (at(31.5, 20.0), at(32.0, 20.0), at(32.5, 20.0));
    assert!(near(crest, [0.0, 0.0, 1.0], 1e-6), "crest {crest:?}");
    let face = 1.0 / 5.0f64.sqrt();
    assert!(near(left, [-2.0 * face, 0.0, face], 1e-6), "left {left:?}");
    assert!(near(right, [2.0 * face, 0.0, face], 1e-6), "right {right:?}");
    let angle = |a: [f32; 3], b: [f32; 3]| {
        (a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(y))
            .sum::<f64>())
        .acos()
        .to_degrees()
    };
    assert!((angle(left, crest) - 63.43).abs() < 0.1, "{}", angle(left, crest));
    assert!((angle(right, crest) - 63.43).abs() < 0.1);
}

/// In an ENU terrain (world = (east, up, -north)) the plane's normal is up in core's frame, unit, and on the front side
/// of every triangle by the winding the pass writes.
#[test]
fn the_normal_is_up_in_cores_frame_and_agrees_with_the_winding() {
    let w = lowered(SourceFrame::ENU, |gx, gy| gx - gy, Pieces::Whole, 1);
    let placed = placed(&w);
    let p = &placed[0].0;
    let (pos, nrm) = streams(p);
    let idx = p.indices.as_ref().expect("indices");
    for t in idx.as_chunks::<3>().0.iter().take(500) {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        let u: [f64; 3] = std::array::from_fn(|k| f64::from(b[k] - a[k]));
        let v: [f64; 3] = std::array::from_fn(|k| f64::from(c[k] - a[k]));
        let face = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for i in t {
            let n = nrm[*i as usize];
            assert!(unit(n));
            assert!(n[1] > 0.0, "up in core's frame (y): {n:?}");
            let dot: f64 = (0..3).map(|k| face[k] * f64::from(n[k])).sum();
            assert!(dot > 0.0, "the normal is on the winding's front side");
        }
    }
}
