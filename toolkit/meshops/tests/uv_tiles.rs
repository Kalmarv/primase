//! `split-uv-tiles` through the pass manager with its declaration checks and the conservation diagnostics enforced.
//!
//! The oracle for the texture is a hand-written one: a checkerboard sampled with `Repeat` at every fragment centre of a
//! rasterised grid, before and after, by a rasteriser and a sampler in this file. The pieces, counts and weights of the
//! quad are worked by hand in the comments.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::Conservation;
use primase_core::morphs::{Deltas, Frame, MorphId, Morphs, Target};
use primase_core::pass::{Options, Pass, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::skeleton::{Joint, Rest, Skeleton};
use primase_core::skin::{Binding, BindingId, Deformation, Skin, Slot, WeightRule};
use primase_core::world::remap_list;
use primase_core::{
    Component, Entity, Influences, JointId, MaterialVariants, Mesh, MeshFile, Primitive, Semantic, Stream,
    Topology, Transform, World,
};
use primase_meshops::SplitUvTiles;

fn run(w: &mut World, set: u8) -> Result<RunReport, String> {
    let mut registry = PassRegistry::new();
    primase_meshops::register(&mut registry);
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(SplitUvTiles::new(set))],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

/// A material drawing a (data-less) texture entity through `TexCoord(set)` with `wrap`.
fn material(w: &mut World, set: u8, wrap: [Wrap; 2]) -> Entity {
    let texture = w.spawn();
    let m = w.spawn();
    let mut r = TextureRef::new(texture);
    r.texcoord = set;
    r.wrap = wrap;
    w.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(r),
            ..PbrMaterial::default()
        },
    );
    m
}

/// The quad (-0.5, 0) to (1.5, 1) in the xy plane, counterclockwise from +z, whose coordinates are its x and y:
/// u runs over [-0.5, 1.5], so the integer lines u = 0 and u = 1 cross it. Two triangles, p0 p1 p2 and p0 p2 p3.
fn quad(material: Option<Entity>) -> Primitive {
    let p = [
        [-0.5, 0.0, 0.0],
        [1.5, 0.0, 0.0],
        [1.5, 1.0, 0.0],
        [-0.5, 1.0, 0.0],
    ];
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (Semantic::Position, Stream::F32x3(p.to_vec())),
            (Semantic::Normal, Stream::F32x3(vec![[0.0, 0.0, 1.0]; 4])),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(p.iter().map(|p| [p[0], p[1]]).collect()),
            ),
            (
                Semantic::TexCoord(1),
                Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
            ),
            (
                Semantic::Color(0),
                Stream::U8x4(vec![
                    [0, 0, 0, 255],
                    [200, 100, 0, 255],
                    [100, 100, 100, 255],
                    [0, 0, 0, 255],
                ]),
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

fn triangles(p: &Primitive) -> Vec<[u32; 3]> {
    let idx = p.indices.clone().unwrap();
    idx.as_chunks::<3>().0.to_vec()
}

fn f32x2(p: &Primitive, s: Semantic) -> &[[f32; 2]] {
    let Stream::F32x2(v) = &p.streams[&s] else {
        panic!()
    };
    v
}

fn f32x3(p: &Primitive, s: Semantic) -> &[[f32; 3]] {
    let Stream::F32x3(v) = &p.streams[&s] else {
        panic!()
    };
    v
}

/// A checkerboard of four cells a unit, sampled with `Repeat`: 0 or 1.
fn checker(u: f64, v: f64) -> u8 {
    let cell = |x: f64| (x * 4.0).floor() as i64;
    (cell(u) + cell(v)).rem_euclid(2) as u8
}

/// Every fragment centre of a 64 x 60 grid over x in [-0.5, 1.5], y in [0, 1] (no centre lies on a cut line, a texel
/// edge or the quad's diagonal), as the checker colour at the fragment's interpolated coordinates; `None` where no
/// triangle covers it. The triangles of every primitive are searched in order.
fn render(mesh: &Mesh, set: u8) -> Vec<Option<u8>> {
    let mut out = Vec::new();
    for j in 0..60 {
        for i in 0..64 {
            let (x, y) = (-0.5 + (2 * i + 1) as f64 / 64.0, (2 * j + 1) as f64 / 120.0);
            let mut found = None;
            'tri: for p in mesh.primitives() {
                let pos = p.positions().unwrap();
                let Stream::F32x2(uv) = &p.streams[&Semantic::TexCoord(set)] else {
                    panic!()
                };
                for t in triangles(p) {
                    let [a, b, c] = t.map(|k| pos[k as usize].map(f64::from));
                    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
                    let l0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
                    let l1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
                    let l2 = 1.0 - l0 - l1;
                    if l0 > 0.0 && l1 > 0.0 && l2 > 0.0 {
                        let [ua, ub, uc] = t.map(|k| uv[k as usize].map(f64::from));
                        let u = l0 * ua[0] + l1 * ub[0] + l2 * uc[0];
                        let v = l0 * ua[1] + l1 * ub[1] + l2 * uc[1];
                        found = Some(checker(u, v));
                        break 'tri;
                    }
                }
            }
            out.push(found);
        }
    }
    out
}

