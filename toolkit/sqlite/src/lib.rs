//! A strict, read-only reader of SQLite 3 database files held in memory: the header, the table b-trees (interior and
//! leaf pages, overflow chains), records, and the column names of a table's `CREATE TABLE` text. Games ship small
//! SQLite files as data tables; this reads their rows and nothing else (no SQL, no indexes, no journals).
//!
//! What it does not read is refused with the file offset that shows it, never skipped: a write-ahead-log database, a
//! text encoding other than UTF-8, a virtual or `WITHOUT ROWID` table, a row whose cell count differs from its
//! table's declared columns, a b-tree that reaches a page twice or lists rowids out of order, text that is not valid
//! UTF-8. File-format reference: <https://www.sqlite.org/fileformat2.html>.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use primase_bytes::{Endian, Field, FieldError, View};

/// Why a file was refused: where, and what.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("at {offset:#x}: {detail}")]
    Invalid { offset: usize, detail: String },
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error("no table {0:?}")]
    NoTable(String),
}

pub type Result<T> = std::result::Result<T, Error>;

fn invalid(offset: usize, detail: impl ToString) -> Error {
    Error::Invalid {
        offset,
        detail: detail.to_string(),
    }
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
#[derive(Debug)]
pub struct Database<'a> {
    view: View<'a>,
    page_size: usize,
    /// Page size less the reserved bytes at each page's end.
    usable: usize,
    pages: usize,
    tables: Vec<TableInfo>,
}

const SCHEMA_COLUMNS: [&str; 5] = ["type", "name", "tbl_name", "rootpage", "sql"];

fn schema_columns() -> Vec<Column> {
    SCHEMA_COLUMNS
        .iter()
        .map(|n| Column {
            name: (*n).into(),
            rowid_alias: false,
            real_affinity: false,
        })
        .collect()
}

