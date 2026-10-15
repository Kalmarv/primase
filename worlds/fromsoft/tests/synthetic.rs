use primase_compress::{Compressor, Zlib};
use primase_fromsoft::{
    Binder, NameDictionary,
    bhd::{AesKey, Entry, Header, Range, unwrap_rsa},
    dcx::Decoder,
};
fn u32le(b: &mut [u8], p: usize, x: u32) {
    b[p..p + 4].copy_from_slice(&x.to_le_bytes());
}
fn u64le(b: &mut [u8], p: usize, x: u64) {
    b[p..p + 8].copy_from_slice(&x.to_le_bytes());
}
fn u32be(b: &mut [u8], p: usize, x: u32) {
    b[p..p + 4].copy_from_slice(&x.to_be_bytes());
}
fn decoder() -> Decoder<'static> {
    Decoder {
        oodle: None,
        max_output: 1024,
    }
}
fn bhd() -> Vec<u8> {
    let mut b = vec![0; 76];
    b[..4].copy_from_slice(b"BHD5");
    b[4] = 255;
    u32le(&mut b, 8, 1);
    u32le(&mut b, 12, 76);
    u32le(&mut b, 16, 1);
    u32le(&mut b, 20, 28);
    u32le(&mut b, 28, 1);
    u32le(&mut b, 32, 36);
    u32le(&mut b, 36, 123);
    u32le(&mut b, 40, 32);
    u64le(&mut b, 44, 128);
    u64le(&mut b, 68, 17);
    b
}
fn dcx() -> Vec<u8> {
    let payload = Zlib.compress(b"synthetic payload").expect("synthetic fixture");
    let mut b = vec![0; 76];
    b[..4].copy_from_slice(b"DCX\0");
    for (p, x) in [
        (4, 0x10000),
        (8, 0x18),
        (12, 0x24),
        (16, 0x24),
        (20, 0x2c),
        (28, 17),
        (32, payload.len() as u32),
        (44, 0x20),
        (64, 0x10100),
        (72, 8),
    ] {
        u32be(&mut b, p, x);
    }
    b[24..28].copy_from_slice(b"DCS\0");
    b[36..40].copy_from_slice(b"DCP\0");
    b[40..44].copy_from_slice(b"DFLT");
    b[48] = 9;
    b[68..72].copy_from_slice(b"DCA\0");
    b.extend(payload);
    b
}
fn binder() -> Vec<u8> {
    let mut b = vec![0; 116];
    b[..4].copy_from_slice(b"BND4");
    b[10] = 1;
    u32le(&mut b, 12, 1);
    u64le(&mut b, 16, 64);
    u64le(&mut b, 32, 28);
    u64le(&mut b, 40, 96);
    b[48] = 1;
    b[49] = 6u8.reverse_bits();
    b[64] = 2u8.reverse_bits();
    b[68..72].fill(255);
    u64le(&mut b, 72, 20);
    u32le(&mut b, 80, 96);
    u32le(&mut b, 84, 7);
    u32le(&mut b, 88, 92);
    b[92] = b'a';
    b[96..].copy_from_slice(b"twenty payload bytes");
    b
}
#[test]
fn bhd_fields_padding_and_bounds() {
    let mut b = bhd();
    let h = Header::read(&b).expect("synthetic fixture");
    assert_eq!(h.entries[0].hash, 123);
    assert_eq!(h.entries[0].offset, 128);
    assert_eq!(h.entries[0].logical_size, 17);
    b.extend([0; 13]);
    assert_eq!(Header::read(&b).expect("synthetic fixture").rsa_padding, 13);
    *b.last_mut().expect("synthetic fixture") = 1;
    assert!(Header::read(&b).is_err());
    let mut b = bhd();
    u64le(&mut b, 68, 33);
    assert!(Header::read(&b).is_err());
    let mut b = bhd();
    u64le(&mut b, 60, u64::MAX);
    assert!(Header::read(&b).is_err());
}
#[test]
fn rsa_block_geometry() {
    let mut modulus = [0; 256];
    modulus[254] = 0x0c;
    modulus[255] = 0xa1;
    let mut cipher = [0; 256];
    cipher[254] = 0x02;
    cipher[255] = 0x4c;
    let result = unwrap_rsa(&cipher, &modulus, &[17]).expect("synthetic fixture");
    assert_eq!(result.len(), 255);
    assert_eq!(result[254], 65);
    assert!(result[..254].iter().all(|b| *b == 0));
    assert!(unwrap_rsa(&cipher[..255], &modulus, &[17]).is_err());
    assert!(unwrap_rsa(&modulus, &modulus, &[17]).is_err());
    assert!(unwrap_rsa(&cipher, &modulus, &[0]).is_err());
}
/// NIST FIPS-197 AES-128 example; bytes outside the selected range stay unchanged.
#[test]
fn aes_known_vector_and_ranges() {
    let key = std::array::from_fn(|i| i as u8);
    let ciphertext = [
        0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a,
    ];
    let mut bytes = vec![0xaa; 48];
    bytes[16..32].copy_from_slice(&ciphertext);
    let mut e = Entry {
        hash: 0,
        offset: 0,
        padded_size: 48,
        logical_size: 48,
        sha: None,
        aes: Some(AesKey {
            key,
            ranges: vec![Range { start: -1, end: -1 }, Range { start: 16, end: 32 }],
        }),
    };
    e.decrypt(&mut bytes).expect("synthetic fixture");
    assert_eq!(&bytes[16..32], &(0..16).map(|i| i * 17).collect::<Vec<u8>>());
    assert!(bytes[..16].iter().chain(&bytes[32..]).all(|b| *b == 0xaa));
    e.aes
        .as_mut()
        .expect("synthetic fixture")
        .ranges
        .push(Range { start: 32, end: 64 });
    assert!(e.decrypt(&mut bytes).is_err());
}
#[test]
fn dcx_roundtrip_and_rejections() {
    let b = dcx();
    assert_eq!(
        decoder().decode(&b).expect("synthetic fixture"),
        b"synthetic payload"
    );
    for p in [
        0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72,
    ] {
        let mut bad = b.clone();
        bad[p] ^= 1;
        assert!(decoder().decode(&bad).is_err(), "offset {p}");
    }
    let mut padded = b.clone();
    padded.extend([0; 15]);
    assert!(decoder().decode(&padded).is_ok());
    padded.push(0);
    assert!(decoder().decode(&padded).is_err());
    assert!(
        Decoder {
            oodle: None,
            max_output: 1
        }
        .decode(&b)
        .is_err()
    );
}
#[test]
fn binder_files_and_strict_coverage() {
    let b = binder();
    let out = Binder::read(&b, &decoder()).expect("synthetic fixture");
    assert_eq!(out.files[0].name.as_deref(), Some("a"));
    assert_eq!(out.files[0].id, Some(7));
    assert_eq!(out.files[0].bytes, b"twenty payload bytes");
    for p in [0, 6, 11, 16, 32, 49, 51, 56, 65, 68, 80, 88] {
        let mut bad = b.clone();
        bad[p] ^= 128;
        assert!(Binder::read(&bad, &decoder()).is_err(), "offset {p}");
    }
}
#[test]
fn every_truncation_is_an_error() {
    for n in 0..76 {
        assert!(Header::read(&bhd()[..n]).is_err());
    }
    let d = dcx();
    for n in 0..d.len() {
        assert!(decoder().decode(&d[..n]).is_err());
    }
    let b = binder();
    for n in 0..b.len() {
        assert!(Binder::read(&b[..n], &decoder()).is_err());
    }
}
#[test]
fn dictionary_normalizes_and_deduplicates() {
    let d = NameDictionary::parse("# comment\nA\\B\n/a/b\n").expect("synthetic fixture");
    assert_eq!(d.names.len(), 1);
    assert_eq!(d.names[&primase_hash::fromsoft_path("/a/b")], vec!["/a/b"]);
    assert!(NameDictionary::parse("é").is_err());
}
#[test]
fn bhd_encryption_ranges_are_structurally_checked() {
    let mut b = bhd();
    b.resize(112, 0);
    u32le(&mut b, 12, 112);
    u64le(&mut b, 60, 76);
    u32le(&mut b, 92, 1);
    u64le(&mut b, 96, 0);
    u64le(&mut b, 104, 16);
    assert_eq!(
        Header::read(&b).expect("valid AES table").entries[0]
            .aes
            .as_ref()
            .expect("AES")
            .ranges
            .len(),
        1
    );
    for (start, end) in [(1, 16), (16, 0), (0, 48), (u64::MAX, 0)] {
        let mut bad = b.clone();
        u64le(&mut bad, 96, start);
        u64le(&mut bad, 104, end);
        assert!(Header::read(&bad).is_err());
    }
    u64le(&mut b, 96, u64::MAX);
    u64le(&mut b, 104, u64::MAX);
    assert!(Header::read(&b).is_ok());
}
#[test]
fn binder_hash_table_and_nested_dcx() {
    let mut b = binder();
    let payload = b.split_off(96);
    b.resize(128, 0);
    b.extend(payload);
    b[50] = 4;
    u64le(&mut b, 40, 128);
    u64le(&mut b, 56, 96);
    u32le(&mut b, 80, 128);
    u64le(&mut b, 96, 120);
    u32le(&mut b, 104, 1);
    b[108..112].copy_from_slice(&[16, 8, 8, 0]);
    u32le(&mut b, 112, 1);
    u32le(&mut b, 120, primase_hash::fromsoft_path("a"));
    assert_eq!(Binder::read(&b, &decoder()).expect("hash table").files.len(), 1);
    for p in [56, 96, 104, 108, 112, 116, 124] {
        let mut bad = b.clone();
        bad[p..p + 4].fill(255);
        assert!(Binder::read(&bad, &decoder()).is_err(), "{p}");
    }
    let mut b = binder();
    b.truncate(96);
    let d = dcx();
    u64le(&mut b, 72, d.len() as u64);
    b.extend(d);
    assert_eq!(
        Binder::read(&b, &decoder()).expect("nested DCX").files[0].bytes,
        b"synthetic payload"
    );
}
#[test]
fn arbitrary_single_byte_mutations_never_panic() {
    for mut b in [bhd(), binder(), dcx()] {
        for i in 0..b.len() {
            let old = b[i];
            for val in [0, 1, 127, 128, 255] {
                b[i] = val;
                let _ = Header::read(&b);
                let _ = Binder::read(&b, &decoder());
                let _ = decoder().decode(&b);
            }
            b[i] = old;
        }
    }
}

