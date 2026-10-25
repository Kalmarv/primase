//! `add-back-faces` on hand-built quads. The oracles are worked by hand: the geometric normal of a triangle (the cross
//! product of its edges, which is what a rasteriser's winding test sees) against the vertex normals it is lit with;
//! a headlight Lambert term at both viewers; and the relief of a sloped sheet, whose normal seen from behind is the
//! front's reversed, against the tangent frame the back vertices carry.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::Conservation;
use primase_core::morphs::{Deltas, Frame, MorphId, Morphs, Target};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::skeleton::{Joint, Rest, Skeleton};
use primase_core::skin::{Binding, BindingId, Deformation, Skin, Slot, WeightRule};
use primase_core::world::remap_list;
use primase_core::{
    Component, Entity, Influences, JointId, MaterialVariants, Mesh, MeshFile, Name, Primitive, Semantic,
    Stream, Topology, Transform, World,
};
use primase_meshops::AddBackFaces;

type V = [f32; 3];

fn run(w: &mut World) -> Result<RunReport, String> {
    let mut registry = PassRegistry::new();
    primase_meshops::register(&mut registry);
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(AddBackFaces)],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn material(w: &mut World, double_sided: bool) -> Entity {
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            double_sided,
            ..PbrMaterial::default()
        },
    );
    m
}

