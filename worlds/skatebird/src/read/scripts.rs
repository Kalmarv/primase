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

/// An object reference as stored: the file (0: this one, else an index into its externals) and the path id; `None`
/// when null (path id 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PPtr {
    pub file_id: i32,
    pub path_id: i64,
}

/// A `GrindableRail`'s fields as stored: the previous and next node of its chain, two more references (meaning not
/// known) and eight flag bytes (meaning not known).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrindableRail {
    pub prev: Option<PPtr>,
    pub next: Option<PPtr>,
    pub third: Option<PPtr>,
    pub fourth: Option<PPtr>,
    pub flags: [u8; 8],
}

/// `GrindableRail`, from its script bytes: on Unity [`MEASURED`] four object references (a 32-bit file id and a 64-bit
/// path id each: `prev`, `next`, two more) and eight flag bytes, each 0 or 1: 56 bytes (measured over level1's 540).
pub fn grindable_rail(version: &str, data: &[u8]) -> Result<GrindableRail> {
    const WHAT: &str = "GrindableRail";
    if version != MEASURED {
        return Err(refuse(WHAT, version, "another Unity version".into()));
    }
    let (Ok(refs), Ok(flags)) = (
        <[u8; 48]>::try_from(data.get(..48).unwrap_or_default()),
        <[u8; 8]>::try_from(data.get(48..).unwrap_or_default()),
    ) else {
        return Err(refuse(
            WHAT,
            version,
            format!("{} script bytes, not 56", data.len()),
        ));
    };
    if let Some(b) = flags.iter().find(|&&b| b > 1) {
        return Err(refuse(WHAT, version, format!("flag byte {b:#04x}, not 0 or 1")));
    }
    let pptr = |i: usize| -> Option<PPtr> {
        let at = i * 12;
        let file_id = i32::from_le_bytes(refs.get(at..at + 4)?.try_into().ok()?);
        let path_id = i64::from_le_bytes(refs.get(at + 4..at + 12)?.try_into().ok()?);
        (path_id != 0).then_some(PPtr { file_id, path_id })
    };
    Ok(GrindableRail {
        prev: pptr(0),
        next: pptr(1),
        third: pptr(2),
        fourth: pptr(3),
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four references and eight flag bytes read; a null reference is none; another length or a flag that is not 0
    /// or 1 is refused.
    #[test]
    fn a_grindable_rail_is_four_references_and_eight_flags() {
        let mut data = Vec::new();
        for (file, path) in [(0i32, 0i64), (0, 1234), (0, 0), (1, -77)] {
            data.extend_from_slice(&file.to_le_bytes());
            data.extend_from_slice(&path.to_le_bytes());
        }
        data.extend_from_slice(&[1, 0, 0, 0, 1, 0, 0, 0]);
        let r = grindable_rail(MEASURED, &data).unwrap();
        assert_eq!(r.prev, None);
        assert_eq!(
            r.next,
            Some(PPtr {
                file_id: 0,
                path_id: 1234
            })
        );
        assert_eq!(r.third, None);
        assert_eq!(
            r.fourth,
            Some(PPtr {
                file_id: 1,
                path_id: -77
            })
        );
        assert_eq!(r.flags, [1, 0, 0, 0, 1, 0, 0, 0]);
        assert!(
            grindable_rail(MEASURED, &data[..52])
                .unwrap_err()
                .to_string()
                .contains("52 script bytes")
        );
        let mut bad = data.clone();
        bad[50] = 2;
        assert!(
            grindable_rail(MEASURED, &bad)
                .unwrap_err()
                .to_string()
                .contains("flag byte 0x02")
        );
        assert!(grindable_rail("2021.3.1f1", &data).is_err());
    }

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
