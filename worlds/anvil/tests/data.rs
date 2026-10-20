//! A forge entry's data blocks: entries laid out here by hand (zstd frames built by hand too), their refusals, and two
//! real Mesh resources whose sizes, checksums, names and class ids were hand-checked in a separate read-only decode.
use std::fs::File;

use primase_anvil::read::data::{BLOCK_MAGIC, Error, Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_testdata::real_data;

const BOUND: usize = 1 << 20;

/// A one-frame zstd stream of `bytes` (at most 255) as one raw block, laid out by hand (RFC 8878).
fn zstd_raw(bytes: &[u8]) -> Vec<u8> {
    let n = u32::try_from(bytes.len()).expect("a test length");
    assert!(n <= 255);
    let mut f = vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, n as u8];
    f.extend_from_slice(&(1 | (n << 3)).to_le_bytes()[..3]);
    f.extend_from_slice(bytes);
    f
}

/// One data block: version, algorithm, count, the stored bytes for `decoded` bytes, the checksum as given.
fn block(version: u16, algorithm: u8, count: u32, decoded: u32, stored: &[u8], checksum: u32) -> Vec<u8> {
    let mut b = BLOCK_MAGIC.to_le_bytes().to_vec();
    b.extend_from_slice(&version.to_le_bytes());
    b.push(algorithm);
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&32772u16.to_le_bytes());
    b.extend_from_slice(&count.to_le_bytes());
    b.extend_from_slice(&decoded.to_le_bytes());
    b.extend_from_slice(&u32::try_from(stored.len()).expect("a test length").to_le_bytes());
    b.extend_from_slice(&checksum.to_le_bytes());
    b.extend_from_slice(stored);
    b
}

fn good(decoded: &[u8], stored: &[u8]) -> Vec<u8> {
    block(
        2,
        5,
        1,
        u32::try_from(decoded.len()).expect("a test length"),
        stored,
        primase_hash::adler32(0, stored),
    )
}

/// A resource: type, payload length, name, the file header byte, class id, extension, body.
fn resource(type_id: u32, name: &str, class_id: u64, body: &[u8]) -> Vec<u8> {
    let payload = 12 + body.len();
    let mut r = type_id.to_le_bytes().to_vec();
    r.extend_from_slice(&u32::try_from(payload).expect("a test length").to_le_bytes());
    r.extend_from_slice(&u32::try_from(name.len()).expect("a test length").to_le_bytes());
    r.extend_from_slice(name.as_bytes());
    r.push(0);
    r.extend_from_slice(&class_id.to_le_bytes());
    r.extend_from_slice(&type_id.to_le_bytes());
    r.extend_from_slice(body);
    r
}

/// A metadata block stored raw and a resource block stored as a zstd frame read back: sizes, checksum and the
/// resource's header and body.
#[test]
fn an_entry_reads_its_two_blocks_and_its_resource() {
    let metadata = [7u8; 16];
    let res = resource(0x415D_9568, "Box_LOD0", 0x01D7_F412_6B45, b"mesh body");
    let mut entry = good(&metadata, &metadata);
    entry.extend(good(&res, &zstd_raw(&res)));
    let e = read_entry(&entry, BOUND).unwrap();
    assert_eq!(
        (e.metadata.data.as_slice(), e.metadata.decoded_size),
        (&metadata[..], 16)
    );
    assert_eq!(e.resource.data, res);
    let r = Resource::parse(&e.resource.data).unwrap();
    assert_eq!(
        (
            r.type_id,
            r.name.as_str(),
            r.file_header,
            r.class_id,
            r.extension,
            r.body.as_slice()
        ),
        (
            0x415D_9568,
            "Box_LOD0",
            0,
            0x01D7_F412_6B45,
            0x415D_9568,
            &b"mesh body"[..]
        )
    );
}

