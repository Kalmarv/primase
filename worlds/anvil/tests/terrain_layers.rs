//! `lower-anvil-terrain{ground=layers}` on a hand-made terrain: a level-2 quadtree of 64 m nodes drawn at 0.5 m over
//! its corner 128 m, its material raster two halves of one node (materials 0 and 1) and six stripes of another (2 to
//! 7), each material a detail diffuse. Against the notes' blend (terrain.md §3.5) worked here: each vertex's layer
//! weights; at most 4 layers a piece (the striped node cut); the pieces' triangles the whole ground's to the bit.
use primase_anvil::components::{AnvilTerrain, AnvilTextureMap, TerrainDetail, TerrainTile};
use primase_anvil::passes::{CellPoint, GroundColour, LowerAnvilTerrain, Pieces, SplatUv, TerrainLayers};
use primase_core::frame::SourceFrame;
use primase_core::layered::LayeredMaterial;
use primase_core::pass::{Pass, PassLog};
use primase_core::{
    ColorSpace, Entity, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Transform,
    World,
};

/// The material at core sample (i, j) of node (row, column): node (0, 0) material 0 left of its middle, 1 right;
/// node (0, 1) six stripes along x, 2 to 7; every other node 0.
fn material(row: u32, column: u32, j: usize) -> u8 {
    match (row, column) {
        (0, 0) => u8::from(j >= 64),
        (0, 1) => 2 + (j * 6 / 128) as u8,
        _ => 0,
    }
}

const NOISE: u8 = 128;

/// Node (row, column)'s block 8 value.
fn occlusion(row: u32, column: u32) -> u8 {
    (40 * (row * 3 + column) % 256) as u8
}
const K: f64 = 12.0;

fn world() -> (World, Vec<Entity>) {
    let mut w = World::new();
    let texture = |w: &mut World, rgba: Vec<u8>, side: u32| {
        let e = w.spawn();
        w.insert(
            e,
            Texture {
                width: side,
                height: side,
                format: TexFormat::Rgba8Unorm,
                levels: vec![rgba],
                color_space: ColorSpace::Linear,
                role: TexRole::Other,
                normal: None,
            },
        );
        e
    };
    let noise = texture(&mut w, vec![NOISE; 16], 2);
    let normals: Vec<Entity> = (0..8u8)
        .map(|_| texture(&mut w, [128, 128, 255, 255].repeat(4), 2))
        .collect();
    let details: Vec<Entity> = (0..8u8)
        .map(|m| {
            let e = texture(&mut w, [m * 30, 100, 200, 255].repeat(4), 2);
            w.insert(
                e,
                AnvilTextureMap {
                    index_id: u64::from(m),
                    code: 0,
                    field_25: 1,
                },
            );
            e
        })
        .collect();
    let mut tiles = Vec::new();
    for row in 0..3u32 {
        for column in 0..3u32 {
            let mut raster = vec![0u8; 132 * 132];
            for i in 0..132 {
                for j in 0..132 {
                    // the core sample (i - 2, j - 2) at index (i, j) (TerrainTile::CORE_AT); the borders the core's
                    // edge values
                    let jj = (j as i64 - TerrainTile::CORE_AT as i64).clamp(0, 127) as usize;
                    raster[i * 132 + j] = material(row, column, jj);
                }
            }
            tiles.push(TerrainTile {
                row,
                column,
                heights: vec![0; 132 * 132],
                colour: vec![0; 33 * 33 * 8],
                material: raster,
                // block 8 one value a node (BC4 blocks of equal endpoints)
                block_8: [occlusion(row, column), occlusion(row, column), 0, 0, 0, 0, 0, 0].repeat(33 * 33),
            });
        }
    }
    let e = w.spawn();
    w.insert(
        e,
        AnvilTerrain {
            frame: SourceFrame::WORLD,
            level: 2,
            origin: [0.0, 0.0],
            side: 256.0,
            tiles,
            materials: Vec::new(),
            details: details
                .iter()
                .zip(&normals)
                .map(|(&d, &n)| {
                    Some(TerrainDetail {
                        uv_scale: 1.0,
                        f32_6: 4.0,
                        diffuse: Some(d),
                        normal: Some(n),
                    })
                })
                .collect(),
            noise: Some(noise),
        },
    );
    (w, details)
}

