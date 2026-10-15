use primase_fromsoft::mtd::MaterialDefinition;
fn word(b: &mut Vec<u8>, x: u32) {
    b.extend(x.to_le_bytes());
}
fn marker(b: &mut Vec<u8>, x: u8) {
    b.push(x);
    b.resize(b.len().next_multiple_of(4), 0);
}
fn string(b: &mut Vec<u8>, s: &[u8], m: u8) {
    word(b, s.len() as u32);
    b.extend(s);
    marker(b, m);
}
fn begin(b: &mut Vec<u8>, kind: u32, version: u32, m: u8) -> usize {
    let p = b.len();
    word(b, 0);
    word(b, 0);
    word(b, kind);
    word(b, version);
    marker(b, m);
    p
}
fn end(b: &mut [u8], p: usize) {
    let n = (b.len() - p - 8) as u32;
    b[p + 4..p + 8].copy_from_slice(&n.to_le_bytes());
}
fn fixture() -> Vec<u8> {
    let mut b = vec![];
    let file = begin(&mut b, 0, 3, 1);
    let header = begin(&mut b, 1, 2, 0xb0);
    string(&mut b, b"MTD ", 0x34);
    word(&mut b, 1000);
    end(&mut b, header);
    marker(&mut b, 1);
    let data = begin(&mut b, 2, 4, 0xa3);
    string(&mut b, b"synthetic.sps", 0xa3);
    string(&mut b, &[0x93, 0xfa, 0x96, 0x7b], 3);
    word(&mut b, 1);
    let lists = begin(&mut b, 3, 4, 0xa3);
    word(&mut b, 0);
    marker(&mut b, 3);
    word(&mut b, 0);
    marker(&mut b, 3);
    word(&mut b, 0);
    marker(&mut b, 4);
    word(&mut b, 0);
    end(&mut b, lists);
    marker(&mut b, 4);
    word(&mut b, 0);
    end(&mut b, data);
    marker(&mut b, 4);
    word(&mut b, 0);
    end(&mut b, file);
    b
}
#[test]
fn strict_shift_jis_and_empty_lists() {
    let m = MaterialDefinition::read(&fixture()).unwrap();
    assert_eq!(m.description, "日本");
    assert_eq!(m.shader, "synthetic.sps");
    assert!(m.parameters.is_empty() && m.textures.is_empty());
}
#[test]
fn malformed_text_is_an_error_at_the_string() {
    let mut b = fixture();
    let p = b.windows(4).position(|x| x == [0x93, 0xfa, 0x96, 0x7b]).unwrap();
    b[p..p + 4].copy_from_slice(&[0x81, 0x30, 0x96, 0x7b]);
    let error = MaterialDefinition::read(&b).unwrap_err().to_string();
    assert!(
        error.contains("Shift-JIS") && error.contains(&format!("{p:#x}")),
        "{error}"
    );
}
#[test]
fn rejects_truncation_and_trailing_bytes_but_records_declared_lengths() {
    let b = fixture();
    for n in 0..b.len() {
        assert!(MaterialDefinition::read(&b[..n]).is_err());
    }
    let mut bad = b.clone();
    bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let parsed = MaterialDefinition::read(&bad).unwrap();
    let outer = parsed.block_lengths.last().unwrap();
    assert_eq!(outer.declared, u32::MAX as usize);
    assert_eq!(outer.consumed, b.len() - 8);
    let mut bad = b;
    bad.push(0);
    assert!(MaterialDefinition::read(&bad).is_err());
}
