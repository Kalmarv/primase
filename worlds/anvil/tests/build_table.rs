//! A BuildTable laid out by hand (two columns, two rows, resources), and the census of every BuildTable of Riders
//! Republic's forges read to its last byte: counts of columns, rows, properties and links, and that the rider part
//! tables name what they should.
use primase_anvil::components::{BuildLink, StoredBuildTable};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::build_table::BUILD_TABLE;
use primase_testdata::{Game, install};

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn link(tag: u8, id: u64) -> Vec<u8> {
    let mut b = vec![tag];
    b.extend(id.to_le_bytes());
    b
}

/// A typed value of kind 0x12 (a link) to the class `class`.
fn linked(class: u32, id: u64) -> Vec<u8> {
    let mut b = u32s(&[class]);
    b.extend([0, 0]);
    b.extend(u32s(&[0x12]));
    b.extend([0, 0]);
    b.extend(link(0, id));
    b
}

fn column(id: u64, number: u32, own: u64, class: u32, nodes: &[Vec<u8>]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(id.to_le_bytes());
    b.extend(u32s(&[0x3683_9608]));
    b.extend((id + 1).to_le_bytes());
    b.extend(u32s(&[0x913E_9298]));
    b.push(4);
    b.extend(0u64.to_le_bytes());
    b.extend(u32s(&[0x3C00_B2E0, u32::try_from(nodes.len()).expect("count")]));
    for n in nodes {
        b.extend(n);
    }
    b.extend([1, 0]);
    b.extend(u32s(&[number, 0, 0]));
    b.extend(link(1, own));
    b.extend(u32s(&[1, 0x2CEC_F817]));
    b.extend(linked(class, 0));
    b
}

fn row(id: u64, own: u64, weight: f32, condition: u32, props: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(id.to_le_bytes());
    b.extend(u32s(&[0x348B_28D6]));
    b.extend(link(1, own));
    b.extend(weight.to_le_bytes());
    b.extend(0u64.to_le_bytes());
    b.extend(u32s(&[0xB332_698E, condition]));
    b.extend((id + 1).to_le_bytes());
    b.extend(u32s(&[
        0x11BD_5345,
        0,
        u32::try_from(props.len()).expect("count"),
    ]));
    for (c, v) in props {
        b.extend(u32s(&[*c]));
        b.extend(v);
    }
    b
}

fn body() -> Vec<u8> {
    let own = 0x01B5_C14C_FB5E;
    // a node: id 0, class, 41 bytes, a null solver
    let mut node = 0u64.to_le_bytes().to_vec();
    node.extend(u32s(&[0x1A87_50BA]));
    node.extend([7u8; 41]);
    node.push(3);
    let mut b = vec![1];
    b.extend(u32s(&[2]));
    b.extend(column(0x100, 1, own, 0x24AE_CB7C, &[]));
    b.extend(column(0x200, 2, own, 0x85C8_17C3, &[node]));
    b.extend(u32s(&[2]));
    b.extend(row(
        0x300,
        own,
        1.0,
        0x0B27_268E,
        &[(1, linked(0x24AE_CB7C, 0xAA))],
    ));
    b.extend(row(
        0x400,
        own,
        0.0,
        0xE2B6_0126,
        &[(2, linked(0x85C8_17C3, 0xBB))],
    ));
    b.extend([0, 1, 0, 0]);
    b.extend(u32s(&[1]));
    b.extend(0x500u64.to_le_bytes());
    b.extend(u32s(&[0xDED3_E212, 2]));
    b.extend(link(0, 0xAA));
    b.extend(link(0, 0xBB));
    b
}

#[test]
fn a_table_reads_its_columns_rows_and_resources() {
    let t = StoredBuildTable::parse(&body()).expect("table");
    assert_eq!(t.columns.len(), 2);
    assert_eq!(t.columns[0].fields[0], 1);
    assert_eq!(t.columns[0].declared.class, 0x24AE_CB7C);
    assert_eq!(t.columns[1].path.len(), 1);
    assert_eq!(t.columns[1].path[0].fields, vec![7u8; 41]);
    assert_eq!(t.columns[1].path[0].solver, None);
    assert_eq!(t.rows.len(), 2);
    assert_eq!((t.rows[0].weight, t.rows[0].condition), (1.0, 0x0B27_268E));
    assert_eq!((t.rows[1].weight, t.rows[1].condition), (0.0, 0xE2B6_0126));
    let p = &t.rows[0].properties[0];
    assert_eq!(p.column, 1);
    assert_eq!(p.value.link().expect("link"), BuildLink { tag: 0, id: 0xAA });
    assert!(p.value.reference().is_err());
    assert_eq!(t.flags, [0, 1, 0, 0]);
    assert_eq!(t.resources.len(), 1);
    assert_eq!(
        t.resources[0].links,
        vec![BuildLink { tag: 0, id: 0xAA }, BuildLink { tag: 0, id: 0xBB }]
    );
}

