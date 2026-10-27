//! `clip-alpha-cutouts` through the pass manager with its declaration checks and the conservation diagnostics enforced.
//!
//! The oracle is independent of the pass: textures built from a formula (a disc, a checkerboard of blocks) sampled by
//! the toolkit's bilinear sampler (`sampling::Levels`) at the fragment centres of a fine grid; the clipped quad is
//! rasterised on the same grid by a rasteriser in this file.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::{Conservation, Measure};
use primase_core::pass::{Options, Pass, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, MaterialVariants, Mesh, MeshFile, Name, Primitive, Semantic, Stream, TexFormat,
    TexRole, Texture, Topology, World,
};
use primase_foundations::{sampling::Address, sampling::Levels, sampling::Transfer};
use primase_image::ClipAlphaCutouts;

fn run_passes(w: &mut World, passes: Vec<Box<dyn Pass>>) -> Result<RunReport, String> {
    let mut registry = PassRegistry::new();
    primase_image::register(&mut registry);
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(w, &Pipeline { passes }, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn run(w: &mut World, resolution: u32) -> Result<RunReport, String> {
    run_passes(w, vec![Box::new(ClipAlphaCutouts::new(resolution)?)])
}

/// An RGBA8 texture whose alpha at texel (x, y) is `alpha(x, y)`, with the alphas.
fn texture(w: &mut World, n: usize, alpha: impl Fn(usize, usize) -> u8) -> (Entity, Vec<u8>) {
    let a: Vec<u8> = (0..n * n).map(|i| alpha(i % n, i / n)).collect();
    let e = w.spawn();
    w.insert(
        e,
        Texture {
            width: n as u32,
            height: n as u32,
            format: TexFormat::Rgba8Unorm,
            levels: vec![a.iter().flat_map(|&v| [90, 90, 90, v]).collect()],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    (e, a)
}

fn masked(w: &mut World, tex: Entity, cutoff: f32, factor: f32, wrap: Wrap) -> Entity {
    let m = w.spawn();
    let mut r = TextureRef::new(tex);
    r.wrap = [wrap; 2];
    w.insert(
        m,
        PbrMaterial {
            base_color: [1.0, 1.0, 1.0, factor],
            base_color_texture: Some(r),
            alpha_mode: AlphaMode::Mask { cutoff },
            ..PbrMaterial::default()
        },
    );
    w.insert(m, Name("straw".into()));
    m
}

/// The quad (0, 0) to (1, 1) tilted so z = x / 2, counterclockwise from above, whose coordinates are its x and y;
/// `TexCoord(1)` = (2x, 3y) is a linear function of them, as is the colour.
fn quad(material: Option<Entity>) -> Primitive {
    let p = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(p.iter().map(|p| [p[0], p[1], p[0] / 2.0]).collect()),
            ),
            (Semantic::TexCoord(0), Stream::F32x2(p.to_vec())),
            (
                Semantic::TexCoord(1),
                Stream::F32x2(p.iter().map(|p| [2.0 * p[0], 3.0 * p[1]]).collect()),
            ),
        ]),
        indices: Some(vec![0, 1, 2, 0, 2, 3]),
        material,
    }
}

fn mesh_of(w: &mut World, prims: Vec<Primitive>) -> Entity {
    let e = w.spawn();
    w.insert(e, Mesh::new(prims));
    e
}

fn prim(w: &World, e: Entity) -> &Primitive {
    &w.get::<Mesh>(e).unwrap().primitives()[0]
}

fn f32x2(p: &Primitive, s: Semantic) -> &[[f32; 2]] {
    let Stream::F32x2(v) = &p.streams[&s] else {
        panic!()
    };
    v
}

/// The triangles of a primitive as UV corners.
fn uv_triangles(p: &Primitive) -> Vec<[[f64; 2]; 3]> {
    let uv = f32x2(p, Semantic::TexCoord(0));
    let indices = p.indices.as_ref().unwrap();
    indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| uv[i as usize].map(f64::from)))
        .collect()
}

/// Whether `p` is inside (or on) triangle `t`, either winding.
fn inside(t: &[[f64; 2]; 3], p: [f64; 2]) -> bool {
    let s = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let d = [s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0])];
    d.iter().all(|&d| d >= -1e-12) || d.iter().all(|&d| d <= 1e-12)
}

const GRID: usize = 256;

