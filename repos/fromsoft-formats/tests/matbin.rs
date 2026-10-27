use fromsoft_formats::matbin::{Material, Value};

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}
fn put64(b: &mut [u8], at: usize, x: usize) {
    b[at..at + 8].copy_from_slice(&(x as u64).to_le_bytes());
}
/// A material with a bool (odd-length, then one realignment byte), a three-float and a five-float colour, a float2
/// stored as one float, one sampler, and `tail` after the last referenced string.
fn fixture(tail: &[u8]) -> Vec<u8> {
    let params = 4;
    let records = 0x38 + params * 0x28 + 0x30;
    let mut b = vec![0u8; records];
    b[..4].copy_from_slice(b"MAB\0");
    b[4] = 2;
    b[0x18..0x1c].copy_from_slice(&7u32.to_le_bytes());
    b[0x1c..0x20].copy_from_slice(&(params as u32).to_le_bytes());
    b[0x20..0x24].copy_from_slice(&1u32.to_le_bytes());
    let push = |b: &mut Vec<u8>, bytes: &[u8]| {
        let at = b.len();
        b.extend_from_slice(bytes);
        at
    };
    let one = 1f32.to_le_bytes();
    let half = 0.5f32.to_le_bytes();
    let values: [(u32, Vec<u8>); 4] = [
        (0, vec![1]),
        (10, [one, one, one].concat()),
        (10, [one, half, one, one, one].concat()),
        (9, half.to_vec()),
    ];
    for (i, (code, value)) in values.iter().enumerate() {
        let r = 0x38 + i * 0x28;
        if b.len() % 2 == 1 {
            b.push(0);
        }
        let name = push(&mut b, &utf16(&format!("p{i}")));
        let at = push(&mut b, value);
        put64(&mut b, r, name);
        put64(&mut b, r + 8, at);
        b[r + 0x10..r + 0x14].copy_from_slice(&(i as u32 + 10).to_le_bytes());
        b[r + 0x14..r + 0x18].copy_from_slice(&code.to_le_bytes());
    }
    let s = 0x38 + params * 0x28;
    let kind = push(&mut b, &utf16("M_snp_Texture2D_0_AlbedoMap_0"));
    let path = push(&mut b, &utf16("N:\\a.tif"));
    put64(&mut b, s, kind);
    put64(&mut b, s + 8, path);
    b[s + 0x14..s + 0x18].copy_from_slice(&2f32.to_le_bytes());
    let shader = push(&mut b, &utf16("x.spx"));
    let source = push(&mut b, &utf16("x.matxml"));
    put64(&mut b, 8, shader);
    put64(&mut b, 16, source);
    b.extend_from_slice(tail);
    b
}

#[test]
fn values_keep_their_stored_extent() {
    let m = Material::read(&fixture(&[])).unwrap();
    assert_eq!(
        (m.shader.as_str(), m.source.as_str(), m.key),
        ("x.spx", "x.matxml", 7)
    );
    assert_eq!(m.params[0].value, Value::Bool(true));
    assert_eq!(m.params[1].value, Value::Float3([1.; 3], None));
    assert_eq!(m.params[2].value, Value::Float3([1., 0.5, 1.], Some([1., 1.])));
    assert_eq!(
        m.params[3].value,
        Value::Truncated {
            code: 9,
            bytes: 0.5f32.to_le_bytes().to_vec()
        }
    );
    assert_eq!(m.params[3].key, 13);
    assert_eq!(m.samplers[0].kind, "M_snp_Texture2D_0_AlbedoMap_0");
    assert_eq!(m.samplers[0].path, "N:\\a.tif");
    assert_eq!(m.samplers[0].unknown, [2., 0.]);
    assert!(m.unreferenced_tail.is_empty());
    assert_eq!(m.param("p1"), Some(&Value::Float3([1.; 3], None)));
}

#[test]
fn unreferenced_tails_are_kept_and_gaps_are_refused() {
    let tail = utf16("old revision");
    assert_eq!(Material::read(&fixture(&tail)).unwrap().unreferenced_tail, tail);
    // A gap of two bytes inside the referenced area is not alignment.
    let mut b = fixture(&[]);
    let at = b.len() - utf16("x.matxml").len();
    b.splice(at..at, [0, 0]);
    put64(&mut b, 16, at + 2);
    assert!(Material::read(&b).is_err());
    // A nonzero alignment byte is refused.
    let mut b = fixture(&[]);
    let r = 0x38 + 0x28;
    let name = u64::from_le_bytes(b[r..r + 8].try_into().unwrap()) as usize;
    b[name - 1] = 1;
    assert!(Material::read(&b).is_err());
    for code in [1u32, 13] {
        let mut b = fixture(&[]);
        b[0x38 + 0x14..0x38 + 0x18].copy_from_slice(&code.to_le_bytes());
        assert!(Material::read(&b).is_err());
    }
}

#[test]
fn every_truncation_is_an_error() {
    let b = fixture(&[]);
    for n in 0..b.len() {
        assert!(Material::read(&b[..n]).is_err(), "{n}");
    }
}

/// A material whose last stored item is a float3 value (after its name, shader and source strings), then `tail`.
fn last_value(tail: &[u8]) -> Vec<u8> {
    let records = 0x38 + 0x28;
    let mut b = vec![0u8; records];
    b[..4].copy_from_slice(b"MAB\0");
    b[4] = 2;
    b[0x1c..0x20].copy_from_slice(&1u32.to_le_bytes());
    let push = |b: &mut Vec<u8>, bytes: &[u8]| {
        let at = b.len();
        b.extend_from_slice(bytes);
        at
    };
    let shader = push(&mut b, &utf16("x.spx"));
    let source = push(&mut b, &utf16("x.matxml"));
    let name = push(&mut b, &utf16("p0"));
    let one = 1f32.to_le_bytes();
    let at = push(&mut b, &[one, one, one].concat());
    put64(&mut b, 8, shader);
    put64(&mut b, 16, source);
    put64(&mut b, 0x38, name);
    put64(&mut b, 0x38 + 8, at);
    b[0x38 + 0x14..0x38 + 0x18].copy_from_slice(&10u32.to_le_bytes());
    b.extend_from_slice(tail);
    b
}

#[test]
fn the_last_value_is_bounded_by_its_type() {
    // eight tail bytes after a float3 are the tail, not the two further floats a five-float colour stores
    let tail = [2f32.to_le_bytes(), 3f32.to_le_bytes()].concat();
    let m = Material::read(&last_value(&tail)).unwrap();
    assert_eq!(m.params[0].value, Value::Float3([1.; 3], None));
    assert_eq!(m.unreferenced_tail, tail);
    let m = Material::read(&last_value(&[])).unwrap();
    assert_eq!(m.params[0].value, Value::Float3([1.; 3], None));
    assert!(m.unreferenced_tail.is_empty());
    // a last value the file cuts short is still kept as stored
    let b = last_value(&[]);
    let m = Material::read(&b[..b.len() - 4]).unwrap();
    assert_eq!(
        m.params[0].value,
        Value::Truncated {
            code: 10,
            bytes: [1f32.to_le_bytes(), 1f32.to_le_bytes()].concat()
        }
    );
}
