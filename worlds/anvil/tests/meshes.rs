//! Real Meshes read into a world through a stated frame and lowered to core meshes, under the pass manager with its
//! checks on (Riders Republic build 25394987). Counts, splits and bounds as a separate read-only decode found them
//! (games/riders-republic docs/recon/mesh-second-fixture.md).
use std::path::Path;

use primase_anvil::components::AnvilMesh;
use primase_anvil::passes::{LowerAnvilMesh, MeshNormals, UvV};
use primase_anvil::read::meshes::{Error, read_meshes};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Instance, Mesh, Semantic, Stream, World};
use primase_testdata::real_data;

/// The entry's buffers as read in the diagnostic frame (the stored numbers unconverted, not a claim that they are
/// metres): the world, the prototype's buffers.
fn read(path: &Path, entry: usize) -> (World, AnvilMesh) {
    let mut w = World::new();
    let placed = read_meshes(path, &[entry], SourceFrame::WORLD, &mut w).expect("read");
    let prototype = w.get::<Instance>(placed[0]).expect("placed").prototype;
    let mesh = w.get::<AnvilMesh>(prototype).expect("buffers").clone();
    (w, mesh)
}

fn lower(w: &mut World, normals: MeshNormals) {
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![Box::new(LowerAnvilMesh {
            uv_divisor: 2048.0,
            uv_v: UvV::AsStored,
            normals,
        })],
    };
    let mut report = RunReport::default();
    manager.run(w, &pipeline, &mut report).expect("lower");
}

/// The least and greatest of each coordinate.
fn extrema(positions: &[[f32; 3]]) -> [[f32; 3]; 2] {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    [lo, hi]
}

/// How many triangles' stored normals (the first direction group read as `b / 127.5 - 1`) face the way their winding
/// does, and how many don't.
fn facing(m: &AnvilMesh) -> (usize, usize) {
    let n = |i: u32| m.direction_a[i as usize].map(|b| f32::from(b) / 127.5 - 1.0);
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let mut agree = (0, 0);
    for &[a, b, c] in &m.triangles {
        let p = |i: u32| m.positions[i as usize];
        let (u, v) = (sub(p(b), p(a)), sub(p(c), p(a)));
        let face = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let normal = [0, 1, 2].map(|i| n(a)[i] + n(b)[i] + n(c)[i]);
        if face.iter().zip(normal).map(|(f, n)| f * n).sum::<f32>() > 0.0 {
            agree.0 += 1;
        } else {
            agree.1 += 1;
        }
    }
    agree
}

#[test]
fn black_box_and_reversed_sphere_read_and_lower() {
    let Some(path) = real_data("PRIMASE_ANVIL_FORGE_BASE") else {
        return;
    };
    for (entry, vertices, triangles) in [(161, 56, 46), (318, 559, 960)] {
        let (mut w, m) = read(&path, entry);
        assert_eq!(
            (m.entry as usize, m.positions.len(), m.triangles.len()),
            (entry, vertices, triangles)
        );
        let prototype = w.iter::<AnvilMesh>().map(|(e, _)| e).next().expect("one");
        lower(&mut w, MeshNormals::Packed);
        assert!(!w.has::<AnvilMesh>(prototype));
        let p = &w.get::<Mesh>(prototype).expect("lowered").primitives()[0];
        assert_eq!(p.indices.as_ref().expect("indexed").len(), triangles * 3);
        let Some(Stream::F32x3(normals)) = p.streams.get(&Semantic::Normal) else {
            panic!("normals stated packed");
        };
        assert_eq!(normals.len(), vertices);
        let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(0)) else {
            panic!("texture coordinates");
        };
        assert_eq!(uv.len(), vertices);
    }
    // a Mesh entry is found by the record's type; the MeshShape a few entries on is refused
    let mut w = World::new();
    let not = (0..388)
        .map(|e| read_meshes(&path, &[e], SourceFrame::WORLD, &mut w))
        .find_map(|r| match r {
            Err(Error::NotAMesh { entry, .. }) => Some(entry),
            _ => None,
        });
    assert!(not.is_some(), "the base forge holds entries that aren't Meshes");
}

#[test]
fn stand_reads_its_split_and_its_bounds() {
    let Some(path) = real_data("PRIMASE_ANVIL_FORGE") else {
        return;
    };
    let (mut w, m) = read(&path, 2);
    assert_eq!((m.positions.len(), m.triangles.len()), (1154, 864));
    // the second range (indices from 2016) rebased by 904
    assert!(m.triangles[672..].iter().flatten().all(|&i| i >= 904));
    let [lo, hi] = extrema(&m.positions);
    let bounds = [
        [-0.246_128, -0.145_569, 0.000_442],
        [0.246_128, 0.282_392, 0.739_698],
    ];
    for i in 0..3 {
        assert!((lo[i] - bounds[0][i]).abs() < 1e-3, "{lo:?}");
        assert!((hi[i] - bounds[1][i]).abs() < 1e-3, "{hi:?}");
    }
    assert_eq!(facing(&m), (864, 0));
    lower(&mut w, MeshNormals::Omit);
    let (_, mesh) = w.iter::<Mesh>().next().expect("lowered");
    assert!(!mesh.primitives()[0].streams.contains_key(&Semantic::Normal));
}

#[test]
fn prada_barrier_reads_three_records_and_its_first_range_normals_diverge() {
    let Some(path) = real_data("PRIMASE_ANVIL_FORGE_USNAT_PATCH") else {
        return;
    };
    let (_, m) = read(&path, 136_194);
    assert_eq!((m.positions.len(), m.triangles.len()), (110, 60));
    assert!(m.triangles[40..].iter().flatten().all(|&i| i >= 70));
    // the second range's normals face their triangles; the first range's don't all (the reason normals are stated)
    let first = AnvilMesh {
        triangles: m.triangles[..40].to_vec(),
        ..m.clone()
    };
    let second = AnvilMesh {
        triangles: m.triangles[40..].to_vec(),
        ..m.clone()
    };
    assert_eq!(facing(&second), (20, 0));
    let (agree, disagree) = facing(&first);
    assert!(agree > 0 && disagree > 0, "{agree}/{disagree}");
}

#[test]
fn the_lowering_states_every_choice() {
    let o = |kv: &[(&str, &str)]| -> Options {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    let all = [
        ("uv-divisor", "2048"),
        ("uv-v", "one-minus"),
        ("normals", "packed"),
    ];
    assert_eq!(
        LowerAnvilMesh::from_options(&o(&all)),
        Ok(LowerAnvilMesh {
            uv_divisor: 2048.0,
            uv_v: UvV::OneMinus,
            normals: MeshNormals::Packed,
        })
    );
    for missing in 0..3 {
        let mut some = all.to_vec();
        some.remove(missing);
        assert!(LowerAnvilMesh::from_options(&o(&some)).is_err());
    }
    for bad in [("uv-divisor", "0"), ("uv-v", "flip"), ("normals", "guess")] {
        let mut some = all.to_vec();
        some.retain(|(k, _)| *k != bad.0);
        some.push(bad);
        assert!(LowerAnvilMesh::from_options(&o(&some)).is_err());
    }
    let mut extra = all.to_vec();
    extra.push(("scale", "1"));
    assert!(LowerAnvilMesh::from_options(&o(&extra)).is_err());
}
