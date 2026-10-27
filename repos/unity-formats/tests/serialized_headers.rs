//! Hand-worked container headers: empty metadata and one object at a padded data boundary.
#![allow(clippy::unwrap_used)]

use unity_formats::SerializedFile;

fn header(version: u32, metadata: &[u8], data: &[u8]) -> Vec<u8> {
    let header_size = if version == 21 { 20 } else { 48 };
    let offset = header_size + metadata.len() + 7;
    let size = offset + data.len();
    let mut out = vec![0; header_size];
    out[8..12].copy_from_slice(&version.to_be_bytes());
    if version == 21 {
        out[0..4].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
        out[4..8].copy_from_slice(&(size as u32).to_be_bytes());
        out[12..16].copy_from_slice(&(offset as u32).to_be_bytes());
    } else {
        out[20..24].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
        out[24..32].copy_from_slice(&(size as u64).to_be_bytes());
        out[32..40].copy_from_slice(&(offset as u64).to_be_bytes());
    }
    out.extend_from_slice(metadata);
    out.resize(offset, 0);
    out.extend_from_slice(data);
    out
}

fn fixture(version: u32) -> Vec<u8> {
    // Version string, platform 19, no embedded tree; one class 1 type with no script ID and a zero hash.
    let mut metadata = b"2019.4.20f1\0".to_vec();
    metadata.extend_from_slice(&19_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    metadata.push(0);
    metadata.extend_from_slice(&(-1_i16).to_le_bytes());
    metadata.extend_from_slice(&[0; 16]);
    metadata.extend_from_slice(&1_i32.to_le_bytes());
    // Both header sizes are divisible by four; align the object table's path ID.
    while !metadata.len().is_multiple_of(4) {
        metadata.push(0);
    }
    metadata.extend_from_slice(&42_i64.to_le_bytes());
    if version == 21 {
        metadata.extend_from_slice(&0_u32.to_le_bytes());
    } else {
        metadata.extend_from_slice(&0_i64.to_le_bytes());
    }
    metadata.extend_from_slice(&3_u32.to_le_bytes());
    metadata.extend_from_slice(&0_u32.to_le_bytes());
    // No script references, externals or reference types; empty user string.
    metadata.extend_from_slice(&[0; 12]);
    metadata.push(0);
    header(version, &metadata, &[0xaa, 0xbb, 0xcc])
}

#[test]
fn versions_place_metadata_after_their_headers_and_read_object_widths() {
    for version in [21, 22] {
        let bytes = fixture(version);
        let size = bytes.len();
        assert!(SerializedFile::looks_like(&bytes));
        let file = SerializedFile::parse(bytes).unwrap();
        assert_eq!(file.version, version);
        assert_eq!(file.objects.len(), 1);
        let object = file.object(42).unwrap();
        assert_eq!(object.class_id, 1);
        assert_eq!(object.offset, (size - 3) as u64);
        assert_eq!(file.bytes(object), &[0xaa, 0xbb, 0xcc]);
    }
}

#[test]
fn both_headers_refuse_metadata_overlap_and_data_past_eof() {
    for version in [21, 22] {
        for offset in [0, u64::MAX] {
            let mut bytes = fixture(version);
            if version == 21 {
                bytes[12..16].copy_from_slice(&(offset as u32).to_be_bytes());
            } else {
                bytes[32..40].copy_from_slice(&offset.to_be_bytes());
            }
            let error = SerializedFile::parse(bytes).unwrap_err().to_string();
            assert!(error.contains("metadata ends"), "{error}");
        }
        let mut bytes = fixture(version);
        let at = if version == 21 { 0 } else { 20 };
        bytes[at..at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(SerializedFile::parse(bytes).is_err());
    }
}

#[test]
fn truncated_headers_and_unsupported_versions_are_refused() {
    for version in [21, 22] {
        let bytes = fixture(version);
        let header_size = if version == 21 { 20 } else { 48 };
        for size in 0..header_size {
            assert!(!SerializedFile::looks_like(&bytes[..size]));
            assert!(SerializedFile::parse(bytes[..size].to_vec()).is_err());
        }
    }
    for version in [20_u32, 23] {
        let mut bytes = fixture(21);
        bytes[8..12].copy_from_slice(&version.to_be_bytes());
        assert!(!SerializedFile::looks_like(&bytes));
        assert!(SerializedFile::parse(bytes).is_err());
    }
}
