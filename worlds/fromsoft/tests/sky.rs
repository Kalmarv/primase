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
                let states = part.states;
                assert_eq!(states.count_ones(), 1, "{source}");
                let c = world.get::<Condition>(e).unwrap();
                assert_eq!((c.0.len(), c.0[0].0.len()), (1, 1), "{source}");
                assert_eq!((c.0[0].0[0].axis, c.0[0].0[0].values), (axis, states), "{source}");
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
                    assert_eq!(states, 1 << value(dome), "{source}");
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
            specular: None,
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

/// Limgrave's tile `m60_42_36_00` (creation limit 0) with `m60_00_00_99`'s weathers, in (fire-ash state, weather)
/// order.
const LIMGRAVE_WEATHERS: [&str; 32] = [
    "weather-0",
    "weather-1",
    "weather-10",
    "weather-11",
    "weather-20",
    "weather-21",
    "weather-30",
    "weather-31",
    "weather-40",
    "weather-41",
    "weather-50",
    "weather-51",
    "weather-52",
    "weather-60",
    "weather-82",
    "weather-99",
    "weather-0-ashen",
    "weather-1-ashen",
    "weather-10-ashen",
    "weather-11-ashen",
    "weather-20-ashen",
    "weather-21-ashen",
    "weather-30-ashen",
    "weather-31-ashen",
    "weather-40-ashen",
    "weather-41-ashen",
    "weather-50-ashen",
    "weather-51-ashen",
    "weather-52-ashen",
    "weather-60-ashen",
    "weather-82-ashen",
    "weather-99-ashen",
];

/// The sky map's parts the tile's weathers show, and the mask of the values showing each.
const LIMGRAVE_ASSETS: [(&str, u64); 11] = [
    ("AEG094_104_0000", 0x7fff_0000),
    ("AEG096_010_0000", 0x0007_0007),
    ("AEG096_102_0000", 0x8000_8000),
    ("AEG096_400_0000", 0x0001_0001),
    ("AEG096_402_0000", 0x0004_0004),
    ("AEG096_403_0000", 0x2008_2008),
    ("AEG096_404_0000", 0x5ff0_5ff0),
    ("AEG096_410_0000", 0x0000_203f),
    ("AEG096_411_0000", 0x203f_203f),
    ("AEG096_414_0000", 0x203f_203f),
    ("AEG096_420_0000", 0x203f_0000),
];

