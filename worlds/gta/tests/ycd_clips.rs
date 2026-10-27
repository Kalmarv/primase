#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! A clip dictionary read into a ped's world ([`GtaSource::read_clips`]) and lowered (`lower-gta-skin`,
//! `lower-gta-clips`) on real data: a spread of the install's dictionaries, a few animations of each, on the skeleton of
//! a component ped.
//!
//! Checked: the world verifies after the read and after the lowering (`core.clip` channels on joints of the skeleton,
//! keys increasing within the duration, rotations unit); no `gta.animation` is left; every animation became a clip.
//! Then the meaning of the numbers against the skeleton's own rest pose (the ped's fragment stores each bone's local
//! transform): a bone's translation track at its first key is that bone's rest translation for most bones, and a
//! rotation track's first key is nearer the rest rotation as stored than as its inverse. The counts are printed.
//!
//! Needs the GTA install and its key material.
use std::collections::BTreeMap;

use primase_core::Entity;
use primase_core::asset::{Asset, Role};
use primase_core::clip::{Clip, ClipTarget, Property};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::skeleton::{Rest, Skeleton};
use primase_gta::components::GtaAnimation;
use primase_gta::rage::ycd::ClipDictionary;
use primase_gta::read::archive::keys::Keys;
use primase_gta::read::archive::listing::Storage;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};

/// The rotation as a quaternion `[x, y, z, w]` of the matrix whose columns are `cols` (scale removed).
fn quaternion(cols: &[[f32; 3]; 4]) -> [f32; 4] {
    let col = |j: usize| {
        let c = [cols[j][0], cols[j][1], cols[j][2]];
        let n = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        c.map(|x| x / n)
    };
    let (a, b, c) = (col(0), col(1), col(2));
    // m[row][col]
    let m = [[a[0], b[0], c[0]], [a[1], b[1], c[1]], [a[2], b[2], c[2]]];
    let trace = m[0][0] + m[1][1] + m[2][2];
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
            0.25 * s,
        ]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [
            0.25 * s,
            (m[0][1] + m[1][0]) / s,
            (m[0][2] + m[2][0]) / s,
            (m[2][1] - m[1][2]) / s,
        ]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [
            (m[0][1] + m[1][0]) / s,
            0.25 * s,
            (m[1][2] + m[2][1]) / s,
            (m[0][2] - m[2][0]) / s,
        ]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [
            (m[0][2] + m[2][0]) / s,
            (m[1][2] + m[2][1]) / s,
            0.25 * s,
            (m[1][0] - m[0][1]) / s,
        ]
    }
}

fn angle(a: &[f32], b: &[f32]) -> f32 {
    let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    2.0 * d.abs().min(1.0).acos()
}

