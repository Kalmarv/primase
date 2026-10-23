//! Records that name one range, or one string, over and over cost the file 32 bytes a record and the reader a copy
//! each: every reader that copies what its records name refuses them before the copy (a reader spends at most four
//! times its input), while a file that shares a little reads.
use primase_fromsoft::{Binder, dcx::Decoder, emevd::Script, flver::Model, tpf::Package};

fn u32le(b: &mut [u8], p: usize, x: u32) {
    b[p..p + 4].copy_from_slice(&x.to_le_bytes());
}
fn u64le(b: &mut [u8], p: usize, x: u64) {
    b[p..p + 8].copy_from_slice(&x.to_le_bytes());
}

/// `n` instructions of one event, each naming the whole `args`-byte argument block.
fn emevd(n: usize, args: usize) -> Vec<u8> {
    let (events_at, ins_at) = (0x90, 0x90 + 0x30);
    let args_at = ins_at + n * 0x20;
    let mut b = vec![0u8; args_at + args];
    b[..8].copy_from_slice(b"EVD\0\x00\xff\x01\xff");
    u32le(&mut b, 8, 0xCD);
    let len = b.len() as u32;
    u32le(&mut b, 12, len);
    for (at, value) in [
        (0x10, 1),
        (0x18, events_at),
        (0x20, n),
        (0x28, ins_at),
        (0x70, args),
        (0x78, args_at),
        (0x88, args_at + args),
    ] {
        u64le(&mut b, at, value as u64);
    }
    u64le(&mut b, events_at + 8, n as u64);
    for i in 0..n {
        let p = ins_at + i * 0x20;
        u64le(&mut b, p + 8, args as u64);
        u64le(&mut b, p + 24, u64::MAX);
    }
    b
}

/// `n` files of a BND4 naming one `payload`-byte range and one `name`-character ASCII name.
fn binder(n: usize, payload: usize, name: usize) -> Vec<u8> {
    let name_at = 64 + n * 28;
    let header_end = name_at + name + 1;
    let mut b = vec![0u8; header_end + payload];
    b[..4].copy_from_slice(b"BND4");
    b[10] = 1;
    u32le(&mut b, 12, n as u32);
    u64le(&mut b, 16, 64);
    u64le(&mut b, 32, 28);
    u64le(&mut b, 40, header_end as u64);
    b[49] = 6u8.reverse_bits();
    for i in 0..n {
        let p = 64 + i * 28;
        b[p + 4..p + 8].fill(255);
        u64le(&mut b, p + 8, payload as u64);
        u32le(&mut b, p + 16, header_end as u32);
        u32le(&mut b, p + 20, i as u32);
        u32le(&mut b, p + 24, name_at as u32);
    }
    b[name_at..name_at + name].fill(b'a');
    b
}

/// `n` textures of a TPF naming one `dds`-byte DDS.
fn tpf(n: usize, dds: usize) -> Vec<u8> {
    let name = 16 + 20 * n;
    let payload = name + 2;
    let mut b = vec![0u8; payload + dds];
    b[..4].copy_from_slice(b"TPF\0");
    u32le(&mut b, 4, dds as u32);
    u32le(&mut b, 8, n as u32);
    for i in 0..n {
        let p = 16 + i * 20;
        u32le(&mut b, p, payload as u32);
        u32le(&mut b, p + 4, dds as u32);
        u32le(&mut b, p + 12, name as u32);
    }
    b[name] = b'a';
    b[payload..payload + 4].copy_from_slice(b"DDS ");
    b
}

fn flver_header(b: &mut [u8], data_start: usize, data_len: usize) {
    b[..8].copy_from_slice(b"FLVER\0L\0");
    u32le(b, 8, 0x20014);
    u32le(b, 12, data_start as u32);
    u32le(b, 16, data_len as u32);
    b[73] = 1;
}

/// `bones` bones of a FLVER naming one `name`-character name.
fn flver_bones(bones: usize, name: usize) -> Vec<u8> {
    let name_at = 128 + bones * 128;
    let data_start = name_at + 2 * (name + 1);
    let mut b = vec![0u8; data_start];
    flver_header(&mut b, data_start, 0);
    u32le(&mut b, 28, bones as u32);
    for i in 0..bones {
        u32le(&mut b, 128 + i * 128 + 12, name_at as u32);
    }
    for i in 0..name {
        b[name_at + 2 * i] = b'a';
    }
    b
}

/// `sets` face sets of a FLVER naming the same `indices` 16-bit indices (no mesh claims them).
fn flver_faces(sets: usize, indices: usize) -> Vec<u8> {
    let data_start = 128 + sets * 32;
    let mut b = vec![0u8; data_start + 2 * indices];
    flver_header(&mut b, data_start, 2 * indices);
    u32le(&mut b, 80, sets as u32);
    for i in 0..sets {
        let o = 128 + i * 32;
        u32le(&mut b, o + 8, indices as u32);
        u32le(&mut b, o + 16, (2 * indices) as u32);
        u32le(&mut b, o + 24, 16);
    }
    b
}

fn decoder() -> Decoder<'static> {
    Decoder {
        oodle: None,
        max_output: 1 << 30,
    }
}

fn refused_for_sharing(e: &primase_fromsoft::Error) -> bool {
    e.to_string().contains("several times over")
}

#[test]
fn instructions_naming_one_argument_block_are_refused_before_it_is_copied() {
    assert_eq!(
        Script::read(&emevd(2, 1 << 16)).unwrap().events[0]
            .instructions
            .len(),
        2
    );
    let e = Script::read(&emevd(200, 1 << 16)).unwrap_err();
    assert!(refused_for_sharing(&e), "{e}");
}

#[test]
fn binder_files_naming_one_payload_or_one_name_are_refused() {
    assert_eq!(
        Binder::read(&binder(2, 1 << 16, 1), &decoder())
            .unwrap()
            .files
            .len(),
        2
    );
    let e = Binder::read(&binder(200, 1 << 16, 1), &decoder()).unwrap_err();
    assert!(refused_for_sharing(&e), "{e}");
    assert_eq!(
        Binder::read(&binder(2, 0, 1 << 16), &decoder())
            .unwrap()
            .files
            .len(),
        2
    );
    let e = Binder::read(&binder(200, 0, 1 << 16), &decoder()).unwrap_err();
    assert!(refused_for_sharing(&e), "{e}");
}

#[test]
fn texture_package_entries_naming_one_dds_are_refused() {
    assert_eq!(Package::read(&tpf(2, 1 << 16)).unwrap().textures.len(), 2);
    let e = Package::read(&tpf(200, 1 << 16)).unwrap_err();
    assert!(refused_for_sharing(&e), "{e}");
}

#[test]
fn flver_names_and_face_sets_naming_one_range_are_refused() {
    assert_eq!(Model::read(&flver_bones(2, 1 << 14)).unwrap().bones.len(), 2);
    let e = Model::read(&flver_bones(200, 1 << 14)).unwrap_err();
    assert!(refused_for_sharing(&e), "{e}");
    // the face sets are refused for sharing, not (as they would be last) for being unclaimed
    let e = Model::read(&flver_faces(200, 1 << 14)).unwrap_err();
    assert!(refused_for_sharing(&e), "{e}");
}
