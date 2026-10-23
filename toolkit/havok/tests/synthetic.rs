//! Hand-authored tagfile records exercise serialized references independently
//! of the production parser, including runtime-pointer versus stored-index size.
use primase_havok::Tagfile;
fn section(name: &[u8; 4], leaf: bool, bytes: Vec<u8>) -> Vec<u8> {
    let mut out = (((bytes.len() + 8) as u32) | if leaf { 0x40000000 } else { 0 })
        .to_be_bytes()
        .to_vec();
    out.extend(name);
    out.extend(bytes);
    out
}
fn packed(value: u32) -> Vec<u8> {
    if value < 128 {
        vec![value as u8]
    } else if value < 16384 {
        ((value as u16) | 0x8000).to_be_bytes().to_vec()
    } else {
        vec![0xc0 | ((value >> 16) as u8), (value >> 8) as u8, value as u8]
    }
}
fn fixture() -> Vec<u8> {
    fixture_with(b"20160200", packed(3))
}
/// The fixture with another SDK version and Root's packed member counts (fields in the low 16 bits, reflection
/// properties above).
fn fixture_with(sdk: &[u8; 8], root_counts: Vec<u8>) -> Vec<u8> {
    let strings = b"Root\0uint\0Pointer\0Array\0".to_vec();
    let names = vec![5, 0, 0, 1, 0, 2, 0, 3, 0];
    // Root{u32 value @0; pointer @8; array @16}, uint, ptr<uint>, array<ptr>.
    let mut body = vec![1, 0, 41, 7, 32, 8];
    body.extend(root_counts);
    body.extend([0, 32, 0, 2, 1, 32, 8, 3, 2, 32, 16, 4]);
    body.extend([2, 0, 9]);
    body.extend(packed(0x8004));
    body.extend([4, 4]);
    body.extend([3, 0, 11, 6, 2, 8, 8]);
    body.extend([4, 0, 11, 8, 3, 16, 8]);
    let mut types = section(b"TSTR", true, strings);
    types.extend(section(b"TNA1", true, names));
    types.extend(section(b"FSTR", true, b"value\0pointer\0array\0".to_vec()));
    types.extend(section(b"TBDY", true, body));
    let mut data = vec![0u8; 44];
    data[0..4].copy_from_slice(&17u32.to_le_bytes());
    data[8..12].copy_from_slice(&2u32.to_le_bytes());
    data[16..20].copy_from_slice(&3u32.to_le_bytes());
    data[32..36].copy_from_slice(&91u32.to_le_bytes());
    data[36..40].copy_from_slice(&2u32.to_le_bytes());
    data[40..44].copy_from_slice(&2u32.to_le_bytes());
    let mut items = vec![0u8; 12];
    for (kind, offset, count) in [
        (0x10000001u32, 0u32, 1u32),
        (0x10000002, 32, 1),
        (0x20000003, 36, 2),
    ] {
        items.extend(kind.to_le_bytes());
        items.extend(offset.to_le_bytes());
        items.extend(count.to_le_bytes());
    }
    let mut root = section(b"SDKV", true, sdk.to_vec());
    root.extend(section(b"DATA", true, data));
    root.extend(section(b"TYPE", false, types));
    root.extend(section(b"INDX", false, section(b"ITEM", true, items)));
    section(b"TAG0", false, root)
}
#[test]
fn pointer_arrays_use_serialized_stride() {
    let bytes = fixture();
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let root = tag.root().expect("root");
    assert_eq!(root.field("value").expect("value").unsigned().expect("uint"), 17);
    assert_eq!(
        root.field("pointer")
            .expect("pointer")
            .dereference()
            .expect("target")
            .unsigned()
            .expect("uint"),
        91
    );
    let array = root.field("array").expect("array").array().expect("array view");
    assert_eq!(array.len(), 2);
    for i in 0..2 {
        assert_eq!(
            array
                .get(i)
                .expect("entry")
                .dereference()
                .expect("target")
                .unsigned()
                .expect("uint"),
            91
        );
    }
    assert!(array.get(2).is_err());
    assert!(root.field("absent").is_err());
}
#[test]
fn all_truncations_are_errors() {
    let bytes = fixture();
    for n in 0..bytes.len() {
        assert!(Tagfile::read(&bytes[..n], None).is_err(), "prefix {n}");
    }
}
#[test]
fn oversized_item_and_bad_reference_fail() {
    let mut bytes = fixture();
    let last = bytes.len();
    bytes[last - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Tagfile::read(&bytes, None).is_err());
    let mut bytes = fixture();
    let data = bytes.windows(4).position(|b| b == b"DATA").expect("DATA") + 4;
    bytes[data + 8..data + 12].copy_from_slice(&99u32.to_le_bytes());
    let tag = Tagfile::read(&bytes, None).expect("container");
    assert!(
        tag.root()
            .expect("root")
            .field("pointer")
            .expect("field")
            .dereference()
            .is_err()
    );
}
#[test]
fn sdk_2018_member_counts_carry_property_counts() {
    let bytes = fixture_with(b"20180100", packed(0x0002_0003));
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    assert_eq!(&tag.version, b"20180100");
    let root = tag.root().expect("root");
    assert_eq!(root.field("value").expect("value").unsigned().expect("uint"), 17);
    assert_eq!(tag.types[1].fields.len(), 3);
    assert_eq!(tag.types[1].properties, 2);
    let old = fixture();
    let tag = Tagfile::read(&old, None).expect("fixture");
    assert_eq!(tag.types[1].properties, 0);
    assert!(Tagfile::read(&fixture_with(b"20170100", packed(3)), None).is_err());
}

/// A string index equal to the table's length is out of bounds, not the empty string the split after the last NUL would
/// make of it.
#[test]
fn a_string_index_at_the_end_of_the_table_is_an_error() {
    let mut bytes = fixture();
    assert!(Tagfile::read(&bytes, None).is_ok());
    // the type names section: 5 types (the null one counted), each a string index and no template parameters; the last
    // type's name is string 3 of the 4 the table holds: 4 names no string
    let names = [5u8, 0, 0, 1, 0, 2, 0, 3, 0];
    let at = bytes
        .windows(names.len())
        .position(|w| w == names)
        .expect("the names section");
    bytes[at + 7] = 4;
    let error = Tagfile::read(&bytes, None).unwrap_err().to_string();
    assert!(error.contains("string index out of bounds"), "{error}");
}
