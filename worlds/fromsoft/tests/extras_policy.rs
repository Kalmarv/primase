use fromsoft_formats::btl::{Kind, Light as BtlLight};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Light, LightKind, Transform, World};
use primase_fromsoft::sekiro::{light_record, passes, source::LightSource};
fn light() -> BtlLight {
    BtlLight {
        name: "fixture".into(),
        kind: Kind::Spot,
        diffuse: [0, 128, 255],
        diffuse_power: 2.,
        specular: [0; 3],
        specular_power: 0.,
        cast_shadows: true,
        cone_angle: 30.,
        position: [1., 2., 3.],
        rotation: [0.; 3],
        radius: 5.,
        auxiliary: Default::default(),
    }
}
const IDENTITY: Transform = Transform {
    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
};
/// The records of `lights` lowered by `pipeline`, with the report.
fn lowered(lights: &[BtlLight], pipeline: &str) -> Result<(World, RunReport), String> {
    let mut world = World::default();
    for l in lights {
        let (t, record) = light_record(l, &IDENTITY).map_err(|e| e.to_string())?;
        let e = world.spawn();
        world.insert(e, t);
        world.insert(e, record);
    }
    let mut registry = PassRegistry::new();
    passes::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager
        .run(
            &mut world,
            &manager.registry.parse(pipeline).map_err(|e| e.to_string())?,
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok((world, report))
}
#[test]
fn source_light_records_and_their_lowering() {
    // the record: where the light stands (Z mirrored into the world frame), what it stores
    let (t, record) = light_record(&light(), &IDENTITY).unwrap();
    assert_eq!(t.cols[3], [1., 2., -3.]);
    // the beam is the transform's local -Z
    assert_eq!(t.cols[2].map(|x| -x), [0., 0., -1.]);
    assert_eq!(record.tangent, [1., 0., 0.]);
    assert_eq!(
        (record.diffuse_power, record.radius, record.cone_angle),
        (2., 5., 30.)
    );
    let mut turned = light();
    // the rotation is stored in radians: a yaw of a quarter turn
    turned.rotation = [0., std::f32::consts::FRAC_PI_2, 0.];
    let (t, record) = light_record(&turned, &IDENTITY).unwrap();
    assert!((-t.cols[2][0] - 1.).abs() < 1e-6 && t.cols[2][2].abs() < 1e-6);
    assert!((record.tangent[2] - 1.).abs() < 1e-6, "{:?}", record.tangent);

    // the old route's calibration, as a program states it: 100 Studio watts per unit, P = 4 pi^2 I
    let calibration = 100. / (4. * std::f64::consts::PI * std::f64::consts::PI);
    let mut point = light();
    point.kind = Kind::Point;
    point.radius = 0.25;
    let mut directional = light();
    directional.kind = Kind::Directional;
    let (world, report) = lowered(
        &[light(), point, directional],
        &format!("lower-fromsoft-lights{{candela-per-power={calibration},spot-blend=0.15,min-range=0.5}}"),
    )
    .unwrap();
    let lights: Vec<&Light> = world.iter::<Light>().map(|(_, l)| l).collect();
    assert_eq!(lights.len(), 2);
    let spot = lights[0];
    assert_eq!(spot.intensity, (2. * calibration) as f32);
    assert_eq!(spot.range, Some(5.));
    assert_eq!(spot.cast_shadows, Some(true));
    assert_eq!(
        spot.kind,
        // a stored cone angle of 30 is the full angle of the edge (a running game's spot records scale each axis by
        // cot(angle / 2), `engine_light_records.rs`): a half-angle of 15 degrees
        LightKind::Spot {
            inner_cone: 30f32.to_radians() / 2. * 0.85,
            outer_cone: 30f32.to_radians() / 2.
        }
    );
    // the game's transfer, the square of byte / 255, not the sRGB curve (0.2158605 for 128): a running game's lighting
    // records hold `(byte / 255)^2 * power` for every BTL light read (`engine_light_records.rs`)
    assert_eq!(spot.color[0], 0.);
    assert_eq!(spot.color[2], 1.);
    assert!((f64::from(spot.color[1]) - (128f64 / 255.) * (128f64 / 255.)).abs() < 1e-7);
    assert!((spot.color[1] - 0.2158605).abs() > 0.03);
    assert_eq!(lights[1].kind, LightKind::Point);
    assert_eq!(lights[1].range, Some(0.5));
    // the raw record stays beside the core light
    assert_eq!(world.iter::<LightSource>().count(), 3);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["lights lowered"], 2);
    assert_eq!(log.counts["directional lights not lowered"], 1);
    assert_eq!(log.counts["ranges raised to the minimum"], 1);
    assert_eq!(log.approximations["colour bytes squared, the game's transfer"], 2);
    assert_eq!(log.approximations["spot inner cone from the fixed blend"], 1);

    // the calibration is the program's: no default, and bad numbers are refused
    for pipeline in [
        "lower-fromsoft-lights",
        "lower-fromsoft-lights{candela-per-power=nan}",
        "lower-fromsoft-lights{candela-per-power=1}",
        "lower-fromsoft-lights{candela-per-power=1,spot-blend=2}",
        "lower-fromsoft-lights{candela-per-power=-1}",
    ] {
        assert!(lowered(&[light()], pipeline).is_err(), "{pipeline}");
    }
}

