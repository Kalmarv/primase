//! Hand-worked quartet: disjoint vertex ranges, two draws, shared materials and a mirrored instance.
use primase_core::{
    Component, Instance, Mesh, Semantic, Stream, Transform,
    convert::Reader,
    frame::{Axis, SourceFrame},
};
use primase_shred::{
    components::{
        MaterialId, MaterialNamespace, MeshId, RefusedMesh, SourceSkeleton, SourceSkin, VertexInputs,
    },
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
    /// Literal two-palette processed stream, with a separate rigid sibling and named source skeleton.
    fn processed() -> Self {
        let mut f = Self::new();
        let positions: Vec<_> = f.mmp[..96]
            .as_chunks::<16>()
            .0
            .iter()
            .map(|v| v[..12].to_vec())
            .collect();
        let ep = 432;
        let stream_size = 640;
        let metadata_size = ep + stream_size + 16;
        f.mlp.splice(84 + ep..84 + ep, vec![0; stream_size + 16]);
        word(&mut f.mlp, 28 + 16, metadata_size as u32);
        word(&mut f.mlp, 28 + 20, 12);
        word(&mut f.mlp, 56 + 4, metadata_size as u32);
        word(&mut f.mlp, 56 + 8, 12);
        word(&mut f.mlp, 84 + 0x8c, 0x8007f);
        word(&mut f.mlp, 84 + 0x9c, ep as u32);
        word(&mut f.mlp, 84 + 0xb4, 1);
        word(&mut f.mlp, 84 + 384 + 8, 36);
        word(&mut f.mlp, 84 + 384 + 20, 6 * 36);
        word(&mut f.mlp, 84 + 408 + 8, 0);
        word(&mut f.mlp, 84 + 192 + 56, 0x0100ffff);
        word(&mut f.mlp, 84 + 192 + 96 + 56, 0xffffffff);
        f.mmp.drain(..96);
        let start = 84 + ep;
        word(&mut f.mlp, start, stream_size as u32);
        for i in 0..15 {
            word(&mut f.mlp, start + 4 + i * 4, 0xaabbccdd);
        }
        for batch in 0..2 {
            let header = start + 64 + batch * 256;
            f.mlp[header..header + 4].copy_from_slice(if batch == 0 {
                &[0, 1, 255, 255]
            } else {
                &[2, 255, 255, 255]
            });
            f.mlp[header + 4..header + 6].copy_from_slice(&3u16.to_be_bytes());
            f.mlp[header + 6..header + 8].copy_from_slice(&(if batch == 0 { 2u16 } else { 1 }).to_be_bytes());
            for i in 0..14 {
                word(&mut f.mlp, header + 8 + i * 4, 0xddccbbaa);
            }
            for j in 0..3 {
                let p = header + 64 + j * 64;
                f.mlp[p..p + 12].copy_from_slice(&positions[batch * 3 + j]);
                let weights = if batch == 0 {
                    [[128, 127, 0, 0], [64, 191, 0, 0], [0, 255, 0, 0]][j]
                } else {
                    [255, 0, 0, 0]
                };
                f.mlp[p + 12..p + 16].copy_from_slice(&weights);
                float(&mut f.mlp, p + 24, 1.);
                word(&mut f.mlp, p + 28, 0x11223344);
                float(&mut f.mlp, p + 32, 1.);
                float(&mut f.mlp, p + 44, -1.);
                for (k, value) in [0x3800u16, 0xbc00, 0x4000, 0x3400].into_iter().enumerate() {
                    f.mlp[p + 48 + k * 2..p + 50 + k * 2].copy_from_slice(&value.to_be_bytes());
                }
                f.mlp[p + 56..p + 64].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
            }
        }
        // Zero-count terminator occupies the last 64 stream bytes; the 16-byte footer stays opaque.
        let table = f.scp.len();
        f.scp.resize(table + 3 * 16 + 3 * 64, 0);
        word(&mut f.scp, 0x20, 3);
        word(&mut f.scp, 0x24, table as u32);
        for (i, name) in [b"root".as_slice(), b"joint", b"toe"].into_iter().enumerate() {
            let text = f.scp.len();
            f.scp.extend_from_slice(name);
            f.scp.push(0);
            let transform = table + 48 + i * 64;
            matrix(&mut f.scp, transform, false, 10. + i as f32);
            word(
                &mut f.scp,
                table + i * 16,
                if i == 0 { 254 } else { 96 + i as u32 },
            );
            word(
                &mut f.scp,
                table + i * 16 + 4,
                if i == 0 { u32::MAX } else { i as u32 - 1 },
            );
            word(&mut f.scp, table + i * 16 + 8, text as u32);
            word(&mut f.scp, table + i * 16 + 12, transform as u32);
        }
        f
    }
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
        self.pak_named("scene")
    }
    fn pak_named(&self, stem: &str) -> Pak {
        let paths: Vec<_> = ["scp", "mlp", "mmp", "mgp"]
            .map(|ext| format!("{stem}.{ext}"))
            .into_iter()
            .collect();
        let mut names = Vec::new();
        let mut name_offsets = Vec::new();
        for path in &paths {
            name_offsets.push(names.len());
            names.extend(path.as_bytes());
            names.push(0);
        }
        let mut bytes = vec![0; 16 + 4 * 32 + names.len()];
        bytes[..4].copy_from_slice(b"PAK\0");
        bytes[4..8].copy_from_slice(&[0xfe, 0xeb, 0, 4]);
        word(&mut bytes, 8, 4);
        word(&mut bytes, 12, names.len() as u32);
        bytes[144..].copy_from_slice(&names);
        for (i, payload) in [&self.scp, &self.mlp, &self.mmp, &self.mgp]
            .into_iter()
            .enumerate()
        {
            let at = 16 + i * 32;
            word(
                &mut bytes,
                at,
                primase_foundations::hash::sdbm(paths[i].as_bytes()),
            );
            word(&mut bytes, at + 4, name_offsets[i] as u32);
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
fn zero_material_ids_are_scoped_by_quartet_namespace_in_one_world() {
    let mut f = Fixture::new();
    for at in [84 + 192 + 52, 84 + 192 + 96 + 52, 84 + 432 + 192 + 52] {
        word(&mut f.mlp, at, 0);
    }
    let env = f.pak_named("levels\\fixture");
    let actor = f.pak_named("actors\\fixture");
    let (world, _) = level::read_level(&[("env", &env), ("actor", &actor)], SourceFrame::WORLD).unwrap();
    assert_eq!(world.iter::<MaterialId>().count(), 2);
    for (e, mesh) in world.iter::<Mesh>() {
        let expected = if world.get::<MeshId>(e).unwrap().pak == "env" {
            b"levels".as_slice()
        } else {
            b"actors".as_slice()
        };
        for primitive in mesh.primitives() {
            let mat = primitive.material.unwrap();
            assert_eq!(world.get::<MaterialId>(mat).unwrap().0, 0);
            assert_eq!(world.get::<MaterialNamespace>(mat).unwrap().0, expected);
        }
    }
}

/// Literal placement words survive frame conversion and keep both graph and prototype reachable.
#[test]
fn placement_source_identity_survives_a_subset_without_selecting_a_session() {
    use primase_shred::components::{SessionGraph, SourcePlacement};
    let mut f = Fixture::new();
    let off = u32::from_be_bytes(f.scp[20..24].try_into().expect("placement offset")) as usize;
    word(&mut f.scp, off + 12, 0x00411041); // sdbm("AB")
    word(&mut f.scp, off + 24, 0x12345678); // unknown stays exact
    let p = f.pak();
    let graph = SessionGraph::from_main_db(include_bytes!("fixtures/sessions.db"), 7).expect("graph");
    let mut reader = ShredReader::new(vec![("fixture", &p)], SourceFrame::WORLD).with_sessions(graph);
    let (world, _) = reader.read().expect("fixture read");
    assert_eq!(world.iter::<Instance>().count(), 3);
    assert_eq!(world.iter::<SourcePlacement>().count(), 3);
    let (e, source) = world
        .iter::<SourcePlacement>()
        .find(|(_, s)| s.index == 0)
        .expect("placement0");
    assert_eq!(source.name, 0x00411041);
    assert_eq!(source.words[6], 0x12345678);
    assert_eq!(source.offset, off);
    assert_eq!(
        world.get::<Instance>(e).expect("instance").prototype,
        source.prototype
    );
    let (part, _) = reader.load(&world, &BTreeSet::from([e])).expect("subset");
    assert!(part.resource::<SessionGraph>().is_some());
    assert!(part.get::<Mesh>(source.prototype).is_some());
    assert_eq!(part.get::<SourcePlacement>(e), Some(source));
    assert!(part.verify_components().is_empty());
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
    let mut gate = primase_testdata::gate::GateReport::new(
        "hand-worked quartet/frame/index ranges",
        primase_testdata::gate::Tolerance::Exact,
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

/// A lowering that runs on the read world needs the meshes there, and what it changes is what `load` returns.
#[test]
fn a_pass_on_the_read_world_sees_meshes_and_its_change_survives_load() {
    let p = Fixture::new().pak();
    let frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0);
    let mut reader = ShredReader::new(vec![("fixture", &p)], frame);
    let (mut world, _) = reader.read().unwrap();
    assert_eq!(world.entities_named("core.mesh").len(), 2);
    assert!(world.entities_named("shred.model_dependencies").is_empty());
    let placements: Vec<_> = world
        .column::<Instance>()
        .unwrap()
        .iter()
        .map(|(e, i)| (e, i.prototype))
        .collect();
    let (root, prototype) = placements[0];
    // the pass: lift every vertex of the prototype by 100 and drop its second draw
    let mesh = world.remove::<Mesh>(prototype).unwrap();
    let mut primitives = mesh.primitives().to_vec();
    primitives.truncate(1);
    for v in primitives[0].streams.values_mut() {
        if let Stream::F32x3(points) = v {
            points.iter_mut().for_each(|p| p[1] += 100.);
        }
    }
    let lowered = Mesh::new(primitives);
    world.insert(prototype, lowered.clone());
    // the selection: the root alone
    let (part, _) = reader.load(&world, &BTreeSet::from([root])).unwrap();
    assert_eq!(part.get::<Mesh>(prototype), Some(&lowered));
    assert!(part.verify_components().is_empty());
    // the whole source loads in place, unchanged
    let n = world.entities().count();
    let (all, _) = reader.load_all(world).unwrap();
    assert_eq!(all.entities().count(), n);
    assert_eq!(all.get::<Mesh>(prototype), Some(&lowered));
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
fn processed_source_geometry_skeleton_and_remapping() {
    let fixture = Fixture::processed();
    let pak = fixture.pak();
    let frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 0.5);
    let (mut world, report) = level::read_level(&[("fixture", &pak)], frame).unwrap();
    assert_eq!(
        (
            report.rigid,
            report.processed,
            report.refused,
            report.processed_batches
        ),
        (1, 1, 0, 2)
    );
    assert_eq!((report.source_skeletons, report.source_bones), (1, 3));
    assert!(world.verify_components().is_empty());
    let entity = world.column::<SourceSkin>().unwrap().iter().next().unwrap().0;
    let skin = world.get::<SourceSkin>(entity).unwrap().clone();
    let skeleton = world.get::<SourceSkeleton>(skin.skeleton).unwrap();
    assert_eq!(skeleton.frame, frame);
    assert_eq!(skeleton.bones[0].name, b"root");
    assert_eq!(skeleton.bones[1].words, [97, 0]);
    assert_eq!(skeleton.bones[1].matrix.cols[3], [11., 0., 0.]);
    assert!(world.column::<primase_core::skeleton::Skeleton>().is_none());
    assert!(world.column::<primase_core::skin::Skin>().is_none());
    let mesh = world.get::<Mesh>(entity).unwrap();
    let first = &mesh.primitives()[0];
    assert_eq!(first.positions().unwrap()[0], [0.5, 1., -1.5]);
    assert_eq!(
        first.streams[&Semantic::Normal],
        Stream::F32x3(vec![[0., 0., -1.]; 3])
    );
    assert_eq!(
        first.streams[&Semantic::TexCoord(0)],
        Stream::F32x2(vec![[2., 0.25]; 3])
    );
    assert_eq!(
        first.streams[&Semantic::TexCoord(1)],
        Stream::F32x2(vec![[0.5, -1.]; 3])
    );
    assert!(!first.streams.contains_key(&Semantic::TexCoord(2)));
    assert!(!first.streams.contains_key(&Semantic::Tangent(0)));
    assert_eq!(
        world.get::<VertexInputs>(entity).unwrap().primitives[0]["skin_basis"].values[0],
        [1., 0., 0., -1.]
    );
    assert_eq!(skin.primitives[0][0].bones, [0, 1, 255, 255]);
    assert_eq!(skin.primitives[0][0].weights, [128, 127, 0, 0]);
    assert_eq!(skin.primitives[0][0].extra, 0x11223344);
    assert_eq!(skin.primitives[0][0].padding, [1, 2, 3, 4, 5, 6, 7, 8]);
    let mapped = skin.remap_primitives(&[1, 0, 1]).unwrap().unwrap();
    assert_eq!(mapped.primitives[0][0].bones, [2, 255, 255, 255]);
    assert_eq!(mapped.primitives[1], skin.primitives[0]);
    world.remap_vertices(entity, 0, &[2, 0, 2]).unwrap();
    let mapped = world.get::<SourceSkin>(entity).unwrap();
    assert_eq!(
        mapped.primitives[0].iter().map(|v| v.weights).collect::<Vec<_>>(),
        vec![[0, 255, 0, 0], [128, 127, 0, 0], [0, 255, 0, 0]]
    );
    assert!(skin.remap_vertices(0, &[99]).unwrap().is_err());
    assert!(world.verify_components().is_empty());
    let mut reader = ShredReader::new(vec![("fixture", &pak)], frame);
    let (mut plan, _) = reader.read().unwrap();
    assert!(plan.get::<Mesh>(entity).is_some());
    assert_eq!(plan.get::<SourceSkin>(entity), Some(&skin));
    assert!(plan.get::<SourceSkeleton>(skin.skeleton).is_some());
    plan.remap_vertices(entity, 0, &[2, 0, 2]).unwrap();
    assert!(plan.verify_components().is_empty());
    let (loaded, _) = reader.load(&plan, &BTreeSet::from([entity])).unwrap();
    assert_eq!(loaded.get::<SourceSkin>(entity), world.get::<SourceSkin>(entity));
    assert!(loaded.get::<SourceSkeleton>(skin.skeleton).is_some());
    assert!(loaded.verify_components().is_empty());
}

#[test]
fn processed_nan_normals_preserve_bits_and_omit_only_the_affected_primitive() {
    let mut fixture = Fixture::processed();
    word(&mut fixture.mlp, 84 + 432 + 128 + 64 + 16, 0x7fa12345);
    let pak = fixture.pak();
    let (world, report) = level::read_level(&[("fixture", &pak)], SourceFrame::WORLD).unwrap();
    assert_eq!((report.nonfinite_normals, report.omitted_normal_streams), (1, 1));
    assert_eq!(report.nonfinite_normal_exceptions.len(), 1);
    let (entity, skin) = world.column::<SourceSkin>().unwrap().iter().next().unwrap();
    assert_eq!(skin.primitives[0][1].normal_bits[0], 0x7fa12345);
    let mesh = world.get::<Mesh>(entity).unwrap();
    assert!(!mesh.primitives()[0].streams.contains_key(&Semantic::Normal));
    assert!(mesh.primitives()[1].streams.contains_key(&Semantic::Normal));
    assert!(world.verify_components().is_empty());
}

#[test]
fn processed_refusals_and_malformed_data() {
    let mut fixture = Fixture::processed();
    word(&mut fixture.mlp, 84 + 0xb4, 0);
    let scene = fixture.decode().unwrap();
    assert!(
        scene.models[0]
            .refusal
            .unwrap()
            .1
            .contains("processed skin format")
    );
    assert!(scene.models[1].refusal.is_none());
    for (offset, bytes, reason) in [
        (84 + 432 + 64, vec![3], "bone palette"),
        (84 + 432 + 64 + 4, vec![0, 0], "batch vertex count"),
        (84 + 432 + 128 + 12, vec![127], "weights must sum"),
        (84 + 432 + 128 + 48, vec![0x7c, 0], "processed UV"),
        (84 + 432 + 640 - 64 + 4, vec![0, 1], "terminal/size"),
    ] {
        let mut fixture = Fixture::processed();
        fixture.mlp[offset..offset + bytes.len()].copy_from_slice(&bytes);
        assert!(
            fixture.decode().unwrap_err().to_string().contains(reason),
            "{reason}"
        );
    }
    let mut fixture = Fixture::processed();
    word(&mut fixture.scp, 0x20, 0);
    assert!(
        fixture
            .decode()
            .unwrap_err()
            .to_string()
            .contains("no source skeleton")
    );
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
    assert_eq!(plan.get::<VertexInputs>(e), Some(&inputs));
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
