//! Real LODSelectors read as stored (Riders Republic build 25394987, base WhiteRoom forge), every field against the
//! bytes (games/riders-republic docs/recon/placement-darkroom-handoff.md, b8dae7b), every reference and dependency
//! resolving within the forge to a Mesh record.
use std::fs::File;

use anvil_formats::archive::MAX_RECORD;
use anvil_formats::data::{Resource, read_entry};
use anvil_formats::forge::Forge;
use anvil_formats::lod::{LOD_SELECTOR, StoredLodSelector};
use anvil_formats::mesh::MESH;
use anvil_formats::reference::Reference;
use primase_testdata::{Game, install_file};

#[test]
fn lod_selectors_read_their_levels_and_dependencies() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_WhiteRoom.forge") else {
        return;
    };
    let forge = Forge::open(&path).expect("forge");
    let mut file = File::open(&path).expect("open");
    let mut resource = |entry: usize| {
        let payload = forge.payload(&mut file, entry).expect("payload");
        let block = read_entry(&payload, MAX_RECORD).expect("entry");
        Resource::parse(&block.resource.data).expect("resource")
    };
    let mesh = |id: u64| Some(Reference { mode: 1, id });
    // entry, body length, references, first floats, bytes 34..39, u32s at 39, dependencies, their words
    let fixtures = [
        (
            24,
            266,
            [mesh(2_008_284_084_116), None, None, None, None],
            [20.0, 40.0, 80.0, 160.0, 320.0],
            [[0, 0, 0, 0, 1]; 5],
            [0, 0, 0, 0, 0],
            vec![],
            vec![],
        ),
        (
            159,
            292,
            [None, None, mesh(2_027_024_444_217), mesh(2_027_024_444_229), None],
            [30.0, 80.0, 120.0, 640.0, 641.0],
            [
                [0, 0, 0, 0, 1],
                [0, 0, 0, 0, 1],
                [0, 0, 0, 0, 1],
                [1, 0, 0, 0, 1],
                [0, 0, 0, 0, 1],
            ],
            [0, 0, 1, 2, 2],
            vec![2_027_024_443_838, 2_027_024_444_205],
            vec![103_208, 43_820],
        ),
    ];
    for (entry, length, references, firsts, bytes_34, u32s, dependencies, words) in fixtures {
        let r = resource(entry);
        assert_eq!((r.type_id, r.body.len()), (LOD_SELECTOR, length), "entry {entry}");
        let s = StoredLodSelector::parse(&r.body).expect("selector");
        assert_eq!((s.head, s.head_rest, s.trailer), (0, [0; 7], [0, 0, 0, 0, 1, 0]));
        for (k, level) in s.levels.iter().enumerate() {
            assert_eq!(level.head, r.class_id + 13 + k as u64, "entry {entry} level {k}");
            assert_eq!(level.reference, references[k], "entry {entry} level {k}");
            assert_eq!(level.floats, [firsts[k], 0.0, 1.0], "entry {entry} level {k}");
            assert_eq!(
                (level.bytes_34, level.u32_39, level.last),
                (bytes_34[k], u32s[k], 0)
            );
        }
        assert_eq!((&s.dependencies, &s.dependency_words), (&dependencies, &words));
        let named = s.levels.iter().filter_map(|l| l.reference.map(|r| r.id));
        for id in named.chain(dependencies.iter().copied()) {
            let m = resource(forge.find(id).expect("in this forge"));
            assert_eq!((m.type_id, m.class_id), (MESH, id));
        }
    }
}
