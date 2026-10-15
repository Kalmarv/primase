//! Streamed resources: bulk data (texture pixels, vertex data, audio) kept in a `.resS` or `.resource` file beside
//! the serialized file, named by an object's `StreamingInfo` (`offset`, `size`, `path`).

use std::path::{Path, PathBuf};

use crate::value::Value;
use crate::{Error, Result, invalid};

/// Where an object's streamed data is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamingInfo {
    pub offset: u64,
    pub size: u64,
    pub path: String,
}

impl StreamingInfo {
    /// From a decoded `StreamingInfo` struct; `None` when it names no data (empty path or size 0).
    pub fn of(v: &Value) -> Result<Option<StreamingInfo>> {
        let num = |k: &str| {
            v.get(k)
                .and_then(|x| match x {
                    Value::UInt(u) => Some(*u),
                    Value::Int(i) => u64::try_from(*i).ok(),
                    _ => None,
                })
                .ok_or_else(|| invalid(format!("StreamingInfo: no {k}")))
        };
        let path = v
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("StreamingInfo: no path"))?;
        let (offset, size) = (num("offset")?, num("size")?);
        if path.is_empty() || size == 0 {
            return Ok(None);
        }
        Ok(Some(StreamingInfo {
            offset,
            size,
            path: path.to_owned(),
        }))
    }

    /// The file in `dir` (the data folder) that holds the data: the path's file name (player files name it plainly or
    /// as `archive:/<cab>/<name>`).
    pub fn file_in(&self, dir: &Path) -> Result<PathBuf> {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
            return Err(invalid(format!("streamed resource path {:?}", self.path)));
        }
        Ok(dir.join(name))
    }

    /// Read the data from the file in `dir`, checking it lies inside it.
    pub fn read(&self, dir: &Path) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let path = self.file_in(dir)?;
        let io = |source| Error::Io {
            path: path.clone(),
            source,
        };
        let mut f = std::fs::File::open(&path).map_err(io)?;
        let len = f.metadata().map_err(io)?.len();
        if self.offset.checked_add(self.size).is_none_or(|end| end > len) {
            return Err(invalid(format!(
                "{}: {}+{} past its end ({len} bytes)",
                path.display(),
                self.offset,
                self.size
            )));
        }
        f.seek(SeekFrom::Start(self.offset)).map_err(io)?;
        let mut out = vec![0; self.size as usize];
        f.read_exact(&mut out).map_err(io)?;
        Ok(out)
    }
}

/// Every `StreamingInfo` struct in a decoded value (by its fields), with its path in the value.
pub fn streaming_infos(v: &Value) -> Vec<(String, &Value)> {
    fn walk<'a>(v: &'a Value, path: &str, out: &mut Vec<(String, &'a Value)>) {
        match v {
            Value::Struct(fields) => {
                if v.get("offset").is_some() && v.get("size").is_some() && v.get("path").is_some() {
                    out.push((path.to_owned(), v));
                    return;
                }
                for (k, x) in fields {
                    walk(x, &format!("{path}.{k}"), out);
                }
            }
            Value::Array(crate::value::Array::Values(items)) => {
                for (i, x) in items.iter().enumerate() {
                    walk(x, &format!("{path}[{i}]"), out);
                }
            }
            Value::Pair(p) => {
                walk(&p.0, path, out);
                walk(&p.1, path, out);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, "$", &mut out);
    out
}
