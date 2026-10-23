//! `lower-anvil-terrain{steep=chart, steep-threshold=T}` on a hand-made ramp: one 64 m node at 0.5 m samples, flat for
//! 20 m, then rising 3 m a sample (6 : 1, 80.5 degrees) for 20 m, then flat at 120 m. The ramp's faces look along -x:
//! they leave the top-down primitive for a chart in the (y, z) plane, at the top-down bake's pixel along the surface
//! (against 1.5 m a pixel in the top-down texture), baked with the vertical program's colour (the detail at the point's
//! (y, z), its mix weight 1 on a face looking along x); the collision is the whole ground's either way.
use primase_anvil::components::{AnvilTerrain, TerrainDetail, TerrainTile};
use primase_anvil::passes::{
    CellPoint, GroundColour, GroundNormals, LOWER_ANVIL_TERRAIN, LowerAnvilTerrain, Pieces, Splat, SplatUv,
    Steep, SurfaceBy,
};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Collision, ColorSpace, Entity, Instance, Mesh, Primitive, Semantic, Stream, TexFormat, TexRole, Texture,
    Transform, World,
};
use primase_image::srgb_to_linear;

const PIXEL: f64 = 0.25;
/// The detail's uv scale: a 4 x 4 texture repeating every metre of its coordinates.
const SCALE: f32 = 0.25;

/// The height of sample column `j`, metres.
fn ramp(j: i64) -> i32 {
    match j {
        ..=40 => 0,
        41..=80 => 3 * (j as i32 - 40),
        _ => 120,
    }
}

/// A 4 x 4 detail whose red is 60 x its row and green 60 x its column (linear: no sRGB flag), blue 0.
fn detail_texture() -> Texture {
    let mut rgba = Vec::new();
    for row in 0..4u8 {
        for column in 0..4u8 {
            rgba.extend([60 * row, 60 * column, 0, 255]);
        }
    }
    Texture {
        width: 4,
        height: 4,
        format: TexFormat::Rgba8Unorm,
        levels: vec![rgba],
        color_space: ColorSpace::Linear,
        role: TexRole::Other,
        normal: None,
    }
}

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
    let diffuse = w.spawn();
    w.insert(diffuse, detail_texture());
    let heights = (0..132 * 132)
        .map(|k| ramp((k % 132) as i64 - TerrainTile::CORE_AT as i64))
        .collect();
    // one RGB565 colour: both endpoints, every index 0
    let c = 0x07e0u16.to_le_bytes();
    let tile = TerrainTile {
        row: 0,
        column: 0,
        heights,
        colour: [c[0], c[1], c[0], c[1], 0, 0, 0, 0].repeat(33 * 33),
        material: vec![0; 132 * 132],
        block_8: vec![0; 33 * 33 * 8],
    };
    let e = w.spawn();
    w.insert(
        e,
        AnvilTerrain {
            frame: SourceFrame::WORLD,
            level: 2,
            origin: [0.0, 0.0],
            side: 256.0,
            tiles: vec![tile],
            materials: vec![None],
            details: vec![Some(TerrainDetail {
                uv_scale: SCALE,
                f32_6: 0.0,
                diffuse: Some(diffuse),
                normal: None,
            })],
            noise: Some(noise),
        },
    );
    w
}

fn pass(steep: Steep) -> LowerAnvilTerrain {
    LowerAnvilTerrain {
        rect: [0.0, 0.0, 63.5, 63.5],
        step: 1,
        // one height value is one metre: range x 2^(level - 28) with the quadtree at level 2
        range: 67_108_864.0,
        offset: 0.0,
        sample: CellPoint::Corner,
        colour: ColorSpace::Srgb,
        collision: Some(SurfaceBy::One),
        surface: "ground".to_owned(),
        ground: GroundColour::Splat(Splat {
            k: 8.0,
            pixel: PIXEL,
            uv: SplatUv::Factor(1.0),
            noise_srgb: false,
            occlusion: false,
        }),
        pieces: Pieces::PerNode,
        normals: GroundNormals::None,
        steep,
    }
}

fn lowered(steep: Steep) -> (World, PassLog) {
    let mut w = world();
    let mut log = PassLog::default();
    pass(steep).run(&mut w, &mut log).expect("lower");
    (w, log)
}

