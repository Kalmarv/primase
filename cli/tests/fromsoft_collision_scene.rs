//! The FromSoft reader's raw collision against the scene our earlier C# exporter wrote for the same region
//! (`PRIMASE_FROMSOFT_SCENE`, a regression baseline), by triangle values, not index encoding. Roof smoothing and
//! opened-up collision are later passes; all remaining non-roof triangles and the water table must agree exactly.
use primase_core::{Collision, World};
use primase_fromsoft::{dcx::Decoder, sekiro, sekiro::source::CollisionSource};
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
