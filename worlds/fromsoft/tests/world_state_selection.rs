//! Selector semantics on the installed script. Flag sets here are synthetic;
//! they are not a snapshot of the flags of a recorded game frame.
mod common;
mod inputs;
use fromsoft_formats::{Game, Keys, dcx::Decoder, emevd::Script, msb::MapLayout};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::world_state::{FlagSet, evaluate};
use std::collections::BTreeMap;

#[test]
fn installed_common_event_selects_groups_and_packages_in_script_order()
-> Result<(), Box<dyn std::error::Error>> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Sekiro),
        inputs::keys(inputs::Game::Sekiro),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let keys = Keys::from_folder(keys, Game::Sekiro, Game::Sekiro.param_key_sha256())?;
    let archive = common::install(root, Game::Sekiro, keys).open()?;
    let script = Script::read(&archive.read("/event/common.emevd.dcx", &decoder)?)?;
    let synthetic = |prefix: u32| FlagSet {
        name: format!("synthetic prefix {prefix}"),
        source: "semantic fixture, not the flags of a captured frame".into(),
        flags: BTreeMap::from_iter(
            [(8302, false), (8305, false), (8306, false)]
                .into_iter()
                .chain((0..4).map(|n| (9800 + n, n < prefix))),
        ),
    };
    // Independently read branches of stock event 100; this is not an hour schedule.
    for (prefix, group, envmap) in [
        (0, 8907000, 1),
        (1, 8907010, 2),
        (2, 8907020, 3),
        (3, 8907030, 4),
        (4, 8907050, 5),
    ] {
        let selected = evaluate(&script, &synthetic(prefix), [11, 1])?;
        assert_eq!(
            selected
                .groups
                .iter()
                .filter_map(|(&id, &on)| on.then_some(id))
                .collect::<Vec<_>>(),
            [group]
        );
        assert_eq!(selected.envmap, Some(envmap));
    }
    // Map overrides precede the ordinary story ladder.
    for (map, group, envmap) in [([10, 0], 8907060, 0), ([25, 0], 8907070, 4)] {
        let selected = evaluate(&script, &synthetic(2), map)?;
        assert_eq!(selected.groups.get(&group), Some(&true));
        assert_eq!(selected.envmap, Some(envmap));
    }
    let mut unknown = synthetic(2);
    unknown.flags.remove(&8305);
    let error = evaluate(&script, &unknown, [11, 1]).unwrap_err();
    assert!(
        error.contains("GOTO IF Event Flag") && error.contains("8305"),
        "{error}"
    );
    // False AND unknown is settled: 8306 and 9810 need not be guessed here.
    let mut partial = synthetic(2);
    partial.flags.remove(&8306);
    assert_eq!(evaluate(&script, &partial, [11, 1])?.envmap, Some(3));

    let mut override_flags = synthetic(2);
    override_flags.flags.insert(8302, true);
    let selected = evaluate(&script, &override_flags, [11, 1])?;
    assert_eq!(selected.envmap, Some(4));
    assert_eq!(selected.groups.get(&8907100), Some(&true));
    assert_eq!(
        selected.gparam_sub_ids,
        BTreeMap::from([
            ("m11_00".into(), 20),
            ("m11_01".into(), 20),
            ("m11_02".into(), 20)
        ])
    );
    // A wait before any selection is refused by name.
    let mut unsupported = script.clone();
    let event = unsupported.events.iter_mut().find(|e| e.id == 100).unwrap();
    event.instructions[1] = event.instructions[123].clone();
    // IF Player Has Changed Map is the explicit end-of-initialization boundary;
    // arriving there before a selection is a named refusal.
    let error = evaluate(&unsupported, &synthetic(2), [11, 1]).unwrap_err();
    assert!(
        error.contains("IF Player Has Changed Map") && error.contains("without an initial"),
        "{error}"
    );

    let sky = MapLayout::read(&archive.read("/map/MapStudio/m89_00_00_99.msb.dcx", &decoder)?)?;
    let mut members: Vec<_> = sky
        .parts
        .iter()
        .filter(|p| p.entity_group_ids.contains(&8907020))
        .map(|p| p.entity_id)
        .collect();
    members.sort();
    // Native observation: this instruction group touched exactly the Evening pair.
    assert_eq!(members, [8904020, 8904021]);
    Ok(())
}