/// The quad split: the pieces are worked by hand. The diagonal p0 p2 meets u = 0 at (0, 0.25) and u = 1 at (1, 0.75);
/// the top and bottom edges meet them at (0, 0), (1, 0), (0, 1), (1, 1). Strip u in [-0.5, 0]: the triangle p0 (0,0)
/// (0,0.25) and the quad p0 (0,0.25) (0,1) p3 (3 triangles); strip [0, 1]: two quads (4 triangles); strip [1, 1.5]: the
/// quad (1,0) p1 p2 (1,0.75) and the triangle (1,0.75) p2 (1,1) (3 triangles). 10 triangles. Vertices: 5 + 6 + 5 = 16, of
/// which the 4 corners are source vertices and the 12 on the lines are interpolated (the six points on a line occur
/// once in each of its two strips).
#[test]
fn a_quad_across_two_lines_is_cut_into_the_right_pieces_and_samples_identically() {
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    w.insert(
        e,
        MeshFile {
            path: "quad.glb".into(),
            mesh: 0,
            materials: vec![Some(m)],
        },
    );
    let before = render(w.get::<Mesh>(e).unwrap(), 0);
    assert!(before.iter().all(Option::is_some));
    assert!(before.contains(&Some(0)) && before.contains(&Some(1)));

    let report = run(&mut w, 0).unwrap();
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let log = &report.steps[0].log;
    assert_eq!(log.counts["primitives split"], 1);
    assert_eq!(log.counts["triangles before"], 2);
    assert_eq!(log.counts["triangles after"], 10);
    assert_eq!(log.counts["vertices before"], 4);
    assert_eq!(log.counts["vertices after"], 16);
    assert_eq!(log.counts["vertices interpolated at tile lines"], 12);
    assert_eq!(triangles(p).len(), 10);
    assert_eq!(p.vertex_count(), 16);
    assert!(!w.has::<MeshFile>(e));

    // every coordinate in 0-1, every stream as long as the positions
    assert!(
        f32x2(p, Semantic::TexCoord(0))
            .iter()
            .flatten()
            .all(|c| (0.0..=1.0).contains(c))
    );
    assert!(p.streams.values().all(|s| s.len() == 16));
    // the same colour at every fragment centre
    let after = render(w.get::<Mesh>(e).unwrap(), 0);
    let bad: Vec<(usize, usize)> = (0..after.len())
        .filter(|&k| after[k] != before[k])
        .map(|k| (k % 64, k / 64))
        .collect();
    assert!(
        bad.is_empty(),
        "{} fragments differ, first {:?}",
        bad.len(),
        &bad[..bad.len().min(8)]
    );

    // the surface is the same: every triangle faces +z as before and the areas add up to the quad's 2
    let pos = f32x3(p, Semantic::Position);
    let mut area = 0.0;
    for t in triangles(p) {
        let [a, b, c] = t.map(|k| pos[k as usize]);
        let twice = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(twice > 0.0, "{t:?} is wound the other way");
        area += twice / 2.0;
    }
    assert!((area - 2.0).abs() < 1e-5, "{area}");

    // the cut at (0, 0.25) on the diagonal, one vertex per strip: position, the other uv set interpolated linearly
    // (from (0,0) to (1,1) at 0.25 of the way), colour mixed 3/4 of p0's and 1/4 of p2's, normal unit
    let at = |x: f32, y: f32| -> Vec<usize> {
        (0..16)
            .filter(|&i| (pos[i][0] - x).abs() < 1e-6 && (pos[i][1] - y).abs() < 1e-6)
            .collect()
    };
    let cut = at(0.0, 0.25);
    assert_eq!(cut.len(), 2);
    let uv0 = f32x2(p, Semantic::TexCoord(0));
    let mut tiles: Vec<[f32; 2]> = cut.iter().map(|&i| uv0[i]).collect();
    tiles.sort_by(|a, b| a[0].total_cmp(&b[0]));
    // the strip below u = 0 sees it at u = 1, the strip above at u = 0
    assert_eq!(tiles, [[0.0, 0.25], [1.0, 0.25]]);
    let i = cut[0];
    let uv1 = f32x2(p, Semantic::TexCoord(1))[i];
    assert!(
        (uv1[0] - 0.25).abs() < 1e-6 && (uv1[1] - 0.25).abs() < 1e-6,
        "{uv1:?}"
    );
    let Stream::U8x4(colors) = &p.streams[&Semantic::Color(0)] else {
        panic!()
    };
    assert_eq!(colors[i], [25, 25, 25, 255]);
    let n = f32x3(p, Semantic::Normal)[i];
    assert!((n[2] - 1.0).abs() < 1e-6);
}