/// The one placed piece: its mesh, its collision and its placement's translation.
fn piece(w: &World) -> (Mesh, Collision, [f32; 3]) {
    let placed: Vec<_> = w.iter::<Instance>().collect();
    assert_eq!(placed.len(), 1, "one node, one piece");
    let (e, i) = placed[0];
    let t = w.get::<Transform>(e).expect("transform");
    (
        w.get::<Mesh>(i.prototype).expect("mesh").clone(),
        w.get::<Collision>(i.prototype).expect("collision").clone(),
        t.cols[3],
    )
}

fn texture<'w>(w: &'w World, p: &Primitive) -> &'w Texture {
    let m = p.material.expect("material");
    let pbr = w.get::<primase_core::pbr::PbrMaterial>(m).expect("pbr");
    let e: Entity = pbr.base_color_texture.as_ref().expect("texture").texture;
    w.get::<Texture>(e).expect("texture")
}

fn positions(p: &Primitive) -> &[[f32; 3]] {
    p.positions().expect("positions")
}

fn uvs(p: &Primitive) -> &[[f32; 2]] {
    match p.streams.get(&Semantic::TexCoord(0)) {
        Some(Stream::F32x2(v)) => v,
        _ => panic!("no TexCoord(0)"),
    }
}

fn triangle_count(p: &Primitive) -> usize {
    p.indices.as_ref().expect("indices").len() / 3
}

/// The steep squares: columns 40 to 79 of the 127 columns of squares, every one of the 127 rows, two triangles each.
const STEEP_TRIANGLES: usize = 40 * 127 * 2;

#[test]
fn the_steep_faces_leave_the_top_down_primitive_for_a_chart_at_the_surface_density() {
    let (w0, _) = lowered(Steep::None);
    let (m0, c0, _) = piece(&w0);
    assert_eq!(m0.primitives().len(), 1);
    let whole = triangle_count(&m0.primitives()[0]);
    assert_eq!(whole, 127 * 127 * 2);
    let (w1, log) = lowered(Steep::Chart {
        threshold: 60.0,
        max_atlas: 4096,
    });
    let (m1, c1, _) = piece(&w1);
    // the collision is the whole ground's, to the bit
    assert_eq!(c1.triangles, c0.triangles);
    assert_eq!(c1.positions, c0.positions);
    // the draw: the flat remainder and the chart, each triangle in exactly one
    assert_eq!(m1.primitives().len(), 2);
    let (flat, chart) = (&m1.primitives()[0], &m1.primitives()[1]);
    assert_eq!(triangle_count(chart), STEEP_TRIANGLES);
    assert_eq!(triangle_count(flat), whole - STEEP_TRIANGLES);
    assert_eq!(
        log.counts["steep ground triangles drawn from charts"],
        STEEP_TRIANGLES as u64
    );
    assert_eq!(log.counts["ground charts"], 1);
    assert_eq!(log.counts["ground chart atlases"], 1);
    // every flat vertex is used (the vertices of the steep triangles alone are not carried)
    let used: std::collections::BTreeSet<u32> =
        flat.indices.as_ref().expect("indices").iter().copied().collect();
    assert_eq!(used.len(), positions(flat).len());
    // the chart's atlas: the (y, z) plane, 63.5 m by 120 m at 0.25 m, two pixels of padding a side, one chart
    let atlas = texture(&w1, chart);
    assert_eq!((atlas.width, atlas.height), (512, 484));
    // the surface length of one chart pixel along the fall line, against the top-down texture's
    let along = |p: &Primitive, size: &Texture, from: usize, to: usize, axis: usize| {
        let (a, b) = (positions(p)[from], positions(p)[to]);
        let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        let (ua, ub) = (uvs(p)[from], uvs(p)[to]);
        let pixels = ((ua[axis] - ub[axis]) * if axis == 0 { size.width } else { size.height } as f32).abs();
        f64::from(d) / f64::from(pixels)
    };
    // two chart vertices one sample apart in height (same y, 3 m apart in z), found by position
    let at = |p: &Primitive, x: f32, y: f32| {
        positions(p)
            .iter()
            .position(|q| (q[0] - x).abs() < 1e-4 && (q[1] - y).abs() < 1e-4)
            .expect("vertex")
    };
    // the piece's origin is the grid's least corner rounded down to 64 m: 0
    let (lo, hi) = (at(chart, 20.5, 10.0), at(chart, 21.0, 10.0));
    let chart_surface_per_pixel = along(chart, atlas, lo, hi, 1);
    assert!(
        (chart_surface_per_pixel - 0.25 * (1.0 + (0.5f64 / 3.0).powi(2)).sqrt()).abs() < 2e-3,
        "{chart_surface_per_pixel}"
    );
    // the same two vertices in the top-down texture (steep=none): a whole sample of 0.5 m in x, 2 pixels
    let top = &m0.primitives()[0];
    let top_tex = texture(&w0, top);
    let top_surface_per_pixel = along(top, top_tex, at(top, 20.5, 10.0), at(top, 21.0, 10.0), 0);
    assert!(top_surface_per_pixel > 1.4, "{top_surface_per_pixel}");
    assert!(chart_surface_per_pixel * 5.0 < top_surface_per_pixel);
}

