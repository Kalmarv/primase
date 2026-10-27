//! Full source skin census; source counts and signed face checks accompany native-writer/hand-worked fixtures.
mod inputs;

use inputs::{Game, Query, expect_found, install};
use primase_core::{
    Instance, Mesh, Semantic, Stream,
    frame::{Axis, SourceFrame},
};
use primase_shred::{
    components::{SourceSkeleton, SourceSkin},
    read::{level, pak::Pak},
};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("disc directory") {
        let path = entry.expect("disc entry").path();
        if path.is_dir() {
            archives(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")) {
            out.push(path);
        }
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

fn census(paths: &[PathBuf], frame: SourceFrame) {
    let mut totals = [0usize; 8];
    let mut placed = 0;
    let mut layers = 0;
    let mut cosines = Vec::new();
    let mut exceptions = Vec::new();
    for path in paths {
        let pak = Pak::open(path).expect("disc PAK");
        let label = path
            .file_name()
            .expect("PAK name")
            .to_str()
            .expect("UTF-8 PAK name");
        let (world, report) = level::read_level(&[(label, &pak)], frame).expect("disc source skin decode");
        assert!(
            world.verify_components().is_empty(),
            "{}: {:?}",
            path.display(),
            world.verify_components()
        );
        for (total, count) in totals.iter_mut().zip([
            report.processed,
            report.processed_vertices,
            report.processed_batches,
            report.source_skeletons,
            report.source_bones,
            report.nonfinite_normals,
            report.omitted_normal_streams,
            report.refused,
        ]) {
            *total += count;
        }
        exceptions.extend(
            report
                .nonfinite_normal_exceptions
                .iter()
                .map(|(source, count)| (source.id, *count)),
        );
        for (source, count) in &report.nonfinite_normal_exceptions {
            eprintln!(
                "source normal bits retained: {} {:?} {:08x}: {count} vertices",
                source.pak,
                String::from_utf8_lossy(&source.quartet),
                source.id
            );
        }
        let mut local_placed = 0;
        if let Some(instances) = world.column::<Instance>() {
            for (_, instance) in instances.iter() {
                local_placed += usize::from(world.get::<SourceSkin>(instance.prototype).is_some());
            }
        }
        placed += local_placed;
        if report.processed > 0
            && path
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase()
                .contains("/layers/")
        {
            assert_eq!(
                (report.processed, report.source_skeletons, local_placed),
                (97, 2, 97)
            );
            layers += 1;
        }
        if let Some(skins) = world.column::<SourceSkin>() {
            assert_eq!(skins.len(), report.processed);
            for (entity, skin) in skins.iter() {
                let skeleton = world
                    .get::<SourceSkeleton>(skin.skeleton)
                    .expect("same-quartet source skeleton");
                assert!(!skeleton.bones.is_empty());
                assert!(world.closure([entity]).contains(&skin.skeleton));
                let mesh = world.get::<Mesh>(entity).expect("source skin mesh");
                for (primitive, source) in mesh.primitives().iter().zip(&skin.primitives) {
                    let positions = primitive.positions().expect("processed positions");
                    let authored: Vec<_> = source
                        .iter()
                        .map(|v| frame.direction(v.normal_bits.map(f32::from_bits)))
                        .collect();
                    if let Some(stream) = primitive.streams.get(&Semantic::Normal) {
                        assert_eq!(stream, &Stream::F32x3(authored.clone()));
                    } else {
                        assert!(authored.iter().flatten().any(|v| !v.is_finite()));
                    }
                    for triangle in primitive
                        .indices
                        .as_ref()
                        .expect("processed indices")
                        .as_chunks::<3>()
                        .0
                    {
                        let [a, b, c] = triangle.map(|i| positions[i as usize].map(f64::from));
                        let u = std::array::from_fn::<_, 3, _>(|k| b[k] - a[k]);
                        let v = std::array::from_fn::<_, 3, _>(|k| c[k] - a[k]);
                        let face = [
                            u[1] * v[2] - u[2] * v[1],
                            u[2] * v[0] - u[0] * v[2],
                            u[0] * v[1] - u[1] * v[0],
                        ];
                        // In the four omitted normal streams, finite authored normals remain inspectable as bits.
                        let average = std::array::from_fn::<_, 3, _>(|k| {
                            triangle
                                .iter()
                                .map(|&i| f64::from(authored[i as usize][k]))
                                .sum::<f64>()
                        });
                        let scale = (dot(face, face) * dot(average, average)).sqrt();
                        if scale.is_finite() && scale > 1e-12 {
                            cosines.push((dot(face, average) / scale).clamp(-1., 1.));
                        }
                    }
                }
            }
        }
    }
    assert_eq!(totals, [3_412, 3_302_877, 311_835, 145, 18_455, 14, 4, 0]);
    assert_eq!((placed, layers), (3_412, 21));
    exceptions.sort_unstable();
    assert_eq!(
        exceptions,
        [(0x0c190ea7, 4), (0x2494c946, 4), (0x5c6bd829, 4), (0xc5f5bfcf, 2)]
    );
    cosines.sort_by(f64::total_cmp);
    let agree = cosines.iter().filter(|&&v| v > 0.5).count();
    let oppose = cosines.iter().filter(|&&v| v < -0.5).count();
    let ambiguous = cosines.len() - agree - oppose;
    assert_eq!(
        (cosines.len(), agree, oppose, ambiguous),
        (4_877_718, 4_860_801, 528, 16_389)
    );
    let p1 = cosines[cosines.len() / 100];
    let median = cosines[cosines.len() / 2];
    assert!(agree as f64 / (agree + oppose) as f64 > 0.999);
    eprintln!(
        "processed source census: [meshes, stored vertices, batches, skeletons, bones, nonfinite normals, omitted streams, refusals]={totals:?}; {placed} placements / {layers} crowd layer PAKs; frame={frame:?}"
    );
    eprintln!(
        "processed authored normals vs emitted faces: {} valid; {agree} agree / {oppose} oppose / {ambiguous} ambiguous; signed p1={p1:.6}, median={median:.6}",
        cosines.len()
    );
}

#[test]
fn every_processed_skin_and_crowd_placement() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(
        Query::Install(Game::Shred, None),
        "processed-skin source PAKs",
        paths.len(),
    );
    for frame in [
        SourceFrame::WORLD,
        SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.),
    ] {
        census(&paths, frame);
    }
}