/// Marks the fragment centres of the fine grid that lie in any triangle.
fn rasterise(tris: &[[[f64; 2]; 3]]) -> Vec<bool> {
    let centre = |i: usize| {
        [
            ((i % GRID) as f64 + 0.5) / GRID as f64,
            ((i / GRID) as f64 + 0.5) / GRID as f64,
        ]
    };
    (0..GRID * GRID)
        .map(|i| tris.iter().any(|t| inside(t, centre(i))))
        .collect()
}

/// What the bilinear sampler says the alpha is at each fragment centre of the fine grid.
fn oracle(n: usize, alpha: &[u8], factor: f32, wrap: Address) -> Vec<f32> {
    let rgba: Vec<u8> = alpha.iter().flat_map(|&a| [0, 0, 0, a]).collect();
    let levels = Levels::one(n as u32, n as u32, rgba).unwrap();
    (0..GRID * GRID)
        .map(|i| {
            let uv = [
                ((i % GRID) as f64 + 0.5) / GRID as f64,
                ((i / GRID) as f64 + 0.5) / GRID as f64,
            ];
            factor * levels.bilinear(0, uv, [wrap; 2], Transfer::Stored).unwrap()[3] as f32
        })
        .collect()
}

/// How a clipped quad differs from the oracle: the fraction of the grid drawn where the field is below the cutoff, the
/// fraction dropped where it is at or above it, and the fraction drawn.
fn compare(w: &World, e: Entity, field: &[f32], cutoff: f32) -> (f64, f64, f64) {
    let drawn = rasterise(&uv_triangles(prim(w, e)));
    let total = (GRID * GRID) as f64;
    let count = |f: &dyn Fn(usize) -> bool| (0..GRID * GRID).filter(|&i| f(i)).count() as f64 / total;
    (
        count(&|i| drawn[i] && field[i] < cutoff),
        count(&|i| !drawn[i] && field[i] >= cutoff),
        count(&|i| drawn[i]),
    )
}

fn disc(n: usize, radius: f64) -> impl Fn(usize, usize) -> u8 {
    move |x, y| {
        let c = n as f64 / 2.0;
        let d = ((x as f64 + 0.5 - c).powi(2) + (y as f64 + 0.5 - c).powi(2)).sqrt();
        if d < radius { 255 } else { 0 }
    }
}

/// A hard-edged disc at the texture's own resolution: the clipped quad draws where the bilinear alpha is at the cutoff
/// or above, to the contour's chords, and its area is the disc's. The material is opaque afterwards.
#[test]
fn a_disc_is_kept_to_the_bilinear_contour_and_its_area() {
    let mut w = World::new();
    let (tex, alpha) = texture(&mut w, 64, disc(64, 24.0));
    let m = masked(&mut w, tex, 0.5, 1.0, Wrap::Clamp);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    let report = run(&mut w, 64).unwrap();
    let (kept_below, removed_above, fraction) = compare(&w, e, &oracle(64, &alpha, 1.0, Address::Clamp), 0.5);
    // a chord is not the hyperbola the bilinear contour is: the fragments between them are few
    assert!(
        kept_below < 0.004 && removed_above < 0.004,
        "{kept_below} {removed_above}"
    );
    let visible = alpha.iter().filter(|&&a| a >= 128).count() as f64 / 4096.0;
    assert!((fraction - visible).abs() < 0.01, "{fraction} against {visible}");
    assert_eq!(w.get::<PbrMaterial>(m).unwrap().alpha_mode, AlphaMode::Opaque);
    assert_eq!(
        report.steps[0].log.counts["material straw: masked made opaque"],
        1
    );
    assert!(w.verify_components().is_empty());
}

/// A coarser grid makes fewer triangles and keeps the disc to within its own bins.
#[test]
fn a_coarser_grid_makes_fewer_triangles() {
    let triangles = |resolution: u32| {
        let mut w = World::new();
        let (tex, _) = texture(&mut w, 128, disc(128, 40.0));
        let m = masked(&mut w, tex, 0.5, 1.0, Wrap::Clamp);
        mesh_of(&mut w, vec![quad(Some(m))]);
        run(&mut w, resolution).unwrap().steps[0].log.counts["triangles after"]
    };
    assert!(
        triangles(16) * 4 < triangles(128),
        "{} and {}",
        triangles(16),
        triangles(128)
    );
}

