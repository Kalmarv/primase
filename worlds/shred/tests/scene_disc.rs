//! The disc's level archives: manifest from MAIN.DB and the archives' names, SCP nodes and spot chunks, spawns,
//! splines and lights.
//! Counts are those of the recon census (`scratch/scene`), recounted here by this reader.
mod inputs;

use inputs::{Game, Query, expect_found, install, install_file};
use primase_core::{Marker, frame::SourceFrame};
use primase_shred::read::database::Database;
use primase_shred::{
    components::{Locator, Occluder, SceneLight, Spline, Spot, SpotCamera},
    read::{
        level,
        manifest::{self, Difficulty, Manifest},
        names,
        pak::Pak,
        scp_tables,
    },
};
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
    // splines by section type, lights by kind code, names given
    let mut spline_classes = std::collections::BTreeMap::<u32, usize>::new();
    let mut light_kinds = std::collections::BTreeMap::<u32, usize>::new();
    let mut rail_surfaces = std::collections::BTreeMap::<u32, usize>::new();
    let (mut cameras, mut locators, mut waypoints) = (0usize, 0usize, 0usize);
    let mut occluder_shapes = std::collections::BTreeMap::<u32, usize>::new();
    let occluder_names: std::collections::BTreeSet<u32> = (0..1000)
        .map(|n| primase_foundations::hash::sdbm(format!("RMOccluder{n}").as_bytes()))
        .chain([primase_foundations::hash::sdbm(b"RMOccluder")])
        .collect();
    let mut occluders_named = 0usize;
    let (mut named_guides, mut named_rails) = (0usize, 0usize);
    let surface_ids: std::collections::BTreeSet<i64> = db
        .table("surfaces")
        .unwrap()
        .rows
        .iter()
        .map(|r| r.rowid)
        .collect();
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
        let (mut world, r) = level::read_level(&level_paks, SourceFrame::WORLD).unwrap();
        assert!(world.verify_components().is_empty(), "{level_name}");
        for (_, sp) in world.iter::<Spline>() {
            *spline_classes.entry(sp.flags >> 16).or_default() += 1;
            if sp.flags >> 16 == 0 {
                *rail_surfaces.entry(sp.surface).or_default() += 1;
                assert!(
                    sp.surface == 0 || surface_ids.contains(&i64::from(sp.surface)),
                    "{level_name}: rail candidate surface {}",
                    sp.surface
                );
            } else {
                assert_eq!((sp.words, sp.surface), ([0, 0], 0), "{level_name}");
            }
            assert_eq!(sp.counts[0] as usize, sp.params.len());
        }
        for (_, l) in world.iter::<SceneLight>() {
            *light_kinds.entry(l.kind).or_default() += 1;
        }
        for (_, c) in world.iter::<SpotCamera>() {
            cameras += 1;
            assert_eq!(
                (c.value.to_bits(), c.flag),
                (0x4256_8d50, 1),
                "{level_name}: spot camera"
            );
        }
        for (e, l) in world.iter::<Locator>() {
            locators += 1;
            assert_eq!(
                (l.class, l.kind),
                (
                    primase_foundations::hash::sdbm(b"RMLocator"),
                    primase_foundations::hash::sdbm(b"Waypoint")
                )
            );
            assert_eq!(world.get::<Marker>(e).map(|m| m.kind.as_str()), Some("waypoint"));
            waypoints += 1;
        }
        assert_eq!(r.spot_cameras, world.iter::<SpotCamera>().count());
        assert_eq!(
            (r.locators, r.waypoints),
            (
                world.iter::<Locator>().count(),
                world
                    .iter::<Marker>()
                    .filter(|(_, m)| m.kind == "waypoint")
                    .count()
            )
        );
        for (_, o) in world.iter::<Occluder>() {
            *occluder_shapes.entry(o.flags & 0xffff).or_default() += 1;
            assert_eq!(o.outline.len(), if o.flags & 0xffff == 0x0800 { 4 } else { 12 });
            occluders_named += usize::from(occluder_names.contains(&o.name));
        }
        assert_eq!(r.occluders, world.iter::<Occluder>().count());
        assert_eq!(r.lights, world.iter::<SceneLight>().count());
        assert_eq!(r.light_kinds.values().sum::<usize>(), r.lights);
        assert_eq!(r.splines, r.spline_classes.iter().sum::<usize>());
        assert_eq!(r.splines, world.iter::<Spline>().count());
        let named = names::name_splines(&mut world, &[]);
        named_guides += named;
        let rails = names::flythrough_rails(&db, &{
            let mut c = level_name.chars();
            c.next().unwrap().to_ascii_uppercase().to_string() + c.as_str()
        })
        .unwrap()
        .unwrap_or_default();
        named_rails += names::name_splines(&mut world, &rails) - named;
        let spots: Vec<_> = world.iter::<Spot>().collect();
        let marked = world
            .iter::<Marker>()
            .filter(|(_, m)| m.kind == "teleport")
            .count();
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
    eprintln!(
        "SHRED spline/light census: splines by type {spline_classes:?}; lights by kind {light_kinds:?}; named by NavGuide scheme {named_guides}, by flythrough rails {named_rails}"
    );
    assert_eq!(total, [166_089, 167_503, 53_015, 538, 194, 183]);
    // 605 before splines (186) and lights (55) were read, 364 before the splines of types 0xA and 0xC and the
    // ones whose flags had other low halves (45 more sections)
    // and 11 more once the spot cameras (10 sections) and the locator (1) were read, 9 more with the occluders
    assert_eq!(undecoded_sections, 299);
    // 341 occluders: 269 rectangle outlines (flags 0x0800) and 72 box outlines (0x1801)
    // 276 of them are named RMOccluder or RMOccluder0 to 999; the other 65 hash names not known
    assert_eq!(occluders_named, 276);
    assert_eq!(
        occluder_shapes.into_iter().collect::<Vec<_>>(),
        [(0x0800, 269), (0x1801, 72)]
    );
    assert_eq!((cameras, locators, waypoints), (281, 1, 1));
    // 33,575 splines in the five classes the game keeps them in, by section type (SCENE.md's 31,105 total does
    // not reconcile with any recount)
    assert_eq!(
        spline_classes.into_iter().collect::<Vec<_>>(),
        [(0, 2_334), (1, 2_200), (9, 25_250), (0xa, 3_760), (0xc, 31)]
    );
    assert_eq!(
        light_kinds.into_iter().collect::<Vec<_>>(),
        [(0, 5_251), (1, 23), (3, 4), (4, 23), (7, 4_079), (8, 398)]
    );
    assert_eq!((named_guides, named_rails), (1_897, 300));
    // the rail candidates' surface ids as the recon census counted them
    assert_eq!(
        rail_surfaces.into_iter().collect::<Vec<_>>(),
        [
            (0, 666),
            (2, 34),
            (3, 683),
            (11, 381),
            (14, 13),
            (32, 290),
            (37, 5),
            (39, 23),
            (40, 12),
            (41, 141),
            (43, 43),
            (46, 19),
            (53, 24)
        ]
    );
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
    assert!(scp_tables::class_name(primase_foundations::hash::sdbm(b"Teleport")).is_some());
}
