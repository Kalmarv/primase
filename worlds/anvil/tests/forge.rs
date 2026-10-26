//! The forge index reader: a two-entry version 27 file laid out here by hand, its refusals, and the hand-checked rows
//! of a real forge (`installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge`).
use std::collections::BTreeSet;
use std::io::Cursor;

use primase_anvil::read::forge::{Error, Forge};
use primase_testdata::{Game, install_file};

/// One entry of a hand-laid forge: payload offset, index id, size, name id, type id, name.
struct Row {
    offset: i64,
    id: u64,
    size: i32,
    name_id: u64,
    type_id: u32,
    name: &'static str,
}

const INDEX_TABLE: usize = 1142;

/// A version 27 forge as the format lays it out: header size 1050, DataHeader1 there, DataHeader2 at 1094, the index
/// table at 1142 and the name table after it, each with one spare record (`MaxFilesForThisIndex` = rows + 1), then
/// `payload` bytes of payload.
fn forge(rows: &[Row], payload: usize) -> Vec<u8> {
    let n = rows.len();
    let names = INDEX_TABLE + 20 * (n + 1);
    let end = names + 192 * (n + 1);
    let mut b = vec![0u8; end + payload];
    let mut put = |at: usize, bytes: &[u8]| b[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, b"scimitar\0");
    put(0x09, &27i32.to_le_bytes());
    put(0x0D, &1050u64.to_le_bytes());
    put(0x15, &16i64.to_le_bytes());
    put(0x1D, &1i32.to_le_bytes());
    // DataHeader1
    put(1050, &(n as i32).to_le_bytes());
    put(1054, &2i32.to_le_bytes());
    put(1070, &(-1i64).to_le_bytes());
    put(1078, &(n as i32 + 1).to_le_bytes());
    put(1082, &1i32.to_le_bytes());
    put(1086, &1094i64.to_le_bytes());
    // DataHeader2
    put(1094, &(n as i32).to_le_bytes());
    put(1098, &2i32.to_le_bytes());
    put(1102, &(INDEX_TABLE as i64).to_le_bytes());
    put(1110, &(-1i64).to_le_bytes());
    put(1118, &0i32.to_le_bytes());
    put(1122, &(n as i32).to_le_bytes());
    put(1126, &(names as i64).to_le_bytes());
    put(1134, &(end as i64).to_le_bytes());
    for (k, r) in rows.iter().enumerate() {
        let i = INDEX_TABLE + 20 * k;
        put(i, &r.offset.to_le_bytes());
        put(i + 8, &r.id.to_le_bytes());
        put(i + 16, &r.size.to_le_bytes());
        let m = names + 192 * k;
        put(m, &r.size.to_le_bytes());
        put(m + 4, &r.name_id.to_le_bytes());
        put(m + 16, &r.type_id.to_le_bytes());
        put(m + 28, &(k as i32 + 1).to_le_bytes());
        put(m + 40, &1_789_743_211i32.to_le_bytes());
        put(m + 44, r.name.as_bytes());
        put(m + 172, &4i32.to_le_bytes());
    }
    b
}

/// Two entries after the tables: a 10-byte one and a 6-byte one, ids that differ between the tables.
fn two() -> Vec<u8> {
    let start = (INDEX_TABLE + 20 * 3 + 192 * 3) as i64;
    forge(
        &[
            Row {
                offset: start,
                id: 16,
                size: 10,
                name_id: 0x5E5E_0000_0000_0001,
                type_id: 0,
                name: "GlobalMetaFile",
            },
            Row {
                offset: start + 10,
                id: 2_008_284_084_114,
                size: 6,
                name_id: 0x5E5E_0000_0000_0002,
                // CRC-32 of "Mesh"
                type_id: 0x415D_9568,
                name: "Sphere",
            },
        ],
        16,
    )
}

fn read(b: Vec<u8>) -> Result<Forge, Error> {
    Forge::read(&mut Cursor::new(b))
}

/// The hand-laid forge reads to the values laid: its headers as stored, each entry's payload offset and size and both
/// ids kept apart, the name records' fields by position.
#[test]
fn a_two_entry_forge_reads_as_laid() {
    let b = two();
    let len = b.len() as u64;
    let f = read(b).unwrap();
    assert_eq!(f.file_size, len);
    assert_eq!((f.header.version, f.header.header_size), (27, 1050));
    assert_eq!((f.header.unknown_15, f.header.unknown_1d), (16, 1));
    let d1 = &f.data_header_1;
    assert_eq!(
        (
            d1.entries,
            d1.max_files_for_this_index,
            d1.unknown_20,
            d1.offset_to_data
        ),
        (2, 3, 1, 1094)
    );
    assert_eq!((d1.unknown_04.as_slice(), d1.unknown_14), (&[2, 0, 0, 0][..], -1));
    let d2 = &f.data_header_2;
    assert_eq!(
        (
            d2.index_count,
            d2.unknown_04,
            d2.offset_to_index_table,
            d2.offset_to_next_data_section
        ),
        (2, 2, 1142, -1)
    );
    assert_eq!((d2.index_start, d2.index_end), (0, 2));
    assert_eq!(d2.offset_to_name_table, 1142 + 60);
    assert_eq!(d2.unknown_28, 1142 + 60 + 576);
    let rows: Vec<_> = f.entries().collect();
    assert_eq!(rows.len(), 2);
    let (i, n) = rows[1];
    assert_eq!(
        (i.offset, i.id, i.size),
        (1142 + 60 + 576 + 10, 2_008_284_084_114, 6)
    );
    assert_eq!((n.size, n.id, n.type_id), (6, 0x5E5E_0000_0000_0002, 0x415D_9568));
    assert_eq!(n.name, "Sphere");
    assert_eq!(
        (
            n.unknown_1c,
            n.timestamp,
            n.unknown_ac,
            n.meta_file_key,
            n.unknown_b8
        ),
        (2, 1_789_743_211, 4, 0, [0, 0])
    );
    assert_eq!((rows[0].0.id, rows[0].1.name.as_str()), (16, "GlobalMetaFile"));
}

