//! The file a glTF document's `uri` names. A buffer or image `uri` of a glTF 2.0 document is a URI reference
//! (RFC 3986): a relative file name with its reserved bytes percent-encoded (`my%20texture.png`), or a `data:` URI. A
//! caller that resolves a relative file for [`primase_glb::Gltf::from_gltf`] or [`crate::read_with_images`] decodes it
//! here first, so the reader's contract (the `uri` as the document writes it) and the file system's names meet in one
//! place.

/// The file name a relative `uri` names, its `%XX` escapes decoded as bytes (`%20` is a space, `%C3%A9` an `é`). An
/// escape that is not two hex digits is kept as written (a `%` is also a valid byte of a file name), and the decoded
/// bytes must be UTF-8. A `data:` URI names no file: it is refused.
pub fn relative_file_name(uri: &str) -> Result<String, String> {
    if uri.starts_with("data:") {
        return Err("a data URI names no file".to_owned());
    }
    let bytes = uri.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(hex) = uri.get(at + 1..at + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            decoded.push(byte);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).map_err(|error| format!("uri {uri:?}: {error}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn escapes_decode_as_bytes() {
        assert_eq!(relative_file_name("a%20b.png").unwrap(), "a b.png");
        assert_eq!(relative_file_name("caf%C3%A9.bin").unwrap(), "café.bin");
        assert_eq!(relative_file_name("sub/plain.bin").unwrap(), "sub/plain.bin");
    }

    #[test]
    fn a_malformed_escape_is_kept_and_a_data_uri_is_refused() {
        assert_eq!(relative_file_name("100%.png").unwrap(), "100%.png");
        assert_eq!(relative_file_name("a%zzb").unwrap(), "a%zzb");
        assert!(relative_file_name("data:image/png;base64,AAAA").is_err());
        assert!(relative_file_name("%FF.png").is_err());
    }
}
