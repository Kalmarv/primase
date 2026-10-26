//! MAIN.DB's level/session/goal/task rows. This reader records relationships without activating them.
use super::database::{Database, Table, Value};
use crate::{
    Result,
    components::{SessionGraph, SessionIdentifier, SessionRow},
    invalid,
};
use std::collections::BTreeMap;

fn text<'a>(row: &'a SessionRow, column: &str, table: &str) -> Result<&'a str> {
    row.fields.get(column).and_then(Option::as_deref).ok_or_else(|| {
        invalid(
            0,
            format!("MAIN.DB {table} row {}: missing text {column}", row.rowid),
        )
    })
}

fn rows(table: &Table) -> Result<Vec<SessionRow>> {
    if table.column("id").is_none() {
        return Err(invalid(0, format!("MAIN.DB {}: no id column", table.name)));
    }
    let mut out = Vec::new();
    for row in &table.rows {
        let mut fields = BTreeMap::new();
        for (column, value) in table.columns.iter().zip(&row.values) {
            let value = match value {
                Value::Text(s) => Some(s.clone()),
                Value::Null => None,
                _ => {
                    return Err(invalid(
                        0,
                        format!(
                            "MAIN.DB {} row {} column {column}: expected TEXT or NULL",
                            table.name, row.rowid
                        ),
                    ));
                }
            };
            if fields.insert(column.clone(), value).is_some() {
                return Err(invalid(
                    0,
                    format!("MAIN.DB {}: duplicate column {column}", table.name),
                ));
            }
        }
        let mut source = SessionRow {
            rowid: row.rowid,
            id: 0,
            fields,
            identifiers: Vec::new(),
        };
        source.id = text(&source, "id", &table.name)?.parse::<u32>().map_err(|_| {
            invalid(
                0,
                format!("MAIN.DB {} row {}: id is not u32", table.name, row.rowid),
            )
        })?;
        if let Some(Some(cell)) = source.fields.get("IDENTIFIERS") {
            // The native parser splits on '&' and hashes exact bytes; no lowercasing or semantic aliases.
            if !cell.is_empty()
                && cell != "-1"
                && !cell.eq_ignore_ascii_case("none")
                && !cell.eq_ignore_ascii_case("null")
            {
                source.identifiers = cell
                    .split('&')
                    .filter(|s| !s.is_empty())
                    .map(|name| SessionIdentifier {
                        name: name.into(),
                        hash: primase_hash::sdbm(name.as_bytes()),
                    })
                    .collect();
            }
        }
        out.push(source);
    }
    Ok(out)
}

impl SessionGraph {
    /// Read the caller's explicit levels.id and its NAME-prefixed tables. All rows/cells are retained;
    /// source expressions and dangling references are data, not repaired or selected here.
    pub fn from_main_db(bytes: &[u8], level_id: u32) -> Result<Self> {
        let db = Database::open(bytes).map_err(|e| invalid(0, format!("MAIN.DB: {e}")))?;
        let levels = db
            .table("levels")
            .map_err(|e| invalid(0, format!("MAIN.DB: {e}")))?;
        let candidates: Vec<_> = rows(&levels)?.into_iter().filter(|r| r.id == level_id).collect();
        let [level] = candidates.as_slice() else {
            return Err(invalid(
                0,
                format!(
                    "MAIN.DB levels id {level_id}: {} row candidates",
                    candidates.len()
                ),
            ));
        };
        let level = level.clone();
        let prefix = text(&level, "NAME", "levels")?;
        let session_table = format!("{prefix}_sessions");
        let goal_table = format!("{prefix}_goals");
        let task_table = format!("{prefix}_tasks");
        let read = |name: &str| -> Result<_> {
            rows(&db.table(name).map_err(|e| invalid(0, format!("MAIN.DB: {e}")))?)
        };
        let sessions = read(&session_table)?;
        let goals = read(&goal_table)?;
        let tasks = read(&task_table)?;
        Ok(Self {
            level_id,
            level,
            session_table,
            goal_table,
            task_table,
            sessions,
            goals,
            tasks,
        })
    }
}