/// Blocks of four by four texels alternating hidden and visible: a saddle at every block corner, so the convex pieces
/// are cut off squares; the drawn part follows the field, and a block's middle is kept where its neighbour's is not.
#[test]
fn a_checkerboard_keeps_its_visible_blocks() {
    let mut w = World::new();
    let (tex, alpha) = texture(&mut w, 32, |x, y| if (x / 4 + y / 4) % 2 == 0 { 255 } else { 0 });
    let m = masked(&mut w, tex, 0.3, 1.0, Wrap::Clamp);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    run(&mut w, 32).unwrap();
    let field = oracle(32, &alpha, 1.0, Address::Clamp);
    let (kept_below, removed_above, fraction) = compare(&w, e, &field, 0.3);
    let wanted = field.iter().filter(|&&a| a >= 0.3).count() as f64 / (GRID * GRID) as f64;
    assert!((fraction - wanted).abs() < 0.005, "{fraction} against {wanted}");
    assert!(kept_below + removed_above < 0.002, "{kept_below} {removed_above}");
    let drawn = rasterise(&uv_triangles(prim(&w, e)));
    let centre = |bx: usize, by: usize| drawn[((by * 4 + 2) * GRID / 32) * GRID + (bx * 4 + 2) * GRID / 32];
    assert!(centre(0, 0) && !centre(1, 0) && !centre(0, 1) && centre(1, 1));
}

/// Winding is kept, every other stream is the source's function of position, and no two vertices are one point.
#[test]
fn winding_and_every_stream_follow_the_source() {
    let mut w = World::new();
    let (tex, _) = texture(&mut w, 32, disc(32, 12.0));
    let m = masked(&mut w, tex, 0.5, 1.0, Wrap::Clamp);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    let report = run(&mut w, 32).unwrap();
    let p = prim(&w, e);
    for t in uv_triangles(p) {
        let area2 = (t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]);
        assert!(area2 > 0.0, "{t:?}");
    }
    let Stream::F32x3(pos) = &p.streams[&Semantic::Position] else {
        panic!()
    };
    let (uv, uv1) = (f32x2(p, Semantic::TexCoord(0)), f32x2(p, Semantic::TexCoord(1)));
    for i in 0..pos.len() {
        let (x, y) = (uv[i][0], uv[i][1]);
        assert_eq!([pos[i][0], pos[i][1]], [x, y]);
        assert!((pos[i][2] - x / 2.0).abs() < 1e-6, "z {:?}", pos[i]);
        assert!((uv1[i][0] - 2.0 * x).abs() < 1e-5 && (uv1[i][1] - 3.0 * y).abs() < 1e-5);
    }
    let mut seen = std::collections::BTreeSet::new();
    for q in uv {
        assert!(
            seen.insert(((q[0] * 1e5).round() as i64, (q[1] * 1e5).round() as i64)),
            "{q:?} twice"
        );
    }
    let log = &report.steps[0].log;
    assert!(log.counts["vertices after"] > log.counts["vertices before"]);
    assert!(w.verify_components().is_empty());
}

/// A wholly visible quad is unchanged; a wholly hidden one keeps no triangle, and its mesh goes with it; the material's
/// base-colour alpha multiplies the texture's (200 at 0.5 is 100 against a cutoff of 0.5, hidden).
#[test]
fn wholly_visible_is_unchanged_and_wholly_hidden_is_removed() {
    let drawn = |alpha: u8, factor: f32| {
        let mut w = World::new();
        let (tex, _) = texture(&mut w, 8, |_, _| alpha);
        let m = masked(&mut w, tex, 0.5, factor, Wrap::Repeat);
        let e = mesh_of(&mut w, vec![quad(Some(m))]);
        let before = prim(&w, e).clone();
        let report = run(&mut w, 8).unwrap();
        if w.has::<Mesh>(e) {
            assert_eq!(prim(&w, e), &before);
            None
        } else {
            Some(report.steps[0].log.clone())
        }
    };
    assert!(drawn(255, 1.0).is_none() && drawn(200, 1.0).is_none());
    let log = drawn(10, 1.0).unwrap();
    assert_eq!(log.counts["triangles after"], 0);
    assert_eq!(log.counts["meshes left with no primitive, removed"], 1);
    assert_eq!(
        log.drops[&(
            Measure::Primitives,
            "masked primitives the cut-out leaves nothing of".to_owned()
        )],
        1
    );
    assert!(drawn(200, 0.5).is_some());
}

