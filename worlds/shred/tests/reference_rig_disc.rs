//! All five explicitly selected animation containers. No motion/network decoding is claimed.
mod inputs;

use inputs::{Game, Query, expect_found, install};
use primase_core::frame::{Axis, SourceFrame};
use primase_shred::{components::ReferenceRig, read::reference_rig::read_reference_rigs};
use rme_formats::{pak::Pak, reference_rig::Archive};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn all_reference_rigs_in_all_animation_containers() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paks = Vec::new();
    let mut names = Vec::new();
    for name in ["actor_male", "ai_skate", "skate", "snowboard", "ui"] {
        paks.push(Pak::open(root.join(format!("PS3_GAME/USRDIR/anim/{name}.pak"))).expect("animation PAK"));
        names.push(format!("anim\\{name}\\{name}").into_bytes());
    }
    expect_found(
        Query::Install(Game::Shred, None),
        "animation reference bundles",
        paks.len(),
    );
    let archives: Vec<_> = paks
        .iter()
        .zip(&names)
        .map(|(pak, entry)| Archive {
            label: std::str::from_utf8(entry).expect("fixture label"),
            pak,
            entry,
        })
        .collect();
    for frame in [
        SourceFrame::WORLD,
        SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 0.01),
    ] {
        let (world, report) = read_reference_rigs(&archives, frame).expect("all reference rigs");
        assert!(
            world.verify_components().is_empty(),
            "{:?}",
            world.verify_components()
        );
        assert_eq!(
            (report.bundles, report.records, report.rigs, report.joints),
            (5, 1680, 10, 812)
        );
        assert_eq!(report.bundles_without_rigs, 0);
        assert_eq!(
            (report.source_space_rigs, report.rigs_with_uninterpreted_fields),
            (10, 10)
        );
        let mut types = BTreeMap::new();
        for asset in &report.uninterpreted_assets {
            *types.entry(asset.asset_type).or_insert(0) += 1;
        }
        assert_eq!(
            types,
            BTreeMap::from([(0, 5), (2, 594), (3, 972), (4, 89), (6, 5), (8, 5)])
        );
        let mut identities = BTreeSet::new();
        let mut checked = 0;
        for (_, rig) in world.column::<ReferenceRig>().expect("rig column").iter() {
            identities.insert((rig.asset_id, rig.guid));
            let matrices = rig.source_matrices().expect("source globals");
            let converted = rig.matrices().expect("converted globals");
            for (i, (matrix, converted)) in matrices.iter().zip(&converted).enumerate() {
                assert_eq!(rig.rig.joints[i].id as usize, i);
                for point in [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]] {
                    let p = point.map(|v| v as f32);
                    let left = frame.point(matrix.apply(p));
                    let right = converted.apply(frame.point(p));
                    for (a, b) in left.into_iter().zip(right) {
                        assert!((a - b).abs() < 0.0001);
                    }
                    checked += 1;
                }
            }
        }
        assert_eq!(identities.len(), 7);
        eprintln!(
            "reference rigs: bundles={} records={} occurrences={} joints={} identities={} unknown_assets={} frame_point_checks={checked} frame={frame:?}",
            report.bundles,
            report.records,
            report.rigs,
            report.joints,
            identities.len(),
            report.uninterpreted_assets.len()
        );
    }
}
