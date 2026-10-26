//! Hand-worked quartet: disjoint vertex ranges, two draws, shared materials and a mirrored instance.
use primase_core::{
    Component, Instance, Mesh, Semantic, Stream, Transform,
    convert::Reader,
    frame::{Axis, SourceFrame},
};
use primase_shred::{
    components::{MaterialId, RefusedMesh, VertexInputs},
    read::{convert::ShredReader, level, pak::Pak, scene},
};
use std::collections::BTreeSet;

fn word(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn float(b: &mut [u8], at: usize, n: f32) {
    word(b, at, n.to_bits());
}
fn matrix(b: &mut [u8], at: usize, mirror: bool, translation: f32) {
    for i in 0..4 {
        float(b, at + i * 20, if i == 0 && mirror { -1. } else { 1. });
    }
    float(b, at + 48, translation);
}

struct Fixture {
    scp: Vec<u8>,
    mlp: Vec<u8>,
    mmp: Vec<u8>,
    mgp: Vec<u8>,
}
impl Fixture {
    /// Hand-written native packed declaration families. The six vertices each receive the same known lanes.
    fn attributes(mask: u32, stride: usize) -> Self {
        let mut f = Self::new();
        f.mlp.splice(84 + 432..84 + 432, [0; 24]);
        word(&mut f.mlp, 28 + 16, 456);
        word(&mut f.mlp, 56 + 4, 456);
        word(&mut f.mlp, 84 + 0x8c, mask);
        word(&mut f.mlp, 84 + 0x94, 432);
        let mut row = vec![];
        if mask & 2 != 0 {
            row.extend([255, 128, 0, 64]);
        }
        if mask & 8 != 0 {
            row.extend(0x000003ffu32.to_be_bytes());
        }
        if mask & 16 != 0 {
            row.extend(0x001ff800u32.to_be_bytes());
        }
        // half values: UV0(.5,-1), UV1(2,.25), UV2(-2,.75), UV3(1,0).
        for (bit, halves) in [
            (5, [0x3800u16, 0xbc00]),
            (6, [0x4000, 0x3400]),
            (7, [0xc000, 0x3a00]),
            (8, [0x3c00, 0]),
        ] {
            if mask & (1 << bit) != 0 {
                for h in halves {
                    row.extend(h.to_be_bytes());
                }
            }
        }
        if mask & 0x600 != 0 {
            for h in [0x3c00u16, 0xc000, 0x3800, 0, 0x4000, 0x4200, 0x4400, 0] {
                row.extend(h.to_be_bytes());
            }
        }
        assert_eq!(row.len(), stride, "hand-worked family {mask:x}");
        for _ in 0..6 {
            f.mgp.extend(&row);
        }
        word(&mut f.mlp, 28 + 24, f.mgp.len() as u32);
        word(&mut f.mlp, 56 + 12, f.mgp.len() as u32);
        for (k, v) in [6, 6, stride as u32, 0, 0, f.mgp.len() as u32]
            .into_iter()
            .enumerate()
        {
            word(&mut f.mlp, 84 + 432 + k * 4, v);
        }
        f
    }
    fn new() -> Self {
        let mut mlp = vec![0; 84 + 432 + 336];
        word(&mut mlp, 0, 0xfafa0003);
        word(&mut mlp, 16, 2);
        word(&mut mlp, 20, 84);
        word(&mut mlp, 24, 28);
        let mut mmp = vec![0; 108 + 54];
        for (i, (base, nv, ns, offset)) in [(84, 6, 2, 0), (84 + 432, 3, 1, 108)].into_iter().enumerate() {
            let meta = if i == 0 { 432 } else { 336 };
            for (k, v) in [
                100 + i as u32,
                (base - 84) as u32,
                offset as u32,
                0,
                meta,
                nv * 18,
                0,
            ]
            .into_iter()
            .enumerate()
            {
                word(&mut mlp, 28 + i * 28 + k * 4, v);
            }
            word(&mut mlp, base, 0xbaba0031);
            matrix(&mut mlp, base + 16, false, 0.);
            for k in 0..3 {
                float(&mut mlp, base + 0x60 + k * 4, -100.);
                float(&mut mlp, base + 0x70 + k * 4, 100.);
            }
            word(&mut mlp, base + 0x80, 100 + i as u32);
            word(&mut mlp, base + 0x8c, 0x80005);
            let vp = 192 + ns * 96;
            let ip = vp + 24;
            for (k, v) in [vp, u32::MAX as usize, ip, u32::MAX as usize, 192, ns]
                .into_iter()
                .enumerate()
            {
                word(&mut mlp, base + 0x90 + k * 4, v as u32);
            }
            for (k, v) in [nv, nv, 16, 0, 0, nv * 16].into_iter().enumerate() {
                word(&mut mlp, base + vp + k * 4, v);
            }
            for (k, v) in [nv, 5, nv * 16, nv * 2, 1, 0].into_iter().enumerate() {
                word(&mut mlp, base + ip + k * 4, v);
            }
            for j in 0..ns {
                word(&mut mlp, base + 192 + j * 96 + 32, j as u32 * 3);
                word(&mut mlp, base + 192 + j * 96 + 36, 3);
                word(&mut mlp, base + 192 + j * 96 + 52, 7 + j as u32);
            }
            let points = [
                [1., 2., 3.],
                [4., 2., 3.],
                [1., 5., 3.],
                [10., 2., 3.],
                [11., 2., 3.],
                [10., 3., 3.],
            ];
            for (j, point) in points.iter().take(nv as usize).enumerate() {
                for (k, &value) in point.iter().enumerate() {
                    float(&mut mmp, offset + j * 16 + k * 4, value);
                }
                word(&mut mmp, offset + j * 16 + 12, 0x7fc00000); // +Z =511*64/32767, the RSX10-bit lane.
                mmp[offset + nv as usize * 16 + j * 2..offset + nv as usize * 16 + j * 2 + 2]
                    // Source CW winding, matching the disc's render convention.
                    .copy_from_slice(&([0u16, 2, 1, 3, 5, 4][j]).to_be_bytes());
            }
        }
        let mut scp = vec![0; 72 + 3 * 48 + 3 * 64];
        word(&mut scp, 0, 0xbabf003c);
        word(&mut scp, 16, 3);
        word(&mut scp, 20, 72);
        for i in 0..3 {
            word(&mut scp, 72 + i * 48, (216 + i * 64) as u32);
            word(&mut scp, 72 + i * 48 + 8, if i == 2 { 101 } else { 100 });
            matrix(&mut scp, 216 + i * 64, i == 1, if i == 1 { 20. } else { 0. });
        }
        Self {
            scp,
            mlp,
            mmp,
            mgp: vec![],
        }
    }
    fn decode(&self) -> primase_shred::Result<scene::Scene> {
        scene::decode(&self.scp, &self.mlp, &self.mmp, &self.mgp)
    }
    fn pak(&self) -> Pak {
        let names = b"scene.scp\0scene.mlp\0scene.mmp\0scene.mgp\0";
        let mut bytes = vec![0; 16 + 4 * 32 + names.len()];
        bytes[..4].copy_from_slice(b"PAK\0");
        bytes[4..8].copy_from_slice(&[0xfe, 0xeb, 0, 4]);
        word(&mut bytes, 8, 4);
        word(&mut bytes, 12, names.len() as u32);
        bytes[144..].copy_from_slice(names);
        for (i, payload) in [&self.scp, &self.mlp, &self.mmp, &self.mgp]
            .into_iter()
            .enumerate()
        {
            let at = 16 + i * 32;
            word(&mut bytes, at, primase_hash::sdbm(&names[i * 10..i * 10 + 9]));
            word(&mut bytes, at + 4, i as u32 * 10);
            word(&mut bytes, at + 12, 16);
            let len = bytes.len() as u32;
            word(&mut bytes, at + 16, len);
            word(&mut bytes, at + 20, payload.len() as u32);
            bytes.extend(payload);
        }
        Pak::from_bytes(bytes).expect("valid hand-worked PAK envelope")
    }
}

#[test]
fn hand_worked_world_and_mirror() {
    let f = Fixture::new();
    let p = f.pak();
    let frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 0.5);
    let (w, r) = level::read_level(&[("fixture", &p)], frame).unwrap();
    assert_eq!(
        (
            r.quartets,
            r.meshes,
            r.rigid,
            r.refused,
            r.submeshes,
            r.placements
        ),
        (1, 2, 2, 0, 3, 3)
    );
    assert!(w.verify_components().is_empty());
    assert_eq!(w.column::<MaterialId>().unwrap().len(), 2);
    let meshes = w.column::<Mesh>().unwrap();
    let first = meshes.iter().next().unwrap().1;
    let mut gate = primase_core::gate::GateReport::new(
        "hand-worked quartet/frame/index ranges",
        primase_core::gate::Tolerance::Exact,
    );
    assert_eq!(first.primitives().len(), 2);
    assert_eq!(
        first.primitives()[0].positions().unwrap(),
        &[[0.5, 1., -1.5], [2., 1., -1.5], [0.5, 2.5, -1.5]]
    );
    assert_eq!(first.primitives()[1].positions().unwrap()[0], [5., 1., -1.5]);
    for prim in first.primitives() {
        assert_eq!(prim.indices.as_deref(), Some([0, 2, 1].as_slice()));
        let actual: Vec<_> = prim
            .indices
            .as_ref()
            .unwrap()
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect();
        let expected: Vec<_> = [0u32, 2, 1].into_iter().flat_map(|n| n.to_le_bytes()).collect();
        gate.compare_bytes("submesh indices rebased and mirrored", &actual, &expected);
    }
    gate.assert_passed();
    eprintln!("{gate}");
    let placements: Vec<_> = w.column::<Instance>().unwrap().iter().collect();
    assert_eq!(placements[0].1.prototype, placements[1].1.prototype);
    assert_ne!(placements[0].1.prototype, placements[2].1.prototype);
    let t = w.get::<Transform>(placements[1].0).unwrap();
    assert_eq!(
        t.cols,
        [[-1., 0., -0.], [0., 1., -0.], [-0., -0., 1.], [10., 0., -0.]]
    );
    let mut reader = ShredReader::new(vec![("fixture", &p)], frame);
    let (plan, _) = reader.read().unwrap();
    assert!(plan.verify_components().is_empty());
    assert_eq!(
        reader.prototype_box(&plan, placements[0].1.prototype),
        Some([[0.5, 1., -1.5], [5.5, 2.5, -1.5]])
    );
    let (part, _) = reader.load(&plan, &BTreeSet::from([placements[1].0])).unwrap();
    assert_eq!(part.column::<Mesh>().unwrap().len(), 1);
    assert_eq!(part.column::<MaterialId>().unwrap().len(), 2);
    assert_eq!(part.get::<Mesh>(placements[0].1.prototype).unwrap(), first);
}