/// Elden Ring's sky read with an open-world tile: the weathers of its sky map's `WeatherAssetReplaceParam` rows that
/// apply at the tile's creation limit as the `fromsoft.sky` values, each shown asset with the values that show it as its
/// condition; an asset drawn only with sky programs (the spheres and cloud layers: their material binaries name
/// `M[Sky9]`, `M[Clouds5]`, `M[Atmosphere][Clouds]..`, `M[DistantClouds]_001`) a `WeatherSky` anchored horizontally at
/// its position relative to the sky map's origin, any other (the Erdtree's light shafts `AEG096_411`/`_414`,
/// `M[WorldTree_LightShaft..]`) a `Weather` placed where the layout places it moved into the tile's frame (the tile's
/// centre stands at (128, 640) in the sky map's frame; source z reversed in core's); the sky map's other parts counted.
/// The pass puts the sky programs' materials on the sky layer writing depth and leaves the light shafts'.
///
/// The expected values come from an independent walk of the params (typed by the Smithbox paramdefs) and of the sky
/// map's layout, reading no output of this crate: limit 0 from `MapDefaultInfoParam` row 60423600, 32 weathers, 11
/// assets shown (all at the sky map's origin), 32 parts other regions' rows name, 594 parts no row names, none of them
/// an `AEG096` model.
///
/// Needs `installs.elden`, `keys.elden` and `tools.oodle6`.
#[test]
fn elden_rings_sky_is_read_as_weathers() -> Result<(), Box<dyn std::error::Error>> {
    use fromsoft_formats::{Game as FsGame, Keys};
    use primase_fromsoft::game::ParamKey;
    let (Some(root), Some(keys), Some(dll)) = (
        install(Game::Elden),
        inputs::keys(Game::Elden),
        oodle(Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        FsGame::EldenRing,
        Keys::from_folder(keys, FsGame::EldenRing, FsGame::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut world = World::default();
    let report = sekiro::read_game_area(
        &install,
        "m60_42_36_00",
        &decoder,
        &sekiro::Options {
            weather: Some(sekiro::weather::Snapshot {
                name: "installed clear-weather fixture".into(),
                source: "independent regulation census: WeatherParam row 0 GparamId 0, WeatherAssetReplaceParam map60423600 limit0".into(),
                weather: 0,
                fire_ash: false,
            }),
            // a small region: the sky is kept whatever the region
            region: Some(sekiro::Region {
                rect: [-10.0, -10.0, 10.0, 10.0],
                margin: 0.0,
            }),
            sky: sekiro::sky::Sky::Include,
            ..Default::default()
        },
        &mut world,
    )?;
    let sky = &report.sky;
    let weather = report.weather.as_ref().expect("explicit weather read");
    assert_eq!(
        weather.weather_outdoor,
        "/param/drawparam/m00_00_0000_weatheroutdoor.gparam.dcx"
    );
    assert_eq!(weather.sky, "weather-0");
    assert_eq!(
        world
            .resource::<primase_core::state::StateAssignment>()
            .expect("weather assignment")
            .0["fromsoft.sky"],
        0
    );
    assert!(sky.selection.is_some());
    assert_eq!(
        (sky.sky_map.as_deref(), sky.limit, sky.missing_sky_map.as_deref()),
        (Some("m60_00_00_99"), Some(0), None)
    );
    assert_eq!(sky.states, LIMGRAVE_WEATHERS);
    assert_eq!(
        (sky.weather_assets, sky.unshown_parts, sky.unnamed_parts),
        (11, 32, 594)
    );
    assert!(sky.unnamed_sky_models.is_empty(), "{:?}", sky.unnamed_sky_models);
    assert_eq!((sky.domes, sky.rings, sky.moons), (0, 0, 0));
    let axes: Vec<_> = world
        .iter::<StateAxis>()
        .filter(|(_, a)| a.key == "fromsoft.sky")
        .collect();
    assert_eq!(axes.len(), 1);
    let (axis, a) = axes[0];
    assert_eq!(a.default, None);
    assert_eq!(
        a.domain,
        Domain::Enum(LIMGRAVE_WEATHERS.map(String::from).to_vec())
    );
    fn material_of(w: &World, m: primase_core::Entity) -> &primase_fromsoft::materials::Material {
        w.get::<primase_fromsoft::materials::Material>(m).unwrap()
    }
    let is_shadow = |m: &primase_fromsoft::materials::Material| {
        m.skip == Some(primase_fromsoft::materials::SkipClass::Shadow)
    };
    let is_sky = |m: &primase_fromsoft::materials::Material| {
        m.skip == Some(primase_fromsoft::materials::SkipClass::Sky)
    };
    // per asset: its name, material slots and whether it is to follow the camera
    let mut seen = Vec::new();
    let mut parts = Vec::new();
    for (e, part) in world.iter::<SkyPart>() {
        let source = &world.get::<Source>(e).unwrap().0;
        let name = source.strip_prefix("m60_00_00_99:").unwrap();
        let &(_, mask) = LIMGRAVE_ASSETS
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("{source}: not a shown asset"));
        let prototype = world.get::<primase_core::Instance>(e).unwrap().prototype;
        let slots = world
            .get::<primase_fromsoft::sekiro::source::MaterialSlots>(prototype)
            .map(|s| s.0.clone())
            .unwrap_or_default();
        let drawn: Vec<_> = slots
            .iter()
            .map(|&m| material_of(&world, m))
            .filter(|m| !is_shadow(m))
            .collect();
        let follows = !drawn.is_empty() && drawn.iter().all(|m| is_sky(m));
        let model = &name[..10];
        match model {
            "AEG096_411" | "AEG096_414" => assert!(!follows, "{source}: a light shaft"),
            "AEG094_104" => {}
            _ => assert!(follows, "{source}: a sky sphere or cloud layer"),
        }
        let role = if follows {
            SkyRole::WeatherSky
        } else {
            SkyRole::Weather
        };
        assert_eq!((part.role, part.states), (role, mask), "{source}");
        let c = world.get::<Condition>(e).unwrap();
        assert_eq!(
            (c.0.len(), c.0[0].0.len(), c.0[0].0[0].axis, c.0[0].0[0].values),
            (1, 1, axis, mask),
            "{source}"
        );
        let t = world.get::<Transform>(e).unwrap();
        if follows {
            assert_eq!(
                world.get::<SkyAnchor>(e),
                Some(&SkyAnchor::Horizontal),
                "{source}"
            );
            assert_eq!(
                t.cols[3],
                [0.0, 0.0, 0.0],
                "{source}: relative to the sky map's origin"
            );
        } else {
            assert!(world.get::<SkyAnchor>(e).is_none(), "{source}");
            assert_eq!(t.cols[3], [-128.0, 0.0, 640.0], "{source}");
        }
        seen.push(name.to_owned());
        parts.push((name.to_owned(), slots, follows));
    }
    seen.sort();
    assert_eq!(seen, LIMGRAVE_ASSETS.map(|(n, _)| n.to_owned()));
    let followers = parts.iter().filter(|p| p.2).count();
    assert!(followers >= 8, "{followers}");
    // the pass puts the sky programs' materials on the sky layer writing depth, leaves the light shafts', keeps the
    // anchors and removes the records
    common::run(&mut world, "lower-fromsoft-sky")?;
    assert!(world.iter::<SkyPart>().next().is_none());
    assert_eq!(world.iter::<SkyAnchor>().count(), followers);
    let sky_order = primase_core::DrawOrder {
        layer: primase_core::DrawLayer::Sky,
        queue: primase_core::DrawOrder::SKY,
        depth_write: true,
    };
    for (name, slots, _) in &parts {
        for &m in slots {
            let material = material_of(&world, m);
            if is_shadow(material) {
                continue;
            }
            let order = world.get::<primase_core::DrawOrder>(m);
            if is_sky(material) {
                assert_eq!(order, Some(&sky_order), "{name}: {}", material.shader);
            } else {
                assert_ne!(order, Some(&sky_order), "{name}: {}", material.shader);
            }
            if name.starts_with("AEG096_411") || name.starts_with("AEG096_414") {
                assert!(
                    material.shader.starts_with("M[WorldTree_LightShaft"),
                    "{name}: {}",
                    material.shader
                );
            } else if !name.starts_with("AEG094_104") {
                assert!(
                    [
                        "M[Sky9]",
                        "M[Clouds5]",
                        "M[Atmosphere][Clouds]",
                        "M[DistantClouds]"
                    ]
                    .iter()
                    .any(|p| material.shader.starts_with(p)),
                    "{name}: {}",
                    material.shader
                );
            }
        }
    }
    Ok(())
}
