//! Structural census against the game's own tables and AABBs, not a second decoder as semantic oracle.
use primase_core::{
    Instance, Mesh, Semantic, Stream,
    frame::{Axis, SourceFrame},
};
use primase_shred::components::SourceSkin;
use primase_shred::read::{level, pak::Pak};
use primase_testdata::{Game, Query, expect_found, install};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = e.expect("directory entry").path();
        if path.is_dir() {
            archives(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")) {
            out.push(path);
        }
    }
}

#[test]
fn every_quartet_and_mesh_including_mixed_embedded() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", paths.len());
    // These are explicit census frames, with no assertion about the game's physical unit scale.
    for frame in [
        SourceFrame::WORLD,
        SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.),
    ] {
        census(&paths, frame);
    }
}

fn census(paths: &[PathBuf], frame: SourceFrame) {
    let mut totals = [0; 7];
    let mut with_geometry = 0;
    let mut zero_normals = 0;
    let mut cosines = Vec::new();
    for path in paths {
        let pak = Pak::open(path).expect("disc PAK parses");
        let label = path
            .file_name()
            .expect("PAK filename")
            .to_str()
            .expect("UTF-8 PAK filename");
        let (world, r) =
            level::read_level(&[(label, &pak)], frame).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if r.quartets > 0 {
            with_geometry += 1;
        }
        for (a, b) in totals.iter_mut().zip([
            r.quartets,
            r.meshes,
            r.rigid,
            r.processed,
            r.refused,
            r.submeshes,
            r.placements,
        ]) {
            *a += b;
        }
        assert!(
            world.verify_components().is_empty(),
            "{}: {:?}",
            path.display(),
            world.verify_components()
        );
        assert_eq!(
            world.column::<Mesh>().map_or(0, |c| c.len()),
            r.rigid + r.processed
        );
        assert_eq!(world.column::<Instance>().map_or(0, |c| c.len()), r.placements);
        zero_normals += r.zero_normals;
        for (source, count) in &r.normal_exceptions {
            eprintln!(
                "authored zero normals: {} {:?} {:08x}: {count}",
                source.pak,
                String::from_utf8_lossy(&source.quartet),
                source.id
            );
        }
        if let Some(meshes) = world.column::<Mesh>() {
            for (entity, mesh) in meshes.iter() {
                if world.get::<SourceSkin>(entity).is_some() {
                    continue;
                }
                for primitive in mesh.primitives() {
                    let positions = primitive.positions().expect("rigid position stream");
                    let Stream::F32x3(normals) = &primitive.streams[&Semantic::Normal] else {
                        panic!("normal stream missing")
                    };
                    for tri in primitive
                        .indices
                        .as_ref()
                        .expect("rigid index stream")
                        .as_chunks::<3>()
                        .0
                    {
                        let [a, b, c] = tri.map(|i| positions[i as usize].map(f64::from));
                        let u = std::array::from_fn::<_, 3, _>(|k| b[k] - a[k]);
                        let v = std::array::from_fn::<_, 3, _>(|k| c[k] - a[k]);
                        let face = [
                            u[1] * v[2] - u[2] * v[1],
                            u[2] * v[0] - u[0] * v[2],
                            u[0] * v[1] - u[1] * v[0],
                        ];
                        let average = std::array::from_fn::<_, 3, _>(|k| {
                            tri.iter()
                                .map(|&i| f64::from(normals[i as usize][k]))
                                .sum::<f64>()
                        });
                        let dot =
                            |a: [f64; 3], b: [f64; 3]| a.into_iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
                        let scale = (dot(face, face) * dot(average, average)).sqrt();
                        if scale > 1e-12 {
                            cosines.push((dot(face, average) / scale).clamp(-1., 1.));
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "SHRED geometry census: {with_geometry} PAKs; [quartets, meshes, rigid, processed, refused, submeshes, placements] = {totals:?}; positions checked against source AABB (0.05 source units)"
    );
    assert_eq!(with_geometry, 324);
    assert_eq!(totals, [657, 21_309, 17_897, 3_412, 0, 35_114, 168_887]);
    assert_eq!(
        zero_normals, 11_488,
        "stock exceptions counted per stored resource vertex"
    );
    cosines.sort_by(f64::total_cmp);
    let p1 = cosines[cosines.len() / 100];
    let median = cosines[cosines.len() / 2];
    eprintln!(
        "rigid attributes: all 17897 meshes decoded; {zero_normals} zero normals; geometric signed face-normal cosines {} triangles: p1={p1:.6}, median={median:.6}; frame={frame:?}",
        cosines.len()
    );
    let agree = cosines.iter().filter(|&&v| v > 0.5).count();
    let oppose = cosines.iter().filter(|&&v| v < -0.5).count();
    let ambiguous = cosines.len() - agree - oppose;
    eprintln!("signed winding: {agree} agree, {oppose} oppose, {ambiguous} ambiguous (threshold +/-0.5)");
    assert_eq!((agree, oppose, ambiguous), (5_384_944, 1_469, 23_293));
    assert!(agree as f64 / (agree + oppose) as f64 > 0.999);
    assert!(
        median > 0.9,
        "CMP normals do not align with geometric faces: median {median}"
    );
}
