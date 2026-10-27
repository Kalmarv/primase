//! Archive key material in the forms a caller supplies it: an archive pair's RSA public key as a PKCS#1 PEM block
//! ([`pem_public_key`]), and a 32-byte key as hexadecimal digits, held to a digest when the caller states one
//! ([`checked_key`]).
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

/// A 32-byte key written as 64 hexadecimal digits, refused unless its SHA-256 is `sha256` when that is given: key
/// material the user supplies is held to a digest the caller states, never to the key itself.
pub(crate) fn checked_key(hex: &str, sha256: Option<&[u8; 32]>) -> Result<[u8; 32], String> {
    use sha2::{Digest, Sha256};
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "{} characters, not a key's 64 hexadecimal digits",
            hex.len()
        ));
    }
    let mut key = [0u8; 32];
    for (i, k) in key.iter_mut().enumerate() {
        *k = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    if sha256.is_some_and(|sha256| Sha256::digest(key)[..] != sha256[..]) {
        return Err("not the key the game states (its SHA-256 differs)".into());
    }
    Ok(key)
}

#[cfg(test)]
mod checked_key_tests {
    use super::checked_key;
    use sha2::{Digest, Sha256};

    #[test]
    fn a_key_is_held_to_its_digest() {
        let key = [7u8; 32];
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let digest: [u8; 32] = Sha256::digest(key).into();
        assert_eq!(checked_key(&hex, Some(&digest)).unwrap(), key);
        assert_eq!(checked_key(&hex, None).unwrap(), key);
        assert_eq!(
            checked_key(&hex, Some(&[0; 32])).unwrap_err(),
            "not the key the game states (its SHA-256 differs)"
        );
        assert!(
            checked_key(&hex[..62], Some(&digest))
                .unwrap_err()
                .contains("62 characters")
        );
        assert!(checked_key(&hex[..62], None).is_err());
        assert!(checked_key(&"zz".repeat(32), None).is_err());
    }
}
