//! Archive public keys: Sekiro's, published by the community unpackers, and the PKCS#1 PEM form a caller supplies
//! for a game whose keys are not distributed with primase ([`pem_public_key`]).
pub(crate) const SEKIRO: [(&[u8], &[u8]); 5] = [
    (
        &[
            247, 105, 126, 1, 108, 117, 105, 94, 230, 206, 223, 186, 175, 77, 27, 155, 250, 167, 115, 134,
            250, 52, 125, 213, 86, 191, 239, 225, 76, 76, 113, 252, 238, 178, 18, 252, 142, 201, 253, 101,
            35, 245, 178, 207, 121, 162, 223, 234, 186, 79, 29, 56, 155, 41, 211, 239, 219, 191, 17, 254, 11,
            208, 196, 123, 60, 205, 45, 42, 170, 0, 103, 118, 142, 173, 143, 98, 139, 35, 255, 161, 97, 96,
            2, 163, 140, 201, 74, 140, 220, 92, 84, 251, 10, 66, 232, 52, 101, 188, 190, 41, 139, 4, 54, 42,
            230, 17, 119, 38, 3, 87, 243, 150, 218, 200, 73, 254, 157, 232, 178, 129, 213, 65, 71, 28, 223,
            77, 70, 66, 194, 155, 120, 157, 19, 131, 238, 247, 206, 118, 144, 241, 219, 136, 189, 120, 18,
            167, 195, 8, 56, 48, 155, 185, 11, 121, 7, 109, 222, 69, 27, 114, 103, 97, 187, 254, 157, 188,
            254, 54, 28, 108, 65, 152, 6, 11, 52, 80, 247, 136, 7, 187, 168, 179, 11, 48, 92, 230, 40, 161,
            208, 15, 103, 209, 64, 175, 248, 223, 86, 119, 196, 242, 186, 38, 157, 184, 228, 7, 81, 208, 12,
            20, 97, 91, 231, 94, 175, 191, 247, 188, 12, 95, 64, 186, 80, 171, 192, 103, 230, 255, 215, 43,
            67, 54, 239, 47, 102, 223, 190, 150, 18, 116, 213, 40, 211, 115, 96, 56, 133, 83, 243, 238, 13,
            68, 16, 238, 127, 77,
        ],
        &[42, 72, 249, 39],
    ),
    (
        &[
            170, 24, 232, 78, 21, 151, 241, 92, 44, 76, 164, 200, 214, 72, 233, 208, 144, 101, 160, 37, 225,
            87, 200, 189, 244, 253, 74, 61, 48, 147, 12, 25, 230, 133, 84, 15, 118, 239, 187, 81, 14, 96,
            227, 255, 222, 135, 74, 42, 41, 163, 148, 141, 235, 126, 209, 35, 253, 225, 11, 28, 140, 130, 43,
            133, 95, 117, 59, 55, 200, 162, 226, 22, 128, 146, 74, 253, 19, 142, 63, 240, 55, 177, 190, 64,
            151, 51, 218, 233, 131, 220, 55, 195, 131, 197, 31, 158, 179, 42, 214, 133, 206, 171, 28, 43,
            120, 168, 107, 215, 85, 122, 224, 135, 172, 233, 100, 206, 97, 229, 80, 147, 73, 171, 169, 226,
            170, 136, 90, 110, 56, 16, 93, 254, 21, 230, 18, 237, 246, 133, 131, 94, 43, 31, 68, 4, 162, 200,
            158, 32, 134, 69, 102, 106, 72, 233, 32, 137, 106, 121, 107, 85, 223, 153, 135, 93, 236, 225,
            119, 25, 126, 77, 9, 116, 45, 128, 18, 60, 66, 67, 105, 74, 139, 117, 39, 88, 3, 14, 112, 151,
            62, 242, 225, 232, 50, 227, 77, 163, 83, 204, 165, 132, 58, 103, 195, 115, 131, 144, 162, 139,
            177, 39, 193, 176, 131, 154, 226, 112, 231, 203, 69, 6, 102, 206, 84, 118, 105, 145, 143, 119,
            84, 135, 133, 153, 162, 87, 72, 78, 243, 181, 58, 116, 169, 54, 180, 229, 134, 84, 178, 37, 55,
            131, 209, 233, 150, 71, 147, 83,
        ],
        &[255, 241, 41, 145],
    ),
    (
        &[
            199, 152, 229, 128, 139, 232, 29, 2, 240, 72, 91, 0, 192, 161, 89, 108, 218, 55, 126, 6, 124,
            244, 46, 237, 143, 136, 112, 137, 155, 16, 86, 15, 16, 156, 214, 104, 133, 121, 121, 75, 145, 41,
            246, 148, 182, 110, 35, 133, 177, 41, 46, 100, 197, 202, 153, 246, 4, 174, 29, 130, 177, 209, 91,
            175, 180, 17, 238, 75, 207, 62, 38, 167, 92, 218, 228, 8, 142, 49, 149, 244, 48, 38, 243, 38,
            144, 185, 189, 235, 242, 102, 68, 133, 51, 77, 106, 116, 144, 162, 84, 37, 146, 71, 253, 66, 43,
            92, 217, 251, 126, 105, 60, 86, 108, 117, 109, 2, 34, 241, 187, 166, 25, 41, 119, 131, 23, 140,
            134, 235, 111, 254, 80, 189, 146, 27, 24, 78, 125, 0, 5, 50, 64, 108, 134, 240, 53, 158, 70, 61,
            124, 115, 65, 155, 144, 61, 117, 195, 99, 157, 134, 155, 225, 113, 237, 140, 181, 46, 34, 191, 7,
            168, 172, 32, 50, 84, 55, 76, 93, 131, 23, 90, 95, 17, 141, 127, 154, 79, 161, 195, 9, 142, 120,
            52, 57, 189, 220, 142, 17, 175, 118, 76, 172, 150, 95, 227, 96, 89, 209, 199, 66, 218, 220, 33,
            166, 180, 183, 23, 50, 209, 225, 120, 68, 49, 204, 107, 158, 24, 54, 191, 150, 73, 37, 2, 251,
            196, 94, 215, 41, 41, 252, 133, 65, 103, 20, 133, 228, 25, 240, 185, 224, 2, 31, 212, 76, 145,
            237, 67,
        ],
        &[234, 85, 161, 79],
    ),
    (
        &[
            171, 196, 114, 2, 185, 62, 122, 163, 0, 115, 18, 192, 29, 64, 209, 97, 94, 242, 73, 195, 74, 102,
            146, 146, 196, 105, 160, 37, 148, 59, 203, 166, 60, 127, 156, 29, 172, 208, 173, 111, 37, 223,
            154, 199, 80, 16, 36, 173, 32, 105, 100, 193, 104, 237, 126, 111, 45, 253, 180, 89, 187, 28, 79,
            114, 32, 24, 104, 91, 22, 163, 70, 94, 216, 16, 122, 185, 142, 153, 181, 227, 105, 44, 153, 252,
            218, 166, 203, 77, 112, 19, 127, 230, 159, 222, 68, 171, 188, 128, 159, 188, 156, 17, 205, 183,
            192, 252, 244, 73, 70, 65, 227, 218, 2, 122, 77, 206, 224, 232, 114, 187, 75, 254, 19, 136, 38,
            98, 174, 175, 156, 255, 50, 216, 222, 6, 252, 3, 203, 136, 20, 210, 56, 143, 74, 177, 79, 55,
            232, 2, 84, 187, 43, 171, 41, 102, 55, 231, 133, 158, 165, 238, 150, 5, 135, 232, 37, 207, 94,
            112, 1, 186, 105, 11, 211, 178, 254, 238, 111, 81, 167, 40, 205, 254, 47, 157, 167, 27, 80, 112,
            248, 237, 219, 61, 17, 147, 217, 12, 121, 55, 45, 242, 91, 138, 129, 240, 181, 76, 201, 127, 32,
            106, 102, 98, 41, 35, 125, 242, 139, 2, 38, 62, 150, 253, 235, 60, 47, 65, 79, 58, 77, 165, 202,
            176, 154, 211, 153, 43, 63, 227, 192, 94, 14, 196, 48, 94, 230, 170, 58, 15, 186, 158, 129, 19,
            7, 29, 233, 129,
        ],
        &[24, 205, 142, 135],
    ),
    (
        &[
            187, 190, 127, 81, 181, 240, 29, 219, 238, 254, 158, 61, 79, 9, 216, 236, 235, 186, 12, 11, 153,
            96, 80, 45, 45, 73, 48, 253, 30, 39, 190, 109, 7, 89, 89, 81, 177, 152, 129, 44, 109, 204, 56,
            110, 172, 62, 98, 182, 101, 205, 201, 44, 21, 54, 64, 174, 46, 41, 172, 217, 245, 90, 230, 162,
            211, 56, 58, 150, 43, 211, 122, 182, 182, 33, 103, 191, 115, 95, 10, 240, 117, 81, 226, 9, 32,
            167, 183, 145, 88, 71, 53, 146, 27, 190, 188, 4, 63, 214, 185, 80, 41, 235, 72, 112, 254, 187,
            225, 99, 228, 196, 1, 175, 140, 171, 18, 43, 112, 172, 213, 111, 32, 46, 9, 208, 76, 245, 62,
            153, 247, 248, 63, 31, 127, 197, 224, 65, 19, 253, 168, 37, 75, 35, 117, 255, 193, 212, 3, 121,
            175, 129, 230, 143, 15, 89, 166, 4, 47, 211, 10, 158, 114, 245, 78, 174, 33, 251, 131, 209, 96,
            119, 16, 71, 207, 198, 92, 20, 235, 115, 124, 125, 171, 119, 15, 65, 16, 214, 29, 180, 88, 132,
            135, 214, 72, 255, 110, 174, 214, 160, 141, 203, 38, 114, 225, 190, 167, 126, 31, 167, 226, 178,
            88, 236, 36, 138, 93, 197, 38, 172, 7, 212, 168, 18, 77, 185, 61, 77, 189, 40, 4, 252, 234, 76,
            34, 74, 120, 79, 240, 130, 238, 161, 99, 239, 165, 6, 46, 83, 220, 10, 199, 101, 151, 44, 109,
            124, 213, 235,
        ],
        &[249, 81, 76, 199],
    ),
];

