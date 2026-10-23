//! `select-anvil-entities{collision-only=keep|drop|overlap, ...}` on hand-worked boxes: a collision placement of an owner with
//! no visual ([`AnvilCollisionOnly`]) stays under `keep`, goes under `drop`, and under `overlap` stays when at least the
//! stated fraction of its box lies inside the box of one visual placement (of another owner, no more than the stated
//! ratio times its volume, sides taken at no less than the stated extent) and goes otherwise; every decision is counted and each owner listed in
//! the log by its name; the marks and the visual placements' bounds are gone afterwards.
use primase_anvil::components::{AnvilBounds, AnvilCollisionOnly};
use primase_anvil::passes::{CollisionOnly, Keep, Overlap, SELECT_ANVIL_ENTITIES, SelectAnvilEntities};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Entity, Name, World};

fn opts(pairs: &[(&str, &str)]) -> Options {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn boxed(min: [f32; 3], max: [f32; 3]) -> AnvilBounds {
    AnvilBounds { min, max }
}

fn visual(w: &mut World, name: &str, b: AnvilBounds) -> Entity {
    let e = w.spawn();
    w.insert(e, Name(name.to_owned()));
    w.insert(e, b);
    e
}

fn blocker(w: &mut World, name: &str, b: AnvilBounds) -> Entity {
    let e = w.spawn();
    w.insert(e, Name(name.to_owned()));
    w.insert(e, b);
    w.insert(e, AnvilCollisionOnly);
    e
}

/// The program's numbers for the tests: a fraction, visuals up to 40 times the shape's volume, sides of at least 5 cm.
fn overlap(fraction: f64) -> CollisionOnly {
    CollisionOnly::Overlap(Overlap {
        fraction,
        max_ratio: 40.0,
        min_extent: 0.05,
    })
}

fn select(collision_only: CollisionOnly) -> SelectAnvilEntities {
    SelectAnvilEntities {
        hidden: Keep::Keep,
        phantom: Keep::Keep,
        water_collision: Keep::Keep,
        collision_only,
    }
}

/// The world: a visual box (0..3 each side, a 27 m3 prop) and collision-only boxes of 1 m3 against it: inside, half in
/// (the x side from 2.5 to 3.5 of a 1 m cube: half), a quarter in, outside, one inside a box a million times its
/// volume (a whole building's), and a second one over the first.
fn world() -> (World, [Entity; 6]) {
    let mut w = World::new();
    visual(&mut w, "prop", boxed([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]));
    visual(
        &mut w,
        "building",
        boxed([100.0, 0.0, 0.0], [200.0, 100.0, 100.0]),
    );
    let inside = blocker(&mut w, "inside", boxed([1.0, 1.0, 1.0], [2.0, 2.0, 2.0]));
    let half = blocker(&mut w, "half", boxed([2.5, 1.0, 1.0], [3.5, 2.0, 2.0]));
    let quarter = blocker(&mut w, "quarter", boxed([2.5, 2.5, 1.0], [3.5, 3.5, 2.0]));
    let outside = blocker(&mut w, "outside", boxed([20.0, 1.0, 1.0], [21.0, 2.0, 2.0]));
    let in_building = blocker(
        &mut w,
        "in-a-building",
        boxed([150.0, 50.0, 50.0], [151.0, 51.0, 51.0]),
    );
    // a second blocker over the first: another blocker is no visual, the prop's box is what keeps it
    let lone = blocker(&mut w, "lone", boxed([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]));
    (w, [inside, half, quarter, outside, in_building, lone])
}

#[test]
fn overlap_keeps_the_shapes_inside_a_visuals_box() {
    let (mut w, [inside, half, quarter, outside, in_building, lone]) = world();
    let mut log = PassLog::default();
    select(overlap(0.5)).run(&mut w, &mut log).expect("select");
    // inside: all of it; half: exactly 0.5; a quarter: 0.25; outside: none; in a building a million times its volume: the
    // building's box covers anything and does not count; lone: inside the prop's box: kept
    assert!(w.is_alive(inside));
    assert!(w.is_alive(half), "half of it inside meets 0.5");
    assert!(!w.is_alive(quarter));
    assert!(!w.is_alive(outside));
    assert!(
        !w.is_alive(in_building),
        "a box a million times its volume does not cover it"
    );
    assert!(w.is_alive(lone));
    assert_eq!(log.counts["collision-only placements kept"], 3);
    assert_eq!(log.counts["collision-only placements dropped"], 3);
    assert_eq!(log.counts["collision-only owner dropped: outside"], 1);
    assert_eq!(log.counts["collision-only owner kept: half"], 1);
    // the program's three numbers are echoed in the approximation, with the placements tested
    let (echo, tested) = log
        .approximations
        .iter()
        .find(|(k, _)| k.starts_with("collision-only=overlap"))
        .expect("the approximation is counted");
    assert!(
        echo.contains("fraction 0.5")
            && echo.contains("at most 40 times")
            && echo.contains("at least 0.05 m")
    );
    assert_eq!(*tested, 6);
    // the marks and the bounds are gone from what stays
    assert!(w.get::<AnvilCollisionOnly>(inside).is_none());
    assert!(w.get::<AnvilBounds>(inside).is_none());
    assert!(w.iter::<AnvilBounds>().next().is_none());
}

#[test]
fn a_higher_fraction_drops_the_half_in_shape() {
    let (mut w, [inside, half, ..]) = world();
    select(overlap(0.51))
        .run(&mut w, &mut PassLog::default())
        .expect("select");
    assert!(w.is_alive(inside));
    assert!(!w.is_alive(half));
}

#[test]
fn only_a_visual_counts_not_another_blocker() {
    let mut w = World::new();
    let big = blocker(&mut w, "big", boxed([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
    let small = blocker(&mut w, "small", boxed([4.0, 4.0, 4.0], [5.0, 5.0, 5.0]));
    select(overlap(0.5))
        .run(&mut w, &mut PassLog::default())
        .expect("select");
    assert!(
        !w.is_alive(big) && !w.is_alive(small),
        "no visual anywhere: both go"
    );
}

#[test]
fn a_flat_shape_has_a_volume() {
    let mut w = World::new();
    visual(&mut w, "floor", boxed([0.0, 0.0, 0.0], [2.0, 2.0, 0.2]));
    let flat = blocker(&mut w, "flat", boxed([0.5, 0.5, 0.1], [1.5, 1.5, 0.1]));
    select(overlap(0.5))
        .run(&mut w, &mut PassLog::default())
        .expect("select");
    assert!(
        w.is_alive(flat),
        "a zero-thickness shape inside a slab's box stays"
    );
}

#[test]
fn the_ratio_is_the_programs_choice() {
    // the building's box is a million times the shape's volume: out at 40, in at ten million
    let (mut w, [_, _, _, _, in_building, _]) = world();
    select(CollisionOnly::Overlap(Overlap {
        fraction: 0.5,
        max_ratio: 1.0e7,
        min_extent: 0.05,
    }))
    .run(&mut w, &mut PassLog::default())
    .expect("select");
    assert!(w.is_alive(in_building));
}

#[test]
fn the_least_extent_is_the_programs_choice() {
    // a zero-thickness shape in a 0.2 m slab's box: with sides taken at 5 cm it has a volume of 0.05 and the slab (0.8)
    // is 16 times it, so it stays; at 1 mm its volume is 0.001, the slab is 800 times it and it goes
    let build = || {
        let mut w = World::new();
        visual(&mut w, "floor", boxed([0.0, 0.0, 0.0], [2.0, 2.0, 0.2]));
        let flat = blocker(&mut w, "flat", boxed([0.5, 0.5, 0.1], [1.5, 1.5, 0.1]));
        (w, flat)
    };
    let (mut w, flat) = build();
    select(overlap(0.5))
        .run(&mut w, &mut PassLog::default())
        .expect("select");
    assert!(w.is_alive(flat));
    let (mut w, flat) = build();
    select(CollisionOnly::Overlap(Overlap {
        fraction: 0.5,
        max_ratio: 40.0,
        min_extent: 0.001,
    }))
    .run(&mut w, &mut PassLog::default())
    .expect("select");
    assert!(!w.is_alive(flat));
}

#[test]
fn keep_and_drop_do_not_look_at_the_visuals() {
    let (mut w, shapes) = world();
    let mut log = PassLog::default();
    select(CollisionOnly::Keep).run(&mut w, &mut log).expect("select");
    assert!(shapes.iter().all(|&e| w.is_alive(e)));
    assert_eq!(log.counts["collision-only placements kept"], 6);
    assert!(w.iter::<AnvilCollisionOnly>().next().is_none() && w.iter::<AnvilBounds>().next().is_none());
    let (mut w, shapes) = world();
    let mut log = PassLog::default();
    select(CollisionOnly::Drop).run(&mut w, &mut log).expect("select");
    assert!(shapes.iter().all(|&e| !w.is_alive(e)));
    assert_eq!(log.counts["collision-only placements dropped"], 6);
}

#[test]
fn the_option_is_required_and_checked() {
    let base = [
        ("hidden", "keep"),
        ("phantom", "keep"),
        ("water-collision", "keep"),
    ];
    let with = |extra: &[(&str, &str)]| {
        let mut p = base.to_vec();
        p.extend_from_slice(extra);
        SELECT_ANVIL_ENTITIES.instance(&opts(&p))
    };
    assert!(with(&[]).err().unwrap().contains("collision-only is required"));
    assert_eq!(
        with(&[("collision-only", "keep")]).unwrap().options(),
        select(CollisionOnly::Keep).options()
    );
    assert_eq!(
        with(&[("collision-only", "drop")]).unwrap().options(),
        select(CollisionOnly::Drop).options()
    );
    let all = [
        ("collision-only", "overlap"),
        ("collision-only-overlap", "0.5"),
        ("collision-only-max-ratio", "40"),
        ("collision-only-min-extent", "0.05"),
    ];
    assert_eq!(with(&all).unwrap().options(), select(overlap(0.5)).options());
    // each of the three is needed with overlap, and each is refused without it
    for missing in 1..4 {
        let mut some = all.to_vec();
        let (k, _) = some.remove(missing);
        let err = with(&some).err().unwrap();
        assert!(err.contains(&format!("needs {k}")), "{err}");
    }
    assert!(
        with(&[("collision-only", "overlap")])
            .err()
            .unwrap()
            .contains("needs collision-only-overlap")
    );
    for (k, v) in &all[1..] {
        let err = with(&[("collision-only", "keep"), (k, v)]).err().unwrap();
        assert!(err.contains("overlap only"), "{err}");
        assert!(with(&[("collision-only", "drop"), (k, v)]).is_err());
    }
    // bad values: the fraction in (0, 1], the ratio at least 1, the extent above 0, numbers
    for (k, bad) in [
        ("collision-only-overlap", "0"),
        ("collision-only-overlap", "1.5"),
        ("collision-only-overlap", "-1"),
        ("collision-only-overlap", "x"),
        ("collision-only-max-ratio", "0.5"),
        ("collision-only-max-ratio", "inf"),
        ("collision-only-min-extent", "0"),
        ("collision-only-min-extent", "-0.1"),
    ] {
        let mut some = all.to_vec();
        some.retain(|(key, _)| *key != k);
        some.push((k, bad));
        assert!(with(&some).is_err(), "{k}={bad}");
    }
    assert!(with(&[("collision-only", "sometimes")]).is_err());
    // the options round-trip
    let p = with(&all).unwrap();
    assert_eq!(
        SELECT_ANVIL_ENTITIES.instance(&p.options()).unwrap().options(),
        p.options()
    );
}
