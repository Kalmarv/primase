//! `lower-anvil-terrain{ground=layers}` on a hand-made terrain: a level-2 quadtree of 64 m nodes drawn at 0.5 m over
//! its corner 128 m, its material raster two halves of one node (materials 0 and 1) and stripes of another (2 to
//! 7), each material a detail diffuse (or one without), each node's colour tile one solid colour. Against the notes'
//! blend (terrain.md §3.5) worked here: each vertex's layer weights, the far colour's the weight no detail layer
//! holds; at most 4 layers a piece (the striped node cut); the pieces' triangles the whole ground's to the bit.
use primase_anvil::components::{AnvilTerrain, AnvilTextureMap, TerrainDetail, TerrainTile};
use primase_anvil::passes::{
    CellPoint, GroundColour, GroundNormals, LayersBase, LowerAnvilTerrain, Pieces, SplatUv, Steep, TerrainLayers,
};
use primase_core::frame::SourceFrame;
use primase_core::layered::LayeredMaterial;
use primase_core::pass::Options;
use primase_core::pass::{Pass, PassLog};
use primase_core::{
    ColorSpace, Entity, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Transform,
    World,
};

/// The material at core sample (i, j) of node (row, column): node (0, 0) material 0 left of its middle, 1 right;
/// node (0, 1) stripes along x, 2 to 7: 21 or 22 samples wide to sample 96, then 2 wide, all six every 6 m (more than
/// a piece of 16 squares holds); every other node 0.
fn material(row: u32, column: u32, j: usize) -> u8 {
    match (row, column) {
        (0, 0) => u8::from(j >= 64),
        (0, 1) if j < 96 => 2 + (j * 6 / 128) as u8,
        (0, 1) => 2 + (j / 2 % 6) as u8,
        _ => 0,
    }
}

const NOISE: u8 = 128;

/// Node (row, column)'s block 8 value.
fn occlusion(row: u32, column: u32) -> u8 {
    (40 * (row * 3 + column) % 256) as u8
}
const K: f64 = 12.0;

/// Node (row, column)'s colour: red, green and blue each 0 or 255 by the bits of `row * 3 + column + 1` (the 5- and
/// 6-bit ends, which every BC1 expansion takes to 0 and 255).
fn node_colour(row: u32, column: u32) -> [u8; 4] {
    let i = (row * 3 + column + 1) % 8;
    [i & 1, i >> 1 & 1, i >> 2 & 1]
        .map(|b| if b == 1 { 255 } else { 0 })
        .into_iter()
        .chain([255])
        .collect::<Vec<_>>()
        .try_into()
        .expect("4")
}

/// A BC1 tile of 33 x 33 blocks all of `c`: both endpoints the colour's RGB565, every index 0 (endpoint 0).
fn solid_bc1(c: [u8; 4]) -> Vec<u8> {
    let c565 = (u16::from(c[0] >> 3) << 11) | (u16::from(c[1] >> 2) << 5) | u16::from(c[2] >> 3);
    let [lo, hi] = c565.to_le_bytes();
    [lo, hi, lo, hi, 0, 0, 0, 0].repeat(33 * 33)
}