#[test]
fn a_chart_texel_is_baked_in_the_plane_of_its_face() {
    let (w, _) = lowered(Steep::Chart {
        threshold: 60.0,
        max_atlas: 4096,
    });
    let (m, _, _) = piece(&w);
    let chart = &m.primitives()[1];
    let atlas = texture(&w, chart);
    // the uv of two chart vertices fix the atlas's linear map to (y, z)
    let find = |y: f32, z: f32| {
        positions(chart)
            .iter()
            .position(|q| (q[1] - y).abs() < 1e-4 && (q[2] - z).abs() < 1e-4)
            .expect("vertex")
    };
    let (a, b) = (find(8.0, 60.0), find(40.0, 90.0));
    let (ua, ub) = (uvs(chart)[a], uvs(chart)[b]);
    let per_y = (ub[0] - ua[0]) / (40.0 - 8.0);
    let per_z = (ub[1] - ua[1]) / (90.0 - 60.0);
    // a texel (i, j): its centre's (y, z)
    let (i, j) = (200u32, 200u32);
    let (cu, cv) = (
        (i as f32 + 0.5) / atlas.width as f32,
        (j as f32 + 0.5) / atlas.height as f32,
    );
    let y = f64::from(8.0 + (cu - ua[0]) / per_y);
    let z = f64::from(60.0 + (cv - ua[1]) / per_z);
    // red is 60 x the detail's row, green 60 x its column, between texel centres linear in the coordinate; the
    // detail repeats every 4 m of (y, z) at 4 texels, so a metre is a texel: row = z, column = y (mod 4)
    let (row, column) = (z.rem_euclid(4.0), y.rem_euclid(4.0));
    assert!(
        (0.5..3.5).contains(&row) && (0.5..3.5).contains(&column),
        "{y} {z}"
    );
    let linear = |c: u8| srgb_to_linear(f64::from(c) / 255.0);
    let at = ((j * atlas.width + i) * 4) as usize;
    let got = &atlas.levels[0][at..at + 4];
    let (red, green) = (linear(got[0]), linear(got[1]));
    let (want_red, want_green) = (60.0 * (row - 0.5) / 255.0, 60.0 * (column - 0.5) / 255.0);
    assert!(
        (red - want_red).abs() < 0.01,
        "red {red} against {want_red} at y {y} z {z}"
    );
    assert!(
        (green - want_green).abs() < 0.01,
        "green {green} against {want_green} at y {y} z {z}"
    );
    // (the top-down bake would have had red follow y, its row axis: the plane is the chart's)
    assert_eq!(got[3], 255);
}

#[test]
fn a_ground_with_nothing_steeper_than_the_threshold_has_no_charts() {
    let (w, log) = lowered(Steep::Chart {
        threshold: 85.0,
        max_atlas: 4096,
    });
    let (m, _, _) = piece(&w);
    assert_eq!(m.primitives().len(), 1);
    assert_eq!(log.counts["ground charts"], 0);
    assert_eq!(log.counts["steep ground triangles drawn from charts"], 0);
}

#[test]
fn an_atlas_past_the_stated_cap_is_baked_at_a_coarser_pixel_and_counted() {
    // the chart is 258 x 484 pixels at 0.25 m: past a cap of 256 it is halved once (131 x 244)
    let (w, log) = lowered(Steep::Chart {
        threshold: 60.0,
        max_atlas: 256,
    });
    let (m, _, _) = piece(&w);
    let atlas = texture(&w, &m.primitives()[1]);
    assert_eq!((atlas.width, atlas.height), (256, 244));
    let (key, n) = log
        .approximations
        .iter()
        .find(|(k, _)| k.starts_with("chart atlases built at a coarser pixel"))
        .expect("the halving is counted");
    assert!(key.contains("steep-max-atlas=256"), "{key}");
    assert_eq!(*n, 1);
    // at a cap it fits, nothing is halved
    let (_, log) = lowered(Steep::Chart {
        threshold: 60.0,
        max_atlas: 512,
    });
    assert!(
        !log.approximations
            .keys()
            .any(|k| k.starts_with("chart atlases built at a coarser pixel"))
    );
}