/// `Repeat` reads the texel across the edge, `Clamp` the edge texel: a texture visible in its last column only is
/// drawn a little into the left edge with `Repeat` and not with `Clamp`.
#[test]
fn the_wrap_decides_what_the_edge_nodes_read() {
    let drawn_near_left_edge = |wrap: Wrap| {
        let mut w = World::new();
        let (tex, _) = texture(&mut w, 8, |x, _| if x == 7 { 255 } else { 0 });
        let m = masked(&mut w, tex, 0.3, 1.0, wrap);
        let e = mesh_of(&mut w, vec![quad(Some(m))]);
        run(&mut w, 8).unwrap();
        // between the left edge and column 0's centre the Repeat field falls from column 7's 1 to column 0's 0
        uv_triangles(prim(&w, e)).iter().any(|t| inside(t, [0.01, 0.5]))
    };
    assert!(drawn_near_left_edge(Wrap::Repeat));
    assert!(!drawn_near_left_edge(Wrap::Clamp));
}

/// `Mirror` maps the node beside the edge to the edge node, which is what `Clamp` reads, and the field reads no node
/// further out (the field asserts it): the same texture clips the same under both, even where the edge is visible.
#[test]
fn mirror_reads_the_edge_node_like_clamp() {
    let clipped = |wrap: Wrap| {
        let mut w = World::new();
        let (tex, _) = texture(&mut w, 8, |x, y| if x < 2 || y > 5 { 255 } else { 0 });
        let m = masked(&mut w, tex, 0.3, 1.0, wrap);
        let e = mesh_of(&mut w, vec![quad(Some(m))]);
        run(&mut w, 8).unwrap();
        prim(&w, e).clone()
    };
    assert_eq!(clipped(Wrap::Mirror), clipped(Wrap::Clamp));
}

/// Coordinates beyond 0-1 are refused, naming `split-uv-tiles`; after it the same quad is clipped, each tile alike.
#[test]
fn tiled_coordinates_need_split_uv_tiles_first() {
    let build = |w: &mut World| {
        let (tex, _) = texture(w, 16, disc(16, 5.0));
        let m = masked(w, tex, 0.5, 1.0, Wrap::Repeat);
        let mut p = quad(Some(m));
        p.streams.insert(
            Semantic::TexCoord(0),
            Stream::F32x2(vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]]),
        );
        (m, mesh_of(w, vec![p]))
    };
    let mut w = World::new();
    build(&mut w);
    assert!(run(&mut w, 16).unwrap_err().contains("split-uv-tiles first"));

    let mut w = World::new();
    let (m, e) = build(&mut w);
    let o: Options = [("set".to_owned(), "0".to_owned())].into();
    let split = primase_meshops::SPLIT_UV_TILES.instance(&o).unwrap();
    run_passes(&mut w, vec![split, Box::new(ClipAlphaCutouts::new(16).unwrap())]).unwrap();
    assert_eq!(w.get::<PbrMaterial>(m).unwrap().alpha_mode, AlphaMode::Opaque);
    // the disc is in each of the two tiles: the middle of both is kept, the corners are not
    let tris = uv_triangles(prim(&w, e));
    let at = |u: f64, v: f64| tris.iter().any(|t| inside(t, [u, v]));
    assert!(at(0.25, 0.5) && at(0.75, 0.5) && !at(0.02, 0.02));
}

