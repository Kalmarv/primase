//! The sky read with an area (`Options::sky: Include`) and `lower-fromsoft-sky`, on slice A of m11_01: the sky map's
//! fourteen domes and the area's six own domes as the values of the `fromsoft.sky` axis, each ring in its dome's
//! state, the moon, all kept whatever the read region, and the moon anchored towards a sun at 25 units.
//!
//! The expected values are the install's own as listed independently of this reader: the domes, rings and their
//! materials by `fromsoft-formats`' `find_material` example over `m89_00_00_99` and the area's binders, the entity ids
//! by a layout census of `m89_00_00_99.msb`.
//!
//! Needs `installs.sekiro` and `tools.oodle6`.
#![allow(clippy::unwrap_used)]
mod inputs;

use fromsoft_formats::dcx::Decoder;
use inputs::{Game, Tool, install, oodle};
use primase_core::state::{Condition, Domain, StateAxis};
use primase_core::{Environment, Light, LightKind, SkyAnchor, Source, Transform, World};
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::source::{SkyPart, SkyRole};
mod common;

/// The sky map's domes in its layout's order, then m11_01's own (`<model>-<the word after SkyDome_ in its material>`).
const STATES: [&str; 20] = [
    "m900000-Morning",
    "m900010-Afternoon",
    "m900015-Afternoon",
    "m900020-Evening",
    "m900030-Night",
    "m900040-PastNight",
    "m900050-Dawn",
    "m900060-Night1",
    "m900070-Night1",
    "m900075-DarkClouds",
    "m900080-Storm",
    "m900085-Fog",
    "m900090-CloudSea",
    "m900095-Fire",
    "m11_01-m900000-Morning",
    "m11_01-m900010-Evening",
    "m11_01-m900020-Afternoon",
    "m11_01-m900030-Night",
    "m11_01-m900040-Past",
    "m11_01-m900050-Dawn",
];

/// Each ring of the sky map and the dome whose state it is drawn in (entity id + 1, `m900175` by model).
const RINGS: [(&str, &str); 13] = [
    ("m900100_0000", "m900000-Morning"),
    ("m900110_0000", "m900010-Afternoon"),
    ("m900115_0000", "m900015-Afternoon"),
    ("m900120_0000", "m900020-Evening"),
    ("m900130_0000", "m900030-Night"),
    ("m900140_0000", "m900040-PastNight"),
    ("m900150_0000", "m900050-Dawn"),
    ("m900160_0000", "m900060-Night1"),
    ("m900170_0000", "m900070-Night1"),
    ("m900175_0000", "m900075-DarkClouds"),
    ("m900180_0000", "m900080-Storm"),
    ("m900190_0000", "m900090-CloudSea"),
    ("m900200_0000", "m900095-Fire"),
];

#[test]
fn the_sky_is_read_as_states_and_its_moon_stands_towards_the_sun() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(sekiro) = common::sekiro(&root)? else {
        return Ok(());
    };
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll).unwrap();
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let mut world = World::default();
    let report = sekiro::read_game_area(
        &sekiro,
        "m11_01",
        &decoder,
        &sekiro::Options {
            // slice A, which the origin (where the sky stands) is outside
            region: Some(sekiro::Region {
                rect: [-170.0, 180.0, -50.0, 300.0],
                margin: 30.0,
            }),
            sky: sekiro::sky::Sky::Include,
            ..Default::default()
        },
        &mut world,
    )?;
    assert_eq!(report.sky.states, STATES);
    assert_eq!(
        (
            report.sky.domes,
            report.sky.area_domes,
            report.sky.rings,
            report.sky.moons
        ),
        (20, 6, 13, 1)
    );
    assert!(
        report.sky.unpaired_rings.is_empty(),
        "{:?}",
        report.sky.unpaired_rings
    );
    assert_eq!(report.sky.other_parts, ["c0000_0000"]);
    // one global axis, no default, its values the states
    let axes: Vec<_> = world
        .iter::<StateAxis>()
        .filter(|(_, a)| a.key == "fromsoft.sky")
        .collect();
    assert_eq!(axes.len(), 1);
    let (axis, a) = axes[0];
    assert_eq!(a.default, None);
    assert_eq!(a.domain, Domain::Enum(STATES.map(String::from).to_vec()));
    let value = |name: &str| STATES.iter().position(|s| *s == name).unwrap() as u32;
    // every dome and ring on the axis at its state and anchored horizontally; the moon neither
    let mut domes = 0;
    for (e, part) in world.iter::<SkyPart>() {
        let source = &world.get::<Source>(e).unwrap().0;
        match part.role {
            SkyRole::Moon => {
                assert_eq!(source, "m89_00_00_99:o005900_0000");
                assert!(world.get::<Condition>(e).is_none() && world.get::<SkyAnchor>(e).is_none());
            }
            role => {
                let state = part.state.unwrap();
                let c = world.get::<Condition>(e).unwrap();
                assert_eq!((c.0.len(), c.0[0].0.len()), (1, 1), "{source}");
                assert_eq!(
                    (c.0[0].0[0].axis, c.0[0].0[0].values),
                    (axis, 1u64 << state),
                    "{source}"
                );
                assert_eq!(
                    world.get::<SkyAnchor>(e),
                    Some(&SkyAnchor::Horizontal),
                    "{source}"
                );
                if role == SkyRole::Dome {
                    domes += 1;
                }
                if let Some((_, dome)) = RINGS
                    .iter()
                    .find(|(ring, _)| source == &format!("m89_00_00_99:{ring}"))
                {
                    assert_eq!(state, value(dome), "{source}");
                }
            }
        }
    }
    assert_eq!(domes, 20);
    // a sun, as lower-fromsoft-environment makes one, and the pass
    let sun = world.spawn();
    world.insert(
        sun,
        Light {
            kind: LightKind::Directional,
            color: [1.; 3],
            intensity: 1.,
            range: None,
            cast_shadows: None,
            radius: None,
        },
    );
    world.insert(sun, Transform::IDENTITY);
    world.insert_resource(Environment {
        sun: Some(sun),
        ambient: None,
    });
    common::run(&mut world, "lower-fromsoft-sky")?;
    let moons: Vec<_> = world
        .iter::<SkyAnchor>()
        .filter(|(_, a)| matches!(a, SkyAnchor::TowardsLight { .. }))
        .map(|(_, a)| *a)
        .collect();
    assert_eq!(
        moons,
        [SkyAnchor::TowardsLight {
            light: sun,
            distance: 25.0
        }]
    );
    assert!(world.iter::<SkyPart>().next().is_none());
    Ok(())
}