#[test]
fn precise_refusals_and_eof() {
    let mut f = Fixture::new();
    word(&mut f.mlp, 0, 0);
    assert!(f.decode().unwrap_err().to_string().contains("mlp: bad magic"));
    let mut f = Fixture::new();
    f.mmp[96..98].copy_from_slice(&6u16.to_be_bytes());
    assert!(
        f.decode()
            .unwrap_err()
            .to_string()
            .contains("mmp: index out of range")
    );
    let mut f = Fixture::new();
    word(&mut f.mlp, 84 + 192 + 96 + 32, 6);
    assert!(f.decode().is_err());
    let mut f = Fixture::new();
    word(&mut f.mlp, 84 + 192 + 36, 0);
    assert!(f.decode().unwrap_err().to_string().contains("partition gap"));
    let mut f = Fixture::new();
    word(&mut f.scp, 72 + 8, 999);
    assert!(f.decode().unwrap_err().to_string().contains("missing mesh"));
    let mut f = Fixture::new();
    f.mmp.push(1);
    assert!(f.decode().unwrap_err().to_string().contains("uncovered bytes"));
    let f = Fixture::new();
    for n in 0..f.mlp.len() {
        assert!(
            scene::decode(&f.scp, &f.mlp[..n], &f.mmp, &f.mgp).is_err(),
            "truncated MLP {n}"
        );
    }
}