#[test]
fn the_options_state_the_threshold_and_refuse_what_cannot_chart() {
    let base: Vec<(String, String)> = pass(Steep::None)
        .options()
        .into_iter()
        .filter(|(k, _)| !k.starts_with("steep"))
        .collect();
    let with = |extra: &[(&str, &str)]| {
        let mut o: Options = base.iter().cloned().collect();
        o.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        LOWER_ANVIL_TERRAIN.instance(&o)
    };
    assert!(with(&[]).err().unwrap().contains("steep is required"));
    assert_eq!(
        with(&[("steep", "none")]).unwrap().options(),
        pass(Steep::None).options()
    );
    for k in [("steep-threshold", "60"), ("steep-max-atlas", "4096")] {
        assert!(with(&[("steep", "none"), k]).is_err(), "{k:?}");
    }
    let chart = |extra: &[(&str, &str)]| {
        let mut all = vec![("steep", "chart")];
        all.extend_from_slice(extra);
        with(&all)
    };
    assert_eq!(
        chart(&[("steep-threshold", "60"), ("steep-max-atlas", "4096")])
            .unwrap()
            .options(),
        pass(Steep::Chart {
            threshold: 60.0,
            max_atlas: 4096
        })
        .options()
    );
    // each of the two numbers is needed with chart
    assert!(
        chart(&[("steep-max-atlas", "4096")])
            .err()
            .unwrap()
            .contains("steep-threshold")
    );
    assert!(
        chart(&[("steep-threshold", "60")])
            .err()
            .unwrap()
            .contains("steep-max-atlas")
    );
    for bad in ["0", "90", "-5", "x"] {
        assert!(
            chart(&[("steep-threshold", bad), ("steep-max-atlas", "4096")]).is_err(),
            "{bad}"
        );
    }
    for bad in ["0", "8", "100000", "-1", "x", "1.5"] {
        assert!(
            chart(&[("steep-threshold", "60"), ("steep-max-atlas", bad)]).is_err(),
            "{bad}"
        );
    }
    assert!(with(&[("steep", "sometimes")]).is_err());
    // charts need a per-node splat
    let whole: Vec<(&str, &str)> = vec![
        ("steep", "chart"),
        ("steep-threshold", "60"),
        ("steep-max-atlas", "4096"),
        ("pieces", "whole"),
    ];
    let mut o: Options = base.iter().filter(|(k, _)| k != "pieces").cloned().collect();
    o.extend(whole.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
    assert!(
        LOWER_ANVIL_TERRAIN
            .instance(&o)
            .err()
            .unwrap()
            .contains("pieces=per-node")
    );
    // the options round-trip
    let p = pass(Steep::Chart {
        threshold: 55.0,
        max_atlas: 1024,
    });
    assert_eq!(
        LOWER_ANVIL_TERRAIN.instance(&p.options()).unwrap().options(),
        p.options()
    );
}

/// The splat bake on the world's stated threads (`core.threads`, its rows shaded in parallel) makes the serial
/// ground: every texture byte for byte, the same pieces and log, charts or none.
#[test]
fn the_bake_on_any_thread_count_is_the_serial_one() {
    use primase_core::parallel::Threads;
    for steep in [
        Steep::None,
        Steep::Chart {
            threshold: 45.0,
            max_atlas: 4096,
        },
    ] {
        let (serial, log) = lowered(steep);
        let textures = |w: &World| -> Vec<(Entity, Texture)> {
            w.iter::<Texture>().map(|(e, t)| (e, t.clone())).collect()
        };
        for n in [2, 7] {
            let mut w = world();
            w.insert_resource(Threads::new(n).unwrap());
            let mut l = PassLog::default();
            pass(steep).run(&mut w, &mut l).expect("lower");
            assert_eq!(textures(&w), textures(&serial), "{steep:?} on {n} threads");
            assert_eq!(
                piece(&w).0,
                piece(&serial).0,
                "{steep:?} on {n} threads: the mesh"
            );
            assert_eq!(l, log, "{steep:?} on {n} threads: the log");
        }
    }
}