#[test]
fn spawn_region_values_are_read_without_map_offset_or_rounding() {
    use fromsoft_formats::msb::{Generation, MapLayout, Section};
    // a spawn region (type 8) named "A": the header, then the name; Sekiro's header is 96 bytes, Elden Ring's 104
    // (the pointer to its extra block at +96)
    let region = |generation: &Generation| {
        let header = generation.region_header;
        let mut bytes = vec![0u8; header + 4];
        bytes[..8].copy_from_slice(&(header as u64).to_le_bytes());
        bytes[8..12].copy_from_slice(&8u32.to_le_bytes());
        bytes[header..header + 2].copy_from_slice(&65u16.to_le_bytes());
        for (i, v) in [1f32, 2., 3., 4., 5., 6.].into_iter().enumerate() {
            bytes[20 + i * 4..24 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        bytes
    };
    let mut layout = MapLayout {
        generation: &Generation::SEKIRO,
        models: vec![],
        parts: vec![],
        map_offsets: vec![],
        sections: vec![Section {
            name: "POINT_PARAM_ST".into(),
            version: 35,
            records: vec![region(&Generation::SEKIRO)],
            offsets: vec![],
        }],
    };
    let p = layout.spawn_points().unwrap();
    assert_eq!(p[0].name, "A");
    // Elden Ring's spawn points are the same region type, with the position and rotation at the same offsets
    layout.generation = &Generation::ELDEN_RING;
    layout.sections[0].records = vec![region(&Generation::ELDEN_RING)];
    let elden = layout.spawn_points().unwrap();
    assert_eq!(
        (
            elden[0].name.as_str(),
            elden[0].position,
            elden[0].rotation_degrees
        ),
        ("A", p[0].position, p[0].rotation_degrees)
    );
    // a generation whose spawn region code is not known refuses rather than finding none
    static UNKNOWN: Generation = Generation {
        spawn_region: None,
        ..Generation::SEKIRO
    };
    layout.generation = &UNKNOWN;
    assert!(layout.spawn_points().is_err());
    assert_eq!(p[0].position, [1., 2., 3.]);
    assert_eq!(p[0].rotation_degrees, [4., 5., 6.]);
}

#[test]
fn extras_become_ledge_curves_and_travel_markers_in_source_order() {
    use primase_core::{Curve, Marker, Name, Source};
    use primase_fromsoft::sekiro::source::{AreaExtras, HangSegment, SpawnPoint, SpawnRotation};
    let mut world = World::default();
    world.insert_resource(AreaExtras {
        hang_segments: vec![
            HangSegment {
                file: "a.edge".into(),
                points: [[0., 1., 0.], [1., 1., 0.]],
            },
            HangSegment {
                file: "b.edge".into(),
                points: [[5., 0., 0.], [5., 0., 2.]],
            },
            HangSegment {
                file: "a.edge".into(),
                points: [[1., 1., 0.], [1., 1., 1.]],
            },
        ],
        spawn_points: vec![SpawnPoint {
            name: "チェックポイント_1".into(),
            position: [1., 2., -3.],
            rotation_degrees: [0., 90., 0.],
        }],
    });
    let mut registry = PassRegistry::new();
    passes::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager
        .run(
            &mut world,
            &manager.registry.parse("lower-fromsoft-extras").unwrap(),
            &mut report,
        )
        .unwrap();
    assert!(world.resource::<AreaExtras>().is_none());
    let curves: Vec<(&Curve, &str)> = world
        .iter::<Curve>()
        .map(|(e, c)| (c, world.get::<Source>(e).unwrap().0.as_str()))
        .collect();
    // one curve per stored segment, unjoined, labelled by its place in its file
    assert_eq!(curves.len(), 3);
    assert_eq!(curves[0].0.points, [[0., 1., 0.], [1., 1., 0.]]);
    assert_eq!(curves[0].0.kind, "ledge");
    assert!(!curves[0].0.closed);
    assert_eq!(
        curves.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
        ["a.edge:0", "b.edge:0", "a.edge:1"]
    );
    let (e, marker) = world.iter::<Marker>().next().unwrap();
    assert_eq!(
        (marker.kind.as_str(), marker.ground, marker.yaw),
        ("travel", None, None)
    );
    // placed, not turned: the stored rotation stays as source data; the name as stored
    assert_eq!(
        world.get::<Transform>(e).unwrap().cols,
        [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [1., 2., -3.]]
    );
    assert_eq!(
        world.get::<SpawnRotation>(e).unwrap().rotation_degrees,
        [0., 90., 0.]
    );
    assert_eq!(world.get::<Name>(e).unwrap().0, "チェックポイント_1");
    assert_eq!(world.verify_components(), vec![]);
    let log = &report.steps[0].log;
    assert_eq!(
        (log.counts["ledge segments"], log.counts["travel markers"]),
        (3, 1)
    );
    assert_eq!(log.placed.len(), 4);
}