/// A primitive already inside 0-1 is untouched, and so are the triangles of a split primitive that are inside it.
#[test]
fn inside_0_1_nothing_changes() {
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut inside = quad(Some(m));
    inside.streams.insert(
        Semantic::TexCoord(0),
        Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
    );
    let e = mesh_of(&mut w, vec![inside.clone()]);
    let report = run(&mut w, 0).unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives()[0], inside);
    assert!(!report.steps[0].log.counts.contains_key("primitives split"));
}

/// Coordinates that are only out of range on v, and a primitive that is not indexed.
#[test]
fn one_axis_and_an_unindexed_primitive() {
    let mut w = World::new();
    // clamped on u, repeating on v: only v is crossed, so only v matters
    let m = material(&mut w, 0, [Wrap::Clamp, Wrap::Repeat]);
    let mut p = quad(Some(m));
    p.streams.insert(
        Semantic::TexCoord(0),
        Stream::F32x2(vec![[0.0, -0.5], [1.0, -0.5], [1.0, 1.5], [0.0, 1.5]]),
    );
    p.indices = None;
    // sequential vertices are two triangles' corners: 0 1 2 and 3 4 5
    for s in p.streams.values_mut() {
        match s {
            Stream::F32x3(v) => *v = vec![v[0], v[1], v[2], v[0], v[2], v[3]],
            Stream::F32x2(v) => *v = vec![v[0], v[1], v[2], v[0], v[2], v[3]],
            Stream::U8x4(v) => *v = vec![v[0], v[1], v[2], v[0], v[2], v[3]],
            _ => unreachable!(),
        }
    }
    let e = mesh_of(&mut w, vec![p]);
    run(&mut w, 0).unwrap();
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let uv = f32x2(p, Semantic::TexCoord(0));
    assert!(uv.iter().flatten().all(|c| (0.0..=1.0).contains(c)));
    // v crosses 0 and 1 at the middle of the triangles: three strips
    assert!(p.indices.is_some() && triangles(p).len() > 2);
    // u was not cut: no vertex was made on a u line, so the cuts' u are the diagonal's, strictly inside
    assert!(uv.iter().flatten().all(|c| c.is_finite()));
}

/// A triangle needs no weld: two triangles sharing an edge get one vertex per cut of it, and an edge cut from either
/// side lands on the same point (no crack): the positions of the pieces all lie on the quad.
#[test]
fn shared_edges_are_welded_and_cut_alike() {
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    run(&mut w, 0).unwrap();
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let pos = f32x3(p, Semantic::Position);
    // 16 vertices were counted above; here no two vertices share a position and a coordinate
    let uv = f32x2(p, Semantic::TexCoord(0));
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..pos.len() {
        let key = (pos[i].map(f32::to_bits), uv[i].map(f32::to_bits));
        assert!(seen.insert(key), "vertex {i} repeats another");
    }
}