/// The modulus and exponent (big-endian, without leading zeros) of a PKCS#1 `RSA PUBLIC KEY` PEM block.
pub(crate) fn pem_public_key(text: &str) -> crate::Result<(Vec<u8>, Vec<u8>)> {
    let err = |detail: &str| crate::invalid("PEM", 0, detail);
    let body = text
        .trim()
        .strip_prefix("-----BEGIN RSA PUBLIC KEY-----")
        .and_then(|s| s.strip_suffix("-----END RSA PUBLIC KEY-----"))
        .ok_or_else(|| err("expected one RSA PUBLIC KEY block"))?;
    let der = base64(body).ok_or_else(|| err("invalid base64"))?;
    let (tag, sequence, rest) = der_item(&der).ok_or_else(|| err("invalid DER"))?;
    if tag != 0x30 || !rest.is_empty() {
        return Err(err("expected one DER sequence"));
    }
    let (tag_n, n, rest) = der_item(sequence).ok_or_else(|| err("invalid DER"))?;
    let (tag_e, e, rest) = der_item(rest).ok_or_else(|| err("invalid DER"))?;
    if tag_n != 2 || tag_e != 2 || !rest.is_empty() {
        return Err(err("expected two DER integers"));
    }
    // a DER integer is signed: a positive one whose top bit is set has one leading zero byte, and no other
    let unsigned = |x: &[u8]| match x {
        [0, rest @ ..] if rest.first().is_some_and(|&b| b >= 0x80) => Some(rest.to_vec()),
        [first, ..] if *first != 0 && *first < 0x80 => Some(x.to_vec()),
        _ => None,
    };
    Ok((
        unsigned(n).ok_or_else(|| err("modulus is not a positive minimal DER integer"))?,
        unsigned(e).ok_or_else(|| err("exponent is not a positive minimal DER integer"))?,
    ))
}

