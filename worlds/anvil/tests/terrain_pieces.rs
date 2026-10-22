//! `lower-anvil-terrain{pieces=per-node}` against `pieces=whole` on a hand-made terrain: a level-2 quadtree of 64 m
//! nodes, the 128 m square at its corner drawn at 0.5 m samples (the nodes on its far edges read too, as the reader
//! reads the nodes meeting a rectangle), its near colour baked at 0.25 m: four pieces, each a prototype of its own
//! with its vertices local to an origin on the 64 m grid, each texture 256 pixels, their triangles (drawn and
//! collided) the whole's to the bit once placed, their pixels the whole's at the same places.
use primase_anvil::components::{AnvilTerrain, TerrainTile};
use primase_anvil::passes::{
    CellPoint, GroundColour, GroundNormals, LowerAnvilTerrain, Pieces, Splat, SplatUv, SurfaceBy,
};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, ColorSpace, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture,
    Transform, World,
};

/// The world with the terrain: heights varying over every sample, colour tiles of one colour per node (BC1 blocks of
/// one endpoint), a flat noise.
fn world() -> World {
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
    for row in 0..3u32 {
        for column in 0..3u32 {
            let heights = (0..132 * 132)
                .map(|k| (k * 7919 + (row * 3 + column) as i32 * 104_729) % 1_000_000)
                .collect();
            // one RGB565 colour per node: both endpoints, every index 0
            let c = ((row * 3 + column) as u16 * 0x0841).to_le_bytes();
            let block = [c[0], c[1], c[0], c[1], 0, 0, 0, 0];
            tiles.push(TerrainTile {
                row,
                column,
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
            frame: SourceFrame::WORLD,
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

fn lowered(pieces: Pieces) -> World {
    let mut w = world();
    let pass = LowerAnvilTerrain {
        rect: [0.0, 0.0, 128.0, 128.0],
        step: 1,
        range: 1.0,
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
        normals: GroundNormals::None,
    };
    pass.run(&mut w, &mut PassLog::default()).expect("lower");
    w
}

/// Each placed ground piece: its primitive, its origin (the placement's translation, its linear part the identity,
/// on the 64 m grid) and its collision, by origin.
fn placed(w: &World) -> Vec<(Primitive, [f32; 3], Collision)> {
    let mut out: Vec<_> = w
        .iter::<Instance>()
        .map(|(e, i)| {
            let t = w.get::<Transform>(e).expect("transform");
            assert_eq!(t.cols[..3], Transform::IDENTITY.cols[..3]);
            assert!(t.cols[3].iter().all(|c| c % 64.0 == 0.0), "{:?}", t.cols[3]);
            let mesh = w.get::<Mesh>(i.prototype).expect("mesh");
            assert_eq!(mesh.primitives().len(), 1);
            let collision = w.get::<Collision>(i.prototype).expect("collision").clone();
            (mesh.primitives()[0].clone(), t.cols[3], collision)
        })
        .collect();
    out.sort_by(|a, b| a.1.map(f32::to_bits).cmp(&b.1.map(f32::to_bits)));
    out
}

/// Each triangle of `indices` over `positions` placed at `origin`, as its three world positions' bits, sorted.
fn triangles(positions: &[[f32; 3]], indices: &[u32], origin: [f32; 3]) -> Vec<[[u32; 3]; 3]> {
    let mut t: Vec<_> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| std::array::from_fn(|k| (positions[i as usize][k] + origin[k]).to_bits())))
        .collect();
    t.sort_unstable();
    t
}

fn drawn(p: &Primitive, origin: [f32; 3]) -> Vec<[[u32; 3]; 3]> {
    triangles(
        p.positions().expect("positions"),
        p.indices.as_ref().expect("indices"),
        origin,
    )
}

fn collided(c: &Collision, origin: [f32; 3]) -> Vec<[[u32; 3]; 3]> {
    triangles(&c.positions, c.triangles.as_flattened(), origin)
}

/// A primitive's texture.
fn texture<'w>(w: &'w World, p: &Primitive) -> &'w Texture {
    let m = w.get::<PbrMaterial>(p.material.expect("material")).expect("pbr");
    w.get::<Texture>(m.base_color_texture.as_ref().expect("base").texture)
        .expect("texture")
}

#[test]
fn per_node_pieces_are_the_whole_ground_cut_at_its_nodes() {
    let whole_w = lowered(Pieces::Whole);
    let w = lowered(Pieces::PerNode);
    let whole = placed(&whole_w);
    assert_eq!(whole.len(), 1);
    let pieces = placed(&w);
    assert_eq!(pieces.len(), 4);
    // the triangles drawn and collided, to the bit once placed
    let (whole_p, whole_o, whole_c) = &whole[0];
    let want = drawn(whole_p, *whole_o);
    assert_eq!(collided(whole_c, *whole_o), want);
    let mut all: Vec<_> = pieces.iter().flat_map(|(p, o, _)| drawn(p, *o)).collect();
    all.sort_unstable();
    assert_eq!(all, want);
    let mut all_c: Vec<_> = pieces.iter().flat_map(|(_, o, c)| collided(c, *o)).collect();
    all_c.sort_unstable();
    assert_eq!(all_c, want);
    // the textures: 256 pixels a side, the whole's pixels at the same places
    let big = texture(&whole_w, whole_p);
    assert_eq!((big.width, big.height), (512, 512));
    for (p, origin, _) in &pieces {
        let t = texture(&w, p);
        assert_eq!((t.width, t.height), (256, 256));
        // the node's corner: the least x and y of its placed vertices (the world frame keeps x and y)
        let pos = p.positions().expect("positions");
        let least = |k: usize| pos.iter().map(|q| q[k] + origin[k]).fold(f32::INFINITY, f32::min);
        let (nc, nr) = ((least(0) / 64.0) as usize, (least(1) / 64.0) as usize);
        for y in 0..256usize {
            let from = ((nr * 256 + y) * 512 + nc * 256) * 4;
            assert_eq!(
                t.levels[0][y * 256 * 4..(y + 1) * 256 * 4],
                big.levels[0][from..from + 256 * 4]
            );
        }
        let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(0)) else {
            panic!("uvs")
        };
        assert!(uv.iter().flatten().all(|c| (0.0..=1.0).contains(c)));
    }
}