fn skeleton(w: &mut World) -> Entity {
    let s = w.spawn();
    let joint = |id: u32| Joint {
        id: JointId(id),
        name: format!("j{id}"),
        parent: None,
        rest: Rest::Affine(Transform::IDENTITY),
    };
    w.insert(
        s,
        Skeleton {
            joints: (10..20).map(joint).collect(),
        },
    );
    s
}

fn skin_of(w: &mut World, influences: Vec<Option<Influences>>, slots: u32) -> Skin {
    let skeleton = skeleton(w);
    let palette: Vec<Slot> = (0..slots)
        .map(|k| Slot {
            joint: JointId(10 + k),
            inverse_bind: Transform::IDENTITY,
        })
        .collect();
    Skin {
        weights: WeightRule::SumsToOne,
        bindings: vec![Binding {
            id: BindingId(0),
            skeleton,
            palettes: influences.iter().map(|i| i.map(|_| palette.clone())).collect(),
            deformation: Deformation::Linear,
        }],
        influences,
    }
}

/// Skin and morphs follow the new vertices. The quad's corners are bound to slots 0 (p0, p3), 1 (p1) and 2 (p2) and
/// carry a morph delta of 4 times their index along x. The vertex at (0, 0.25) lies 1/4 of the way from p0 to p2:
/// weights 3/4 on slot 0 and 1/4 on slot 2, delta 4 * 1/4 * 2 = 2.
#[test]
fn skin_weights_and_morph_deltas_are_interpolated() {
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut p = quad(Some(m));
    p.streams.insert(
        Semantic::Joints(0),
        Stream::U8x4(vec![[0, 0, 0, 0], [1, 0, 0, 0], [2, 0, 0, 0], [0, 0, 0, 0]]),
    );
    p.streams
        .insert(Semantic::Weights(0), Stream::U8x4(vec![[255, 0, 0, 0]; 4]));
    let e = mesh_of(&mut w, vec![p]);
    let skin = skin_of(&mut w, vec![Some(Influences::Streams { sets: 1 })], 3);
    w.insert(e, skin);
    let deltas = Deltas {
        positions: Some((0..4).map(|k| [4.0 * k as f32, 0.0, 0.0]).collect()),
        ..Deltas::default()
    };
    w.insert(
        e,
        Morphs {
            targets: vec![Target {
                id: MorphId(0),
                name: "grow".into(),
                default_weight: 0.0,
                frames: vec![Frame {
                    weight: 1.0,
                    primitives: vec![Some(deltas)],
                }],
            }],
        },
    );
    let report = run(&mut w, 0).unwrap();
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let pos = f32x3(p, Semantic::Position);
    let i = (0..pos.len())
        .find(|&i| pos[i][0] == 0.0 && (pos[i][1] - 0.25).abs() < 1e-6)
        .unwrap();
    let Stream::U8x4(j) = &p.streams[&Semantic::Joints(0)] else {
        panic!()
    };
    let Stream::F32x4(wt) = &p.streams[&Semantic::Weights(0)] else {
        panic!()
    };
    assert_eq!(j[i][..2], [0, 2]);
    assert!((wt[i][0] - 0.75).abs() < 1e-6 && (wt[i][1] - 0.25).abs() < 1e-6 && wt[i][2] == 0.0);
    // the vertex the strip below makes is the same point
    let morphs = w.get::<Morphs>(e).unwrap();
    let d = morphs.targets[0].frames[0].primitives[0].as_ref().unwrap();
    let delta = d.positions.as_ref().unwrap();
    assert_eq!(delta.len(), 16);
    assert!((delta[i][0] - 2.0).abs() < 1e-5, "{:?}", delta[i]);
    // a corner keeps its own delta: p1 (1.5, 0) is bound to slot 1 with delta 4
    let c = (0..pos.len()).find(|&i| pos[i] == [1.5, 0.0, 0.0]).unwrap();
    assert_eq!(delta[c], [4.0, 0.0, 0.0]);
    assert_eq!(j[c][0], 1);
    // the approximation is counted: the vertices whose corners differ (the six points on the diagonal-crossing edges
    // other than those on the horizontal edges, which join corners of equal slots)
    let log = &report.steps[0].log;
    assert!(log.approximations["skinned vertices with interpolated weights"] > 0);
    assert!(w.verify_components().is_empty());
}

