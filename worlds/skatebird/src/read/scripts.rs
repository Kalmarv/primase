//! SkateBIRD's script fields, decoded from the bytes a MonoBehaviour stores after its base fields
//! ([`primase_unity::components::UnityBehaviour::data`]; a stripped player has no type tree for them). Each layout is
//! named for the player's Unity version it was measured on. Bytes that are not that layout (another length, a bool
//! that is neither 0 nor 1, padding that is not zero, a string past the end) are refused with what was found, never
//! guessed at; another build's layout is a new named layout.

use crate::{Error, Result};

/// The Unity version of the SkateBIRD player these layouts were measured on (every serialized file of it): 134
/// `ResetPosition`s and the `MissionGiver`s of levels 1-5, 7, 12 and 13.
pub const MEASURED: &str = "2019.4.20f1";

fn refuse(what: &str, version: &str, detail: String) -> Error {
    Error::Layout(format!(
        "{what}: {detail}: not the layout measured on Unity {MEASURED} (this player: {version})"
    ))
}

/// `ResetPosition.startingPosition`, from its script bytes: on Unity [`MEASURED`] exactly one bool (0 or 1) and three
/// zero alignment bytes.
pub fn reset_position(version: &str, data: &[u8]) -> Result<bool> {
    const WHAT: &str = "ResetPosition";
    if version != MEASURED {
        return Err(refuse(WHAT, version, "another Unity version".into()));
    }
    let [flag, a, b, c] = data else {
        return Err(refuse(
            WHAT,
            version,
            format!("{} script bytes, not 4", data.len()),
        ));
    };
    if [*a, *b, *c] != [0; 3] {
        return Err(refuse(
            WHAT,
            version,
            format!("alignment bytes {a:#04x} {b:#04x} {c:#04x}, not zero"),
        ));
    }
    match flag {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(refuse(
            WHAT,
            version,
            format!("startingPosition byte {other:#04x}, not 0 or 1"),
        )),
    }
}

/// `MissionGiver`'s title, from its script bytes: on Unity [`MEASURED`] its first field is a string (a 32-bit length,
/// that many UTF-8 bytes, aligned to 4), the rest of the script's fields following; `None` for an empty one.
pub fn mission_giver_title(version: &str, data: &[u8]) -> Result<Option<String>> {
    const WHAT: &str = "MissionGiver";
    if version != MEASURED {
        return Err(refuse(WHAT, version, "another Unity version".into()));
    }
    let Some(len) = data
        .get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    else {
        return Err(refuse(
            WHAT,
            version,
            format!("{} script bytes, no title length", data.len()),
        ));
    };
    let Some(bytes) = data.get(4..).and_then(|rest| rest.get(..len)) else {
        return Err(refuse(
            WHAT,
            version,
            format!("a title of {len} bytes in {} script bytes", data.len()),
        ));
    };
    let title = std::str::from_utf8(bytes)
        .map_err(|e| refuse(WHAT, version, format!("a title that is not UTF-8 ({e})")))?;
    Ok((!title.is_empty()).then(|| title.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured layout reads; another length, a bad bool, nonzero padding or another version is refused, naming
    /// what was found.
    #[test]
    fn reset_position_reads_the_measured_layout_and_refuses_others() {
        assert!(!reset_position(MEASURED, &[0, 0, 0, 0]).unwrap());
        assert!(reset_position(MEASURED, &[1, 0, 0, 0]).unwrap());
        let refused = |v: &str, d: &[u8]| reset_position(v, d).unwrap_err().to_string();
        assert!(refused(MEASURED, &[1, 0, 0, 0, 0, 0, 0, 0]).contains("8 script bytes"));
        assert!(refused(MEASURED, &[2, 0, 0, 0]).contains("byte 0x02"));
        assert!(refused(MEASURED, &[1, 0, 9, 0]).contains("alignment bytes"));
        let e = refused("2021.3.1f1", &[1, 0, 0, 0]);
        assert!(
            e.contains("another Unity version") && e.contains("2021.3.1f1"),
            "{e}"
        );
    }

    /// A title as stored reads, an empty one is none, and a length past the end or bytes that aren't UTF-8 are
    /// refused.
    #[test]
    fn a_mission_givers_title_is_its_first_field() {
        let mut data = 8u32.to_le_bytes().to_vec();
        data.extend_from_slice(b"bowl cut");
        data.extend_from_slice(&[0; 40]);
        assert_eq!(
            mission_giver_title(MEASURED, &data).unwrap().as_deref(),
            Some("bowl cut")
        );
        let mut empty = 0u32.to_le_bytes().to_vec();
        empty.extend_from_slice(&[0; 48]);
        assert_eq!(mission_giver_title(MEASURED, &empty).unwrap(), None);
        let mut long = 200u32.to_le_bytes().to_vec();
        long.extend_from_slice(&[b'a'; 48]);
        assert!(
            mission_giver_title(MEASURED, &long)
                .unwrap_err()
                .to_string()
                .contains("200 bytes in 52")
        );
        let mut bad = 2u32.to_le_bytes().to_vec();
        bad.extend_from_slice(&[0xff, 0xfe, 0, 0]);
        assert!(
            mission_giver_title(MEASURED, &bad)
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
        assert!(mission_giver_title(MEASURED, &[1, 0]).is_err());
    }
}
