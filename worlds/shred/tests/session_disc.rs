//! Source census, not a visibility oracle. Native/hash proof and hand-worked fixtures cover interpretation;
//! this checks all ten installed base maps and every layer PAK whose quartet namespace names that map.
use primase_core::{Instance, convert::Reader, frame::SourceFrame};
use primase_shred::{
    components::{SessionGraph, SourcePlacement},
    read::{
        convert::ShredReader,
        database::{Database, Value},
        pak::Pak,
    },
};
use primase_testdata::{Game, install, install_file};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn ten_maps_keep_every_placement_and_their_complete_graphs() {
    let (Some(root), Some(db_path)) = (
        install(Game::Shred),
        install_file(Game::Shred, "PS3_GAME/USRDIR/database/main.db"),
    ) else {
        return;
    };
    let bytes = std::fs::read(db_path).expect("MAIN.DB");
    let levels = Database::open(&bytes)
        .expect("SQLite")
        .table("levels")
        .expect("levels");
    let id = levels.column("id").expect("id");
    let name = levels.column("NAME").expect("NAME");
    let dir = root.join("PS3_GAME/USRDIR/levels");
    let mut maps = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("levels directory") {
        let path = entry.expect("directory entry").path();
        if path.is_file() && path.extension().is_some_and(|s| s.eq_ignore_ascii_case("pak")) {
            maps.insert(
                path.file_stem().expect("stem").to_string_lossy().to_lowercase(),
                path,
            );
        }
    }
    assert_eq!(maps.len(), 10);
    let mut layers = Vec::new();
    for entry in std::fs::read_dir(dir.join("layers")).expect("layers directory") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|s| s.eq_ignore_ascii_case("pak")) {
            layers.push(path);
        }
    }
    layers.sort();
    let mut matched_layers = BTreeMap::<String, Vec<(String, Pak)>>::new();
    for path in layers {
        let pak = Pak::open(&path).expect("layer PAK");
        let mut owners = BTreeSet::new();
        for entry in pak.entries() {
            let path = String::from_utf8_lossy(&entry.name)
                .replace('/', "\\")
                .to_lowercase();
            let mut parts = path.split('\\');
            if parts.next() == Some("levels")
                && let Some(map) = parts.next()
                && maps.contains_key(map)
            {
                owners.insert(map.to_string());
            }
        }
        assert!(
            owners.len() <= 1,
            "layer belongs to several maps: {}",
            path.display()
        );
        if let Some(owner) = owners.into_iter().next() {
            matched_layers
                .entry(owner)
                .or_default()
                .push((path.file_name().expect("name").to_string_lossy().into(), pak));
        }
    }
    for (map, path) in maps {
        // Each map's first level row is an explicit source-data census input, not an active-session choice.
        let row = levels
            .rows
            .iter()
            .find(|r| matches!(&r.values[name], Value::Text(s) if s.eq_ignore_ascii_case(&map)))
            .expect("map's level row");
        let Value::Text(level_id) = &row.values[id] else {
            panic!("level id not text")
        };
        let graph =
            SessionGraph::from_main_db(&bytes, level_id.parse().expect("level id")).expect("session graph");
        let task_hashes: BTreeSet<_> = graph
            .tasks
            .iter()
            .flat_map(|r| &r.identifiers)
            .map(|i| i.hash)
            .collect();
        let goal_hashes: BTreeSet<_> = graph
            .goals
            .iter()
            .flat_map(|r| &r.identifiers)
            .map(|i| i.hash)
            .collect();
        let session_hashes: BTreeSet<_> = graph
            .sessions
            .iter()
            .flat_map(|r| &r.identifiers)
            .map(|i| i.hash)
            .collect();
        let base = Pak::open(path).expect("base PAK");
        let mut paks = vec![(map.as_str(), &base)];
        let extra = matched_layers.remove(&map).unwrap_or_default();
        paks.extend(extra.iter().map(|(n, p)| (n.as_str(), p)));
        let mut reader = ShredReader::new(paks, SourceFrame::WORLD).with_sessions(graph.clone());
        let (world, _) = reader.read().expect("source world");
        assert!(world.verify_components().is_empty());
        let mut counts = [0usize; 4];
        for (e, p) in world.iter::<SourcePlacement>() {
            assert_eq!(p.name, p.words[3]);
            assert_eq!(
                world
                    .get::<Instance>(e)
                    .expect("all instances retained")
                    .prototype,
                p.prototype
            );
            counts[0] += 1;
            counts[1] += usize::from(task_hashes.contains(&p.name));
            counts[2] += usize::from(goal_hashes.contains(&p.name));
            counts[3] += usize::from(session_hashes.contains(&p.name));
        }
        assert_eq!(counts[0], world.iter::<Instance>().count());
        assert_eq!(world.resource::<SessionGraph>(), Some(&graph));
        eprintln!(
            "SHRED session source {map} level={level_id}: {} PAKs; {} placements, task/goal/session identifier matches={}/{}/{}; {} sessions, {} goals, {} tasks; dropped=0 (reader selects nothing)",
            extra.len() + 1,
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            graph.sessions.len(),
            graph.goals.len(),
            graph.tasks.len()
        );
        if map == "charlie" {
            assert_eq!(counts[1], 202);
            for (index, name) in [
                (154, "Grind7_Grind"),
                (155, "Grind8_Grind"),
                (156, "Grind9_Grind"),
                (170, "Flatland7_Flatland"),
                (197, "Air_Icon13_Air"),
            ] {
                let source = world
                    .iter::<SourcePlacement>()
                    .find(|(_, p)| p.index == index && p.quartet.ends_with(b"challengesession"))
                    .expect("picked icon")
                    .1;
                assert_eq!(source.name, primase_foundations::hash::sdbm(name.as_bytes()));
            }
        }
    }
}