/// Each refusal names what it refuses: another version, an entry past the file or at a negative offset, index and
/// name sizes that differ, an entry count over `MaxFilesForThisIndex`, a name table outside the file, a second
/// FileSet.
#[test]
fn a_forge_out_of_its_layout_is_refused_by_name() {
    let set = |at: usize, bytes: &[u8]| {
        let mut b = two();
        b[at..at + bytes.len()].copy_from_slice(bytes);
        read(b).unwrap_err().to_string()
    };
    assert_eq!(
        set(0x09, &28i32.to_le_bytes()),
        "forge version 28: only versions 26 and 27 are read"
    );
    assert!(set(0, b"scimitaR").starts_with("not a forge"));
    let e = set(INDEX_TABLE + 20 + 16, &1000i32.to_le_bytes());
    assert_eq!(
        e,
        format!(
            "entry 1: payload offset {}, size 1000: past the end of the file",
            1142 + 60 + 576 + 10
        )
    );
    let e = set(INDEX_TABLE, &(-4i64).to_le_bytes());
    assert_eq!(e, "entry 0: payload offset -4, size 10: negative offset");
    let e = set(1142 + 60, &11i32.to_le_bytes());
    assert_eq!(
        e,
        "entry 0: index size 10, name record size 11: the tables do not correspond"
    );
    let e = set(1078, &1i32.to_le_bytes());
    assert_eq!(e, "the entry count 2: over MaxFilesForThisIndex 1");
    let e = set(1126, &(1i64 << 40).to_le_bytes());
    assert!(
        e.starts_with("the name table at 1099511627776, 384 bytes: outside the file"),
        "{e}"
    );
    assert_eq!(
        set(1110, &5000i64.to_le_bytes()),
        "a second FileSet at 5000: only a forge with one is read"
    );
    let e = set(1094, &3i32.to_le_bytes());
    assert_eq!(e, "the index count 3: not the entry count 2");
}

/// A real forge's hand-checked rows (the archive-reader fit report and a direct read of the file): 337 entries, each
/// table's ids unique, rows 0, 1, 2 and 5 as checked, the tables where its headers put them.
#[test]
fn a_real_forge_s_rows_are_the_hand_checked_ones() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_WhiteRoom_patch_01.forge") else {
        return;
    };
    let f = Forge::open(&path).unwrap();
    assert_eq!(f.file_size, 1_021_872);
    assert_eq!(f.index.len(), 337);
    assert_eq!(f.data_header_1.max_files_for_this_index, 338);
    assert_eq!(
        (
            f.data_header_2.offset_to_index_table,
            f.data_header_2.offset_to_name_table,
            f.data_header_2.unknown_28
        ),
        (1142, 7902, 72798)
    );
    let ids: BTreeSet<u64> = f.index.iter().map(|i| i.id).collect();
    let name_ids: BTreeSet<u64> = f.names.iter().map(|n| n.id).collect();
    assert_eq!((ids.len(), name_ids.len()), (337, 337));
    let row = |k: usize| {
        let (i, n) = (&f.index[k], &f.names[k]);
        (i.offset, i.id, i.size, n.id, n.type_id, n.name.as_str())
    };
    assert_eq!(
        row(0),
        (132_287, 16, 7281, 6_796_468_573_833_355_514, 0, "GlobalMetaFile")
    );
    // LODSelector
    assert_eq!(
        row(1),
        (
            139_568,
            2_008_284_084_114,
            205,
            8_454_181_524_015_169_305,
            0x51DC_6B80,
            "MAD_PRP_DARKROOM_Stand"
        )
    );
    // Mesh
    assert_eq!(
        row(2),
        (
            139_773,
            2_008_284_084_116,
            14_047,
            718_289_710_375_149_047,
            0x415D_9568,
            "MAD_PRP_DARKROOM_Stand_LOD0"
        )
    );
    // TextureMap
    assert_eq!(
        row(5),
        (
            154_384,
            1_797_335_016_519,
            43_301,
            11_409_991_704_625_460_451,
            0xA2B7_E917,
            "MAD_TEX_TIL_Metal_Painted_A_DiffuseMap_PC_Scarlett_Prospero"
        )
    );
}

/// An entry is found by its index id; two entries with one index id are refused (a reference would be ambiguous).
#[test]
fn entries_are_found_by_index_id_and_a_repeated_id_is_refused() {
    let f = read(two()).unwrap();
    assert_eq!(f.find(16), Some(0));
    assert_eq!(f.find(2_008_284_084_114), Some(1));
    // a name record's id is not an index id
    assert_eq!(f.find(0x5E5E_0000_0000_0001), None);
    let start = (INDEX_TABLE + 20 * 3 + 192 * 3) as i64;
    let row = |offset, name| Row {
        offset,
        id: 7,
        size: 4,
        name_id: offset as u64,
        type_id: 0,
        name,
    };
    let twice = forge(&[row(start, "A"), row(start + 4, "B")], 8);
    assert!(matches!(
        read(twice),
        Err(Error::DuplicateId {
            id: 7,
            first: 0,
            second: 1
        })
    ));
}