impl<'a> Database<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self> {
        let view = View::new(data);
        let magic = view.slice(0, 16).field("header")?;
        if magic != b"SQLite format 3\0" {
            return Err(invalid(0, "not an SQLite 3 database"));
        }
        let raw = view.u16(16, Endian::Big).field("page size")?;
        let page_size = match raw {
            1 => 65536,
            n if n >= 512 && n.is_power_of_two() => usize::from(n),
            n => {
                return Err(invalid(
                    16,
                    format!("page size {n} is not a power of two from 512 to 32768"),
                ));
            }
        };
        for (at, name) in [(18, "write version"), (19, "read version")] {
            match view.u8(at).field(name)? {
                1 => {}
                2 => {
                    return Err(invalid(
                        at,
                        format!("{name} 2: a write-ahead-log database is not read"),
                    ));
                }
                n => return Err(invalid(at, format!("{name} {n} is not a known file format"))),
            }
        }
        let reserved = usize::from(view.u8(20).field("reserved bytes")?);
        let usable = page_size - reserved;
        if usable < 480 {
            return Err(invalid(
                20,
                format!("{reserved} reserved bytes leave fewer than 480 usable"),
            ));
        }
        for (at, want) in [(21, 64), (22, 32), (23, 32)] {
            let got = view.u8(at).field("payload fraction")?;
            if got != want {
                return Err(invalid(
                    at,
                    format!("payload fraction {got}, the format fixes {want}"),
                ));
            }
        }
        let encoding = view.u32(56, Endian::Big).field("text encoding")?;
        if encoding != 1 {
            return Err(invalid(
                56,
                format!("text encoding {encoding}: only UTF-8 (1) is read"),
            ));
        }
        if data.is_empty() || !data.len().is_multiple_of(page_size) {
            return Err(invalid(
                data.len(),
                format!("file size is not a whole number of {page_size}-byte pages"),
            ));
        }
        let pages = data.len() / page_size;
        let counter = view.u32(24, Endian::Big).field("change counter")?;
        let valid_for = view.u32(92, Endian::Big).field("version-valid-for")?;
        let stated = view.u32(28, Endian::Big).field("page count")?;
        if counter == valid_for && stated != 0 && stated as usize != pages {
            return Err(invalid(
                28,
                format!("header page count {stated}, file holds {pages}"),
            ));
        }
        let mut db = Database {
            view,
            page_size,
            usable,
            pages,
            tables: Vec::new(),
        };
        let master = db.read_btree(1, &schema_columns())?;
        for row in master {
            let at = db.page_offset(1);
            let [kind, name, _tbl, root, sql] = &row.values[..] else {
                return Err(invalid(at, "schema row without five columns"));
            };
            let Value::Text(kind) = kind else {
                return Err(invalid(at, "schema row type is not text"));
            };
            if kind != "table" {
                continue;
            }
            let (Value::Text(name), Value::Int(root)) = (name, root) else {
                return Err(invalid(at, "schema table row without a name and root page"));
            };
            let sql = match sql {
                Value::Text(s) => s.clone(),
                _ => return Err(invalid(at, format!("table {name:?} has no CREATE text"))),
            };
            let root_page =
                u32::try_from(*root).map_err(|_| invalid(at, format!("table {name:?} root page {root}")))?;
            db.tables.push(TableInfo {
                name: name.clone(),
                root_page,
                sql,
            });
        }
        Ok(db)
    }

    /// The schema's tables, in schema order.
    pub fn tables(&self) -> &[TableInfo] {
        &self.tables
    }

    /// Every row of table `name` (ASCII case-insensitive), in rowid order. `sqlite_master` is the schema table.
    pub fn table(&self, name: &str) -> Result<Table> {
        if name.eq_ignore_ascii_case("sqlite_master") || name.eq_ignore_ascii_case("sqlite_schema") {
            let rows = self.read_btree(1, &schema_columns())?;
            return Ok(Table {
                name: "sqlite_master".into(),
                columns: SCHEMA_COLUMNS.iter().map(|s| (*s).into()).collect(),
                rows,
            });
        }
        let info = self
            .tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::NoTable(name.into()))?;
        if info.root_page == 0 {
            return Err(invalid(0, format!("table {:?} is virtual", info.name)));
        }
        let columns = parse_columns(&info.sql)
            .map_err(|e| invalid(self.page_offset(1), format!("{:?}: {e}", info.name)))?;
        let rows = self.read_btree(info.root_page, &columns)?;
        Ok(Table {
            name: info.name.clone(),
            columns: columns.into_iter().map(|c| c.name).collect(),
            rows,
        })
    }

    fn page_offset(&self, page: u32) -> usize {
        (page as usize - 1) * self.page_size
    }

    /// Depth-first over a table b-tree: leaves left to right, each page visited once, rowids strictly increasing.
    fn read_btree(&self, root: u32, columns: &[Column]) -> Result<Vec<Row>> {
        let mut rows: Vec<Row> = Vec::new();
        let mut seen = vec![false; self.pages];
        let mut stack = vec![root];
        while let Some(page) = stack.pop() {
            let at = self.page_offset_checked(page)?;
            if std::mem::replace(&mut seen[page as usize - 1], true) {
                return Err(invalid(at, format!("page {page} is reached twice")));
            }
            let head = if page == 1 { at + 100 } else { at };
            let kind = self.view.u8(head).field("b-tree page type")?;
            let cells = usize::from(self.view.u16(head + 3, Endian::Big).field("cell count")?);
            match kind {
                0x05 => {
                    let right = self.view.u32(head + 8, Endian::Big).field("right-most child")?;
                    let mut children = Vec::with_capacity(cells + 1);
                    for i in 0..cells {
                        let cell = self.cell_offset(at, head + 12, i)?;
                        children.push(self.view.u32(cell, Endian::Big).field("child page")?);
                        let (_key, _) = self.varint(cell + 4)?;
                    }
                    children.push(right);
                    // the stack is last in, first out: push right to left so the leftmost child pops first
                    stack.extend(children.into_iter().rev());
                }
                0x0d => {
                    for i in 0..cells {
                        let cell = self.cell_offset(at, head + 8, i)?;
                        let row = self.leaf_cell(at, cell, columns)?;
                        if rows.last().is_some_and(|r| r.rowid >= row.rowid) {
                            return Err(invalid(cell, format!("rowid {} does not increase", row.rowid)));
                        }
                        rows.push(row);
                    }
                }
                0x02 | 0x0a => return Err(invalid(head, format!("index b-tree page {kind:#x} in a table"))),
                n => {
                    return Err(invalid(
                        head,
                        format!("page type {n:#x} is not a table b-tree page"),
                    ));
                }
            }
        }
        Ok(rows)
    }

    fn page_offset_checked(&self, page: u32) -> Result<usize> {
        if page == 0 || page as usize > self.pages {
            return Err(invalid(
                0,
                format!("page {page} is outside the file's {} pages", self.pages),
            ));
        }
        Ok(self.page_offset(page))
    }

    /// Absolute offset of cell `i` from the pointer array at `array`, within its page's usable bytes.
    fn cell_offset(&self, page_at: usize, array: usize, i: usize) -> Result<usize> {
        let p = usize::from(self.view.u16(array + 2 * i, Endian::Big).field("cell pointer")?);
        if p < array - page_at + 2 * (i + 1) || p >= self.usable {
            return Err(invalid(
                array + 2 * i,
                format!("cell pointer {p} outside the page's content"),
            ));
        }
        Ok(page_at + p)
    }

    /// A SQLite varint at `at`: value and byte length.
    fn varint(&self, at: usize) -> Result<(u64, usize)> {
        let mut v = 0u64;
        for i in 0..9 {
            let b = self.view.u8(at + i).field("varint")?;
            if i == 8 {
                return Ok(((v << 8) | u64::from(b), 9));
            }
            v = (v << 7) | u64::from(b & 0x7f);
            if b & 0x80 == 0 {
                return Ok((v, i + 1));
            }
        }
        unreachable!("the ninth byte returns")
    }

    fn leaf_cell(&self, page_at: usize, cell: usize, columns: &[Column]) -> Result<Row> {
        let (total, n1) = self.varint(cell)?;
        let (rowid, n2) = self.varint(cell + n1)?;
        let total = usize::try_from(total).map_err(|_| invalid(cell, "payload size overflows"))?;
        let body = cell + n1 + n2;
        // payload split between the page and its overflow chain (file format, "B-tree Pages")
        let u = self.usable;
        let max_local = u - 35;
        let min_local = (u - 12) * 32 / 255 - 23;
        let local = if total <= max_local {
            total
        } else {
            let k = min_local + (total - min_local) % (u - 4);
            if k <= max_local { k } else { min_local }
        };
        if body + local + if total > local { 4 } else { 0 } > page_at + u {
            return Err(invalid(cell, "cell runs past the page's usable bytes"));
        }
        let mut payload = Vec::with_capacity(total);
        payload.extend_from_slice(self.view.slice(body, local).field("cell payload")?);
        if total > local {
            let mut next = self
                .view
                .u32(body + local, Endian::Big)
                .field("first overflow page")?;
            let mut hops = 0usize;
            while payload.len() < total {
                hops += 1;
                if next == 0 || hops > self.pages {
                    return Err(invalid(
                        body + local,
                        "overflow chain ends or loops before the payload is complete",
                    ));
                }
                let at = self.page_offset_checked(next)?;
                let want = (total - payload.len()).min(u - 4);
                next = self.view.u32(at, Endian::Big).field("overflow link")?;
                payload.extend_from_slice(self.view.slice(at + 4, want).field("overflow data")?);
            }
            if next != 0 {
                return Err(invalid(body + local, "overflow chain continues past the payload"));
            }
        }
        let rowid = rowid as i64;
        let values = self
            .record(&payload, cell, columns.len())?
            .into_iter()
            .zip(columns)
            .map(|(v, c)| match v {
                Value::Null if c.rowid_alias => Value::Int(rowid),
                // REAL affinity: a whole-number real is stored as an integer and read back as a real
                Value::Int(i) if c.real_affinity => Value::Real(i as f64),
                v => v,
            })
            .collect();
        Ok(Row { rowid, values })
    }

    /// Decode a record; `at` names the cell in errors.
    fn record(&self, payload: &[u8], at: usize, columns: usize) -> Result<Vec<Value>> {
        let (header_len, n) = varint(payload).ok_or_else(|| invalid(at, "record header size varint"))?;
        let header_len = usize::try_from(header_len).map_err(|_| invalid(at, "record header size"))?;
        if header_len < n || header_len > payload.len() {
            return Err(invalid(
                at,
                format!(
                    "record header size {header_len} outside its {} bytes",
                    payload.len()
                ),
            ));
        }
        let mut types = Vec::new();
        let mut p = n;
        while p < header_len {
            let (t, k) = varint(&payload[p..header_len]).ok_or_else(|| invalid(at, "serial type varint"))?;
            types.push(t);
            p += k;
        }
        if types.len() != columns {
            return Err(invalid(
                at,
                format!("record has {} columns, its table declares {columns}", types.len()),
            ));
        }
        let mut values = Vec::with_capacity(columns);
        let mut q = header_len;
        for t in types {
            let len = match t {
                0 | 8 | 9 => 0,
                1 => 1,
                2 => 2,
                3 => 3,
                4 => 4,
                5 => 6,
                6 | 7 => 8,
                10 | 11 => return Err(invalid(at, format!("reserved serial type {t}"))),
                n => usize::try_from((n - 12) / 2).map_err(|_| invalid(at, "serial type length"))?,
            };
            let bytes = payload
                .get(
                    q..q.checked_add(len)
                        .ok_or_else(|| invalid(at, "value length overflows"))?,
                )
                .ok_or_else(|| invalid(at, "record value runs past its payload"))?;
            q += len;
            values.push(match t {
                0 => Value::Null,
                8 => Value::Int(0),
                9 => Value::Int(1),
                1..=6 => {
                    // big-endian two's complement of `len` bytes
                    let mut v = if bytes[0] & 0x80 != 0 { -1i64 } else { 0 };
                    for &b in bytes {
                        v = (v << 8) | i64::from(b);
                    }
                    Value::Int(v)
                }
                7 => Value::Real(f64::from_be_bytes(bytes.try_into().expect("eight bytes"))),
                n if n % 2 == 0 => Value::Blob(bytes.to_vec()),
                _ => Value::Text(
                    String::from_utf8(bytes.to_vec())
                        .map_err(|_| invalid(at, "text value is not valid UTF-8"))?,
                ),
            });
        }
        if q != payload.len() {
            return Err(invalid(
                at,
                format!("{} bytes after the record's last value", payload.len() - q),
            ));
        }
        Ok(values)
    }
}