/// The notes' blend at (x, y): the noise-jittered texel, its four texels, the sigmoid per axis; per material its
/// summed weight. A texel past the read nodes (rows and columns 0 to 2) has no material: its weight shared out over
/// the others, as the pass states.
fn blend(x: f64, y: f64) -> Vec<(u8, f64)> {
    let n = f64::from(NOISE) / 255.0;
    let (gx, gy) = (x / 0.5, y / 0.5);
    let (qx, qy) = (gx + (2.0 * n - 1.0) * 0.5, gy + (2.0 * n - 1.0) * 0.5);
    let (cx, cy) = (qx.floor() as i64, qy.floor() as i64);
    let (fx, fy) = (qx - qx.floor() - 0.5, qy - qy.floor() - 0.5);
    let (sx, sy) = (if fx >= 0.0 { 1 } else { -1 }, if fy >= 0.0 { 1 } else { -1 });
    let s = |f: f64| 1.0 / (1.0 + (K * (0.5 - f.abs())).exp());
    let (wx, wy) = (s(fx), s(fy));
    let at = |tx: i64, ty: i64| {
        let (row, column) = (ty.div_euclid(128), tx.div_euclid(128));
        ((0..3).contains(&row) && (0..3).contains(&column))
            .then(|| material(row as u32, column as u32, tx.rem_euclid(128) as usize))
    };
    let mut out: Vec<(u8, f64)> = Vec::new();
    for ((tx, ty), wt) in [
        ((cx, cy), (1.0 - wx) * (1.0 - wy)),
        ((cx + sx, cy), wx * (1.0 - wy)),
        ((cx, cy + sy), (1.0 - wx) * wy),
        ((cx + sx, cy + sy), wx * wy),
    ] {
        let Some(m) = at(tx, ty) else {
            continue;
        };
        match out.iter_mut().find(|(n, _)| *n == m) {
            Some((_, v)) => *v += wt,
            None => out.push((m, wt)),
        }
    }
    let sum: f64 = out.iter().map(|(_, v)| v).sum();
    for (_, v) in &mut out {
        *v /= sum;
    }
    out
}

fn lowered(pieces: Pieces) -> World {
    lowered_with(pieces, false)
}

fn lowered_with(pieces: Pieces, occlusion: bool) -> World {
    let (mut w, _) = world();
    let pass = LowerAnvilTerrain {
        rect: [0.0, 0.0, 128.0, 64.0],
        step: 1,
        range: 1.0,
        offset: 0.0,
        sample: CellPoint::Corner,
        colour: ColorSpace::Srgb,
        collision: None,
        surface: String::new(),
        ground: GroundColour::Layers(TerrainLayers {
            k: K,
            uv: SplatUv::Factor(0.25),
            noise_srgb: false,
            occlusion,
        }),
        pieces,
    };
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("lower");
    println!("{log:?}");
    w
}

/// Every placed piece: its primitive and its origin.
fn placed(w: &World) -> Vec<(Primitive, [f32; 3])> {
    w.iter::<Instance>()
        .map(|(e, i)| {
            let mesh = w.get::<Mesh>(i.prototype).expect("mesh");
            (
                mesh.primitives()[0].clone(),
                w.get::<Transform>(e).expect("transform").cols[3],
            )
        })
        .collect()
}

fn triangles(p: &Primitive, o: [f32; 3]) -> Vec<[[u32; 3]; 3]> {
    let pos = p.positions().expect("positions");
    p.indices
        .as_ref()
        .expect("indices")
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| std::array::from_fn(|k| (pos[i as usize][k] + o[k]).to_bits())))
        .collect()
}

