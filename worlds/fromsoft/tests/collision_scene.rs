//! The raw reader is compared with the reference scene by triangle values, not
//! index encoding. Roof smoothing and opened-up collision are later passes;
//! all remaining non-roof triangles and the water table must agree exactly.
use primase_skate_scene::tables::{SoupTable, read_soup};
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
#[test]
fn written_collision_values() -> Result<()> {
    let Some(ours) = real_data("PRIMASE_FROMSOFT_WRITTEN_SCENE") else {
        return Ok(());
    };
    let Some(reference) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let want: serde_json::Value = serde_json::from_slice(&std::fs::read(reference.join("scene.json"))?)?;
    let collisions = want["collision"].as_array().ok_or("collision list")?;
    let game = collisions
        .iter()
        .find_map(|c| c["file"].as_str().filter(|p| p.contains("game_h_")))
        .ok_or("game collision")?;
    let a = read_soup(&std::fs::read(ours.join("collision/game.bin"))?, 17)?;
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
    let wa = read_soup(&std::fs::read(ours.join("collision/water.bin"))?, 17)?;
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