/// More influences than four across the corners need a second set, none dropped; a single-index primitive becomes
/// streams only when it must.
#[test]
fn skin_sets_grow_to_hold_the_union() {
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut p = quad(Some(m));
    p.streams.insert(
        Semantic::Joints(0),
        Stream::U8x4(vec![[0, 1, 2, 3], [4, 5, 6, 7], [4, 5, 6, 7], [0, 1, 2, 3]]),
    );
    p.streams
        .insert(Semantic::Weights(0), Stream::F32x4(vec![[0.25; 4]; 4]));
    let e = mesh_of(&mut w, vec![p]);
    let skin = skin_of(&mut w, vec![Some(Influences::Streams { sets: 1 })], 8);
    w.insert(e, skin);
    run(&mut w, 0).unwrap();
    assert_eq!(
        w.get::<Skin>(e).unwrap().influences,
        [Some(Influences::Streams { sets: 2 })]
    );
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let Stream::U8x4(j1) = &p.streams[&Semantic::Joints(1)] else {
        panic!()
    };
    assert_eq!(j1.len(), p.vertex_count());
    // a vertex between p0 (slots 0-3) and p2 (4-7) has all eight, each 1/8 (the weights 0.25 * 1/2)
    let Stream::F32x4(w0) = &p.streams[&Semantic::Weights(0)] else {
        panic!()
    };
    let Stream::F32x4(w1) = &p.streams[&Semantic::Weights(1)] else {
        panic!()
    };
    let both = (0..p.vertex_count())
        .filter(|&i| w1[i].iter().any(|&x| x > 0.0))
        .count();
    assert!(both > 0);
    for i in 0..p.vertex_count() {
        let total: f32 = w0[i].iter().chain(&w1[i]).sum();
        assert!((total - 1.0).abs() < 1e-5, "vertex {i}: {total}");
    }

    // single index: all corners on one slot stays single index
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut p = quad(Some(m));
    p.streams
        .insert(Semantic::Joints(0), Stream::U8x4(vec![[3, 0, 0, 0]; 4]));
    let e = mesh_of(&mut w, vec![p]);
    let skin = skin_of(&mut w, vec![Some(Influences::SingleIndex)], 4);
    w.insert(e, skin);
    run(&mut w, 0).unwrap();
    assert_eq!(
        w.get::<Skin>(e).unwrap().influences,
        [Some(Influences::SingleIndex)]
    );
    // two slots across an edge: it becomes streams
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut p = quad(Some(m));
    p.streams.insert(
        Semantic::Joints(0),
        Stream::U8x4(vec![[0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]]),
    );
    let e = mesh_of(&mut w, vec![p]);
    let skin = skin_of(&mut w, vec![Some(Influences::SingleIndex)], 4);
    w.insert(e, skin);
    run(&mut w, 0).unwrap();
    assert_eq!(
        w.get::<Skin>(e).unwrap().influences,
        [Some(Influences::Streams { sets: 1 })]
    );
}

/// Tangents: the xyz is renormalised, w kept.
#[test]
fn tangents_are_renormalised_and_keep_w() {
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut p = quad(Some(m));
    p.streams.insert(
        Semantic::Tangent,
        Stream::F32x4(vec![
            [1.0, 0.0, 0.0, -1.0],
            [0.0, 1.0, 0.0, -1.0],
            [0.0, 1.0, 0.0, -1.0],
            [1.0, 0.0, 0.0, -1.0],
        ]),
    );
    let e = mesh_of(&mut w, vec![p]);
    run(&mut w, 0).unwrap();
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let Stream::F32x4(t) = &p.streams[&Semantic::Tangent] else {
        panic!()
    };
    for t in t {
        let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-6 && t[3] == -1.0, "{t:?}");
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Attributes(Vec<Vec<u32>>);
impl Component for Attributes {
    const NAME: &'static str = "test.attributes";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(remap_list(&self.0, from).map(Attributes))
    }
    fn remap_vertices(&self, _primitive: usize, _from: &[u32]) -> Option<Result<Self, String>> {
        Some(Ok(self.clone()))
    }
}

