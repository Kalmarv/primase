#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! `lower-gta-skin` on real peds: a streamed ped fragment and the component dictionaries' entries that carry their
//! skeleton are read as themselves ([`GtaSource::read_entry`]) and lowered; the world must verify (every slot a joint
//! of its skeleton, every weighted slot inside its palette, weights summing to one), and each skinned primitive's joints
//! are the stored blend indices. The counts are printed.
//!
//! Needs the GTA install and its key material.
use primase_core::pass::{PassManager, PassRegistry};
use primase_core::skin::Skin;
use primase_core::{Mesh, Semantic, Stream};
use primase_gta::components::GtaBlendStreams;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};

#[test]
fn peds_lower_to_skins_that_verify() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).expect("install");
    let mut registry = PassRegistry::new();
    primase_gta::register(&mut registry);
    let manager = PassManager::new(registry);
    let pipeline = manager.registry.parse("lower-gta-skin").unwrap();

    let mut files: Vec<_> = source
        .install()
        .listing()
        .filter(|f| {
            (f.extension == "yft" && f.archive.contains("streamedpeds"))
                || (f.extension == "ydd" && f.archive.contains("componentpeds"))
        })
        .collect();
    files.truncate(40);
    let (mut skinned_files, mut primitives) = (0usize, 0usize);
    for f in files {
        let (mut w, _) = source
            .read_entry(f.file)
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        let before: usize = w
            .iter::<GtaBlendStreams>()
            .map(|(_, b)| b.0.iter().flatten().count())
            .sum();
        let mut report = primase_core::pass::RunReport::default();
        manager
            .run(&mut w, &pipeline, &mut report)
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        let bad = w.verify_components();
        assert!(
            bad.is_empty(),
            "{}: {} components fail, e.g. {:?}",
            f.path,
            bad.len(),
            bad.first()
        );
        let after: usize = w
            .iter::<GtaBlendStreams>()
            .map(|(_, b)| b.0.iter().flatten().count())
            .sum();
        let skinned = w
            .iter::<Skin>()
            .map(|(e, s)| {
                let mesh = w.get::<Mesh>(e).unwrap();
                s.influences
                    .iter()
                    .zip(mesh.primitives())
                    .filter(|(i, p)| i.is_some() && p.streams.contains_key(&Semantic::Joints(0)))
                    .inspect(|(_, p)| assert!(matches!(p.streams[&Semantic::Joints(0)], Stream::U8x4(_))))
                    .count()
            })
            .sum::<usize>();
        assert_eq!(
            before - after,
            skinned,
            "{}: what left gta.blend_streams is what was skinned",
            f.path
        );
        primitives += skinned;
        skinned_files += usize::from(skinned > 0);
    }
    println!("{skinned_files} of the files skinned, {primitives} primitives");
    assert!(skinned_files > 0, "no ped file carried its skeleton");
}
