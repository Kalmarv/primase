//! `lower-anvil-terrain{pieces=per-node}` against `pieces=whole` on a hand-made terrain: a level-2 quadtree of 64 m
//! nodes, the 128 m square at its corner drawn at 0.5 m samples (the nodes on its far edges read too, as the reader
//! reads the nodes meeting a rectangle), its near colour baked at 0.25 m: four pieces, each texture 256 pixels, their
//! triangles the whole's to the bit, their pixels the whole's at the same places.
use primase_anvil::components::{AnvilTerrain, TerrainTile};
use primase_anvil::passes::{CellPoint, GroundColour, LowerAnvilTerrain, Pieces, Splat, SplatUv};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    ColorSpace, Entity, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, World,
};

/// The world with the terrain: heights varying over every sample, colour tiles of one colour per node (BC1 blocks of
/// one endpoint), a flat noise.
fn world() -> (World, Entity) {
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
    (w, e)
}

fn lowered(pieces: Pieces) -> (World, Entity) {
    let (mut w, e) = world();
    let pass = LowerAnvilTerrain {
        rect: [0.0, 0.0, 128.0, 128.0],
        step: 1,
        range: 1.0,
        offset: 0.0,
        sample: CellPoint::Corner,
        colour: ColorSpace::Srgb,
        collision: None,
        surface: String::new(),
        ground: GroundColour::Splat(Splat {
            k: 8.0,
            pixel: 0.25,
            uv: SplatUv::Factor(1.0),
            noise_srgb: false,
            occlusion: false,
        }),
        pieces,
    };
    pass.run(&mut w, &mut PassLog::default()).expect("lower");
    (w, e)
}

/// Each triangle of `p` as its three positions' bits.
fn triangles(p: &Primitive) -> Vec<[[u32; 3]; 3]> {
    let pos = p.positions().expect("positions");
    p.indices
        .as_ref()
        .expect("indices")
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| pos[i as usize].map(f32::to_bits)))
        .collect()
}

/// A primitive's texture.
fn texture<'w>(w: &'w World, p: &Primitive) -> &'w Texture {
    let m = w.get::<PbrMaterial>(p.material.expect("material")).expect("pbr");
    w.get::<Texture>(m.base_color_texture.as_ref().expect("base").texture)
        .expect("texture")
}

#[test]
fn per_node_pieces_are_the_whole_ground_cut_at_its_nodes() {
    let (whole_w, whole_e) = lowered(Pieces::Whole);
    let (w, e) = lowered(Pieces::PerNode);
    let whole = &whole_w.get::<Mesh>(whole_e).expect("mesh").primitives()[0];
    let pieces = w.get::<Mesh>(e).expect("mesh").primitives();
    assert_eq!(pieces.len(), 4);
    // the triangles, to the bit
    let mut all: Vec<_> = pieces.iter().flat_map(triangles).collect();
    let mut want = triangles(whole);
    all.sort_unstable();
    want.sort_unstable();
    assert_eq!(all, want);
    // the textures: 256 pixels a side, the whole's pixels at the same places
    let big = texture(&whole_w, whole);
    assert_eq!((big.width, big.height), (512, 512));
    for (k, p) in pieces.iter().enumerate() {
        let t = texture(&w, p);
        assert_eq!((t.width, t.height), (256, 256));
        let (nr, nc) = (k / 2, k % 2);
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
