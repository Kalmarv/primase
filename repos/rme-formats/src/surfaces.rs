//! MAIN.DB's `surfaces` table: the names collision surfaces are numbered by.
use crate::database::{Database, Value};
use crate::{Result, invalid};

/// The `surfaces` table: the name of each id (`surfaces.id`, 0..n in order), so an id is an index into the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceTable(pub Vec<String>);

impl SurfaceTable {
    /// Read the `surfaces` table of the SQLite file `db`. Its ids must be 0, 1, 2, ... in row order.
    pub fn from_main_db(db: &[u8]) -> Result<Self> {
        let table = Database::open(db)
            .and_then(|d| d.table("surfaces"))
            .map_err(|e| invalid(0, format!("MAIN.DB: {e}")))?;
        let (Some(id), Some(name)) = (table.column("id"), table.column("NAME")) else {
            return Err(invalid(0, "MAIN.DB surfaces has no id and NAME columns"));
        };
        let mut names = Vec::new();
        for row in &table.rows {
            let (Some(Value::Text(i)), Some(Value::Text(n))) = (row.values.get(id), row.values.get(name))
            else {
                return Err(invalid(
                    0,
                    format!("MAIN.DB surfaces row {}: id and NAME are not text", row.rowid),
                ));
            };
            if i.parse::<usize>().ok() != Some(names.len()) {
                return Err(invalid(
                    0,
                    format!(
                        "MAIN.DB surfaces row {}: id {i:?} is not {}",
                        row.rowid,
                        names.len()
                    ),
                ));
            }
            names.push(n.clone());
        }
        Ok(Self(names))
    }
}