/// The unit quad in the xy plane facing +z, counterclockwise from +z: p0 p1 p2 and p0 p2 p3. Normals +z, tangents
/// +x with w = 1 (so the bitangent N x T is +y, along v), uv = (x, y), a colour per vertex.
fn quad(material: Option<Entity>) -> Primitive {
    let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (Semantic::Position, Stream::F32x3(p.to_vec())),
            (Semantic::Normal, Stream::F32x3(vec![[0.0, 0.0, 1.0]; 4])),
            (Semantic::Tangent(0), Stream::F32x4(vec![[1.0, 0.0, 0.0, 1.0]; 4])),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(p.iter().map(|p| [p[0], p[1]]).collect()),
            ),
            (
                Semantic::Color(0),
                Stream::U8x4(vec![
                    [1, 2, 3, 4],
                    [5, 6, 7, 8],
                    [9, 10, 11, 12],
                    [13, 14, 15, 16],
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
    p.indices.clone().unwrap().as_chunks::<3>().0.to_vec()
}

fn f32x3(p: &Primitive, s: Semantic) -> &[V] {
    let Stream::F32x3(v) = &p.streams[&s] else {
        panic!()
    };
    v
}

fn f32x4(p: &Primitive, s: Semantic) -> &[[f32; 4]] {
    let Stream::F32x4(v) = &p.streams[&s] else {
        panic!()
    };
    v
}

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The direction a triangle faces: the cross product of its edges (counterclockwise seen from the front).
fn facing(p: &Primitive, t: [u32; 3]) -> V {
    let pos = f32x3(p, Semantic::Position);
    let [a, b, c] = t.map(|k| pos[k as usize]);
    cross(sub(b, a), sub(c, a))
}

/// A headlight at `eye`: the Lambert term `max(n . v, 0)` of the triangle's vertex normals at its centre, and whether a
/// rasteriser draws it from there (a single-sided destination culls the triangle whose geometric normal turns away).
/// `None` when it is culled.
fn lit_from(p: &Primitive, t: [u32; 3], eye: V) -> Option<f32> {
    let pos = f32x3(p, Semantic::Position);
    let nrm = f32x3(p, Semantic::Normal);
    let centre: V = std::array::from_fn(|c| t.iter().map(|&k| pos[k as usize][c]).sum::<f32>() / 3.0);
    let to_eye = sub(eye, centre);
    if dot(facing(p, t), to_eye) <= 0.0 {
        return None;
    }
    let n: V = std::array::from_fn(|c| t.iter().map(|&k| nrm[k as usize][c]).sum::<f32>());
    let len = dot(n, n).sqrt();
    let v_len = dot(to_eye, to_eye).sqrt();
    Some((dot(n, to_eye) / (len * v_len)).max(0.0))
}

/// The quad given back faces: 2 triangles become 4 (each front followed by its back), the 4 vertices 8; the back
/// triangles face -z, and each is lit by vertex normals that point the way it faces.
#[test]
fn a_double_sided_quad_gets_back_faces_that_face_their_normals() {
    let mut w = World::new();
    let m = material(&mut w, true);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    w.insert(e, Name("cloth".into()));
    w.insert(
        e,
        MeshFile {
            path: "quad.glb".into(),
            mesh: 0,
            materials: vec![Some(m)],
        },
    );
    let report = run(&mut w).unwrap();
    let log = &report.steps[0].log;
    assert_eq!(log.counts["primitives given back faces"], 1);
    assert_eq!(log.counts["triangles added"], 2);
    assert_eq!(log.counts["vertices added"], 4);
    assert_eq!(log.counts["material e0: made single-sided"], 1);
    assert!(!w.get::<PbrMaterial>(m).unwrap().double_sided);
    assert!(!w.has::<MeshFile>(e));

    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    assert_eq!(p.vertex_count(), 8);
    assert!(p.streams.values().all(|s| s.len() == 8));
    // front, back, front, back: the back of (0,1,2) is (4,6,5) and of (0,2,3) is (4,7,6)
    assert_eq!(triangles(p), [[0, 1, 2], [4, 6, 5], [0, 2, 3], [4, 7, 6]]);
    let nrm = f32x3(p, Semantic::Normal);
    for (k, t) in triangles(p).into_iter().enumerate() {
        let n = facing(p, t);
        let want = if k % 2 == 0 { 1.0 } else { -1.0 };
        assert_eq!(n[2].signum(), want, "triangle {k} faces {n:?}");
        assert_eq!(n[0], 0.0);
        assert_eq!(n[1], 0.0);
        // the vertex normals it is lit with point the way the triangle faces
        for v in t {
            assert!(dot(nrm[v as usize], n) > 0.0, "triangle {k} vertex {v}");
        }
    }

    // either viewer sees exactly one of each pair, lit fully; a single-sided destination draws nothing else
    for (eye, side) in [([0.5, 0.5, 3.0], 0), ([0.5, 0.5, -3.0], 1)] {
        let drawn: Vec<(usize, f32)> = triangles(p)
            .into_iter()
            .enumerate()
            .filter_map(|(k, t)| lit_from(p, t, eye).map(|l| (k, l)))
            .collect();
        assert_eq!(drawn.len(), 2, "{eye:?}: {drawn:?}");
        for (k, lit) in drawn {
            assert_eq!(k % 2, side, "{eye:?} draws triangle {k}");
            assert!(lit > 0.9, "{eye:?}: triangle {k} lit {lit}");
        }
    }
    // before the pass the single-sided material drew nothing from behind
    let before = quad(None);
    assert!(
        triangles(&before)
            .into_iter()
            .all(|t| lit_from(&before, t, [0.5, 0.5, -3.0]).is_none())
    );

    // every other stream is the source vertex's
    let Stream::U8x4(colours) = &p.streams[&Semantic::Color(0)] else {
        panic!()
    };
    assert_eq!(colours[..4], colours[4..]);
    assert_eq!(
        f32x3(p, Semantic::Position)[..4],
        f32x3(p, Semantic::Position)[4..]
    );
    // the tangent: xyz and w negated
    assert!(
        f32x4(p, Semantic::Tangent(0))[4..]
            .iter()
            .all(|t| *t == [-1.0, 0.0, 0.0, -1.0])
    );
    assert!(w.verify_components().is_empty());
}

/// The relief rule, against the surface itself. The sheet is z = 0.5 x (a slope along u), so its front normal is
/// proportional to (-0.5, 0, 1) and the same sheet seen from behind has the opposite normal. The normal map holds that
/// relief in tangent space as (-0.5, 0, 1)/|.| (x along the tangent, y along the bitangent B = w N x T, z along N). The
/// back vertices' frame must give, for that texel, the reversed normal; the frame that merely keeps the tangent and
/// negates w gives the front's mirrored in the sheet, which is a different relief.
#[test]
fn the_normal_map_reads_as_the_reversed_surface_normal_on_the_back() {
    let mut w = World::new();
    let m = material(&mut w, true);
    let e = mesh_of(&mut w, vec![quad(Some(m))]);
    run(&mut w).unwrap();
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let n = f32x3(p, Semantic::Normal);
    let t = f32x4(p, Semantic::Tangent(0));
    let len = (0.25f32 + 1.0).sqrt();
    let texel = [-0.5 / len, 0.0, 1.0 / len];
    let shade = |v: usize| -> V {
        let tt = [t[v][0], t[v][1], t[v][2]];
        let bitangent = cross(n[v], tt).map(|c| c * t[v][3]);
        std::array::from_fn(|c| texel[0] * tt[c] + texel[1] * bitangent[c] + texel[2] * n[v][c])
    };
    let front = shade(0);
    let back = shade(4);
    // the surface z = 0.5 x has normal (-0.5, 0, 1)/len in front, the reverse behind
    assert!((front[0] - -0.5 / len).abs() < 1e-6 && (front[2] - 1.0 / len).abs() < 1e-6);
    for c in 0..3 {
        assert!((back[c] + front[c]).abs() < 1e-6, "{back:?} against {front:?}");
    }
    // the rejected alternative (tangent kept, w negated, normal negated) mirrors the front in the sheet instead
    let kept = [1.0f32, 0.0, 0.0];
    let alt_b = cross([0.0, 0.0, -1.0], kept).map(|c| -c);
    let alt: V =
        std::array::from_fn(|c| texel[0] * kept[c] + texel[1] * alt_b[c] + texel[2] * -[0.0f32, 0.0, 1.0][c]);
    assert!((alt[0] - front[0]).abs() < 1e-6 && (alt[2] + front[2]).abs() < 1e-6);
    assert!((alt[0] - back[0]).abs() > 0.5);
    // and the back frame is right-handed in the way the front is: B = w N x T on the back is the front's B reversed
    let b_front = cross(n[0], [t[0][0], t[0][1], t[0][2]]).map(|c| c * t[0][3]);
    let b_back = cross(n[4], [t[4][0], t[4][1], t[4][2]]).map(|c| c * t[4][3]);
    assert_eq!(b_back, b_front.map(|c| -c));
}

/// Skinned and morphed streams are copied: a copy has its source vertex's joints and weights, its source's morph
/// position delta and the negation of its normal and tangent deltas, and the palette and influences stand.
#[test]
fn skin_and_morph_streams_follow_the_copies() {
    let mut w = World::new();
    let m = material(&mut w, true);
    let mut p = quad(Some(m));
    p.streams.insert(
        Semantic::Joints(0),
        Stream::U8x4(vec![[0, 0, 0, 0], [1, 0, 0, 0], [2, 1, 0, 0], [0, 0, 0, 0]]),
    );
    p.streams.insert(
        Semantic::Weights(0),
        Stream::F32x4(vec![
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.75, 0.25, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ]),
    );
    let e = mesh_of(&mut w, vec![p]);
    let skeleton = w.spawn();
    w.insert(
        skeleton,
        Skeleton {
            joints: (10..13)
                .map(|id| Joint {
                    id: JointId(id),
                    name: format!("j{id}"),
                    parent: None,
                    rest: Rest::Affine(Transform::IDENTITY),
                })
                .collect(),
        },
    );
    let palette: Vec<Slot> = (0..3)
        .map(|k| Slot {
            joint: JointId(10 + k),
            inverse_bind: Transform::IDENTITY,
        })
        .collect();
    w.insert(
        e,
        Skin {
            influences: vec![Some(Influences::Streams { sets: 1 })],
            weights: WeightRule::SumsToOne,
            bindings: vec![Binding {
                id: BindingId(0),
                skeleton,
                palettes: vec![Some(palette)],
                deformation: Deformation::Linear,
            }],
        },
    );
    let dense = Deltas {
        positions: Some((0..4).map(|k| [0.0, 0.0, k as f32]).collect()),
        normals: Some((0..4).map(|k| [0.0, 0.1 * (k + 1) as f32, 0.0]).collect()),
        tangents: Some((0..4).map(|k| [0.0, 0.0, 0.2 * (k + 1) as f32]).collect()),
        ..Deltas::default()
    };
    // sparse: vertices 1 and 3 only
    let sparse = Deltas {
        indices: Some(vec![1, 3]),
        positions: Some(vec![[5.0, 0.0, 0.0], [7.0, 0.0, 0.0]]),
        normals: Some(vec![[0.0, 0.5, 0.0], [0.0, 0.7, 0.0]]),
        ..Deltas::default()
    };
    let target = |id: u32, d: Deltas| Target {
        id: MorphId(id),
        name: format!("t{id}"),
        default_weight: 0.0,
        frames: vec![Frame {
            weight: 1.0,
            primitives: vec![Some(d)],
        }],
    };
    w.insert(
        e,
        Morphs {
            targets: vec![target(0, dense), target(1, sparse)],
        },
    );
    run(&mut w).unwrap();
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    assert_eq!(p.vertex_count(), 8);
    let Stream::U8x4(j) = &p.streams[&Semantic::Joints(0)] else {
        panic!()
    };
    assert_eq!(j[..4], j[4..]);
    assert_eq!(
        f32x4(p, Semantic::Weights(0))[..4],
        f32x4(p, Semantic::Weights(0))[4..]
    );
    let skin = w.get::<Skin>(e).unwrap();
    assert_eq!(skin.influences, [Some(Influences::Streams { sets: 1 })]);
    assert_eq!(skin.bindings[0].palettes[0].as_ref().unwrap().len(), 3);

    let morphs = w.get::<Morphs>(e).unwrap();
    let d = morphs.targets[0].frames[0].primitives[0].as_ref().unwrap();
    let (pos, nor, tan) = (
        d.positions.as_ref().unwrap(),
        d.normals.as_ref().unwrap(),
        d.tangents.as_ref().unwrap(),
    );
    assert_eq!((pos.len(), nor.len(), tan.len()), (8, 8, 8));
    for k in 0..4 {
        assert_eq!(pos[4 + k], pos[k]);
        assert_eq!(nor[4 + k], nor[k].map(|c| -c));
        assert_eq!(tan[4 + k], tan[k].map(|c| -c));
    }
    let s = morphs.targets[1].frames[0].primitives[0].as_ref().unwrap();
    // sparse: vertices 1 and 3 and their copies 5 and 7
    assert_eq!(s.indices.as_deref(), Some(&[1, 3, 5, 7][..]));
    assert_eq!(
        s.positions.as_deref(),
        Some(&[[5.0, 0.0, 0.0], [7.0, 0.0, 0.0], [5.0, 0.0, 0.0], [7.0, 0.0, 0.0]][..])
    );
    assert_eq!(
        s.normals.as_deref(),
        Some(
            &[
                [0.0, 0.5, 0.0],
                [0.0, 0.7, 0.0],
                [0.0, -0.5, 0.0],
                [0.0, -0.7, 0.0]
            ][..]
        )
    );
    assert!(w.verify_components().is_empty());
}

/// A mesh with a back face already, a coincident-corner triangle, a primitive without indices, a single-sided one, one
/// without a material and a line list: each is counted and only the right triangles are copied.
#[test]
fn what_is_left_is_counted() {
    let mut w = World::new();
    let two = material(&mut w, true);
    let one = material(&mut w, false);
    // the quad plus its first triangle reversed on vertices of its own (positions equal), plus a sliver
    let mut with_reverse = quad(Some(two));
    for s in with_reverse.streams.values_mut() {
        match s {
            Stream::F32x3(v) => v.extend_from_within(..3),
            Stream::F32x4(v) => v.extend_from_within(..3),
            Stream::F32x2(v) => v.extend_from_within(..3),
            Stream::U8x4(v) => v.extend_from_within(..3),
            Stream::U16x4(v) => v.extend_from_within(..3),
        }
    }
    // vertices 4,5,6 equal 0,1,2: the reverse of the first triangle is (4,6,5); and (0,0,3) has coincident corners
    let ix = with_reverse.indices.as_mut().unwrap();
    ix.extend_from_slice(&[4, 6, 5, 0, 0, 3]);
    // non-indexed: three vertices
    let mut bare = quad(Some(two));
    bare.indices = None;
    for s in bare.streams.values_mut() {
        match s {
            Stream::F32x3(v) => v.truncate(3),
            Stream::F32x4(v) => v.truncate(3),
            Stream::F32x2(v) => v.truncate(3),
            Stream::U8x4(v) => v.truncate(3),
            Stream::U16x4(v) => v.truncate(3),
        }
    }
    let mut lines = quad(Some(two));
    lines.topology = Topology::Lines;
    lines.indices = Some(vec![0, 1]);
    let e = mesh_of(
        &mut w,
        vec![with_reverse, bare, quad(Some(one)), quad(None), lines],
    );
    let report = run(&mut w).unwrap();
    let log = &report.steps[0].log;
    assert_eq!(log.counts["primitives given back faces"], 2);
    // first primitive: 4 triangles of which the first has its reverse, and so has the reverse (both skipped), and the
    // sliver is skipped: 2 added (the second triangle and ... the quad's second triangle only)
    assert_eq!(
        log.counts["triangles left without a back face: already reversed in the primitive"],
        2
    );
    assert_eq!(
        log.counts["triangles left without a back face: coincident corners"],
        1
    );
    // the second triangle of the quad, plus the one triangle of the non-indexed primitive
    assert_eq!(log.counts["triangles added"], 2);
    assert_eq!(
        log.counts["primitives left as they are: single-sided material"],
        1
    );
    assert_eq!(log.counts["primitives left as they are: no material"], 1);
    assert_eq!(log.counts["primitives left as they are: not triangles"], 1);
    assert!(!w.get::<PbrMaterial>(two).unwrap().double_sided);
    assert!(!w.get::<PbrMaterial>(one).unwrap().double_sided);
    let prims = w.get::<Mesh>(e).unwrap().primitives();
    // the non-indexed primitive now has indices: its triangle then its back
    assert_eq!(triangles(&prims[1]), [[0, 1, 2], [3, 5, 4]]);
    assert_eq!(prims[1].vertex_count(), 6);
    // the line list and the others are as they were
    assert_eq!(prims[4].indices.as_deref(), Some(&[0, 1][..]));
    assert_eq!(prims[2].vertex_count(), 4);
}

/// Variants of a primitive that differ in sidedness are refused, naming the primitive; agreeing ones are all made
/// single-sided.
#[test]
fn variants_must_agree() {
    let mut w = World::new();
    let two = material(&mut w, true);
    let two_b = material(&mut w, true);
    let one = material(&mut w, false);
    let e = mesh_of(&mut w, vec![quad(Some(two))]);
    w.insert(e, MaterialVariants(vec![BTreeMap::from([(1, one)])]));
    let err = run(&mut w).unwrap_err();
    assert!(
        err.contains("primitive 0") && err.contains("variants differ"),
        "{err}"
    );
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives()[0].vertex_count(), 4);
    assert!(w.get::<PbrMaterial>(two).unwrap().double_sided);

    w.insert(e, MaterialVariants(vec![BTreeMap::from([(1, two_b)])]));
    run(&mut w).unwrap();
    assert!(!w.get::<PbrMaterial>(two).unwrap().double_sided);
    assert!(!w.get::<PbrMaterial>(two_b).unwrap().double_sided);
    assert_eq!(w.get::<Mesh>(e).unwrap().primitives()[0].vertex_count(), 8);
}

/// Per-vertex data that cannot follow refuses the pass, naming it, before any mesh changes.
#[test]
fn a_per_vertex_component_that_cannot_follow_refuses_and_changes_nothing() {
    #[derive(Clone, Debug, PartialEq)]
    struct Weights(Vec<Vec<u8>>);
    impl Component for Weights {
        const NAME: &'static str = "test.vertex_data";
        const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
        fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
            Some(remap_list(&self.0, from).map(Weights))
        }
        /// Does not know how to follow a change to a primitive's vertices.
        fn remap_vertices(&self, _primitive: usize, _from: &[u32]) -> Option<Result<Self, String>> {
            None
        }
    }
    let mut w = World::new();
    let m = material(&mut w, true);
    let first = mesh_of(&mut w, vec![quad(Some(m))]);
    let second = mesh_of(&mut w, vec![quad(Some(m))]);
    w.insert(second, Weights(vec![vec![0; 4]]));
    let err = run(&mut w).unwrap_err();
    assert!(err.contains("test.vertex_data"), "{err}");
    assert_eq!(w.get::<Mesh>(first).unwrap().primitives()[0].vertex_count(), 4);
    assert!(w.get::<PbrMaterial>(m).unwrap().double_sided);
}

/// A material that is not a `pbr.material`, and streams of the wrong type, are refused naming the primitive.
#[test]
fn unusable_inputs_are_refused() {
    let mut w = World::new();
    material(&mut w, true);
    let bare = w.spawn();
    mesh_of(&mut w, vec![quad(Some(bare))]);
    let err = run(&mut w).unwrap_err();
    assert!(err.contains("no pbr.material"), "{err}");

    let mut w = World::new();
    let m = material(&mut w, true);
    let mut p = quad(Some(m));
    p.streams
        .insert(Semantic::Tangent(0), Stream::F32x3(vec![[1.0, 0.0, 0.0]; 4]));
    mesh_of(&mut w, vec![p]);
    let err = run(&mut w).unwrap_err();
    assert!(err.contains("Tangent(0) is not four floats"), "{err}");
}
