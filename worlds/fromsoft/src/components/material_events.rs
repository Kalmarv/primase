//! MaterialEx source records. No parsing, shader evaluation or event-envelope policy lives here.
use std::collections::BTreeMap;
use primase_core::{Resource, World};
use serde::Serialize;

/// Stored ER MaterialEx row. RGBA and I are separate source lanes, not a guessed
/// output value or transition curve. Every row byte survives, including unknown fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaterialExRow {
    pub id: i32,
    pub parameter: String,
    pub material_id: i32,
    pub rgba_i: [f32; 5],
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MaterialExRows {
    pub rows: BTreeMap<i32, MaterialExRow>,
    pub refusal: Option<String>,
}
impl MaterialExRows {
    pub fn refused(reason: impl Into<String>) -> Self { Self {refusal:Some(reason.into()),..Default::default()} }
}
impl Resource for MaterialExRows {
    const NAME: &'static str = "fromsoft.material_ex_rows";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn verify(&self, _: &World) -> Result<(),String> {
        if self.refusal.as_ref().is_some_and(|reason|reason.trim().is_empty()) {return Err("empty MaterialEx read refusal".into());}
        for (id,row) in &self.rows {
            if *id!=row.id || row.parameter.trim().is_empty() || row.rgba_i.iter().any(|value|!value.is_finite()) {
                return Err(format!("MaterialEx row {id}: mismatched identity, empty name or non-finite lane"));
            }
        }
        Ok(())
    }
}
