//! The reference writer's Hash64 (texture content names) against values from the independent C# reference.

use primase_reskate::texture::Hash64;

#[test]
fn a_string_then_a_word() {
    let mut h = Hash64::new();
    h.string("A😀é").word(0x1234_5678);
    assert_eq!(h.value(), 0x30c6_6d82_292e_7540);
}

/// `AddBytes` over 0..n: whole words, a zero-padded tail word, and none for no bytes.
#[test]
fn bytes_of_every_tail_length() {
    for (n, want) in [
        (0u8, 0xf955_b588_1a88_2d0d_u64),
        (1, 0x7694_70f3_b86d_3938),
        (7, 0x0fdc_e44c_f6ec_2477),
        (8, 0x3ae6_b5b0_9d70_917c),
        (9, 0x7218_e617_5608_5f9b),
        (17, 0x9635_1c1b_05ca_d37d),
    ] {
        let b: Vec<u8> = (0..n).collect();
        assert_eq!(Hash64::new().bytes(&b).value(), want, "{n} bytes");
    }
}
