//! Display-only structured read reports. Programs render the fields generically and do not infer game semantics.
use serde::{Deserialize, Serialize};

/// A reader's descriptive card; it carries no selection or rendering decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportCard {
    pub title: String,
    pub description: String,
    pub fields: Vec<ReportField>,
}

/// One named value and its source-stated unit, in the reader's order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportField {
    pub name: String,
    pub value: serde_json::Value,
    pub unit: Option<String>,
}
