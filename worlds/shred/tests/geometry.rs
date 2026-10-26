//! Hand-worked quartet: disjoint vertex ranges, two draws, shared materials and a mirrored instance.
use primase_core::{
    Instance, Mesh, Transform,
    convert::Reader,
    frame::{Axis, SourceFrame},
};
use primase_shred::{
    components::{MaterialId, RefusedMesh},
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
                mmp[offset + nv as usize * 16 + j * 2..offset + nv as usize * 16 + j * 2 + 2]
                    .copy_from_slice(&(j as u16).to_be_bytes());
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
