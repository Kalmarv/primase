//! A forge entry's data blocks: entries laid out here by hand (zstd frames built by hand too), their refusals, and two
//! real Mesh resources whose sizes, checksums, names and class ids were hand-checked in a separate read-only decode.
use std::fs::File;

use primase_anvil::read::data::{BLOCK_MAGIC, Error, Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_testdata::{Game, install_file};

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

/// A block of several chunks, each (decoded length, stored bytes), with their checksums from 0: the sizes first,
/// then each chunk's checksum and bytes.
fn chunked(chunks: &[(usize, &[u8])]) -> Vec<u8> {
    let len = |n: usize| u32::try_from(n).expect("a test length").to_le_bytes();
    let mut b = BLOCK_MAGIC.to_le_bytes().to_vec();
    b.extend_from_slice(&2u16.to_le_bytes());
    b.push(5);
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&32772u16.to_le_bytes());
    b.extend_from_slice(&len(chunks.len()));
    for (decoded, stored) in chunks {
        b.extend_from_slice(&len(*decoded));
        b.extend_from_slice(&len(stored.len()));
    }
    for (_, stored) in chunks {
        b.extend_from_slice(&primase_hash::adler32(0, stored).to_le_bytes());
        b.extend_from_slice(stored);
    }
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
        (e.metadata.data.as_slice(), e.metadata.decoded_size()),
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
        read_entry(&entry(block(2, 5, 0, n, &frame, sum)), BOUND),
        Err(Error::Chunks { .. })
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

/// A block of three chunks (raw, zstd, raw) decodes to their bytes in order; a chunk's wrong checksum names the chunk;
/// a chunk count the bytes can't hold is refused before reading.
#[test]
fn a_block_of_chunks_decodes_in_order() {
    let res = resource(0x1D4B_87A3, "Mip", 3, b"first second third");
    let (a, rest) = res.split_at(10);
    let (b, c) = rest.split_at(12);
    let mut entry = good(&[0u8; 16], &[0u8; 16]);
    entry.extend(chunked(&[(a.len(), a), (b.len(), &zstd_raw(b)), (c.len(), c)]));
    let e = read_entry(&entry, BOUND).unwrap();
    assert_eq!(e.resource.data, res);
    assert_eq!(e.resource.chunks.len(), 3);
    assert_eq!(e.resource.chunks[1].decoded_size, 12);
    assert_eq!(Resource::parse(&e.resource.data).unwrap().name, "Mip");
    // the second chunk's checksum broken
    let mut bad = entry.clone();
    let second = 47 + 19 + 3 * 8 + 4 + a.len();
    bad[second] ^= 1;
    assert!(matches!(
        read_entry(&bad, BOUND),
        Err(Error::Checksum { chunk: 1, .. })
    ));
    // a count far past the bytes
    let mut many = good(&[0u8; 16], &[0u8; 16]);
    many.extend(block(2, 5, 1 << 30, 1, b"x", 0));
    assert!(matches!(read_entry(&many, BOUND), Err(Error::Chunks { .. })));
}

/// Two real Mesh resources, one per forge (Riders Republic build 25394987, hand-checked in a separate read-only
/// decode): base entry 161 and patch entry 2.
#[test]
fn two_real_mesh_resources_decode() {
    let cases = [
        (
            install_file(Game::Riders, "DataPC_MAD_WhiteRoom.forge"),
            161usize,
            (2_512_403i64, 1_280i32),
            (2_058u32, 1_202u32, 0x9633_63DDu32),
            "MAD_PRP_WHITEROOM_Black_Box_A_LOD3",
            0x1D7_F412_6B45u64,
            2_011usize,
        ),
        (
            install_file(Game::Riders, "DataPC_MAD_WhiteRoom_patch_01.forge"),
            2,
            (139_773, 14_047),
            (33_462, 13_969, 0x6026_4258),
            "MAD_PRP_DARKROOM_Stand_LOD0",
            0x1D3_970F_3794,
            33_422,
        ),
    ];
    for (forge, k, (offset, size), (decoded, stored, checksum), name, class_id, payload) in cases {
        let Some(path) = forge else {
            continue;
        };
        let var = path.file_name().unwrap().to_string_lossy();
        let f = Forge::open(&path).unwrap();
        assert_eq!(
            (f.index[k].offset, f.index[k].size),
            (offset, size),
            "{var} entry {k}"
        );
        let bytes = f.payload(&mut File::open(&path).unwrap(), k).unwrap();
        let e = read_entry(&bytes, BOUND).unwrap();
        assert_eq!(e.metadata.decoded_size(), 16, "{var}");
        assert_eq!(e.resource.chunks.len(), 1, "{var}");
        let c = &e.resource.chunks[0];
        assert_eq!(
            (c.decoded_size, c.stored_size, c.checksum),
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

/// A real CompiledMip of three chunks (base forge entry 373, Riders Republic build 25394987; layout and values
/// hand-checked in a separate read-only decode, games/riders-republic docs/recon/texture-layout.md, dcb4f2b): each
/// chunk's sizes and checksum, the total, and the resource's type and name (its name record's).
#[test]
fn a_real_three_chunk_mip_decodes() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_WhiteRoom.forge") else {
        return;
    };
    let f = Forge::open(&path).unwrap();
    let k = 373;
    assert_eq!((f.index[k].offset, f.index[k].size), (3_846_990, 117_193));
    let bytes = f.payload(&mut File::open(&path).unwrap(), k).unwrap();
    let e = read_entry(&bytes, 1 << 21).unwrap();
    let chunks: Vec<(u32, u32, u32)> = e
        .resource
        .chunks
        .iter()
        .map(|c| (c.decoded_size, c.stored_size, c.checksum))
        .collect();
    assert_eq!(
        chunks,
        [
            (262_144, 56_773, 0xB97E_701B),
            (262_144, 60_230, 0x31C3_950E),
            (107, 88, 0x12A0_2467)
        ]
    );
    assert_eq!(e.resource.decoded_size(), 524_395);
    let r = Resource::parse(&e.resource.data).unwrap();
    assert_eq!(r.type_id, 0x1D4B_87A3);
    assert_eq!(r.type_id, f.names[k].type_id);
    assert_eq!(r.name, f.names[k].name);
}

/// A resource's ClassID is its entry's index id, and a reference to another resource resolves by it: one Material ->
/// TextureSet -> textures chain of the base forge (ids and ordinals hand-checked in a separate read-only decode,
/// games/riders-republic docs/recon/material-binding.md, ceebc6e). Only the id resolution is claimed, not the
/// Material's or TextureSet's field layout.
#[test]
fn class_ids_are_index_ids_and_resolve() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_WhiteRoom.forge") else {
        return;
    };
    let f = Forge::open(&path).unwrap();
    let mut file = File::open(&path).unwrap();
    for (k, type_id) in [(162usize, 0x85C8_17C3u32), (163, 0xD70E_6670)] {
        let e = read_entry(&f.payload(&mut file, k).unwrap(), BOUND).unwrap();
        let r = Resource::parse(&e.resource.data).unwrap();
        assert_eq!((r.type_id, r.class_id), (type_id, f.index[k].id), "entry {k}");
        assert_eq!(f.find(r.class_id), Some(k));
    }
    assert_eq!(f.find(0x01D3_970D_C220), Some(163));
    assert_eq!(f.find(0x01D3_970D_C221), Some(217));
    assert_eq!(f.find(0x01D3_970D_C22A), Some(218));
    // the Material's other 64-bit value at body+3 resolves to no entry of this forge
    assert_eq!(f.find(0x01C3_C9ED_9E7E), None);
}

