use primase_core::{JsonError, Resource};
use serde::Serialize;

/// A literal argument of a level script's call.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum ScriptValue {
    Nil,
    Bool(bool),
    Number(f64),
    Text(String),
}

/// One call of a level script as the script makes it: the callee as the global it names (`change_fog`) or a path
/// through fields (`GAME.Level.GetLevelManager`), and its arguments, each a literal.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScriptCall {
    pub function: String,
    pub args: Vec<ScriptValue>,
}

/// The parameters one level's `vfx_level_load_<level>` script sets (fog, shadow, bloom, back light, LUT, ...), as the
/// script's calls in order. Nothing here says what a call's arguments mean or what unit they are in; that is the
/// engine function's, and only the proven ones are lowered. Not a core `Environment` or `Atmosphere`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LevelParams {
    /// The level the caller named.
    pub level: String,
    /// The script's name in the archive, as stored.
    pub script: String,
    pub calls: Vec<ScriptCall>,
}

impl LevelParams {
    /// The calls of `function`, in script order.
    pub fn calls_to<'a>(&'a self, function: &'a str) -> impl Iterator<Item = &'a ScriptCall> {
        self.calls.iter().filter(move |c| c.function == function)
    }
}

impl Resource for LevelParams {
    const NAME: &'static str = "shred.level_params";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
