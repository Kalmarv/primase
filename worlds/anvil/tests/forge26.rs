//! Version-26 forges and compression-version-1 data blocks (Shaun White Skateboarding's PC layout): a forge and
//! blocks laid out here by hand (the LZO2A chunk a hand-worked stream), their refusals, and the hand-checked rows of a
//! real forge (`installs.shaunwhite/DataPC.forge`) with every entry's blocks decoded.
use std::fs::File;
use std::io::Cursor;

use primase_anvil::read::data::{BLOCK_MAGIC, Error as DataError, read_block, read_entry};
use primase_anvil::read::forge::{Error, FILEDATA_HEADER, Forge};
use primase_testdata::{Game, install_file};

const HEADER: usize = 1046;
const D2: usize = HEADER + 40;
const INDEX: usize = D2 + 48;

/// A version-26 forge of `rows` (name, index id, name id, payload) as the format lays it out: header size 1046,
/// DataHeader1 (40) there, DataHeader2 after it, the index table (16-byte records) and the name table (188), each with
/// one spare record, then each entry: its FILEDATA header and its payload.
fn forge(rows: &[(&str, u32, u64, &[u8])]) -> Vec<u8> {
    let n = rows.len();
    let names = INDEX + 16 * (n + 1);
    let end = names + 188 * (n + 1);
    let mut b = vec![0u8; end];
    let put = |b: &mut Vec<u8>, at: usize, bytes: &[u8]| b[at..at + bytes.len()].copy_from_slice(bytes);
    put(&mut b, 0, b"scimitar\0");
    put(&mut b, 0x09, &26i32.to_le_bytes());
    put(&mut b, 0x0D, &(HEADER as u64).to_le_bytes());
    put(&mut b, 0x15, &16i64.to_le_bytes());
    put(&mut b, 0x1D, &1i32.to_le_bytes());
    // DataHeader1: entries, three i32, an i64, MaxFilesForThisIndex, one FileSet, DataHeader2's offset
    put(&mut b, HEADER, &(n as i32).to_le_bytes());
    put(&mut b, HEADER + 4, &1i32.to_le_bytes());
    put(&mut b, HEADER + 16, &(-1i64).to_le_bytes());
    put(&mut b, HEADER + 24, &(n as i32 + 1).to_le_bytes());
    put(&mut b, HEADER + 28, &1i32.to_le_bytes());
    put(&mut b, HEADER + 32, &(D2 as i64).to_le_bytes());
    put(&mut b, D2, &(n as i32).to_le_bytes());
    put(&mut b, D2 + 4, &1i32.to_le_bytes());
    put(&mut b, D2 + 8, &(INDEX as i64).to_le_bytes());
    put(&mut b, D2 + 16, &(-1i64).to_le_bytes());
    put(&mut b, D2 + 28, &(n as i32).to_le_bytes());
    put(&mut b, D2 + 32, &(names as i64).to_le_bytes());
    put(&mut b, D2 + 40, &(end as i64).to_le_bytes());
    for (k, (name, id, name_id, payload)) in rows.iter().enumerate() {
        let offset = b.len();
        let size = payload.len() as i32;
        let i = INDEX + 16 * k;
        put(&mut b, i, &(offset as i64).to_le_bytes());
        put(&mut b, i + 8, &id.to_le_bytes());
        put(&mut b, i + 12, &size.to_le_bytes());
        let m = names + 188 * k;
        put(&mut b, m, &size.to_le_bytes());
        put(&mut b, m + 4, &name_id.to_le_bytes());
        put(&mut b, m + 40, &1_287_524_118i32.to_le_bytes());
        put(&mut b, m + 44, name.as_bytes());
        put(&mut b, m + 176, &7u32.to_le_bytes());
        let mut h = vec![0u8; FILEDATA_HEADER as usize];
        h[..8].copy_from_slice(b"FILEDATA");
        h[8..8 + name.len()].copy_from_slice(name.as_bytes());
        h[391..395].copy_from_slice(&id.to_le_bytes());
        h[395..399].copy_from_slice(&size.to_le_bytes());
        h[399..407].copy_from_slice(&name_id.to_le_bytes());
        b.extend_from_slice(&h);
        b.extend_from_slice(payload);
    }
    b
}

/// A compression-version-1 block of `chunks` (decoded length, stored bytes): count u16, (u16, u16) sizes, then each
/// chunk's Adler-32 from 0 and its bytes.
fn block(chunks: &[(u16, &[u8])]) -> Vec<u8> {
    let mut b = BLOCK_MAGIC.to_le_bytes().to_vec();
    b.extend_from_slice(&1u16.to_le_bytes());
    b.push(2);
    b.extend_from_slice(&32768u16.to_le_bytes());
    b.extend_from_slice(&32768u16.to_le_bytes());
    b.extend_from_slice(&(chunks.len() as u16).to_le_bytes());
    for (decoded, stored) in chunks {
        b.extend_from_slice(&decoded.to_le_bytes());
        b.extend_from_slice(&(stored.len() as u16).to_le_bytes());
    }
    for (_, stored) in chunks {
        b.extend_from_slice(&primase_hash::adler32(0, stored).to_le_bytes());
        b.extend_from_slice(stored);
    }
    b
}

/// The LZO2A stream of 266 bytes `x` (worked by hand in repos/primase-compress/tests/lzo2a.rs): stored in fewer bytes than
/// it decodes to, so the block decodes it rather than taking it as raw.
const XS: [u8; 8] = [0x1E, b'x', 0x01, 0x00, 0x00, 0x01, 0x20, 0x00];