#[test]
fn layers_weigh_each_vertex_as_the_engine_blends() {
    let w = lowered(Pieces::PerNode);
    let pieces = placed(&w);
    let mut checked = 0;
    let mut worst = 0.0f64;
    let mut all = Vec::new();
    for (p, o) in &pieces {
        all.extend(triangles(p, *o));
        let layered = w
            .get::<LayeredMaterial>(p.material.expect("material"))
            .expect("layered");
        assert!(layered.layers.len() <= 4, "{} layers", layered.layers.len());
        // each layer's material by its texture's index (the test's detail entities are its materials, in order)
        let index: Vec<u8> = layered
            .layers
            .iter()
            .map(|l| {
                let pbr = w
                    .get::<primase_core::pbr::PbrMaterial>(l.as_ref().expect("layer").material)
                    .expect("pbr");
                let t = pbr.base_color_texture.as_ref().expect("diffuse").texture;
                w.get::<AnvilTextureMap>(t).expect("map").index_id as u8
            })
            .collect();
        let Some(Stream::F32x4(weights)) = p.streams.get(&Semantic::Color(1)) else {
            panic!("weights")
        };
        let pos = p.positions().expect("positions");
        let used: std::collections::BTreeSet<u8> = pos
            .iter()
            .flat_map(|q| blend(f64::from(q[0] + o[0]), f64::from(q[1] + o[1])))
            .filter(|&(_, v)| v > 0.0)
            .map(|(m, _)| m)
            .collect();
        for (q, wt) in pos.iter().zip(weights) {
            let want = blend(f64::from(q[0] + o[0]), f64::from(q[1] + o[1]));
            // where the piece keeps every material it uses, each weight is the blend's
            if used.len() <= 4 {
                for (m, v) in &want {
                    let slot = index.iter().position(|n| n == m).expect("a used material kept");
                    worst = worst.max((f64::from(wt[slot]) - v).abs());
                }
                checked += 1;
            }
            let sum: f32 = wt.iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "{wt:?}");
        }
    }
    println!(
        "{} pieces, {checked} vertices checked, worst weight difference {worst:e}",
        pieces.len()
    );
    assert!(checked > 0 && worst < 1e-6, "{worst}");
    // the striped node (six materials) is cut: more than the two nodes' two pieces
    assert!(pieces.len() > 2, "{} pieces", pieces.len());
    // the triangles are the whole ground's
    let whole = lowered(Pieces::Whole);
    let mut want: Vec<_> = placed(&whole)
        .iter()
        .flat_map(|(p, o)| triangles(p, *o))
        .collect();
    all.sort_unstable();
    want.sort_unstable();
    assert_eq!(all, want);
}

/// With the occlusion tint: each vertex's COLOR_0 the shader's factor 0.5 + block 8 (bilinear: at a vertex a texel or
/// more inside its node, its node's value); every layer its material's detail normal, marked green-down, z rebuilt.
#[test]
fn layers_carry_their_normals_and_the_occlusion_tint() {
    let w = lowered_with(Pieces::PerNode, true);
    let mut inside = 0;
    for (p, o) in placed(&w) {
        let Some(Stream::F32x4(tint)) = p.streams.get(&Semantic::Color(0)) else {
            panic!("no occlusion tint")
        };
        for (q, t) in p.positions().expect("positions").iter().zip(tint) {
            let (x, y) = (f64::from(q[0] + o[0]), f64::from(q[1] + o[1]));
            let (tx, ty) = (x / 0.5, y / 0.5);
            let (lx, ly) = (tx.rem_euclid(128.0), ty.rem_euclid(128.0));
            if (1.0..=127.0).contains(&lx) && (1.0..=127.0).contains(&ly) {
                let v = f64::from(occlusion((ty / 128.0) as u32, (tx / 128.0) as u32)) / 255.0;
                assert!((f64::from(t[0]) - (0.5 + v)).abs() < 1e-6, "{t:?} at ({x}, {y})");
                assert_eq!((t[0], t[1], t[2], t[3]), (t[0], t[0], t[0], 1.0));
                inside += 1;
            }
        }
        let layered = w
            .get::<LayeredMaterial>(p.material.expect("material"))
            .expect("layered");
        for l in layered.layers.iter().flatten() {
            let pbr = w.get::<primase_core::pbr::PbrMaterial>(l.material).expect("pbr");
            let n = pbr.normal_texture.as_ref().expect("a detail normal").texture;
            let c = w
                .get::<Texture>(n)
                .expect("texture")
                .normal
                .expect("a convention");
            assert_eq!(
                (c.green, c.z),
                (primase_core::NormalGreen::Down, primase_core::NormalZ::Rebuild)
            );
        }
    }
    assert!(inside > 1000, "{inside}");
}
