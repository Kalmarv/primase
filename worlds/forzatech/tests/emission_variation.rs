//! `apply-forzatech-emission-variation` on hand-built prototypes: a rule placement's second variation byte scales the
//! emission by the material's ramp, placements at one factor share one material, a hand-placed instance draws at 1 and is
//! counted.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test over data it builds itself"
)]

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{Entity, Instance, Mesh, Primitive, Semantic, Stream, Topology, World};
use primase_forzatech::components::{EmissionVariation, Placement, PlacementOrigin};
use primase_forzatech::passes::ApplyForzaEmissionVariation;

/// An emitting material (a texture, factor 1 times strength 2) with the ramp `min` to `max`.
fn emitter(world: &mut World, min: f32, max: f32) -> Entity {
    let texture = world.spawn();
    let e = world.spawn();
    world.insert(
        e,
        PbrMaterial {
            emissive_texture: Some(TextureRef::new(texture)),
            emissive: [1.0, 0.5, 0.25],
            emissive_strength: 2.0,
            ..PbrMaterial::default()
        },
    );
    world.insert(e, EmissionVariation { min, max });
    e
}

fn prototype(world: &mut World, material: Entity) -> Entity {
    let e = world.spawn();
    world.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: [(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            )]
            .into_iter()
            .collect(),
            indices: Some(vec![0, 1, 2]),
            material: Some(material),
        }]),
    );
    e
}

fn place(world: &mut World, prototype: Entity, origin: PlacementOrigin) -> Entity {
    let e = world.spawn();
    world.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    world.insert(
        e,
        Placement {
            origin,
            season_exclusion: 0,
            material_variant: 0,
        },
    );
    e
}

fn rule(y: u8) -> PlacementOrigin {
    PlacementOrigin::Rule {
        file: String::new(),
        category: String::new(),
        block: String::new(),
        record: 0,
        route: String::new(),
        gameplay_tag: String::new(),
        variation: [255, y, 255, 255],
        frame: 0,
    }
}

fn hand() -> PlacementOrigin {
    PlacementOrigin::Hand {
        instance: 0,
        flags: 0,
        parent: None,
        band_rows: [None; 6],
        gameplay_tag: None,
        routes: Vec::new(),
        tint: [0.0; 4],
        lod_group: None,
    }
}

/// The material a placement draws.
fn drawn(world: &World, placement: Entity) -> Entity {
    let i = world.get::<Instance>(placement).unwrap();
    world
        .get::<primase_core::MaterialVariants>(i.prototype)
        .and_then(|v| v.0[0].get(&i.variant).copied())
        .or_else(|| world.get::<Mesh>(i.prototype).unwrap().primitives()[0].material)
        .unwrap()
}

#[test]
fn the_second_variation_byte_scales_the_emission_by_the_ramp() {
    let mut world = World::new();
    let material = emitter(&mut world, 0.2, 0.6);
    let proto = prototype(&mut world, material);
    // y = 0 (below the ramp), 255 (above), 102 = 0.4 (the middle: factor 0.5), 102 again, and a hand placement
    let off = place(&mut world, proto, rule(0));
    let on = place(&mut world, proto, rule(255));
    let half = place(&mut world, proto, rule(102));
    let half_again = place(&mut world, proto, rule(102));
    let by_hand = place(&mut world, proto, hand());

    let mut log = PassLog::default();
    ApplyForzaEmissionVariation.run(&mut world, &mut log).unwrap();

    // at 1 (above the ramp, and by hand) the material itself; at 0 a copy with no emission; at the middle a copy at half
    assert_eq!(drawn(&world, on), material);
    assert_eq!(drawn(&world, by_hand), material);
    let dark = world.get::<PbrMaterial>(drawn(&world, off)).unwrap();
    assert!(dark.emissive_texture.is_none());
    assert_eq!(dark.emissive, [0.0; 3]);
    let dim = drawn(&world, half);
    assert_eq!(drawn(&world, half_again), dim);
    assert_ne!(dim, material);
    let dim = world.get::<PbrMaterial>(dim).unwrap();
    assert!(dim.emissive_texture.is_some());
    let want = [1.0, 0.5, 0.25].map(|c| c * (0.4 - 0.2) / (0.6 - 0.2));
    for (got, want) in dim.emissive.iter().zip(want) {
        assert!((got - want).abs() < 1e-3, "{:?}", dim.emissive);
    }
    assert_eq!(dim.emissive_strength, 2.0);
    // the material itself is untouched, and the marker is gone
    assert_eq!(
        world.get::<PbrMaterial>(material).unwrap().emissive,
        [1.0, 0.5, 0.25]
    );
    assert!(!world.has::<EmissionVariation>(material));

    assert_eq!(
        log.counts["emissive material copies made, one per material and factor"],
        2
    );
    assert_eq!(
        log.counts["rule-placed emissive instances at an instance variation factor of 1"],
        1
    );
    assert_eq!(
        log.counts["rule-placed emissive instances at an instance variation factor below 1"],
        3
    );
    assert_eq!(
        log.omitted["hand-placed emissive instances whose variation is not traced: drawn at a factor of 1"],
        1
    );
    assert_eq!(
        log.counts["prototype copies specialize-material-variants adds for the emission's instance variation"],
        2
    );
}

/// Where every placement is at a factor of 1 nothing changes, and it is counted as the identity.
#[test]
fn a_factor_of_one_everywhere_changes_nothing() {
    let mut world = World::new();
    let material = emitter(&mut world, 0.0, 0.2);
    let proto = prototype(&mut world, material);
    let a = place(&mut world, proto, rule(255));
    let b = place(&mut world, proto, rule(255));
    let mut log = PassLog::default();
    ApplyForzaEmissionVariation.run(&mut world, &mut log).unwrap();
    assert_eq!([drawn(&world, a), drawn(&world, b)], [material; 2]);
    assert!(!world.has::<primase_core::MaterialVariants>(proto));
    assert_eq!(
        log.counts["rule-placed emissive instances at an instance variation factor of 1"],
        2
    );
    assert_eq!(
        log.counts
            .get("emissive material copies made, one per material and factor"),
        Some(&0)
    );
}
