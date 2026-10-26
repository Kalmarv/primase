//! The disc's level archives: manifest from MAIN.DB and the archives' names, SCP nodes and spot chunks, spawns.
//! Counts are those of the recon census (`scratch/scene`), recounted here by this reader.
use primase_core::{Marker, frame::SourceFrame};
use primase_shred::{
    components::Spot,
    read::{
        level,
        manifest::{self, Difficulty, Manifest},
        pak::Pak,
        scp_tables,
    },
};
use primase_sqlite::Database;
use primase_testdata::{Game, Query, expect_found, install, install_file};
use std::path::{Path, PathBuf};

fn paks(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")))
        .collect();
    v.sort();
    v
}
fn name(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .expect("file name")
        .to_owned()
}

/// Per level, as recounted by the recon scripts over the level's archives.
struct Level {
    name: &'static str,
    archives: usize,
    nodes: usize,
    placements: usize,
    chunks: usize,
    teleports: usize,
    /// Drop-spot rows, and the rows that name a chunk.
    rows: usize,
    resolved: usize,
}
const LEVELS: [Level; 10] = [
    Level {
        name: "charlie",
        archives: 41,
        nodes: 35_820,
        placements: 49_820,
        chunks: 3_217,
        teleports: 30,
        rows: 26,
        resolved: 26,
    },
    Level {
        name: "delta",
        archives: 19,
        nodes: 12_050,
        placements: 13_323,
        chunks: 1_344,
        teleports: 22,
        rows: 9,
        resolved: 8,
    },
    Level {
        name: "echo",
        archives: 17,
        nodes: 8_382,
        placements: 8_472,
        chunks: 4_714,
        teleports: 57,
        rows: 14,
        resolved: 14,
    },
    Level {
        name: "foxtrot",
        archives: 17,
        nodes: 13_252,
        placements: 9_394,
        chunks: 7_375,
        teleports: 48,
        rows: 14,
        resolved: 14,
    },
    Level {
        name: "india",
        archives: 17,
        nodes: 11_403,
        placements: 9_749,
        chunks: 6_458,
        teleports: 87,
        rows: 30,
        resolved: 27,
    },
    Level {
        name: "juliet",
        archives: 17,
        nodes: 41_184,
        placements: 39_467,
        chunks: 7_695,
        teleports: 60,
        rows: 15,
        resolved: 15,
    },
    Level {
        name: "mike",
        archives: 40,
        nodes: 11_401,
        placements: 8_944,
        chunks: 7_637,
        teleports: 15,
        rows: 4,
        resolved: 4,
    },
    Level {
        name: "november",
        archives: 17,
        nodes: 13_576,
        placements: 9_764,
        chunks: 7_194,
        teleports: 63,
        rows: 26,
        resolved: 26,
    },
    Level {
        name: "oscar",
        archives: 17,
        nodes: 7_746,
        placements: 7_608,
        chunks: 4_012,
        teleports: 75,
        rows: 24,
        resolved: 24,
    },
    Level {
        name: "papa",
        archives: 17,
        nodes: 11_275,
        placements: 10_962,
        chunks: 3_369,
        teleports: 81,
        rows: 32,
        resolved: 25,
    },
];

