//! MaterialEx source adaptation. Public format records retain the bytes; this module assigns no application meaning.
use crate::components::material_events::MaterialExRows;
use fromsoft_formats::{Game, Result, param::Param};
use primase_core::{EventValue, EventValueData};

/// Read the supported public row layout into the source resource without changing its supported values.
pub fn read_rows(table: &Param) -> Result<MaterialExRows> {
    Ok(MaterialExRows {
        rows: fromsoft_formats::material_events::read_rows(table)?,
        refusal: None,
    })
}

/// Preserve structural word views and the observed row link; no envelope or transition is invented.
pub fn event_198(game: Game, bytes: &[u8]) -> (Vec<EventValue>, Option<String>) {
    let read = fromsoft_formats::material_events::Event198::read(game, bytes);
    let mut fields = Vec::new();
    for word in read.words {
        fields.push(EventValue {
            name: format!("material_ex.raw_{:02}_u32", word.offset),
            value: EventValueData::I64(i64::from(word.unsigned)),
        });
        if let Some(value) = word.finite_float {
            fields.push(EventValue {
                name: format!("material_ex.raw_{:02}_f32", word.offset),
                value: EventValueData::F64(f64::from(value)),
            });
        }
    }
    if let Some(id) = read.row_id {
        fields.push(EventValue {
            name: "material_ex.row_id".into(),
            value: EventValueData::I64(i64::from(id)),
        });
    }
    (fields, read.refusal)
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