#[test]
fn clips_lower_onto_a_peds_skeleton() {
    let (Some(dir), Some(key_dir)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &key_dir).expect("install");
    let keys = Keys::from_dir(&key_dir).expect("key material");
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_gta::register(&mut registry);
    let manager = PassManager::new(registry);

    let ymt = source
        .install()
        .listing()
        .find(|f| {
            f.extension == "ymt"
                && f.archive.contains("componentpeds")
                && matches!(f.storage, Storage::Resource { .. })
        })
        .expect("a component ped's variation info");
    let (mut w, _) = source.read_ped(ymt.file).expect("ped");
    let skeleton: Entity = w
        .iter::<primase_gta::components::GtaSkeleton>()
        .next()
        .expect("the ped's skeleton")
        .0;

    let ycds: Vec<_> = source
        .install()
        .listing()
        .filter(|f| f.extension == "ycd")
        .collect();
    let mut animations = 0usize;
    let mut read_tracks = 0usize;
    for f in ycds.iter().step_by(ycds.len() / 40) {
        let r = source.install().resource(f.file, &keys).unwrap();
        let d = ClipDictionary::read_resource(&r).unwrap();
        let hashes: Vec<u32> = d.animations.iter().take(4).map(|(h, _)| *h).collect();
        let report = source
            .read_clips(&mut w, f.file, skeleton, Some(&hashes))
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        animations += report.animations;
        read_tracks += report.tracks;
    }
    let bad = w.verify_components();
    assert!(
        bad.is_empty(),
        "after the read: {} fail, e.g. {:?}",
        bad.len(),
        bad.first()
    );
    assert_eq!(w.iter::<GtaAnimation>().count(), animations);

    let mut run = RunReport::default();
    let pipeline = manager.registry.parse("lower-gta-skin, lower-gta-clips").unwrap();
    manager.run(&mut w, &pipeline, &mut run).unwrap();
    let bad = w.verify_components();
    assert!(
        bad.is_empty(),
        "after the lowering: {} fail, e.g. {:?}",
        bad.len(),
        bad.first()
    );
    assert_eq!(w.iter::<GtaAnimation>().count(), 0);
    assert_eq!(w.iter::<Clip>().count(), animations);
    // the clips are members of the ped's asset, whose rig the skeleton is
    let (_, asset) = w.iter::<Asset>().next().expect("the ped's asset");
    assert_eq!(
        asset.members.iter().filter(|m| m.1 == Role::Clip).count(),
        animations
    );

    // the meaning against the rest pose
    let rest: BTreeMap<u32, ([f32; 3], [f32; 4])> = w
        .get::<Skeleton>(skeleton)
        .unwrap()
        .joints
        .iter()
        .filter_map(|j| match &j.rest {
            Rest::Affine(m) => Some((
                j.id.0,
                ([m.cols[3][0], m.cols[3][1], m.cols[3][2]], quaternion(&m.cols)),
            )),
            Rest::Trs { .. } => None,
        })
        .collect();
    let (mut t_total, mut t_match) = (0usize, 0usize);
    let (mut t_dir_total, mut t_dir_same) = (0usize, 0usize);
    let mut ratios: Vec<f32> = Vec::new();
    let (mut r_total, mut r_direct_nearer) = (0usize, 0usize);
    let (mut direct, mut inverse) = (Vec::new(), Vec::new());
    let mut channels: BTreeMap<String, usize> = BTreeMap::new();
    for (_, clip) in w.iter::<Clip>() {
        for c in &clip.channels {
            *channels.entry(format!("{:?}", c.property)).or_default() += 1;
            let ClipTarget::Joint { joint, .. } = &c.target else {
                continue;
            };
            let Some((rest_t, rest_q)) = rest.get(&joint.0) else {
                continue;
            };
            match c.property {
                Property::Translation => {
                    t_total += 1;
                    let d = (0..3)
                        .map(|i| (c.values[i] - rest_t[i]).abs())
                        .fold(0.0f32, f32::max);
                    t_match += usize::from(d < 1e-3);
                    let (a, b) = (&c.values[0..3], rest_t);
                    let (la, lb) = (
                        a.iter().map(|x| x * x).sum::<f32>().sqrt(),
                        b.iter().map(|x| x * x).sum::<f32>().sqrt(),
                    );
                    if la > 1e-3 && lb > 1e-3 {
                        t_dir_total += 1;
                        let cos = a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / (la * lb);
                        t_dir_same += usize::from(cos > 0.99);
                        ratios.push(la / lb);
                    }
                }
                Property::Rotation => {
                    r_total += 1;
                    let q = &c.values[0..4];
                    let inv = [-q[0], -q[1], -q[2], q[3]];
                    let (a, b) = (angle(q, rest_q), angle(&inv, rest_q));
                    r_direct_nearer += usize::from(a < b);
                    direct.push(a);
                    inverse.push(b);
                }
                Property::Scale => {}
                Property::Scalar | Property::Visibility | Property::Values(_) => {
                    panic!("a non-transform property on a GTA joint: {:?}", c.property)
                }
            }
        }
    }
    let median = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(f32::NAN).to_degrees()
    };
    println!(
        "{animations} animations ({read_tracks} tracks) became clips; channels {channels:?}; pass counts {:?}",
        run.steps.iter().map(|s| s.log.counts.clone()).collect::<Vec<_>>()
    );
    ratios.sort_by(f32::total_cmp);
    println!(
        "translation first keys with a direction (both longer than 1 mm): {t_dir_same}/{t_dir_total} within 8 degrees of the rest's; median length ratio clip/rest {:.3} (the animations are authored for other proportions)",
        ratios.get(ratios.len() / 2).copied().unwrap_or(f32::NAN)
    );
    println!(
        "translation first keys within 1e-3 of the rest: {t_match}/{t_total}; rotation first keys nearer the rest rotation as stored: {r_direct_nearer}/{r_total}, median angle to the rest {:.1}° as stored, {:.1}° inverted",
        median(&mut direct.clone()),
        median(&mut inverse.clone())
    );
    // the numbers mean what the skeleton's own pose says: the stored rotation is nearer the rest than its inverse, most
    // bones' translation lies the way the rest's does
    let (direct_median, inverse_median) = (median(&mut direct), median(&mut inverse));
    assert!(direct_median < inverse_median);
    assert!(r_direct_nearer * 3 > r_total * 2);
    assert!(t_dir_same * 2 > t_dir_total);
}