#[test]
fn manifest_nodes_spot_chunks_and_spawns() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let Some(db_path) = install_file(Game::Shred, "PS3_GAME/USRDIR/database/main.db") else {
        return;
    };
    let levels_dir = root.join("PS3_GAME/USRDIR/levels");
    let mut files = paks(&levels_dir);
    files.extend(paks(&levels_dir.join("layers")));
    expect_found(Query::Install(Game::Shred, None), "level archives", files.len());
    let opened: Vec<(String, Pak)> = files
        .iter()
        .map(|p| {
            (
                name(p),
                Pak::open(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            )
        })
        .collect();
    let data = std::fs::read(&db_path).unwrap();
    let db = Database::open(&data).unwrap();
    let refs: Vec<(&str, &Pak)> = opened.iter().map(|(n, p)| (n.as_str(), p)).collect();
    let m = Manifest::new(&db, &refs).unwrap();

    // 10 bases and 209 layer archives, nothing else
    assert_eq!(
        (m.bases.len(), m.layers.len(), m.others.len()),
        (10, 209, 0),
        "{:?}",
        m.others
    );
    assert_eq!(m.levels.len(), 30);
    assert_eq!(m.session_types.len(), 12);
    assert_eq!(m.session_type(1), Some("TRICK"));
    assert_eq!(m.session_type(8), Some("TUTORIAL"));
    assert_eq!(
        m.layers
            .iter()
            .filter(|l| l.difficulty != Difficulty::Base)
            .count(),
        186
    );
    // the maps a level's archives name; the program chooses among them
    assert_eq!(
        m.map_ids("charlie"),
        [0x04, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1e, 0x1f, 0x20, 0x21]
    );
    assert_eq!(m.map_ids("delta"), [0x05, 0x22, 0x23]);
    assert_eq!(m.map_ids("mike"), [0x0c, 0x0d, 0x0e]);
    assert_eq!(m.map_ids("papa"), [0x11]);
    let files_of = |v: Vec<&manifest::LayerPak>| v.into_iter().map(|l| l.file.clone()).collect::<Vec<_>>();
    assert_eq!(files_of(m.common(0x04)), ["00000004.pak"]);
    assert_eq!(
        files_of(m.session(0x04, Difficulty::Casual, None)),
        ["88000004.pak"]
    );
    assert_eq!(
        files_of(m.session(0x0f, Difficulty::Hardcore, Some(2))),
        ["a200000f.pak"]
    );
    assert_eq!(m.base("Charlie").map(|b| b.file.as_str()), Some("charlie.pak"));

    let mut total = [0usize; 6];
    let mut unnamed = std::collections::BTreeMap::<u32, usize>::new();
    let mut undecoded_sections = 0;
    for Level {
        name: level_name,
        archives: n_paks,
        nodes,
        placements,
        chunks,
        teleports,
        rows,
        resolved,
    } in LEVELS
    {
        let mut files: Vec<String> = vec![m.base(level_name).expect("base").file.clone()];
        files.extend(m.layers_naming(level_name).iter().map(|l| l.file.clone()));
        assert_eq!(files.len(), n_paks, "{level_name} archives");
        let by_name: std::collections::BTreeMap<&str, &Pak> = refs.iter().copied().collect();
        let level_paks: Vec<(&str, &Pak)> = files.iter().map(|f| (f.as_str(), by_name[f.as_str()])).collect();
        let (world, r) = level::read_level(&level_paks, SourceFrame::WORLD).unwrap();
        assert!(world.verify_components().is_empty(), "{level_name}");
        let spots: Vec<_> = world.iter::<Spot>().collect();
        let marked = world.iter::<Marker>().count();
        assert_eq!(
            (r.nodes, r.placements, r.spots, r.teleports, marked, r.chain_stops),
            (nodes, placements, chunks, teleports, teleports, 0),
            "{level_name}"
        );
        assert_eq!(spots.len(), chunks);
        let db_name = {
            let mut c = level_name.chars();
            c.next().unwrap().to_ascii_uppercase().to_string() + c.as_str()
        };
        let drops = manifest::drop_spots(&db, &db_name)
            .unwrap()
            .expect("dropspots table");
        let hits = manifest::resolve_drop_spots(&world, &drops);
        let named = hits.iter().filter(|(_, e)| !e.is_empty()).count();
        assert_eq!((drops.len(), named), (rows, resolved), "{level_name} drop spots");
        // a drop spot's chunk is a Teleport everywhere it resolves
        for (d, es) in &hits {
            for &e in es {
                let s = world.get::<Spot>(e).unwrap();
                assert_eq!(
                    s.class_name.as_deref(),
                    Some("Teleport"),
                    "{level_name} {}",
                    d.spot_name
                );
                assert!(world.has::<Marker>(e));
            }
        }
        for (c, n) in &r.unnamed_classes {
            *unnamed.entry(*c).or_default() += n;
        }
        undecoded_sections += r
            .unread
            .iter()
            .filter(|(_, _, u)| u.reason.starts_with("blob section of a type not decoded"))
            .count();
        for (a, b) in total
            .iter_mut()
            .zip([r.nodes, r.placements, r.spots, r.teleports, drops.len(), named])
        {
            *a += b;
        }
    }
    eprintln!(
        "SHRED scene census: [nodes, placements, spot chunks, teleports, drop-spot rows, rows naming a chunk] = {total:?}; \
         {undecoded_sections} undecoded blob sections; unnamed spot classes {unnamed:08x?}"
    );
    assert_eq!(total, [166_089, 167_503, 53_015, 538, 194, 183]);
    assert_eq!(undecoded_sections, 605);
    assert_eq!(
        unnamed.into_iter().collect::<Vec<_>>(),
        [
            (0x01ab_0a9b, 163),
            (0x145a_f167, 122),
            (0x3278_9c00, 924),
            (0x57ab_8107, 799),
            (0x7641_e379, 4_971),
            (0x7d06_de87, 1_003),
            (0x92b2_eb7c, 1_075),
            (0xc9e2_8dae, 545),
            (0xdccd_6fb0, 860),
            (0xe926_a21d, 1),
            (0xf1a3_2444, 292),
            (0xf3f7_4aad, 585),
        ]
    );
    assert!(scp_tables::class_name(primase_hash::sdbm(b"Teleport")).is_some());
}