#[test]
fn embedded_mesh_does_not_hide_rigid_sibling() {
    let mut f = Fixture::new();
    // First model has index-only MMP and a stride-36 stream descriptor; its payload remains explicitly unread.
    word(&mut f.mlp, 84 + 0x9c, 420);
    word(&mut f.mlp, 84 + 384 + 8, 36);
    word(&mut f.mlp, 84 + 384 + 20, 216);
    word(&mut f.mlp, 84 + 408 + 8, 0);
    word(&mut f.mlp, 28 + 20, 12);
    word(&mut f.mlp, 56 + 8, 12);
    f.mmp.drain(..96);
    let scene = f.decode().unwrap();
    assert!(scene.models[0].refusal.is_some());
    assert_eq!(scene.models[1].positions.len(), 3);
    assert_eq!(scene.placements.len(), 3);
    let p = f.pak();
    let (w, r) = level::read_level(&[("fixture", &p)], SourceFrame::WORLD).unwrap();
    assert_eq!((r.rigid, r.refused), (1, 1));
    assert_eq!(w.column::<Mesh>().unwrap().len(), 1);
    assert_eq!(w.column::<RefusedMesh>().unwrap().len(), 1);
    assert_eq!(w.column::<Instance>().unwrap().len(), 3);
}

#[test]
fn source_clockwise_faces_become_core_counterclockwise_in_either_frame() {
    let f = Fixture::new();
    assert_eq!(&f.decode().unwrap().models[0].indices[..3], &[0, 2, 1]);
    let pak = f.pak();
    for frame in [
        SourceFrame::WORLD,
        SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.),
    ] {
        let (world, _) = level::read_level(&[("fixture", &pak)], frame).unwrap();
        for (_, mesh) in world.column::<Mesh>().unwrap().iter() {
            for primitive in mesh.primitives() {
                let points = primitive.positions().unwrap();
                let Stream::F32x3(normals) = &primitive.streams[&Semantic::Normal] else {
                    panic!("normal stream missing")
                };
                let indices = primitive.indices.as_ref().unwrap();
                assert_eq!(
                    &indices[..3],
                    if frame.flips_winding() {
                        &[0, 2, 1]
                    } else {
                        &[0, 1, 2]
                    }
                );
                let [a, b, c] = [
                    points[indices[0] as usize],
                    points[indices[1] as usize],
                    points[indices[2] as usize],
                ];
                let u = std::array::from_fn::<_, 3, _>(|k| b[k] - a[k]);
                let v = std::array::from_fn::<_, 3, _>(|k| c[k] - a[k]);
                let face = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let n = normals[indices[0] as usize];
                let dot: f32 = face.into_iter().zip(n).map(|(a, b)| a * b).sum();
                assert!(dot > 0., "face must point with its authored normal in {frame:?}");
            }
        }
    }
}

