//! Draw groups as a presence axis on a real map ([`sekiro::DisplayGroups`]): the Academy of Raya Lucaria (m14_00_00_00,
//! an Elden Ring legacy dungeon) read with `display_groups: Axes`. Expected values come from an independent census of the
//! layout (a separate parser of the part records, run for this test): 88 collisions, 72 of them setting a display group;
//! 8,272 map pieces and assets, 1,136 under no draw group, 7,134 under a draw group that a collision displays and 2 under
//! groups none displays.
//!
//! Every placement is conditioned or left present, and the counts add up to the placements; resolving with no group
//! displayed leaves exactly the placements under no draw group, resolving with every group displayed leaves them all.
mod inputs;

use fromsoft_formats::msb::GroupCensus;
use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::state::{Condition, ResolveState, StateAxis};
use primase_core::{Instance, World, pass::Pass, pass::PassLog};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use std::collections::BTreeSet;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

#[test]
fn the_academys_draw_groups_become_axes_and_conditions() -> Result<()> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    // without the option nothing is conditioned and the report says nothing of groups
    let mut plain = World::default();
    let plain_report = sekiro::read_game_area(
        &install,
        "m14_00_00_00",
        &decoder,
        &sekiro::Options::default(),
        &mut plain,
    )?;
    assert!(plain_report.display_groups.is_none());
    assert_eq!(plain.iter::<Condition>().count(), 0);

    let options = sekiro::Options {
        display_groups: sekiro::groups::DisplayGroups::Axes,
        ..Default::default()
    };
    let mut world = World::default();
    let report = sekiro::read_game_area(&install, "m14_00_00_00", &decoder, &options, &mut world)?;
    let groups = report.display_groups.as_ref().expect("the groups' report");
    assert_eq!(
        groups.layout,
        GroupCensus {
            collisions: 88,
            collisions_with_groups: 72,
            parts: 8_272,
            no_groups: 1_136,
            met: 7_134,
            unmet: 2,
        }
    );
    // the same placements as without the axes, each conditioned or left present
    assert_eq!(report.placements, plain_report.placements);
    assert_eq!(groups.conditioned + groups.unconditioned, report.placements);
    assert!(groups.conditioned > 0 && groups.unconditioned > 0);
    // one axis per draw group of a kept placement, none with a default, and every literal names one of them
    let axes: BTreeSet<_> = world
        .iter::<StateAxis>()
        .filter(|(_, a)| a.class == sekiro::groups::CLASS)
        .map(|(e, a)| {
            assert!(a.default.is_none() && a.key.starts_with("fromsoft.display_group."));
            e
        })
        .collect();
    assert_eq!(axes.len(), groups.axes);
    assert_eq!(world.iter::<Condition>().count(), groups.conditioned);
    for (_, condition) in world.iter::<Condition>() {
        assert!(!condition.0.is_empty());
        for clause in &condition.0 {
            assert!(!clause.0.is_empty() && clause.0.iter().all(|l| axes.contains(&l.axis)));
        }
    }
    // with no group displayed only the placements under no draw group stay; with every group displayed all stay
    let resolve = |world: &World, value: &str| -> Result<usize> {
        let mut w = world.clone();
        let mut r = ResolveState::default();
        r.assign_class.insert(sekiro::groups::CLASS.into(), value.into());
        r.run(&mut w, &mut PassLog::default())?;
        Ok(w.iter::<Instance>().count())
    };
    assert_eq!(resolve(&world, "off")?, groups.unconditioned);
    assert_eq!(resolve(&world, "on")?, report.placements);
    // the notices name the parts the axes cannot decide
    let notices = report.notices();
    assert!(notices.iter().any(|n| n.reason.starts_with("placements under no draw group")));
    Ok(())
}