/// One DER item: its tag, its contents and the bytes after it. Lengths are definite and minimal.
fn der_item(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (&first, mut b) = b.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || b.len() < n || b[0] == 0 {
            return None;
        }
        let len = b[..n].iter().fold(0usize, |a, &x| a << 8 | usize::from(x));
        if len < 0x80 {
            return None;
        }
        b = &b[n..];
        len
    };
    (b.len() >= len).then(|| (tag, &b[..len], &b[len..]))
}

/// Standard base64 (RFC 4648) with `=` padding and zero pad bits; ASCII whitespace between characters is ignored.
fn base64(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(digits.len() / 4 * 3);
    let chunks = digits.as_chunks::<4>().0;
    for (i, chunk) in chunks.iter().enumerate() {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && i + 1 != chunks.len()) {
            return None;
        }
        let mut word = 0u32;
        for &c in &chunk[..4 - pad] {
            word = word << 6 | u32::from(value(c)?);
        }
        word <<= 6 * pad as u32;
        // the bits below the last whole byte must be zero
        if word & ((1 << (8 * pad as u32)) - 1) != 0 {
            return None;
        }
        out.extend_from_slice(&word.to_be_bytes()[1..4 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (text, bytes) in [
            ("", &b""[..]),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9vYmE=", b"fooba"),
            ("Zm9vYmFy", b"foobar"),
            (
                "Zm9v
YmFy",
                b"foobar",
            ),
        ] {
            assert_eq!(base64(text).as_deref(), Some(bytes), "{text}");
        }
        // short, over-padded, padding mid-text, a foreign character, non-zero pad bits
        for bad in ["Zg=", "Z===", "Zg==Zg==", "Zm9*", "Zh=="] {
            assert_eq!(base64(bad), None, "{bad}");
        }
    }

    /// A hand-built PKCS#1 key: SEQUENCE { INTEGER 0x00c5..(top bit set, so one leading zero), INTEGER 65537 }.
    #[test]
    fn pem_reads_a_hand_built_key() {
        let mut modulus = vec![0xc5; 256];
        modulus[255] = 0x01;
        let mut der = vec![0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01, 0x00];
        der.extend_from_slice(&modulus);
        der.extend_from_slice(&[0x02, 0x03, 0x01, 0x00, 0x01]);
        assert_eq!(der.len(), 4 + 0x010a);
        let pem = format!(
            "-----BEGIN RSA PUBLIC KEY-----
{}
-----END RSA PUBLIC KEY-----
",
            encode(&der)
        );
        let (n, e) = pem_public_key(&pem).expect("hand-built key");
        assert_eq!(n, modulus);
        assert_eq!(e, [1, 0, 1]);
        // a redundant leading zero, a trailing item and a negative exponent are refused
        for (at, byte) in [(9, 0x45u8), (der.len() - 3, 0x81)] {
            let mut bad = der.clone();
            bad[at] = byte;
            let pem = format!(
                "-----BEGIN RSA PUBLIC KEY-----
{}
-----END RSA PUBLIC KEY-----",
                encode(&bad)
            );
            assert!(pem_public_key(&pem).is_err(), "byte {at}");
        }
        assert!(
            pem_public_key(
                "-----BEGIN PUBLIC KEY-----
AA==
-----END PUBLIC KEY-----"
            )
            .is_err()
        );
    }

    /// RFC 4648 base64 of `bytes`, for the fixture above.
    fn encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let word = chunk
                .iter()
                .enumerate()
                .fold(0u32, |w, (i, &b)| w | u32::from(b) << (16 - 8 * i));
            for i in 0..4 {
                out.push(if i <= chunk.len() {
                    char::from(ALPHABET[(word >> (18 - 6 * i) & 63) as usize])
                } else {
                    '='
                });
            }
        }
        out
    }
}