/// What isn't established is refused, naming it: another version, algorithm or block count, a wrong checksum, a
/// decoded size over the bound or unlike the frame's, bytes after the two blocks, a block with no magic.
#[test]
fn what_is_not_established_is_refused() {
    let meta = [1u8; 16];
    let res = resource(1, "R", 2, b"x");
    let frame = zstd_raw(&res);
    let entry = |second: Vec<u8>| {
        let mut e = good(&meta, &meta);
        e.extend(second);
        e
    };
    let sum = primase_hash::adler32(0, &frame);
    let n = u32::try_from(res.len()).unwrap();
    assert!(matches!(
        read_entry(&entry(block(3, 5, 1, n, &frame, sum)), BOUND),
        Err(Error::Version { .. })
    ));
    assert!(matches!(
        read_entry(&entry(block(2, 4, 1, n, &frame, sum)), BOUND),
        Err(Error::Algorithm { .. })
    ));
    assert!(matches!(
        read_entry(&entry(block(2, 5, 2, n, &frame, sum)), BOUND),
        Err(Error::Blocks { .. })
    ));
    assert!(matches!(
        read_entry(&entry(block(2, 5, 1, n, &frame, sum ^ 1)), BOUND),
        Err(Error::Checksum { .. })
    ));
    // the checksum is from 0, not the standard 1
    let standard = primase_hash::adler32(1, &frame);
    assert!(matches!(
        read_entry(&entry(block(2, 5, 1, n, &frame, standard)), BOUND),
        Err(Error::Checksum { .. })
    ));
    assert!(matches!(
        read_entry(&entry(good(&res, &frame)), 8),
        Err(Error::Size { .. })
    ));
    assert!(matches!(
        read_entry(&entry(block(2, 5, 1, n + 1, &frame, sum)), BOUND),
        Err(Error::Decode { .. })
    ));
    let mut extra = entry(good(&res, &frame));
    extra.push(0);
    assert!(matches!(read_entry(&extra, BOUND), Err(Error::Trailing(1))));
    // GlobalMetaFile-like: no block magic
    assert!(matches!(
        read_entry(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0], BOUND),
        Err(Error::Magic { .. })
    ));
    // a resource whose payload length doesn't end the block
    let mut short = res.clone();
    short.push(9);
    assert!(Resource::parse(&short).is_err());
}

/// Two real Mesh resources, one per forge (Riders Republic build 25394987, hand-checked in a separate read-only
/// decode): base entry 161 and patch entry 2.
#[test]
fn two_real_mesh_resources_decode() {
    let cases = [
        (
            "PRIMASE_ANVIL_FORGE_BASE",
            161usize,
            (2_512_403i64, 1_280i32),
            (2_058u32, 1_202u32, 0x9633_63DDu32),
            "MAD_PRP_WHITEROOM_Black_Box_A_LOD3",
            0x1D7_F412_6B45u64,
            2_011usize,
        ),
        (
            "PRIMASE_ANVIL_FORGE",
            2,
            (139_773, 14_047),
            (33_462, 13_969, 0x6026_4258),
            "MAD_PRP_DARKROOM_Stand_LOD0",
            0x1D3_970F_3794,
            33_422,
        ),
    ];
    for (var, k, (offset, size), (decoded, stored, checksum), name, class_id, payload) in cases {
        let Some(path) = real_data(var) else {
            continue;
        };
        let f = Forge::open(&path).unwrap();
        assert_eq!(
            (f.index[k].offset, f.index[k].size),
            (offset, size),
            "{var} entry {k}"
        );
        let bytes = f.payload(&mut File::open(&path).unwrap(), k).unwrap();
        let e = read_entry(&bytes, BOUND).unwrap();
        assert_eq!(e.metadata.decoded_size, 16, "{var}");
        assert_eq!(
            (
                e.resource.decoded_size,
                e.resource.stored_size,
                e.resource.checksum
            ),
            (decoded, stored, checksum),
            "{var}"
        );
        let r = Resource::parse(&e.resource.data).unwrap();
        assert_eq!(
            (
                r.type_id,
                r.name.as_str(),
                r.file_header,
                r.class_id,
                r.extension,
                r.body.len()
            ),
            (0x415D_9568, name, 0, class_id, 0x415D_9568, payload - 12),
            "{var}"
        );
    }
}