/// A varint at the start of `b`: value and length; `None` when `b` ends inside it.
fn varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for i in 0..9 {
        let byte = *b.get(i)?;
        if i == 8 {
            return Some(((v << 8) | u64::from(byte), 9));
        }
        v = (v << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

const CONSTRAINT_WORDS: [&str; 9] = [
    "PRIMARY",
    "NOT",
    "NULL",
    "DEFAULT",
    "UNIQUE",
    "CHECK",
    "COLLATE",
    "REFERENCES",
    "CONSTRAINT",
];

/// A column a `CREATE TABLE` declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// `INTEGER PRIMARY KEY`: the rowid, stored as NULL in the record.
    pub rowid_alias: bool,
    /// The declared type has REAL affinity (SQLite's rules: not INT, CHAR, CLOB, TEXT or BLOB, and REAL, FLOA or
    /// DOUB in it), so integers stored for whole-number values read back as reals.
    pub real_affinity: bool,
}

/// SQLite's column affinity rule over the declared type's words (<https://www.sqlite.org/datatype3.html>, 3.1).
fn has_real_affinity(type_words: &str) -> bool {
    let t = type_words.to_ascii_uppercase();
    !["INT", "CHAR", "CLOB", "TEXT", "BLOB"]
        .iter()
        .any(|k| t.contains(k))
        && ["REAL", "FLOA", "DOUB"].iter().any(|k| t.contains(k))
}

/// The columns of a `CREATE TABLE name (...)` statement, in order. Refused: `AS SELECT`, `WITHOUT ROWID`, virtual
/// tables, generated columns, a table-level primary key over an `INTEGER` column (all which change what a record holds).
pub fn parse_columns(sql: &str) -> std::result::Result<Vec<Column>, String> {
    let tokens = tokenize(sql)?;
    let word = |i: usize| match tokens.get(i) {
        Some(Token::Word(w)) => Some(w.to_ascii_uppercase()),
        _ => None,
    };
    let mut i = 0;
    if word(i).as_deref() != Some("CREATE") {
        return Err("not a CREATE statement".into());
    }
    i += 1;
    if matches!(word(i).as_deref(), Some("TEMP" | "TEMPORARY")) {
        i += 1;
    }
    if word(i).as_deref() != Some("TABLE") {
        return Err(format!("not a CREATE TABLE ({:?})", word(i)));
    }
    i += 1;
    if word(i).as_deref() == Some("IF") {
        i += 3; // IF NOT EXISTS
    }
    // [schema .] name
    if !matches!(tokens.get(i), Some(Token::Word(_) | Token::Ident(_))) {
        return Err("no table name".into());
    }
    i += 1;
    if tokens.get(i) == Some(&Token::Punct('.')) {
        i += 2;
    }
    if tokens.get(i) != Some(&Token::Punct('(')) {
        return Err("no column list (an AS SELECT or virtual table)".into());
    }
    i += 1;
    // split the column list at top-level commas
    let mut defs: Vec<&[Token]> = Vec::new();
    let (mut depth, mut start) = (0usize, i);
    let mut close = None;
    for (j, t) in tokens.iter().enumerate().skip(i) {
        match t {
            Token::Punct('(') => depth += 1,
            Token::Punct(')') if depth == 0 => {
                defs.push(&tokens[start..j]);
                close = Some(j);
                break;
            }
            Token::Punct(')') => depth -= 1,
            Token::Punct(',') if depth == 0 => {
                defs.push(&tokens[start..j]);
                start = j + 1;
            }
            _ => {}
        }
    }
    let close = close.ok_or("column list is not closed")?;
    if close + 1 != tokens.len() {
        return Err(format!(
            "table options after the column list ({:?})",
            tokens[close + 1]
        ));
    }
    let mut columns: Vec<Column> = Vec::new();
    let mut table_pk: Vec<String> = Vec::new();
    let mut integers: Vec<bool> = Vec::new();
    for def in defs {
        let Some(first) = def.first() else {
            return Err("empty column definition".into());
        };
        if let Token::Word(w) = first {
            let w = w.to_ascii_uppercase();
            if matches!(
                w.as_str(),
                "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN"
            ) {
                if w == "PRIMARY" {
                    table_pk.extend(def.iter().filter_map(|t| match t {
                        Token::Word(n) | Token::Ident(n) => Some(n.clone()),
                        _ => None,
                    }));
                }
                continue;
            }
        }
        let (Token::Word(name) | Token::Ident(name)) = first else {
            return Err(format!("column name expected, found {first:?}"));
        };
        let rest = &def[1..];
        let upper: Vec<String> = rest
            .iter()
            .map(|t| match t {
                Token::Word(w) => w.to_ascii_uppercase(),
                _ => String::new(),
            })
            .collect();
        if upper.iter().any(|w| w == "GENERATED" || w == "AS") {
            return Err(format!("generated column {name:?}"));
        }
        // the declared type: the words before the first constraint
        let type_words = upper
            .iter()
            .position(|w| CONSTRAINT_WORDS.contains(&w.as_str()) || w.is_empty())
            .unwrap_or(upper.len());
        let type_text = rest[..type_words]
            .iter()
            .map(|t| match t {
                Token::Word(w) | Token::Ident(w) => w.as_str(),
                _ => "",
            })
            .collect::<Vec<_>>()
            .join(" ");
        let integer = type_words == 1 && upper[0] == "INTEGER";
        let pk = upper.windows(2).position(|w| w[0] == "PRIMARY" && w[1] == "KEY");
        if let Some(p) = pk
            && integer
            && upper.get(p + 2).is_some_and(|w| w == "DESC")
        {
            return Err(format!("column {name:?}: INTEGER PRIMARY KEY DESC"));
        }
        columns.push(Column {
            name: name.clone(),
            rowid_alias: pk.is_some() && integer,
            real_affinity: has_real_affinity(&type_text),
        });
        integers.push(integer);
    }
    for pk in &table_pk {
        if let Some(i) = columns.iter().position(|c| c.name.eq_ignore_ascii_case(pk))
            && integers[i]
        {
            return Err(format!("table-level primary key over INTEGER column {pk:?}"));
        }
    }
    if columns.is_empty() {
        return Err("no columns".into());
    }
    Ok(columns)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    /// A quoted identifier or string.
    Ident(String),
    Punct(char),
    Other,
}

fn tokenize(sql: &str) -> std::result::Result<Vec<Token>, String> {
    let c: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            _ if ch.is_whitespace() => i += 1,
            '-' if c.get(i + 1) == Some(&'-') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '"' | '`' | '\'' | '[' => {
                let close = if ch == '[' { ']' } else { ch };
                let mut s = String::new();
                i += 1;
                loop {
                    let Some(&x) = c.get(i) else {
                        return Err("unterminated quoted name".into());
                    };
                    i += 1;
                    if x == close {
                        if ch != '[' && c.get(i) == Some(&close) {
                            s.push(close);
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    s.push(x);
                }
                out.push(Token::Ident(s));
            }
            '(' | ')' | ',' | '.' => {
                out.push(Token::Punct(ch));
                i += 1;
            }
            _ if ch.is_alphanumeric() || ch == '_' => {
                let s = i;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '$') {
                    i += 1;
                }
                out.push(Token::Word(c[s..i].iter().collect()));
            }
            _ => {
                out.push(Token::Other);
                i += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(sql: &str) -> Vec<String> {
        parse_columns(sql).unwrap().into_iter().map(|c| c.name).collect()
    }

    #[test]
    fn column_lists() {
        assert_eq!(names("CREATE TABLE t (id,SORT,NAME)"), ["id", "SORT", "NAME"]);
        assert_eq!(
            names(
                "CREATE TABLE \"a b\" (\"x y\" TEXT, [z] varchar(10) NOT NULL DEFAULT (1,2), `w`, PRIMARY KEY (z, w))"
            ),
            ["x y", "z", "w"]
        );
        let c = parse_columns("create table t(a INTEGER PRIMARY KEY, b INT PRIMARY KEY, c)").unwrap();
        assert_eq!(
            c.iter().map(|c| c.rowid_alias).collect::<Vec<_>>(),
            [true, false, false]
        );
    }

    #[test]
    fn refused_declarations() {
        for sql in [
            "CREATE TABLE t AS SELECT 1",
            "CREATE TABLE t (a, b) WITHOUT ROWID",
            "CREATE VIRTUAL TABLE t USING fts5(a)",
            "CREATE TABLE t (a, b INTEGER GENERATED ALWAYS AS (a))",
            "CREATE TABLE t (a INTEGER PRIMARY KEY DESC)",
            "CREATE TABLE t (a",
            "SELECT 1",
        ] {
            assert!(parse_columns(sql).is_err(), "{sql}");
        }
    }

    #[test]
    fn varints() {
        assert_eq!(varint(&[0x00]), Some((0, 1)));
        assert_eq!(varint(&[0x81, 0x00]), Some((128, 2)));
        assert_eq!(varint(&[0xff; 9]), Some((u64::MAX, 9)));
        assert_eq!(varint(&[0x81]), None);
    }

    #[test]
    fn header_refusals() {
        assert!(Database::open(&[]).is_err());
        assert!(Database::open(b"SQLite format 4\0").is_err());
        let mut page = vec![0u8; 512];
        page[..16].copy_from_slice(b"SQLite format 3\0");
        page[16..18].copy_from_slice(&512u16.to_be_bytes());
        page[18] = 2;
        page[19] = 2;
        page[21..24].copy_from_slice(&[64, 32, 32]);
        page[56..60].copy_from_slice(&1u32.to_be_bytes());
        let err = Database::open(&page).unwrap_err();
        assert!(err.to_string().contains("write-ahead"), "{err}");
    }
}