#[test]
fn a_short_body_a_wrong_class_and_trailing_bytes_are_refused() {
    let b = body();
    assert!(StoredBuildTable::parse(&b[..b.len() - 1]).is_err());
    let mut long = b.clone();
    long.push(0);
    assert!(StoredBuildTable::parse(&long).is_err());
    let mut bad = b.clone();
    bad[13] ^= 1; // the first column's class
    assert!(StoredBuildTable::parse(&bad).is_err());
    assert!(StoredBuildTable::parse(&[2]).is_err());
}

fn open() -> Option<Archive> {
    let dir = install(Game::Riders)?;
    let forges = [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ];
    Archive::open(&forges.map(|f| dir.join(f))).ok()
}

/// Every BuildTable of the forges reads to its last byte; the totals the census gives.
#[test]
fn every_build_table_reads_to_its_last_byte() {
    let Some(mut archive) = open() else {
        return;
    };
    let mut at = Vec::new();
    for forge in 0..5 {
        let f = archive.forge(forge).expect("forge");
        at.extend(
            f.entries()
                .enumerate()
                .filter(|(_, (_, n))| n.type_id == BUILD_TABLE)
                .map(|(entry, (_, n))| (At { forge, entry }, n.name.clone())),
        );
    }
    let (mut columns, mut rows, mut props, mut resources, mut links) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for (a, name) in &at {
        let r = archive.resource(*a).unwrap_or_else(|e| panic!("{name}: {e}"));
        let t = StoredBuildTable::parse(&r.body).unwrap_or_else(|e| panic!("{name}: {e}"));
        columns += t.columns.len();
        rows += t.rows.len();
        props += t.rows.iter().map(|r| r.properties.len()).sum::<usize>();
        resources += t.resources.len();
        links += t.resources.iter().map(|r| r.links.len()).sum::<usize>();
    }
    println!(
        "{} BuildTables, {columns} columns, {rows} rows, {props} properties, {resources} DynamicResources of {links} links",
        at.len()
    );
    assert_eq!(at.len(), 10_730);
    assert_eq!(
        (columns, rows, props, resources, links),
        (32_289, 22_728, 43_035, 14_632, 24_709)
    );
}

/// The rider's footwear part: male and female rows (the gender conditions, weights 1 and 0), each naming a LODSelector,
/// its mesh and the addon skeleton, and the `_Materials` table.
#[test]
fn the_footwear_part_table_names_its_selector_skeleton_and_materials() {
    let Some(mut archive) = open() else {
        return;
    };
    let mut found = None;
    for forge in 0..5 {
        let f = archive.forge(forge).expect("forge");
        for (entry, (_, n)) in f.entries().enumerate() {
            if n.type_id == BUILD_TABLE && n.name == "MAD_CHR_FOOTW_001_U" {
                found = Some(At { forge, entry });
            }
        }
    }
    let at = found.expect("MAD_CHR_FOOTW_001_U");
    let t = StoredBuildTable::parse(&archive.resource(at).expect("resource").body).expect("table");
    assert_eq!(t.columns.len(), 6);
    assert_eq!(
        t.rows.iter().map(|r| r.condition).collect::<Vec<_>>(),
        [0x0B27_268E, 0xE2B6_0126]
    );
    assert_eq!(t.rows.iter().map(|r| r.weight).collect::<Vec<_>>(), [1.0, 0.0]);
    // every property names a link or reference that resolves to a resource of the forges
    for row in &t.rows {
        for p in &row.properties {
            let id = match p.value.kind & 0x3F {
                0x12 => p.value.link().expect("link").id,
                0x1C => p.value.reference().expect("reference").map_or(0, |r| r.id),
                k => panic!("kind {k:#x}"),
            };
            assert!(archive.find(id).is_some(), "{id:#x}");
        }
    }
    assert_eq!(t.resources.len(), 2);
}
