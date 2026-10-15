//! A Unity player's data folder: its SerializedFiles by name, pointers (`PPtr`) resolved across them, objects decoded
//! by their classes' type trees.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::serialized::SerializedFile;
use crate::tpk::TypeTreePackage;
use crate::typetree::TypeTree;
use crate::value::Value;
use crate::{Error, Result, invalid};

/// An object of the player: its file (lower-case name) and path id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    pub file: String,
    pub path_id: i64,
}

#[derive(Debug)]
pub struct Player {
    pub dir: PathBuf,
    /// By lower-case file name: `level1`, `sharedassets1.assets`, `unity default resources`, ...
    pub files: BTreeMap<String, SerializedFile>,
    /// The folder each file is in (its streamed resources are beside it).
    pub folders: BTreeMap<String, PathBuf>,
    package: TypeTreePackage,
    trees: Mutex<HashMap<(String, i32), std::sync::Arc<TypeTree>>>,
}

fn file_key(path: &str) -> String {
    path.replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

impl Player {
    /// The serialized files of `dir` (the `<Game>_Data` folder) and of its `Resources` folder, read with `package`'s
    /// class layouts. Files of other kinds (resources, bundles, configs) are skipped.
    pub fn open(dir: &Path, package: TypeTreePackage) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut folders = BTreeMap::new();
        for d in [dir.to_path_buf(), dir.join("Resources")] {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
            paths.sort();
            for p in paths {
                let bytes = std::fs::read(&p).map_err(|source| Error::Io {
                    path: p.clone(),
                    source,
                })?;
                if !SerializedFile::looks_like(&bytes) {
                    continue;
                }
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let f = SerializedFile::parse(bytes).map_err(|e| invalid(format!("{}: {e}", p.display())))?;
                folders.insert(name.clone(), d.clone());
                files.insert(name, f);
            }
        }
        Ok(Player {
            dir: dir.to_path_buf(),
            files,
            folders,
            package,
            trees: Mutex::new(HashMap::new()),
        })
    }

    pub fn file(&self, name: &str) -> Result<&SerializedFile> {
        self.files
            .get(name)
            .ok_or_else(|| invalid(format!("no serialized file {name:?} in the player")))
    }

    /// The class id of an object (`None`: no such object).
    pub fn class_id(&self, r: &ObjRef) -> Option<i32> {
        self.files.get(&r.file)?.object(r.path_id).map(|o| o.class_id)
    }

    /// The class name of an object, from the type tree package.
    pub fn class_name(&self, r: &ObjRef) -> Option<&str> {
        let f = self.files.get(&r.file)?;
        self.package.class_name(f.object(r.path_id)?.class_id, f.unity)
    }

    fn tree(&self, file: &str, f: &SerializedFile, class_id: i32) -> Result<std::sync::Arc<TypeTree>> {
        let key = (f.unity_version.clone(), class_id);
        let mut cache = self.trees.lock().expect("type tree cache");
        if let Some(t) = cache.get(&key) {
            return Ok(t.clone());
        }
        let t = std::sync::Arc::new(
            self.package
                .type_tree(class_id, f.unity)
                .map_err(|e| invalid(format!("{file}: {e}")))?,
        );
        cache.insert(key, t.clone());
        Ok(t)
    }

    /// Decode an object (its type tree must cover it; a MonoBehaviour's base fields only).
    pub fn decode(&self, r: &ObjRef) -> Result<Value> {
        let f = self.file(&r.file)?;
        let o = f
            .object(r.path_id)
            .ok_or_else(|| invalid(format!("{}: no object {}", r.file, r.path_id)))?;
        let d = f.decode_with(o, &*self.tree(&r.file, f, o.class_id)?)?;
        if !d.covered(o.class_id) {
            return Err(invalid(format!(
                "{}/{} (class {}): {} bytes after its type tree",
                r.file, r.path_id, o.class_id, d.rest
            )));
        }
        Ok(d.value)
    }

    /// The object a `PPtr` (`m_FileID`, `m_PathID`) found in `file` points at; `None` for a null pointer.
    pub fn resolve(&self, file: &str, pptr: &Value) -> Result<Option<ObjRef>> {
        let fid = pptr
            .get("m_FileID")
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("PPtr without m_FileID"))?;
        let pid = pptr
            .get("m_PathID")
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("PPtr without m_PathID"))?;
        if pid == 0 {
            return Ok(None);
        }
        let target = if fid == 0 {
            file.to_owned()
        } else {
            let f = self.file(file)?;
            let ext = usize::try_from(fid - 1)
                .ok()
                .and_then(|i| f.externals.get(i))
                .ok_or_else(|| {
                    invalid(format!(
                        "{file}: PPtr file id {fid} of {} externals",
                        f.externals.len()
                    ))
                })?;
            file_key(&ext.path)
        };
        Ok(Some(ObjRef {
            file: target,
            path_id: pid,
        }))
    }
}
