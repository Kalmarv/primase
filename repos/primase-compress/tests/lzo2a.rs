//! LZO2A on hand-worked streams: each fixture's bytes were laid out by hand from lzo2a_d.ch of LZO 2.10 (the flag
//! bits least significant first, taken into the bit buffer a byte at a time when needed), so each covers one kind of
//! step and the end code; then the refusals the decoder must make.
use primase_compress::{Decompressor, Lzo2a};

/// Flags 0 0 0 (three literals) then 1 1 (a two-byte match code) in the first flag byte 0x18; the code 0x20 0x00 is
/// length field 1 with distance 0: the end of the stream.
const LITERALS: [u8; 6] = [0x18, b'a', b'b', b'c', 0x20, 0x00];
/// Flags 0 0, 1 0 with length bits 0 1 (2 + 2 = 4), then 1 1: 0b1110_0100. The short match's distance byte 1 is
/// distance 2, so it repeats `ab` twice; then the end code.
const SHORT_MATCH: [u8; 6] = [0xE4, b'a', b'b', 0x01, 0x20, 0x00];
/// Flags 0, 1 1 (a long match: length field 0, distance 1), then 1 1 (the end code): 0b0001_1110. The length bytes
/// 0 (255) and 1 make 9 + 255 + 1 = 265 copies of the one byte before.
const LONG_MATCH: [u8; 8] = [0x1E, b'x', 0x01, 0x00, 0x00, 0x01, 0x20, 0x00];

#[test]
fn hand_worked_streams() {
    assert_eq!(Lzo2a.decompress(&LITERALS, 3).unwrap(), b"abc");
    assert_eq!(Lzo2a.decompress(&SHORT_MATCH, 6).unwrap(), b"ababab");
    assert_eq!(Lzo2a.decompress(&LONG_MATCH, 266).unwrap(), vec![b'x'; 266]);
}

#[test]
fn refusals() {
    // the expected size must be met exactly, neither more nor less
    assert!(Lzo2a.decompress(&LITERALS, 2).is_err());
    assert!(Lzo2a.decompress(&LITERALS, 4).is_err());
    assert!(Lzo2a.decompress(&LONG_MATCH, 265).is_err());
    // the end code is required, and nothing may follow it
    for n in 0..LITERALS.len() {
        assert!(Lzo2a.decompress(&LITERALS[..n], 3).is_err(), "prefix {n}");
    }
    let mut trailing = LITERALS.to_vec();
    trailing.push(0);
    assert!(Lzo2a.decompress(&trailing, 3).is_err());
    // a match reaching before the output's start: distance 3 after two literals
    assert!(
        Lzo2a
            .decompress(&[0xE4, b'a', b'b', 0x02, 0x20, 0x00], 6)
            .is_err()
    );
    // a long match of distance 0 copies from the output's end: refused, not an end code
    assert!(
        Lzo2a
            .decompress(&[0x1E, b'x', 0x00, 0x00, 0x01, 0x20, 0x00], 11)
            .is_err()
    );
}
