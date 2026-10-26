//! MAIN.DB and other SQLite 3 files the game ships as data tables, read whole: a table's columns and every row's
//! typed values. The file format is read by `quantydb-sqlite` (pure Rust, no unsafe, no dependencies); this module is
//! the one place that crate is named, and gives callers a table read whole with strict refusals.
//!
//! Refused, never skipped: a write-ahead-log database, a virtual or `WITHOUT ROWID` table, a row whose value count
//! differs from its table's declared columns, a b-tree that reaches a page twice or lists rowids out of order, text
//! that is not valid in the file's encoding, a truncated file or a page outside it. File-format reference:
//! <https://www.sqlite.org/fileformat2.html>.
use quantydb_sqlite::{
    Affinity, ObjectKind, Reader, SchemaObject, SliceSource, SqliteError, SqliteValue, TableDef,
};

/// Why a file was refused, and what.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The file or one of its tables is unreadable or uses something this module does not read; the detail names the
    /// page where the file has one.
    #[error("{0}")]
    Invalid(String),
    #[error("no table {0:?}")]
    NoTable(String),
}

pub type Result<T> = std::result::Result<T, Error>;

fn invalid(detail: impl ToString) -> Error {
    Error::Invalid(detail.to_string())
}

/// One stored value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl From<SqliteValue> for Value {
    fn from(v: SqliteValue) -> Self {
        match v {
            SqliteValue::Null => Self::Null,
            SqliteValue::Integer(i) => Self::Int(i),
            SqliteValue::Real(r) => Self::Real(r),
            SqliteValue::Text(t) => Self::Text(t),
            SqliteValue::Blob(b) => Self::Blob(b),
        }
    }
}

/// One row: its rowid and one value per declared column (an `INTEGER PRIMARY KEY` column holds the rowid).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub rowid: i64,
    pub values: Vec<Value>,
}

/// A table read whole.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

impl Table {
    /// The index of a column by name (ASCII case-insensitive, as SQLite compares identifiers).
    pub fn column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.eq_ignore_ascii_case(name))
    }
}

/// A table the schema lists: where its b-tree is and the `CREATE TABLE` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    pub root_page: u32,
    pub sql: String,
}

/// An opened file: the header checked and the schema's tables listed.
pub struct Database<'a> {
    reader: Reader<SliceSource<'a>>,
    tables: Vec<TableInfo>,
}

impl std::fmt::Debug for Database<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("tables", &self.tables)
            .finish_non_exhaustive()
    }
}

const SCHEMA_COLUMNS: [&str; 5] = ["type", "name", "tbl_name", "rootpage", "sql"];

impl<'a> Database<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self> {
        let reader = Reader::open(SliceSource::new(data)).map_err(invalid)?;
        let schema = reader.schema().map_err(invalid)?;
        let mut tables = Vec::new();
        for object in schema.objects().iter().filter(|o| o.kind == ObjectKind::Table) {
            let (Some(root_page), Some(sql)) = (object.root_page, object.sql.clone()) else {
                return Err(invalid(format!(
                    "table {:?} has no root page and CREATE text",
                    object.name
                )));
            };
            tables.push(TableInfo {
                name: object.name.clone(),
                root_page,
                sql,
            });
        }
        Ok(Self { reader, tables })
    }

    /// The schema's tables, in schema order.
    pub fn tables(&self) -> &[TableInfo] {
        &self.tables
    }

    /// Every row of table `name` (ASCII case-insensitive), in rowid order. `sqlite_master` is the schema table.
    pub fn table(&self, name: &str) -> Result<Table> {
        if name.eq_ignore_ascii_case("sqlite_master") || name.eq_ignore_ascii_case("sqlite_schema") {
            let columns = SCHEMA_COLUMNS.map(String::from).to_vec();
            return self.scan("sqlite_master", 1, columns, None, &[]);
        }
        let info = self
            .tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::NoTable(name.into()))?;
        let def = definition(info)?;
        let alias = def
            .rowid_alias()
            .and_then(|a| def.columns.iter().position(|c| c.name == a.name));
        let real: Vec<bool> = def
            .columns
            .iter()
            .map(|c| Affinity::of(c.declared_type.as_deref()) == Affinity::Real)
            .collect();
        let columns = def.columns.into_iter().map(|c| c.name).collect();
        self.scan(&info.name, info.root_page, columns, alias, &real)
    }

    /// Walk a table b-tree, checking each row's width and applying the declared columns' rules: the rowid alias
    /// column (stored as NULL) reads as the rowid, and an integer stored in a REAL-affinity column reads as a real.
    fn scan(
        &self,
        name: &str,
        root: u32,
        columns: Vec<String>,
        alias: Option<usize>,
        real: &[bool],
    ) -> Result<Table> {
        let mut rows = Vec::new();
        for row in self.reader.table_scan(root).map_err(invalid)? {
            let row = row.map_err(invalid)?;
            if row.values.len() != columns.len() {
                return Err(invalid(format!(
                    "table {name:?} row {}: {} values, {} declared columns",
                    row.rowid,
                    row.values.len(),
                    columns.len()
                )));
            }
            let values = row
                .values
                .into_iter()
                .enumerate()
                .map(|(i, v)| match Value::from(v) {
                    Value::Null if alias == Some(i) => Value::Int(row.rowid),
                    // REAL affinity: a whole-number real is stored as an integer and read back as a real
                    #[allow(clippy::cast_precision_loss)]
                    Value::Int(n) if real.get(i) == Some(&true) => Value::Real(n as f64),
                    v => v,
                })
                .collect();
            rows.push(Row {
                rowid: row.rowid,
                values,
            });
        }
        Ok(Table {
            name: name.into(),
            columns,
            rows,
        })
    }
}

/// The column list of a table's `CREATE TABLE`, refusing a `WITHOUT ROWID` table (its records hold the key, not a
/// rowid).
fn definition(info: &TableInfo) -> Result<TableDef> {
    let object = SchemaObject {
        kind: ObjectKind::Table,
        name: info.name.clone(),
        table_name: info.name.clone(),
        root_page: Some(info.root_page),
        sql: Some(info.sql.clone()),
    };
    let def = object
        .table_def()
        .map_err(|e| invalid(format!("table {:?}: {e}", info.name)))?;
    if def.without_rowid {
        return Err(invalid(format!("table {:?} is WITHOUT ROWID", info.name)));
    }
    Ok(def)
}
