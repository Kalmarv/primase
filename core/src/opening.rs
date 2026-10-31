//! Portable alternative read requests produced by readers or registered selection passes.
use crate::{JsonError, Resource};
use serde::Serialize;
/// An explicit alternative request. Presentation text is data, never interpreted by clients.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpeningSuggestion {
    pub label: String,
    pub reasons: Vec<String>,
    /// Public reader name, independent of an application's aliases.
    pub reader: String,
    pub options: serde_json::Value,
    /// Caller options that retain their stated values when following this alternative.
    pub inherit: Vec<String>,
}
/// Alternatives for a read that has no drawable geometry. Clients may offer these generically.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OpeningSuggestions(pub Vec<OpeningSuggestion>);
impl Resource for OpeningSuggestions {
    const NAME: &'static str = "core.opening_suggestions";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}
