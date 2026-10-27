//! Format contracts for the offline-established GI profile; these fixtures do not assert lighting semantics.
use anvil_formats::gidyn::StoredGiDyn;

fn fixture() -> Vec<u8> {
    let mut b = vec![1];
    b.extend(7u64.to_le_bytes());
    b.extend(27u32.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    for class in [0x684A_17ADu32, 0xCE0C_489A] {
        b.extend(0u64.to_le_bytes());
        b.extend(class.to_le_bytes());
    }
    for position in [100f32, 200., 300.] {
        b.extend(position.to_le_bytes());
    }
    b.extend(1u32.to_le_bytes()); // sample count
    b.extend(1u32.to_le_bytes()); // probe count
    b.push(0); // outdoor
    b.extend(1u32.to_le_bytes());
    for class in [0xACC1_B118u32, 0xE96B_B4A2] {
        b.extend(0u64.to_le_bytes());
        b.extend(class.to_le_bytes());
    }
    for corner in [0i32, 0, 0, 1, 1, 1] {
        b.extend(corner.to_le_bytes());
    }
    b.extend(0u32.to_le_bytes());
    let samples = [0xFFFEu16, 5, 0]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let probes = [0x3C00u16, 0xC000, 0x3800, 0]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    for stream in [samples, vec![0xFE, 32, 0], vec![0, 0], probes] {
        b.extend(u32::try_from(stream.len()).expect("fixture length").to_le_bytes());
        b.extend(stream);
    }
    for size in [10u32, 32, 2] {
        b.extend(size.to_le_bytes());
    }
    b
}

#[test]
fn probe_positions_close_and_all_truncations_are_refused() {
    let bytes = fixture();
    let stored = StoredGiDyn::parse(&bytes).expect("established profile");
    assert_eq!(
        (stored.world_id, stored.cell_id, stored.samples, stored.probes),
        (7, 27, 1, 1)
    );
    assert_eq!(
        stored.probe_positions().collect::<Vec<_>>(),
        [[101., 198., 300.5]]
    );
    assert_eq!(stored.decoded_bytes, [10, 32, 2]);
    for end in 0..bytes.len() {
        assert!(
            StoredGiDyn::parse(&bytes[..end]).is_err(),
            "accepted truncated byte {end}"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(StoredGiDyn::parse(&trailing).is_err());
}

#[test]
fn bad_counts_grid_indices_and_rle_expansion_are_refused() {
    let original = fixture();
    for (offset, value) in [(13usize, 2u32), (57, 0), (62, 2)] {
        let mut bytes = original.clone();
        bytes
            .get_mut(offset..offset + 4)
            .expect("field")
            .copy_from_slice(&value.to_le_bytes());
        assert!(
            StoredGiDyn::parse(&bytes).is_err(),
            "accepted invalid count at {offset}"
        );
    }
    // V1 starts after the 118-byte prefix and its count. Run is escape, count, value.
    let mut bytes = original.clone();
    bytes
        .get_mut(124..126)
        .expect("RLE count")
        .copy_from_slice(&6u16.to_le_bytes());
    assert!(StoredGiDyn::parse(&bytes).is_err());
    let mut bytes = original;
    // Grid stream starts at 139 for this fixture: probe 1 is outside the one-probe vector.
    bytes
        .get_mut(139..141)
        .expect("grid")
        .copy_from_slice(&1u16.to_le_bytes());
    assert!(StoredGiDyn::parse(&bytes).is_err());
}
