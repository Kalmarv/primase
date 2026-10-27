//! Names for hashed spline ids, from the game's own names: the database's flythrough rails and the `NavGuide_` scheme.
//!
//! A spline's id is sdbm of its name. Names the data states: each level session's `FLYTHROUGH_RAIL` in MAIN.DB, and
//! the guide splines, which the navigation sets name `NavGuide_<set><n>` (`<set>` one or two capital letters, the
//! sets `NAV_SET_START` in `<Level>_dropspots` names; `<n>` from 0). Splines whose ids hash none of these stay
//! unnamed; the report counts them.
use crate::database::{Database, Value};
use crate::{Result, invalid};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Sets `A` to `ZZ`, indices 0 to 39: the range that the disc's guide ids fall in.
fn guide_names() -> &'static BTreeMap<u32, String> {
    static NAMES: OnceLock<BTreeMap<u32, String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let letters: Vec<String> = ('A'..='Z')
            .map(String::from)
            .chain(('A'..='Z').flat_map(|a| ('A'..='Z').map(move |b| format!("{a}{b}"))))
            .collect();
        let mut m = BTreeMap::new();
        for set in &letters {
            for n in 0..40 {
                let name = format!("NavGuide_{set}{n}");
                m.insert(primase_foundations::hash::sdbm(name.as_bytes()), name);
            }
        }
        m
    })
}

/// The `NavGuide_<set><n>` name that hashes to `id`.
pub fn guide_name(id: u32) -> Option<&'static str> {
    guide_names().get(&id).map(String::as_str)
}

/// The flythrough rail names of `<level>_sessions` (`level` as the database names it), split at `;`, `,`, `&` and `|`;
/// `-1` and empty entries are no name. `None` when the database has no such table.
pub fn flythrough_rails(db: &Database<'_>, level: &str) -> Result<Option<Vec<String>>> {
    let t = match db.table(&format!("{level}_sessions")) {
        Ok(t) => t,
        Err(crate::database::Error::NoTable(_)) => return Ok(None),
        Err(e) => return Err(invalid(0, format!("database: {e}"))),
    };
    let Some(col) = t.column("FLYTHROUGH_RAIL") else {
        return Err(invalid(0, format!("database: {} has no FLYTHROUGH_RAIL", t.name)));
    };
    let mut out: Vec<String> = Vec::new();
    for r in &t.rows {
        if let Value::Text(s) = &r.values[col] {
            for p in s.split([';', ',', '&', '|']) {
                let p = p.trim();
                if !p.is_empty() && p != "-1" && !out.iter().any(|o| o == p) {
                    out.push(p.to_owned());
                }
            }
        }
    }
    Ok(Some(out))
}
