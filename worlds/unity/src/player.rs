//! A Unity player's data folder: its SerializedFiles by name, pointers (`PPtr`) resolved across them, objects decoded
//! by their classes' type trees.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the player's data folder"
)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::serialized::SerializedFile;
use crate::tpk::TypeTreePackage;
use crate::typetree::TypeTree;
use crate::value::Value;
use crate::{Error, Result, invalid};

/// The script class of Universal Render Pipeline's asset (`m_Namespace.m_ClassName` of its MonoScript).
pub const URP_ASSET: &str = "UnityEngine.Rendering.Universal.UniversalRenderPipelineAsset";

/// The render pipeline a player draws with: GraphicsSettings' default pipeline asset (`m_CustomRenderPipeline`), or a
/// quality level's own (`customRenderPipeline`) where it sets one. Which quality level runs is the user's choice, so
/// the pipeline is known only when every level gives the same kind.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum RenderPipeline {
    /// No pipeline asset for any quality level: Unity's built-in renderer.
    BuiltIn,
    /// A Universal Render Pipeline asset ([`URP_ASSET`]) for every quality level.
    Universal,
    /// Anything else, or a mix: each level's asset script class (`built-in` for none), sorted, each once.
    Other(Vec<String>),
}

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

    /// A MonoBehaviour's base fields and its script's serialized bytes, which a stripped build has no type tree for.
    pub fn behaviour(&self, r: &ObjRef) -> Result<(Value, Vec<u8>)> {
        let f = self.file(&r.file)?;
        let o = f
            .object(r.path_id)
            .ok_or_else(|| invalid(format!("{}: no object {}", r.file, r.path_id)))?;
        if o.class_id != crate::serialized::MONO_BEHAVIOUR {
            return Err(invalid(format!("{}/{}: not a MonoBehaviour", r.file, r.path_id)));
        }
        let d = f.decode_with(o, &*self.tree(&r.file, f, o.class_id)?)?;
        let bytes = f.bytes(o);
        Ok((d.value, bytes[bytes.len() - d.rest..].to_vec()))
    }

    /// The player's [`RenderPipeline`], from `globalgamemanagers`' GraphicsSettings and QualitySettings; `None` when
    /// the player has no GraphicsSettings.
    pub fn render_pipeline(&self) -> Result<Option<RenderPipeline>> {
        const FILE: &str = "globalgamemanagers";
        let Some(f) = self.files.get(FILE) else {
            return Ok(None);
        };
        let one = |class: &str| -> Result<Option<Value>> {
            let mut found = None;
            for o in &f.objects {
                let r = ObjRef {
                    file: FILE.into(),
                    path_id: o.path_id,
                };
                if self.class_name(&r) == Some(class) {
                    if found.is_some() {
                        return Err(invalid(format!("{FILE}: more than one {class}")));
                    }
                    found = Some(self.decode(&r)?);
                }
            }
            Ok(found)
        };
        let Some(graphics) = one("GraphicsSettings")? else {
            return Ok(None);
        };
        // a pipeline asset's script class; `None` for no asset
        let class = |pptr: Option<&Value>| -> Result<Option<String>> {
            let Some(asset) = pptr.map(|p| self.resolve(FILE, p)).transpose()?.flatten() else {
                return Ok(None);
            };
            let (base, _) = self.behaviour(&asset)?;
            let script = base
                .get("m_Script")
                .map(|p| self.resolve(&asset.file, p))
                .transpose()?
                .flatten()
                .ok_or_else(|| {
                    invalid(format!(
                        "{}/{}: pipeline asset without a script",
                        asset.file, asset.path_id
                    ))
                })?;
            let s = self.decode(&script)?;
            let field = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("");
            Ok(Some(match field("m_Namespace") {
                "" => field("m_ClassName").to_owned(),
                ns => format!("{ns}.{}", field("m_ClassName")),
            }))
        };
        let default = class(graphics.get("m_CustomRenderPipeline"))?;
        let mut levels = Vec::new();
        if let Some(Value::Array(crate::value::Array::Values(qs))) = one("QualitySettings")?
            .as_ref()
            .and_then(|q| q.get("m_QualitySettings"))
        {
            for q in qs {
                levels.push(class(q.get("customRenderPipeline"))?.or_else(|| default.clone()));
            }
        }
        if levels.is_empty() {
            levels.push(default);
        }
        Ok(Some(if levels.iter().all(Option::is_none) {
            RenderPipeline::BuiltIn
        } else if levels.iter().all(|l| l.as_deref() == Some(URP_ASSET)) {
            RenderPipeline::Universal
        } else {
            let mut names: Vec<String> = levels
                .into_iter()
                .map(|l| l.unwrap_or_else(|| "built-in".into()))
                .collect();
            names.sort();
            names.dedup();
            RenderPipeline::Other(names)
        }))
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