#[test]
fn split_binder_shares_file_table_and_checks_both_files() {
    let mut header = binder();
    let body = header.split_off(96);
    header[..4].copy_from_slice(b"BHF4");
    u64le(&mut header, 40, 0);
    u32le(&mut header, 80, 48);
    let mut payload = vec![0; 48];
    payload[..4].copy_from_slice(b"BDF4");
    payload[10] = 1;
    u64le(&mut payload, 16, 48);
    payload.extend(body);
    let result = Binder::read_split(&header, &payload, &decoder()).expect("split binder");
    assert_eq!(result.files[0].bytes, b"twenty payload bytes");
    assert_eq!(result.files[0].name.as_deref(), Some("a"));
    assert_eq!(Binder::split_names(&header).unwrap(), ["a"]);
    for n in 0..90 {
        assert!(Binder::split_names(&header[..n]).is_err());
    }
    for n in 0..payload.len() {
        assert!(Binder::read_split(&header, &payload[..n], &decoder()).is_err());
    }
    u32le(&mut header, 80, 47);
    assert!(Binder::read_split(&header, &payload, &decoder()).is_err());
    assert!(Binder::split_names(&header).is_err());
}
fn tpf() -> Vec<u8> {
    let mut b = vec![0; 48];
    b[..4].copy_from_slice(b"TPF\0");
    u32le(&mut b, 4, 8);
    u32le(&mut b, 8, 1);
    b[13] = 3;
    b[14] = 1;
    u32le(&mut b, 16, 40);
    u32le(&mut b, 20, 8);
    b[26] = 1;
    u32le(&mut b, 28, 36);
    b[36] = b'a';
    b[40..44].copy_from_slice(b"DDS ");
    b
}
#[test]
fn tpf_pc_records_and_strict_bounds() {
    use primase_fromsoft::tpf::{Package, TextureKind};
    let b = tpf();
    let p = Package::read(&b).expect("package");
    assert_eq!(p.textures[0].name, "a");
    assert_eq!(p.textures[0].kind, TextureKind::Image2D);
    assert_eq!(p.textures[0].dds, &b[40..]);
    for n in 0..b.len() {
        assert!(Package::read(&b[..n]).is_err());
    }
    for offset in [0, 4, 8, 12, 13, 14, 15, 16, 20, 25, 27, 28, 32, 40] {
        let mut bad = b.clone();
        bad[offset] = 255;
        assert!(Package::read(&bad).is_err(), "{offset}");
    }
    let mut cube = b.clone();
    cube[25] = 1;
    assert_eq!(
        Package::read(&cube).expect("cube retained").textures[0].kind,
        TextureKind::Cube
    );
}
#[test]
fn tpf_retains_extra_metadata_words() {
    use primase_fromsoft::tpf::Package;
    let mut b = tpf();
    b.splice(36..36, [0; 12]);
    u32le(&mut b, 16, 52);
    u32le(&mut b, 28, 48);
    u32le(&mut b, 32, 1);
    u32le(&mut b, 36, 7);
    u32le(&mut b, 40, 4);
    u32le(&mut b, 44, 0x7fc00000);
    let p = Package::read(&b).expect("extra");
    let extra = p.textures[0].extra.as_ref().expect("present");
    assert_eq!(extra.id, 7);
    assert_eq!(extra.words, vec![0x7fc00000]);
}
#[test]
fn tpf_zero_alignment_has_two_data_size_conventions() {
    use primase_fromsoft::tpf::Package;
    let mut b = tpf();
    b.splice(40..40, [0; 272]);
    u32le(&mut b, 16, 312);
    assert!(Package::read(&b).is_ok());
    u32le(&mut b, 4, 280);
    assert!(Package::read(&b).is_ok());
    b[50] = 1;
    assert!(Package::read(&b).is_err());
}
#[test]
fn tpf_import_uses_shared_dds_metadata() {
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture as Image};
    use primase_fromsoft::tpf::{Texture, TextureKind};
    let image = Image {
        width: 4,
        height: 4,
        format: TexFormat::Bc1,
        levels: vec![vec![0; 8]],
        color_space: ColorSpace::Srgb,
        role: TexRole::Other,
        normal: None,
    };
    let mut record = Texture {
        name: "synthetic".into(),
        format: 0,
        kind: TextureKind::Image2D,
        mipmaps: 1,
        flags: 0,
        extra: None,
        dds: primase_dds::write_dx10(&image).expect("DDS"),
    };
    assert_eq!(record.image().expect("image"), image);
    record.kind = TextureKind::Cube;
    assert!(record.image().is_err());
}