/// Oracle: literal signed CMP words, IEEE half values and native declaration offsets worked by hand.
#[test]
fn every_packed_layout_family_and_source_lanes() {
    for (mask, stride) in [
        (0x80005, 0),
        (0x80007, 4),
        (0x80025, 4),
        (0x80027, 8),
        (0x8003d, 12),
        (0x8003f, 16),
        (0x80065, 8),
        (0x80067, 12),
        (0x8007d, 16),
        (0x800e5, 12),
        (0x800fd, 20),
        (0x800ff, 24),
        (0x801e5, 16),
        (0x801fd, 24),
        (0x801ff, 28),
        (0x807fd, 40),
    ] {
        let f = Fixture::attributes(mask, stride);
        let scene = f.decode().unwrap();
        let a = &scene.models[0].attributes;
        assert_eq!(a.normals, vec![[0., 0., 32704. / 32767.]; 6]);
        assert_eq!(a.zero_normals, 0);
        assert_eq!(
            a.colors.as_ref().map(|c| c[0]),
            (mask & 2 != 0).then_some([1., 128. / 255., 0., 64. / 255.])
        );
        for (i, uv) in [[0.5, -1.], [2., 0.25], [-2., 0.75], [1., 0.]]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                a.uvs[i].as_ref().map(|v| v[0]),
                (mask & (1 << (i + 5)) != 0).then_some(uv)
            );
        }
        if mask & 8 != 0 {
            assert_eq!(a.raw["attr15"].values[0], [32736. / 32767., 0., 0., 0.]);
        }
        if mask & 16 != 0 {
            assert_eq!(a.raw["attr14"].values[0], [0., 32736. / 32767., 0., 0.]);
        }
        if mask & 0x600 != 0 {
            assert_eq!(a.raw["attr12"].values[0], [1., -2., 0.5, 0.]);
            assert_eq!(a.raw["attr13"].values[0], [2., 3., 4., 0.]);
        }
    }
    let n = primase_shred::read::attributes::cmp(0x80000400); // x=-1024, y=0, z=-512.
    assert_eq!(n, [-32768. / 32767., 0., -32768. / 32767.]);
}

