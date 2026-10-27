//! MaterialEx row and event-payload reading. Typed source facts and all original bytes
//! travel together; this module assigns no transition or application meaning to a word.
use crate::components::material_events::{MaterialExRow, MaterialExRows};
use crate::invalid;
use fromsoft_formats::{Game, Result, param::Param};
use primase_core::{EventValue, EventValueData};

/// Only the ER v4 / 96-byte layout independently inspected in the installed rows is read.
/// Other layouts are reported by the reader rather than interpreted using ER's offsets.
pub fn read_rows(table: &Param) -> Result<MaterialExRows> {
    const FORMAT: &str = "MaterialExParam";
    if table.param_type != "MATERIAL_EX_PARAM_ST" || table.data_version != 4 || table.row_size != 96 {
        return Err(invalid(
            FORMAT,
            0,
            format!(
                "unobserved layout: {} v{} / {} bytes; only ER v4 / 96 is decoded",
                table.param_type, table.data_version, table.row_size
            ),
        ));
    }
    let mut out = MaterialExRows::default();
    for row in &table.rows {
        let error = |at, reason: String| invalid(FORMAT, at, format!("row {}: {reason}", row.id));
        if row.data.len() != 96 {
            return Err(error(0, "row length is not 96".into()));
        }
        let units: Vec<_> = row.data[..64]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect();
        let parameter = String::from_utf16(&units).map_err(|reason| error(0, reason.to_string()))?;
        if parameter.trim().is_empty() {
            return Err(error(0, "empty parameter name".into()));
        }
        let word = |at| <[u8; 4]>::try_from(&row.data[at..at + 4]).expect("checked row width");
        let rgba_i = std::array::from_fn(|i| f32::from_le_bytes(word(68 + i * 4)));
        if rgba_i.iter().any(|value| !value.is_finite()) {
            return Err(error(68, "non-finite stored lane".into()));
        }
        let parsed = MaterialExRow {
            id: row.id,
            parameter,
            material_id: i32::from_le_bytes(word(64)),
            rgba_i,
            bytes: row.data.clone(),
        };
        if out.rows.insert(row.id, parsed).is_some() {
            return Err(error(0, "duplicate row id".into()));
        }
    }
    Ok(out)
}

/// Additional typed source fields for TAE 198, beside the generic community-template fields.
/// A refusal leaves structural word views available; only an observed ER record supplies
/// the row link. The original event retains every byte, including partial words/padding.
pub fn event_198(game: Game, bytes: &[u8]) -> (Vec<EventValue>, Option<String>) {
    let mut fields = Vec::new();
    for (index, word) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let at = index * 4;
        fields.push(EventValue {
            name: format!("material_ex.raw_{at:02}_u32"),
            value: EventValueData::I64(i64::from(u32::from_le_bytes(*word))),
        });
        let value = f32::from_le_bytes(*word);
        if value.is_finite() {
            fields.push(EventValue {
                name: format!("material_ex.raw_{at:02}_f32"),
                value: EventValueData::F64(f64::from(value)),
            });
        }
    }
    let refusal = if game != Game::EldenRing {
        Some("Sekiro event 198 row layout and envelope are unproven; raw event retained".into())
    } else if bytes.len() != 64 && !(bytes.len() == 72 && bytes[64..].iter().all(|byte| *byte == 0)) {
        Some(format!(
            "event 198 has {} bytes, not an observed 64-byte record or zero-padded 72-byte alias",
            bytes.len()
        ))
    } else {
        let id = i32::from_le_bytes(bytes[4..8].try_into().expect("checked event width"));
        fields.push(EventValue {
            name: "material_ex.row_id".into(),
            value: EventValueData::I64(i64::from(id)),
        });
        None
    };
    (fields, refusal)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Hand-worked row: UTF-16LE logical name, id -1, stored RGBA zero and I one.
    #[test]
    fn verified_row_layout_keeps_all_bytes_and_refuses_other_revisions() {
        let mut bytes = vec![0; 96];
        for (at, unit) in "C[PixelKill]_Int".encode_utf16().enumerate() {
            bytes[at * 2..at * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        bytes[64..68].copy_from_slice(&(-1i32).to_le_bytes());
        bytes[84..88].copy_from_slice(&1f32.to_le_bytes());
        let mut table = Param {
            param_type: "MATERIAL_EX_PARAM_ST".into(),
            data_version: 4,
            unk06: 1,
            row_size: 96,
            rows: vec![fromsoft_formats::param::Row {
                id: 10300,
                name: None,
                data: bytes.clone(),
            }],
        };
        let rows = read_rows(&table).unwrap();
        let row = &rows.rows[&10300];
        assert_eq!(row.parameter, "C[PixelKill]_Int");
        assert_eq!(row.material_id, -1);
        assert_eq!(row.rgba_i, [0., 0., 0., 0., 1.]);
        assert_eq!(row.bytes, bytes);
        table.data_version = 3;
        assert!(read_rows(&table).is_err());
        table.data_version = 4;
        table.rows[0].data[68..72].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(read_rows(&table).is_err());
    }
    /// The observed link is byte 4. 0.5 at byte 8 is a raw float, never a target/time claim.
    #[test]
    fn event_words_are_typed_without_guessing_an_envelope() {
        let mut bytes = vec![0; 64];
        bytes[4..8].copy_from_slice(&10300i32.to_le_bytes());
        bytes[8..12].copy_from_slice(&0.5f32.to_le_bytes());
        let (fields, refusal) = event_198(Game::EldenRing, &bytes);
        assert!(refusal.is_none());
        assert!(
            fields
                .iter()
                .any(|field| field.name == "material_ex.row_id" && field.value == EventValueData::I64(10300))
        );
        assert!(
            fields.iter().any(
                |field| field.name == "material_ex.raw_08_f32" && field.value == EventValueData::F64(0.5)
            )
        );
        assert!(event_198(Game::Sekiro, &bytes).1.unwrap().contains("unproven"));
        assert!(event_198(Game::EldenRing, &bytes[..7]).1.is_some());
        bytes.extend_from_slice(&[0; 8]);
        assert!(event_198(Game::EldenRing, &bytes).1.is_none());
        bytes[71] = 1;
        assert!(event_198(Game::EldenRing, &bytes).1.is_some());
    }
}