#[test]
fn a_version_26_forge_reads_as_laid() {
    let entry = [block(&[(4, b"meta")]), block(&[(3, b"xyz"), (266, &XS)])].concat();
    let b = forge(&[
        ("GlobalMetaFile", 16, 0xA018_E7DF_3EC0_103E, b"abc"),
        ("Cell", 0x80C, 2, &entry),
    ]);
    let f = Forge::read(&mut Cursor::new(&b)).unwrap();
    assert_eq!(f.version(), 26);
    assert_eq!(f.data_header_1.unknown_04, vec![1, 0, 0]);
    let rows: Vec<_> = f.entries().collect();
    assert_eq!(
        (rows[1].0.id, rows[1].0.size, rows[1].1.name.as_str()),
        (0x80C, entry.len() as i32, "Cell")
    );
    assert_eq!(
        (rows[1].1.id, rows[1].1.timestamp, rows[1].1.meta_file_key),
        (2, 1_287_524_118, 7)
    );
    let mut r = Cursor::new(&b);
    assert_eq!(f.payload(&mut r, 0).unwrap(), b"abc");
    let e = read_entry(&f.payload(&mut r, 1).unwrap(), 1 << 20).unwrap();
    assert_eq!(e.metadata.data, b"meta");
    assert_eq!(e.resource.data, [&b"xyz"[..], &[b'x'; 266]].concat());
    assert_eq!(
        (e.resource.version, e.resource.algorithm, e.resource.chunks.len()),
        (1, 2, 2)
    );
}

/// A FILEDATA header that disagrees with its records is refused, naming it: another magic, another name, another
/// size; and a version-1 block with another algorithm, or a checksum that is not its bytes'.
#[test]
fn what_disagrees_is_refused() {
    let entry = block(&[(3, b"xyz")]);
    let b = forge(&[("One", 16, 1, &entry)]);
    let start = b.len() - entry.len() - FILEDATA_HEADER as usize;
    let payload = |b: &[u8]| {
        let f = Forge::read(&mut Cursor::new(b)).unwrap();
        f.payload(&mut Cursor::new(b), 0).unwrap_err().to_string()
    };
    let mut bad = b.clone();
    bad[start] = b'X';
    assert!(payload(&bad).contains("not FILEDATA"), "{}", payload(&bad));
    let mut bad = b.clone();
    bad[start + 8] = b'T';
    assert!(
        payload(&bad).contains("not the name record's"),
        "{}",
        payload(&bad)
    );
    let mut bad = b.clone();
    bad[start + 395] ^= 1;
    assert!(payload(&bad).contains("the records hold"), "{}", payload(&bad));
    assert!(matches!(
        Forge::read(&mut Cursor::new(&b[..b.len() - 1])),
        Err(Error::Entry { .. })
    ));
    let mut other = block(&[(3, b"xyz")]);
    other[10] = 5;
    assert!(matches!(
        read_block(&other, 0, 1 << 20),
        Err(DataError::Algorithm {
            version: 1,
            algorithm: 5,
            ..
        })
    ));
    let mut sum = block(&[(3, b"xyz")]);
    sum[21] ^= 1;
    assert!(matches!(
        read_block(&sum, 0, 1 << 20),
        Err(DataError::Checksum { .. })
    ));
}

/// The real forge's first two rows as a hex dump of it shows them, and every entry's blocks decoded: 37 entries of
/// two blocks each (every chunk's checksum checked and its LZO2A stream decoded to its stated size), and the
/// GlobalMetaFile, which holds no blocks.
#[test]
fn a_real_version_26_forge_reads_and_decodes() {
    let Some(path) = install_file(Game::ShaunWhite, "DataPC.forge") else {
        return;
    };
    let f = Forge::open(&path).unwrap();
    assert_eq!((f.version(), f.index.len(), f.file_size), (26, 38, 20_545_833));
    let (i, n) = f.entries().next().unwrap();
    assert_eq!(
        (i.offset, i.id, i.size, n.id, n.timestamp),
        (0x1C_1800, 0x10, 0xFF, 0xA018_E7DF_3EC0_103E, 0x4CBE_0F53)
    );
    assert_eq!(n.name, "GlobalMetaFile");
    let (i, n) = f.entries().nth(1).unwrap();
    assert_eq!(
        (i.offset, i.id, i.size, n.id),
        (0x1C_2000, 0x80C, 0x4_4716, 0xBB29_F1E7_EFAA_2703)
    );
    assert_eq!(n.name, "Graphic Bootstrap");
    let mut r = File::open(&path).unwrap();
    let mut decoded = 0;
    for (k, n) in f.names.iter().enumerate() {
        let payload = f.payload(&mut r, k).unwrap();
        if n.name == "GlobalMetaFile" {
            assert!(read_entry(&payload, 1 << 26).is_err());
            continue;
        }
        let e = read_entry(&payload, 1 << 26).unwrap_or_else(|e| panic!("{}: {e}", n.name));
        assert!(e.resource.chunks.iter().all(|c| c.decoded_size <= 32768));
        decoded += 1;
    }
    assert_eq!(decoded, 37);
    // Graphic Bootstrap: a 26-byte metadata block stored raw, then 16 chunks, 512,822 bytes decoded (hand-checked)
    let e = read_entry(&f.payload(&mut r, 1).unwrap(), 1 << 26).unwrap();
    assert_eq!(
        (e.metadata.data.len(), e.metadata.chunks[0].checksum),
        (26, 0x50B5_0713)
    );
    assert_eq!((e.resource.chunks.len(), e.resource.data.len()), (16, 512_822));
}