/// Everything the pass refuses it names, and it changes nothing when it does.
#[test]
fn refusals_name_the_cause_and_leave_the_world_as_it_was() {
    let refuse = |build: &dyn Fn(&mut World) -> Entity, needle: &str| {
        let mut w = World::new();
        let e = build(&mut w);
        let before = prim(&w, e).clone();
        let err = run(&mut w, 16).unwrap_err();
        assert!(err.contains(needle), "{needle:?} not in {err:?}");
        assert_eq!(prim(&w, e), &before, "{needle}");
    };
    refuse(
        &|w| {
            let (tex, _) = texture(w, 8, |_, _| 255);
            let m = masked(w, tex, 0.5, 1.0, Wrap::Repeat);
            let mut mat = w.get::<PbrMaterial>(m).unwrap().clone();
            mat.base_color_texture.as_mut().unwrap().transform = Some(UvTransform {
                scale: [2.0, 2.0],
                ..UvTransform::IDENTITY
            });
            w.insert(m, mat);
            mesh_of(w, vec![quad(Some(m))])
        },
        "bake-texture-transforms first",
    );
    refuse(
        &|w| {
            let tex = w.spawn();
            let m = masked(w, tex, 0.5, 1.0, Wrap::Repeat);
            mesh_of(w, vec![quad(Some(m))])
        },
        "has no pixels",
    );
    refuse(
        &|w| {
            let (tex, _) = texture(w, 8, |_, _| 255);
            let m = masked(w, tex, 0.5, 1.0, Wrap::Repeat);
            let mut p = quad(Some(m));
            p.topology = Topology::Lines;
            mesh_of(w, vec![p])
        },
        "not triangles",
    );
    refuse(
        &|w| {
            let (tex, _) = texture(w, 8, |_, _| 255);
            let mut t = w.get::<Texture>(tex).unwrap().clone();
            t.format = TexFormat::Rgba16Unorm;
            t.levels = vec![vec![0; 8 * 8 * 8]];
            w.insert(tex, t);
            let m = masked(w, tex, 0.5, 1.0, Wrap::Repeat);
            mesh_of(w, vec![quad(Some(m))])
        },
        "not exact in 8-bit RGBA",
    );
    refuse(
        &|w| {
            let m = w.spawn();
            let mat = PbrMaterial {
                base_color: [1.0, 1.0, 1.0, 0.2],
                alpha_mode: AlphaMode::Mask { cutoff: 0.5 },
                ..PbrMaterial::default()
            };
            w.insert(m, mat);
            mesh_of(w, vec![quad(Some(m))])
        },
        "draws nothing",
    );
    refuse(
        &|w| {
            let (tex, _) = texture(w, 8, |_, _| 255);
            let a = masked(w, tex, 0.5, 1.0, Wrap::Repeat);
            let b = masked(w, tex, 0.3, 1.0, Wrap::Repeat);
            let e = mesh_of(w, vec![quad(Some(a))]);
            w.insert(e, MaterialVariants(vec![BTreeMap::from([(1u32, b)])]));
            e
        },
        "not masked alike",
    );
    let mut w = World::new();
    let opaque = w.spawn();
    w.insert(opaque, PbrMaterial::default());
    mesh_of(&mut w, vec![quad(Some(opaque))]);
    let e = w.spawn();
    w.insert(
        e,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![None],
        },
    );
    let err = run(&mut w, 16).unwrap_err();
    assert!(err.contains("still in its file"), "{err}");
}

/// Primitives with other materials are left, masked materials nothing draws are counted, and the option is checked.
#[test]
fn other_materials_are_left_and_the_option_is_checked() {
    let mut w = World::new();
    let (tex, _) = texture(&mut w, 16, disc(16, 5.0));
    let drawn = masked(&mut w, tex, 0.5, 1.0, Wrap::Clamp);
    let unused = masked(&mut w, tex, 0.25, 1.0, Wrap::Clamp);
    let opaque = w.spawn();
    w.insert(opaque, PbrMaterial::default());
    let e = mesh_of(&mut w, vec![quad(Some(opaque)), quad(Some(drawn)), quad(None)]);
    let report = run(&mut w, 16).unwrap();
    let prims = w.get::<Mesh>(e).unwrap().primitives();
    assert_eq!((&prims[0], &prims[2]), (&quad(Some(opaque)), &quad(None)));
    assert_ne!(prims[1].indices, quad(None).indices);
    assert_eq!(
        report.steps[0].log.counts["masked materials no primitive draws, left as they are"],
        1
    );
    assert!(matches!(
        w.get::<PbrMaterial>(unused).unwrap().alpha_mode,
        AlphaMode::Mask { .. }
    ));

    let make = |o: &[(&str, &str)]| {
        let o: Options = o
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        primase_image::CLIP_ALPHA_CUTOUTS.instance(&o)
    };
    assert_eq!(
        make(&[("resolution", "64")]).unwrap().options(),
        ClipAlphaCutouts::new(64).unwrap().options()
    );
    assert!(make(&[]).err().unwrap().contains("resolution"));
    assert!(
        make(&[("resolution", "x")])
            .err()
            .unwrap()
            .contains("resolution=x")
    );
    assert!(make(&[("resolution", "1")]).err().unwrap().contains("at least 2"));
    assert!(
        make(&[("resolution", "64"), ("size", "1")])
            .err()
            .unwrap()
            .contains("size")
    );
}