#[test]
fn remapped_uvs_source_inputs_frame_and_deferred_loading() {
    let mut f = Fixture::attributes(0x801ff, 28);
    word(&mut f.mlp, 84 + 192 + 56, 0x0200ff01);
    word(&mut f.mlp, 84 + 192 + 96 + 56, 0xffffffff);
    // A distinct third source vertex makes remap ordering observable.
    f.mgp[2 * 28 + 24..2 * 28 + 26].copy_from_slice(&0x4000u16.to_be_bytes());
    let pak = f.pak();
    let frame = SourceFrame::new([Axis::PosZ, Axis::PosX, Axis::NegY], 0.5);
    let (mut world, report) = level::read_level(&[("fixture", &pak)], frame).unwrap();
    assert_eq!(report.zero_normals, 0);
    let (e, mesh) = world.column::<Mesh>().unwrap().iter().next().unwrap();
    let first = &mesh.primitives()[0];
    assert_eq!(
        first.streams[&Semantic::Normal],
        Stream::F32x3(vec![[32704. / 32767., 0., -0.]; 3])
    );
    for (set, uv) in [[-2., 0.75], [0.5, -1.], [-2., 0.75], [2., 0.25]]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            first.streams[&Semantic::TexCoord(set as u8)],
            Stream::F32x2(vec![uv; 3])
        );
    }
    let inputs = world.get::<VertexInputs>(e).unwrap().clone();
    assert_eq!(inputs.frame, frame);
    assert_eq!(
        inputs.primitives[0]["stored_uv3"].values,
        vec![[1., 0., 0., 0.], [1., 0., 0., 0.], [2., 0., 0., 0.]]
    );
    assert!(
        !first.streams.contains_key(&Semantic::Tangent(0)),
        "unproved tangent handedness must remain named source inputs"
    );
    let expected = inputs.remap_vertices(0, &[2, 0, 2]).unwrap().unwrap();
    assert_eq!(
        expected.primitives[0]["stored_uv3"].values,
        vec![[2., 0., 0., 0.], [1., 0., 0., 0.], [2., 0., 0., 0.]]
    );
    world.remap_vertices(e, 0, &[2, 0, 2]).unwrap();
    assert_eq!(world.get::<VertexInputs>(e), Some(&expected));
    assert!(world.verify_components().is_empty());
    assert!(inputs.remap_vertices(0, &[99]).unwrap().is_err());
    assert_eq!(
        inputs
            .remap_primitives(&[1, 0, 1])
            .unwrap()
            .unwrap()
            .primitives
            .len(),
        3
    );
    let mut reader = ShredReader::new(vec![("fixture", &pak)], frame);
    let (plan, _) = reader.read().unwrap();
    assert!(plan.column::<VertexInputs>().is_none_or(|c| c.is_empty()));
    let (loaded, _) = reader.load(&plan, &BTreeSet::from([e])).unwrap();
    assert_eq!(loaded.get::<VertexInputs>(e), Some(&inputs));
    assert!(loaded.verify_components().is_empty());
}

#[test]
fn invalid_masks_refuse_one_mesh_and_bad_values_are_not_repaired() {
    let mut f = Fixture::new();
    word(&mut f.mlp, 84 + 0x8c, 0x180005);
    let scene = f.decode().unwrap();
    assert!(scene.models[0].refusal.unwrap().1.contains("format mask"));
    assert!(scene.models[1].refusal.is_none());
    let mut f = Fixture::new();
    f.mlp[84 + 192 + 96 + 56 + 2] = 4;
    let scene = f.decode().unwrap();
    assert_eq!(
        scene.models[0].refusal,
        Some((84 + 192 + 96 + 56 + 2, "unsupported UV source selector"))
    );
    assert!(scene.models[1].refusal.is_none());
    let mut f = Fixture::attributes(0x80025, 4);
    f.mgp.pop();
    assert!(f.decode().unwrap_err().to_string().contains("mgp:"));
    let mut f = Fixture::attributes(0x80025, 4);
    f.mgp[..2].copy_from_slice(&0x7c00u16.to_be_bytes());
    assert!(f.decode().unwrap_err().to_string().contains("nonfinite half"));
    let mut f = Fixture::attributes(0x80025, 4);
    word(&mut f.mlp, 84 + 0x8c, 0x80065);
    assert!(f.decode().unwrap_err().to_string().contains("MGP stride differs"));
    let mut f = Fixture::new();
    word(&mut f.mmp, 12, 1);
    assert!(f.decode().unwrap_err().to_string().contains("nonunit CMP normal"));
    let mut f = Fixture::new();
    word(&mut f.mmp, 12, 0);
    let pak = f.pak();
    let (world, r) = level::read_level(&[("fixture", &pak)], SourceFrame::WORLD).unwrap();
    assert_eq!(r.zero_normals, 1);
    assert_eq!(r.normal_exceptions.len(), 1);
    assert!(world.verify_components().is_empty());
}
