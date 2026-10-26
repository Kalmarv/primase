//! A level's parameter script: the calls of `script\vfx_level_load_<level>.lua` with literal arguments.
//!
//! The scripts are Lua 5.1 bytecode in the scripts archive (which the game stores encrypted; decrypting it is the
//! caller's, outside primase). A level's script sets its look by calling engine functions with numbers and strings. A
//! call is read when its callee is a global (or a path of fields) and every argument is a literal; the rest are
//! counted in the report with why. A script with control flow is not read at all: which of its calls run depends on
//! values this reader does not evaluate.
use super::pak::Pak;
use crate::{
    Result,
    components::{LevelParams, ScriptCall, ScriptValue},
    invalid,
};
use primase_lua51::{Chunk, Op, Value, calls};

/// The scripts archive and the level whose script to read (`charlie`, `mike`, ...).
#[derive(Clone, Copy, Debug)]
pub struct ScriptInputs<'a> {
    pub scripts: &'a Pak,
    pub level: &'a str,
}

#[derive(Clone, Debug, Default)]
pub struct ParamsReport {
    pub level: String,
    /// The script's name when the archive has one.
    pub script: Option<String>,
    pub read: usize,
    /// Calls not read, each with the instruction it is at and why.
    pub unread: Vec<(usize, String)>,
    /// The script has control flow or nested functions, so none of its calls were taken.
    pub branching: bool,
}

impl ParamsReport {
    pub fn lines(&self) -> primase_core::convert::Report {
        let text = match (&self.script, self.branching) {
            (None, _) => format!("no vfx_level_load script for level {}", self.level),
            (Some(s), true) => {
                format!("{s} has control flow or nested functions; none of its calls are read")
            }
            (Some(s), false) => {
                let mut t = format!(
                    "{s}: {} calls read as data, {} not read",
                    self.read,
                    self.unread.len()
                );
                for (pc, why) in &self.unread {
                    t.push_str(&format!("; instruction {pc}: {why}"));
                }
                t
            }
        };
        vec![("shred level params".into(), text)]
    }
}

fn name_of(v: &Value) -> Option<String> {
    match v {
        Value::Global(n) => String::from_utf8(n.clone()).ok(),
        Value::Field(t, k) => match &**k {
            Value::Str(k) => Some(format!("{}.{}", name_of(t)?, String::from_utf8(k.clone()).ok()?)),
            _ => None,
        },
        _ => None,
    }
}

fn literal(v: &Value) -> std::result::Result<ScriptValue, String> {
    match v {
        Value::Nil => Ok(ScriptValue::Nil),
        Value::Bool(b) => Ok(ScriptValue::Bool(*b)),
        Value::Number(n) if n.is_finite() => Ok(ScriptValue::Number(*n)),
        Value::Number(n) => Err(format!("argument {n} is not finite")),
        Value::Str(s) => String::from_utf8(s.clone())
            .map(ScriptValue::Text)
            .map_err(|_| "a string argument that is not UTF-8".into()),
        _ => Err("an argument that is not a literal".into()),
    }
}

/// The script of `inputs.level`, if the archive has one.
pub fn read_level_params(inputs: &ScriptInputs) -> Result<(Option<LevelParams>, ParamsReport)> {
    let wanted = format!("script\\vfx_level_load_{}.lua", inputs.level);
    let mut report = ParamsReport {
        level: inputs.level.to_owned(),
        ..ParamsReport::default()
    };
    let Some((index, entry)) = inputs
        .scripts
        .entries()
        .iter()
        .enumerate()
        .find(|(_, e)| e.name.eq_ignore_ascii_case(wanted.as_bytes()))
    else {
        return Ok((None, report));
    };
    let script = String::from_utf8_lossy(&entry.name).into_owned();
    report.script = Some(script.clone());
    let payload = inputs.scripts.payload(index)?;
    let chunk = Chunk::parse(&payload).map_err(|e| invalid(e.offset, format!("{script}: {}", e.detail)))?;
    let main = &chunk.main;
    report.branching = !main.protos.is_empty()
        || main.code.iter().any(|i| {
            matches!(
                i.op,
                Op::Jmp
                    | Op::Eq
                    | Op::Lt
                    | Op::Le
                    | Op::Test
                    | Op::TestSet
                    | Op::ForLoop
                    | Op::ForPrep
                    | Op::TForLoop
                    | Op::Closure
            )
        });
    if report.branching {
        return Ok((None, report));
    }
    let mut out = Vec::new();
    for call in calls(main) {
        let read = (|| {
            if call.open_args {
                return Err("an argument list of unknown length".to_string());
            }
            let function =
                name_of(&call.callee).ok_or_else(|| "a callee that is not a global".to_string())?;
            let args = call
                .args
                .iter()
                .map(literal)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(ScriptCall { function, args })
        })();
        match read {
            Ok(c) => out.push(c),
            Err(why) => report.unread.push((call.pc, why)),
        }
    }
    report.read = out.len();
    let params = LevelParams {
        level: inputs.level.to_owned(),
        script,
        calls: out,
    };
    Ok((Some(params), report))
}