/// A resource block of the given file header bytes (after the name) and payload (class id, extension and body).
fn resource_block(type_id: u32, name: &str, file_header: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut b = type_id.to_le_bytes().to_vec();
    b.extend(u32::try_from(payload.len()).expect("a test length").to_le_bytes());
    b.extend(u32::try_from(name.len()).expect("a test length").to_le_bytes());
    b.extend(name.as_bytes());
    b.extend(file_header);
    b.extend(payload);
    b
}

#[test]
fn a_file_header_of_1_is_followed_by_a_table_of_12_byte_rows() {
    let mut payload = 0x01D7_F412_6B45u64.to_le_bytes().to_vec();
    payload.extend(0x0FA3_067Fu32.to_le_bytes());
    payload.extend([9, 8, 7]);
    // the table: 02 00 00, two rows
    let mut header = vec![1, 2, 0, 0];
    header.extend(2u32.to_le_bytes());
    header.extend([[0xAAu8; 12], [0xBB; 12]].concat());
    let r = Resource::parse(&resource_block(0x0FA3_067F, "clip", &header, &payload)).unwrap();
    assert_eq!(
        (
            r.file_header,
            r.header_rows.clone(),
            r.class_id,
            r.extension,
            r.body.clone()
        ),
        (
            1,
            vec![[0xAA; 12], [0xBB; 12]],
            0x01D7_F412_6B45,
            0x0FA3_067F,
            vec![9, 8, 7]
        )
    );
    // a file header of 0 has no table
    let r = Resource::parse(&resource_block(0x0FA3_067F, "clip", &[0], &payload)).unwrap();
    assert_eq!((r.file_header, r.header_rows.len()), (0, 0));
    // the table's tag bytes, a row count the block can't hold, and a payload length off the block are refused
    let mut wrong = header.clone();
    wrong[1] = 3;
    assert!(Resource::parse(&resource_block(0x0FA3_067F, "clip", &wrong, &payload)).is_err());
    let mut many = header.clone();
    many[4..8].copy_from_slice(&1000u32.to_le_bytes());
    assert!(Resource::parse(&resource_block(0x0FA3_067F, "clip", &many, &payload)).is_err());
    let mut long = resource_block(0x0FA3_067F, "clip", &header, &payload);
    long.push(0);
    assert!(Resource::parse(&long).is_err());
}
