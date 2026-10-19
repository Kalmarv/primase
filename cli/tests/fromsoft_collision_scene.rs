//! The FromSoft reader's map collision, lowered by the earlier exporter's choices made explicit (parts of hit filters
//! 8 and 27, classes 22 and 36 as water, 29 and 40 left out, 50 m around the region), against the scene our earlier
//! C# exporter wrote for the same region (`PRIMASE_FROMSOFT_SCENE`, a regression baseline), by triangle values, not
//! index encoding. Roof smoothing and opened-up collision are later passes; all remaining non-roof triangles and the
//! water table must agree exactly.
use primase_core::{Collision, World};
use primase_fromsoft::{
    dcx::Decoder,
    sekiro,
    sekiro::source::{CollisionSource, MapCollision, ObjectBodies},
};
use primase_reskate::interchange::tables::{SoupTable, read_soup};
use primase_testdata::real_data;
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn triangles(s: &SoupTable) -> BTreeMap<(u8, [u32; 9]), usize> {
    let mut result = BTreeMap::new();
    for (tri, &surface) in s.triangles.iter().zip(&s.surfaces) {
        let v = tri.map(|i| s.positions[i as usize]);
        let key = std::array::from_fn(|i| {
            let f = v[i / 3][i % 3];
            if f == 0. { 0 } else { f.to_bits() }
        });
        *result.entry((surface, key)).or_default() += 1;
    }
    result
}
/// The reader's soup of source group `group`, as the reference's table encodes it (surfaces by skate surface byte).
fn ours(world: &World, group: &str) -> Result<SoupTable> {
    let (e, _) = world
        .iter::<Collision>()
        .find(|(e, _)| world.get::<CollisionSource>(*e).is_some_and(|c| c.group == group))
        .ok_or_else(|| format!("no {group} soup"))?;
    let soup = primase_reskate::dialect::soup(world, e, None)?.ok_or("soup")?;
    Ok(SoupTable {
        positions: soup.positions,
        triangles: soup.triangles,
        surfaces: soup.surfaces,
        reserved: 0,
    })
}
#[test]
#[allow(unsafe_code)]
fn reader_collision_values() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(reference) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let want: serde_json::Value = serde_json::from_slice(&std::fs::read(reference.join("scene.json"))?)?;
    // SAFETY: the test configuration names a trusted native version-6 library.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let mut world = World::default();
    let report = sekiro::read_area(
        root,
        "m11_01",
        &decoder,
        &sekiro::Options {
            region: serde_json::from_value(want["region"].clone())?,
            ..Default::default()
        },
        &mut world,
    )?;
    println!("collision {:?}", report.collision);
    // the read keeps what the lowering may leave out: parts of other hit filters, motion-type-2 object bodies
    let other_filters = |world: &World| {
        world
            .iter::<MapCollision>()
            .filter(|(_, m)| m.parts.iter().all(|p| ![8, 27].contains(&p.hit_filter)))
            .count()
    };
    let kept = other_filters(&world);
    assert!(
        kept > 0,
        "no collision file of other hit filters read: {:?}",
        report.collision
    );
    let keyframed = world
        .iter::<ObjectBodies>()
        .flat_map(|(_, b)| &b.bodies)
        .filter(|b| b.motion_type == 2)
        .count();
    assert!(
        keyframed > 0,
        "no motion-type-2 body read: {:?}",
        report.object_collision
    );
    let mut registry = primase_core::pass::PassRegistry::new();
    sekiro::passes::register(&mut registry);
    let manager = primase_core::pass::PassManager::new(registry);
    let mut run = primase_core::pass::RunReport::default();
    manager.run(
        &mut world,
        &manager
            .registry
            .parse("lower-fromsoft-collision{hit-filters=8+27,water=22+36,leave-out=29+40,reach=50}")?,
        &mut run,
    )?;
    println!("lowered {:?}", run.steps[0].log.counts);
    assert_eq!(other_filters(&world), kept, "other filters' files stay as source");
    assert_eq!(
        run.steps[0].log.counts["collision files only other hit filters' parts name, left as source"],
        kept as u64
    );
    let collisions = want["collision"].as_array().ok_or("collision list")?;
    let game = collisions
        .iter()
        .find_map(|c| c["file"].as_str().filter(|p| p.contains("game_h_")))
        .ok_or("game collision")?;
    let a = ours(&world, "game")?;
    let b = read_soup(&std::fs::read(reference.join(game))?, 17)?;
    let mut left = triangles(&a);
    let mut right = triangles(&b);
    let mut exact = 0;
    for (key, count) in &mut left {
        if let Some(other) = right.get_mut(key) {
            let n = (*count).min(*other);
            *count -= n;
            *other -= n;
            exact += n;
        }
    }
    let mut diff = BTreeMap::<u8, (usize, usize)>::new();
    for ((surface, _), n) in left {
        diff.entry(surface).or_default().0 += n;
    }
    for ((surface, _), n) in right {
        diff.entry(surface).or_default().1 += n;
    }
    diff.retain(|_, v| *v != (0, 0));
    eprintln!(
        "map SOUP {} raw / {} reference, {exact} triangles exact; differences by surface (raw,reference): {diff:?}",
        a.triangles.len(),
        b.triangles.len()
    );
    assert!(diff.keys().all(|&s| s == 16), "non-roof mismatch");
    let wa = ours(&world, "water")?;
    let wb = read_soup(&std::fs::read(reference.join("collision/water.bin"))?, 17)?;
    assert_eq!(triangles(&wa), triangles(&wb));
    eprintln!("water SOUP {} triangles exact", wa.triangles.len());
    let mut missing = 0;
    for c in collisions {
        if let Some(path) = c["file"].as_str().filter(|p| p.contains("opened")) {
            let soup = read_soup(&std::fs::read(reference.join(path))?, 17)?;
            missing += soup.triangles.len();
        }
    }
    eprintln!("later opened-up lowering pass: {missing} reference triangles intentionally absent");
    Ok(())
}
