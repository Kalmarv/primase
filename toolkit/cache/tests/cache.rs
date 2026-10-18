use primase_cache::{Cache, KeyBuilder, source_hash};
use std::fs;
use std::path::{Path, PathBuf};

fn fresh(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("primase-cache-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    d
}

fn key(n: u32) -> primase_cache::Key {
    KeyBuilder::new("test").u32("n", n).finish()
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for shard in fs::read_dir(dir.join("v1")).expect("cache folder").flatten() {
        out.extend(
            fs::read_dir(shard.path())
                .expect("shard folder")
                .flatten()
                .map(|f| f.path()),
        );
    }
    out.sort();
    out
}

#[test]
fn every_field_and_its_framing_changes_the_key() {
    let base = || KeyBuilder::new("d").bytes("a", b"xy").bytes("b", b"z");
    let k = base().finish();
    assert_eq!(k, base().finish());
    for other in [
        KeyBuilder::new("e").bytes("a", b"xy").bytes("b", b"z"),
        KeyBuilder::new("d").bytes("a", b"x").bytes("b", b"yz"),
        KeyBuilder::new("d").bytes("b", b"z").bytes("a", b"xy"),
        KeyBuilder::new("d").bytes("a", b"xy").bytes("c", b"z"),
        base().u32("v", 1),
    ] {
        assert_ne!(other.finish(), k);
    }
    assert_ne!(
        KeyBuilder::new("d").f32("x", 0.).finish(),
        KeyBuilder::new("d").f32("x", -0.).finish()
    );
    assert_ne!(
        source_hash(&[("a.rs", b"fn a() {}")]),
        source_hash(&[("a.rs", b"fn a() { }")])
    );
    assert_ne!(source_hash(&[("a.rs", b"x")]), source_hash(&[("b.rs", b"x")]));
}

#[test]
fn hits_misses_and_corruption() {
    let dir = fresh("corrupt");
    let cache = Cache::open(&dir, 1 << 20).unwrap();
    let mut computed = 0;
    let mut get = |n: u32| {
        cache
            .get_or_put::<()>(&key(n), || {
                computed += 1;
                Ok(vec![n as u8; 100])
            })
            .unwrap()
    };
    assert_eq!(get(1), vec![1; 100]);
    assert_eq!(get(1), vec![1; 100]);
    assert_eq!(computed, 1);

    // truncated, bit-flipped and moved (wrong key) entries are refused, removed and recomputed
    let path = entries(&dir)[0].clone();
    let good = fs::read(&path).unwrap();
    let mut flipped = good.clone();
    *flipped.last_mut().unwrap() ^= 1;
    let other = Cache::open(&dir, 1 << 20).unwrap();
    other.put(&key(2), &[2; 100]).unwrap();
    let wrong_key = fs::read(entries(&dir).into_iter().find(|p| *p != path).unwrap()).unwrap();
    for bad in [good[..good.len() - 1].to_vec(), flipped, wrong_key, Vec::new()] {
        fs::write(&path, bad).unwrap();
        assert_eq!(cache.get(&key(1)), None);
        assert!(!path.exists());
        cache.put(&key(1), &[1; 100]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), good);
    }
    let s = cache.stats();
    assert_eq!((s.hits, s.corrupt), (1, 4));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn trim_keeps_the_most_recently_used_under_the_cap() {
    let dir = fresh("trim");
    // 10 entries of 1000 + 80 bytes under a 5000-byte cap: trims run as entries go in
    let cache = Cache::open(&dir, 5000).unwrap();
    for n in 0..10 {
        cache.put(&key(n), &[0; 1000]).unwrap();
        // distinct ages for a deterministic order
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    cache.trim().unwrap();
    let total: u64 = entries(&dir).iter().map(|p| fs::metadata(p).unwrap().len()).sum();
    assert!(total <= 5000, "{total}");
    assert!(cache.get(&key(9)).is_some());
    assert!(cache.get(&key(0)).is_none());
    assert!(cache.stats().evicted >= 5);
    fs::remove_dir_all(&dir).unwrap();
}

/// The child half of [`processes_share_a_directory`].
#[test]
#[ignore = "run by processes_share_a_directory"]
fn cache_child() {
    let dir = std::env::var_os("PRIMASE_CACHE_CHILD_DIR").expect("set by its parent test");
    let cache = Cache::open(&dir, 1 << 30).unwrap();
    for round in 0..20 {
        for n in 0..16u32 {
            let v = cache
                .get_or_put::<()>(&key(n), || Ok(vec![n as u8; 4096 + n as usize]))
                .unwrap();
            assert_eq!(v, vec![n as u8; 4096 + n as usize], "round {round} key {n}");
        }
    }
    assert_eq!(cache.stats().corrupt, 0);
}

#[test]
fn processes_share_a_directory() {
    let dir = fresh("processes");
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..4)
        .map(|_| {
            std::process::Command::new(&exe)
                .args(["--ignored", "--exact", "cache_child", "--test-threads=1"])
                .env("PRIMASE_CACHE_CHILD_DIR", &dir)
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    assert_eq!(entries(&dir).len(), 16);
    fs::remove_dir_all(&dir).unwrap();
}
