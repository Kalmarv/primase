#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! A component ped read as a whole ([`GtaSource::read_ped`]) on real peds, then taken through the passes a program
//! runs on it: `lower-gta-skin` (every part bound to the ped's one skeleton), `resolve-state` (a program's choice of
//! one drawable a slot) and `place-assets` (the chosen parts placed with the rig).
//!
//! Checked: the world verifies at every step; the asset holds the skeleton as its rig and a part per drawable of the
//! variation info; each slot has an axis whose values are its drawables (a default only for a slot with exactly one);
//! every part is skinned to the rig's skeleton; the ped's own texture dictionary resolves textures; choosing the
//! first drawable of every slot leaves one part a slot and places each with a skin instance of the rig. The counts
//! are printed.
//!
//! Needs the GTA install and its key material.
use primase_core::asset::{Asset, Role};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::rig::{RigInstance, SkinInstance};
use primase_core::skin::Skin;
use primase_core::state::StateAxis;
use primase_core::{Instance, World};
use primase_gta::components::{GtaPedPart, GtaSkeleton};
use primase_gta::read::archive::listing::Storage;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};

fn assert_verifies(w: &World, what: &str) {
    let bad = w.verify_components();
    assert!(
        bad.is_empty(),
        "{what}: {} components fail, e.g. {:?}",
        bad.len(),
        bad.first()
    );
}

#[test]
fn a_ped_is_one_skeleton_and_its_drawables_with_every_choice_open() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).expect("install");
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_gta::register(&mut registry);
    let manager = PassManager::new(registry);

    let ymts: Vec<_> = source
        .install()
        .listing()
        .filter(|f| {
            f.extension == "ymt"
                && f.archive.contains("componentpeds")
                && matches!(f.storage, Storage::Resource { .. })
        })
        .collect();
    assert!(!ymts.is_empty());
    // one in every 90, from the first: a spread of peds without reading all 563
    for f in ymts.iter().step_by(90) {
        let (mut w, report) = source
            .read_ped(f.file)
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        assert_verifies(&w, &format!("{} read", f.path));
        let root = w.iter::<Asset>().next().expect("the asset").0;
        let members = w.get::<Asset>(root).unwrap().members.clone();
        let rigs: Vec<_> = members.iter().filter(|m| m.1 == Role::Rig).map(|m| m.0).collect();
        assert_eq!(rigs.len(), 1);
        assert!(w.has::<GtaSkeleton>(rigs[0]));
        assert_eq!(members.len() - 1, report.parts);
        assert_eq!(w.iter::<GtaPedPart>().count(), report.parts);
        let axes: Vec<StateAxis> = w.iter::<StateAxis>().map(|(_, a)| a.clone()).collect();
        assert_eq!(axes.len(), report.slots.len());
        assert_eq!(
            axes.iter().filter(|a| a.default.is_some()).count(),
            report.defaulted_slots
        );
        for ((name, n), axis) in report.slots.iter().zip(&axes) {
            assert_eq!(axis.key, format!("gta.ped.{name}"));
            assert_eq!(axis.domain.len(), *n);
        }
        assert!(
            report.assets.textures > 0,
            "{}: the ped's own dictionary resolved no texture",
            f.path
        );

        // every part bound to the one skeleton
        let skin = manager.registry.parse("lower-gta-skin").unwrap();
        manager
            .run(&mut w, &skin, &mut RunReport::default())
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        assert_verifies(&w, &format!("{} skinned", f.path));
        let skinned: Vec<_> = w.iter::<Skin>().collect();
        assert!(!skinned.is_empty(), "{}: no part was skinned", f.path);
        assert!(
            skinned
                .iter()
                .all(|(_, s)| s.bindings.iter().all(|b| b.skeleton == rigs[0]))
        );

        // a program's choice: the first drawable of every slot
        let choices: Vec<String> = axes
            .iter()
            .filter(|a| a.default.is_none())
            .map(|a| format!("{}:0", a.key))
            .collect();
        let resolve = format!("resolve-state{{assign={}}}, place-assets", choices.join(";"));
        let before = report.parts;
        let pipeline = manager.registry.parse(&resolve).unwrap();
        manager
            .run(&mut w, &pipeline, &mut RunReport::default())
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        assert_verifies(&w, &format!("{} placed", f.path));
        let placed = w.iter::<Instance>().count();
        assert_eq!(placed, report.slots.len(), "{}: a part a slot", f.path);
        assert_eq!(w.iter::<RigInstance>().count(), 1);
        assert!(w.iter::<SkinInstance>().count() > 0);
        println!(
            "{}: {} parts of {} slots, {} textures ({} not found), {} part(s) placed of {before}",
            f.name,
            report.parts,
            report.slots.len(),
            report.assets.textures,
            report.assets.textures_not_found.len(),
            placed
        );
    }
}