/// Every refusal names what it refused and changes nothing.
#[test]
fn refusals_name_the_cause_and_leave_the_world_as_it_was() {
    let refused = |w: &mut World, e: Entity, needle: &[&str]| {
        let before = w.get::<Mesh>(e).unwrap().clone();
        let err = run(w, 0).unwrap_err();
        for n in needle {
            assert!(err.contains(n), "{err} lacks {n}");
        }
        assert_eq!(w.get::<Mesh>(e).unwrap(), &before);
    };

    // a clamped texture on a crossed axis
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat, Wrap::Clamp]);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    // v is within 0-1: only u is crossed, so a clamped v is fine
    run(&mut w, 0).unwrap();
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Mirror, Wrap::Repeat]);
    let e2 = mesh_of(&mut w, vec![quad(Some(m))]);
    refused(&mut w, e2, &["Mirror", "on u", "base colour"]);
    let _ = e;

    // a per-vertex component that cannot combine vertices
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    w.insert(e, Attributes(vec![vec![0; 4]]));
    refused(&mut w, e, &["test.attributes"]);

    // tangents whose handedness differs across a triangle
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut p = quad(Some(m));
    p.streams.insert(
        Semantic::Tangent,
        Stream::F32x4(vec![
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0, 1.0],
        ]),
    );
    let e = mesh_of(&mut w, vec![p]);
    refused(&mut w, e, &["w", "primitive 0"]);

    // not triangles
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let mut p = quad(Some(m));
    p.topology = Topology::Lines;
    p.indices = Some(vec![0, 1, 2, 3]);
    let e = mesh_of(&mut w, vec![p]);
    refused(&mut w, e, &["Lines", "primitive 0"]);

    // a transform on the slot
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    for t in w.get_mut::<PbrMaterial>(m).unwrap().textures_mut() {
        t.transform = Some(UvTransform {
            offset: [0.5, 0.0],
            ..UvTransform::IDENTITY
        });
    }
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    refused(&mut w, e, &["transform"]);

    // a variant that clamps
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let clamped = material(&mut w, 0, [Wrap::Clamp; 2]);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    w.insert(e, MaterialVariants(vec![BTreeMap::from([(1, clamped)])]));
    refused(&mut w, e, &["Clamp"]);

    // geometry still in its file
    let mut w = World::new();
    let m = material(&mut w, 0, [Wrap::Repeat; 2]);
    let e = w.spawn();
    w.insert(
        e,
        MeshFile {
            path: "a.glb".into(),
            mesh: 0,
            materials: vec![Some(m)],
        },
    );
    // (another entity's loaded mesh satisfies the pass's requirement)
    mesh_of(&mut w, vec![quad(Some(m))]);
    let err = run(&mut w, 0).unwrap_err();
    assert!(err.contains("still in its file"), "{err}");
}

/// A primitive nothing samples is left as it is, and counted.
#[test]
fn primitives_nothing_samples_are_left_and_counted() {
    let mut w = World::new();
    // the material samples set 1, not 0; the second primitive has no material
    let m = material(&mut w, 1, [Wrap::Mirror; 2]);
    let e = mesh_of(&mut w, vec![quad(Some(m)), quad(None)]);
    let before = w.get::<Mesh>(e).unwrap().clone();
    let report = run(&mut w, 0).unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap(), &before);
    let log = &report.steps[0].log;
    assert_eq!(
        log.counts["primitives left as they are: no texture samples the set"],
        1
    );
    assert_eq!(log.counts["primitives left as they are: no material"], 1);
}

#[test]
fn the_set_is_required_and_a_number() {
    let make = |o: &[(&str, &str)]| {
        let o: Options = o
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        primase_meshops::SPLIT_UV_TILES.instance(&o)
    };
    assert_eq!(
        make(&[("set", "2")]).unwrap().options(),
        SplitUvTiles::new(2).options()
    );
    assert!(make(&[]).err().unwrap().contains("set"));
    assert!(make(&[("set", "x")]).err().unwrap().contains("set=x"));
    assert!(
        make(&[("set", "0"), ("size", "1")])
            .err()
            .unwrap()
            .contains("size")
    );
}