/// The hand-made terrain, `undetailed` a material without a detail diffuse.
fn world(undetailed: Option<u8>) -> (World, Vec<Entity>) {
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
                colour: solid_bc1(node_colour(row, column)),
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
                .enumerate()
                .map(|(m, (&d, &n))| {
                    Some(TerrainDetail {
                        uv_scale: 1.0,
                        f32_6: 4.0,
                        diffuse: (undetailed != Some(m as u8)).then_some(d),
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
    lowered_with(pieces, false, None)
}

fn lowered_with(pieces: Pieces, occlusion: bool, undetailed: Option<u8>) -> World {
    lowered_over(pieces, occlusion, undetailed, LayersBase::Tile, 1.0)
}

/// The ground with the details at weight `detail` over `base`.
fn lowered_over(
    pieces: Pieces,
    occlusion: bool,
    undetailed: Option<u8>,
    base: LayersBase,
    detail: f64,
) -> World {
    let (mut w, _) = world(undetailed);
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
            base,
            detail,
        }),
        pieces,
        normals: GroundNormals::None,
        steep: Steep::None,
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

/// A piece's layers: per layer its material's index (the test's detail entities are its materials, in order), or
/// none for the far colour (on `TexCoord(1)`).
fn layer_index(w: &World, layered: &LayeredMaterial) -> Vec<Option<u8>> {
    layered
        .layers
        .iter()
        .map(|l| {
            let l = l.as_ref().expect("layer");
            let pbr = w.get::<primase_core::pbr::PbrMaterial>(l.material).expect("pbr");
            let t = pbr.base_color_texture.as_ref().expect("diffuse").texture;
            match l.texcoord {
                Some(1) => None,
                None => Some(w.get::<AnvilTextureMap>(t).expect("map").index_id as u8),
                Some(n) => panic!("a layer on texture coordinates {n}"),
            }
        })
        .collect()
}

/// Checks every piece's weights against the blend: each detail layer's weight the blend's times `detail`, the base's
/// the rest (the materials without a detail, and those the piece doesn't keep, and what `detail` leaves of the kept
/// ones); at most 4 layers, the base last and only where it holds weight. Returns the pieces, the vertices checked and
/// the base layer's vertices.
fn check_weights(
    w: &World,
    undetailed: Option<u8>,
    detail: f64,
) -> (Vec<(Primitive, [f32; 3])>, usize, usize) {
    let pieces = placed(w);
    let (mut checked, mut far_vertices) = (0, 0);
    let mut worst = 0.0f64;
    for (p, o) in &pieces {
        let layered = w
            .get::<LayeredMaterial>(p.material.expect("material"))
            .expect("layered");
        assert!(layered.layers.len() <= 4, "{} layers", layered.layers.len());
        let index = layer_index(w, layered);
        let far = index.iter().position(Option::is_none);
        if let Some(f) = far {
            assert_eq!(f, index.len() - 1, "the far colour last");
        }
        let Some(Stream::F32x4(weights)) = p.streams.get(&Semantic::Color(1)) else {
            panic!("weights")
        };
        let pos = p.positions().expect("positions");
        let mut far_held = 0.0f64;
        for (q, wt) in pos.iter().zip(weights) {
            let mut rest = 0.0;
            for (m, v) in blend(f64::from(q[0] + o[0]), f64::from(q[1] + o[1])) {
                match index.iter().position(|n| *n == Some(m)) {
                    Some(slot) if undetailed != Some(m) => {
                        worst = worst.max((f64::from(wt[slot]) - detail * v).abs());
                        rest += (1.0 - detail) * v;
                    }
                    _ => rest += v,
                }
            }
            match far {
                Some(f) => {
                    worst = worst.max((f64::from(wt[f]) - rest).abs());
                    far_held += f64::from(wt[f]);
                    far_vertices += 1;
                }
                None => assert!(rest < 1e-12, "{rest} weight on no layer"),
            }
            let sum: f32 = wt.iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "{wt:?}");
            checked += 1;
        }
        if far.is_some() {
            assert!(far_held > 0.0, "a far colour layer holding nothing");
        }
    }
    println!(
        "{} pieces, {checked} vertices checked ({far_vertices} with the far colour), worst weight difference {worst:e}",
        pieces.len()
    );
    assert!(checked > 0 && worst < 1e-6, "{worst}");
    (pieces, checked, far_vertices)
}

#[test]
fn layers_weigh_each_vertex_as_the_engine_blends() {
    let w = lowered(Pieces::PerNode);
    let (pieces, _, far_vertices) = check_weights(&w, None, 1.0);
    // the cap at 16 squares: the striped node's pieces holding more than 4 materials keep 3 and the far colour
    assert!(far_vertices > 0, "no piece over the cap");
    let mut all = Vec::new();
    for (p, o) in &pieces {
        all.extend(triangles(p, *o));
    }
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
    let w = lowered_with(Pieces::PerNode, true, None);
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
        for l in layered.layers.iter().flatten().filter(|l| l.texcoord.is_none()) {
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

/// Material 1 (node (0, 0)'s right half) without a detail diffuse: its weight on the far colour, a layer sampling
/// `TexCoord(1)` of the ground's colour mosaic (clamped), each vertex at the texel of its own sample, which is its
/// node's colour.
#[test]
fn undetailed_weight_goes_to_the_far_colour() {
    let w = lowered_with(Pieces::PerNode, false, Some(1));
    let (pieces, _, far_vertices) = check_weights(&w, Some(1), 1.0);
    assert!(far_vertices > 0, "no far colour");
    let mut texels = 0;
    for (p, o) in &pieces {
        let layered = w
            .get::<LayeredMaterial>(p.material.expect("material"))
            .expect("layered");
        let Some(far) = layered.layers.iter().flatten().find(|l| l.texcoord == Some(1)) else {
            assert!(!p.streams.contains_key(&Semantic::TexCoord(1)));
            continue;
        };
        let pbr = w
            .get::<primase_core::pbr::PbrMaterial>(far.material)
            .expect("pbr");
        let r = pbr.base_color_texture.as_ref().expect("far colour");
        assert_eq!(r.wrap, [primase_core::pbr::Wrap::Clamp; 2]);
        let t = w.get::<Texture>(r.texture).expect("texture");
        assert_eq!(
            (t.format, t.color_space, t.role),
            (TexFormat::Rgba8Unorm, ColorSpace::Srgb, TexRole::Color)
        );
        let Some(Stream::F32x2(uvs)) = p.streams.get(&Semantic::TexCoord(1)) else {
            panic!("far colour coordinates")
        };
        for (q, uv) in p.positions().expect("positions").iter().zip(uvs) {
            // the sample's index along each axis (0.5 m apart from the origin), its node, its texel
            let (gx, gy) = (
                (f64::from(q[0] + o[0]) / 0.5).round() as u32,
                (f64::from(q[1] + o[1]) / 0.5).round() as u32,
            );
            let (tx, ty) = (
                f64::from(uv[0]) * f64::from(t.width),
                f64::from(uv[1]) * f64::from(t.height),
            );
            // the texel's centre (to the f32 coordinate)
            assert!(
                (tx.fract() - 0.5).abs() < 1e-4 && (ty.fract() - 0.5).abs() < 1e-4,
                "{uv:?}"
            );
            let (tx, ty) = (tx as usize, ty as usize);
            assert_eq!(
                (tx, ty),
                (gx as usize, gy as usize),
                "the mosaic's first node is (0, 0)"
            );
            let at = (ty * t.width as usize + tx) * 4;
            assert_eq!(
                t.levels[0][at..at + 4],
                node_colour(gy / 128, gx / 128),
                "sample ({gx}, {gy})"
            );
            texels += 1;
        }
    }
    assert_eq!(texels, far_vertices);
}

/// `layers-base=bake{layers-detail=F}`: each detail layer weighs F times its material's blend, the base the rest; the
/// base last, one bake a piece (its texture the piece's extent at `splat-px` rounded up, clamped, its vertices'
/// `TexCoord(1)` their metres from the piece's corner over that extent), and the bake's pixels the blend of the
/// materials' colours there (each detail a solid colour here); at most 4 layers (3 details and the base), the striped
/// node cut to hold them.
#[test]
fn details_over_a_bake_weigh_f_times_the_blend_and_the_bake_takes_the_rest() {
    const F: f64 = 0.4;
    const PIXEL: f64 = 0.5;
    let w = lowered_over(Pieces::PerNode, false, None, LayersBase::Bake { pixel: PIXEL }, F);
    let (pieces, _, base_vertices) = check_weights(&w, None, F);
    let vertices: usize = pieces
        .iter()
        .map(|(p, _)| p.positions().expect("positions").len())
        .sum();
    assert_eq!(base_vertices, vertices, "every vertex carries the base");
    // the striped node (six materials) is cut until a piece holds 3
    assert!(pieces.len() > 2, "{} pieces", pieces.len());
    let mut deep = 0;
    for (p, o) in &pieces {
        let layered = w
            .get::<LayeredMaterial>(p.material.expect("material"))
            .expect("layered");
        let base = layered.layers.last().and_then(Option::as_ref).expect("a base");
        assert_eq!(base.texcoord, Some(1));
        let pbr = w
            .get::<primase_core::pbr::PbrMaterial>(base.material)
            .expect("pbr");
        let r = pbr.base_color_texture.as_ref().expect("the bake");
        assert_eq!(r.wrap, [primase_core::pbr::Wrap::Clamp; 2]);
        let t = w.get::<Texture>(r.texture).expect("texture");
        assert_eq!(
            (t.format, t.color_space, t.role),
            (TexFormat::Rgba8Unorm, ColorSpace::Srgb, TexRole::Color)
        );
        let pos = p.positions().expect("positions");
        let Some(Stream::F32x2(uvs)) = p.streams.get(&Semantic::TexCoord(1)) else {
            panic!("base coordinates")
        };
        // the piece's corner is its least vertex; its extent in metres the texture's pixels at the pixel size
        let corner = [0, 1].map(|a| {
            pos.iter()
                .map(|q| f64::from(q[a] + o[a]))
                .fold(f64::INFINITY, f64::min)
        });
        let side = [0, 1].map(|a| {
            pos.iter()
                .map(|q| f64::from(q[a] + o[a]))
                .fold(f64::NEG_INFINITY, f64::max)
                - corner[a]
        });
        assert_eq!(
            (t.width, t.height),
            ((side[0] / PIXEL).ceil() as u32, (side[1] / PIXEL).ceil() as u32)
        );
        let extent = [f64::from(t.width) * PIXEL, f64::from(t.height) * PIXEL];
        for (q, uv) in pos.iter().zip(uvs) {
            for a in 0..2 {
                let want = (f64::from(q[a] + o[a]) - corner[a]) / extent[a];
                assert!((f64::from(uv[a]) - want).abs() < 1e-6, "{uv:?} for {q:?}");
            }
        }
        // a pixel well inside node (0, 0)'s left half (material 0: colour (0, 100, 200) as stored sRGB) is that colour
        if corner == [0.0, 0.0] && side == [64.0, 64.0] {
            let (px, py) = (20.0 / PIXEL, 20.0 / PIXEL);
            let at = (py as usize * t.width as usize + px as usize) * 4;
            let got = &t.levels[0][at..at + 3];
            for (g, e) in got.iter().zip([0u8, 100, 200]) {
                assert!(g.abs_diff(e) <= 1, "{got:?}");
            }
            deep += 1;
        }
    }
    assert_eq!(deep, 1, "node (0, 0)'s piece");
}

/// F = 1 over the colour tile draws what the layers drew before the options: the details' weights the blend's, the far
/// colour only where weight goes to it.
#[test]
fn full_detail_over_the_tile_is_the_layers_as_they_were() {
    let w = lowered_over(Pieces::PerNode, false, None, LayersBase::Tile, 1.0);
    let before = lowered(Pieces::PerNode);
    let key = |w: &World| {
        let mut v: Vec<_> = placed(w)
            .iter()
            .flat_map(|(p, o)| {
                let Some(Stream::F32x4(wt)) = p.streams.get(&Semantic::Color(1)) else {
                    panic!("weights")
                };
                let pos = p.positions().expect("positions");
                pos.iter()
                    .zip(wt)
                    .map(|(q, wt)| {
                        (
                            std::array::from_fn::<u32, 3, _>(|k| (q[k] + o[k]).to_bits()),
                            wt.map(f32::to_bits),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(key(&w), key(&before));
}

/// A detail under F < 1 over the tile: the far colour layer then holds (1 - F) of the blend where the details had it.
#[test]
fn partial_detail_over_the_tile_puts_the_rest_on_the_far_colour() {
    const F: f64 = 0.6;
    let w = lowered_over(Pieces::PerNode, false, None, LayersBase::Tile, F);
    let (pieces, _, base_vertices) = check_weights(&w, None, F);
    let vertices: usize = pieces
        .iter()
        .map(|(p, _)| p.positions().expect("positions").len())
        .sum();
    assert_eq!(base_vertices, vertices, "every vertex carries the far colour");
}

fn options(pairs: &[(&str, &str)]) -> Options {
    let mut o: Options = [
        ("rect", "0,0,128,64"),
        ("step", "1"),
        ("range", "1"),
        ("offset", "0"),
        ("sample", "corner"),
        ("colour", "srgb"),
        ("collision", "none"),
        ("pieces", "per-node"),
        ("normals", "none"),
        ("steep", "none"),
        ("ground", "layers"),
        ("splat-k", "12"),
        ("splat-uv", "0.25"),
        ("splat-noise", "raw"),
        ("splat-occlusion", "none"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    o.extend(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
    o
}

/// `layers-base` and `layers-detail` are required with `ground=layers`, `splat-px` goes with `bake` only, the weight
/// is in (0, 1], and the options read back as given.
#[test]
fn the_layer_options_are_required_stated_and_read_back() {
    let err = |pairs: &[(&str, &str)]| LowerAnvilTerrain::from_options(&options(pairs)).expect_err("refused");
    assert!(err(&[("layers-detail", "0.5")]).contains("layers-base is required"));
    assert!(err(&[("layers-base", "tile")]).contains("layers-detail is required"));
    assert!(err(&[("layers-base", "bake"), ("layers-detail", "0.5")]).contains("splat-px is required"));
    assert!(
        err(&[
            ("layers-base", "tile"),
            ("layers-detail", "0.5"),
            ("splat-px", "0.0625")
        ])
        .contains("splat-px")
    );
    assert!(err(&[("layers-base", "mix"), ("layers-detail", "0.5")]).contains("layers-base=mix"));
    assert!(err(&[("layers-base", "tile"), ("layers-detail", "1.5")]).contains("layers-detail"));
    assert!(err(&[("layers-base", "tile"), ("layers-detail", "0")]).contains("layers-detail"));
    let mut splat = options(&[("ground", "splat"), ("splat-px", "0.0625")]);
    splat.insert("layers-detail".to_owned(), "0.5".to_owned());
    assert!(
        LowerAnvilTerrain::from_options(&splat)
            .expect_err("refused")
            .contains("only taken with ground=layers")
    );
    let given = options(&[
        ("layers-base", "bake"),
        ("layers-detail", "0.35"),
        ("splat-px", "0.0625"),
    ]);
    let pass = LowerAnvilTerrain::from_options(&given).expect("options");
    assert_eq!(
        pass.ground,
        GroundColour::Layers(TerrainLayers {
            k: 12.0,
            uv: SplatUv::Factor(0.25),
            noise_srgb: false,
            occlusion: false,
            base: LayersBase::Bake { pixel: 0.0625 },
            detail: 0.35,
        })
    );
    let back = pass.options();
    for (k, v) in &given {
        assert_eq!(back.get(k), Some(v), "{k}");
    }
}
