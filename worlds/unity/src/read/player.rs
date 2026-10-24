//! A Unity player's data folder: its SerializedFiles by name, pointers (`PPtr`) resolved across them, objects decoded
//! by their classes' type trees.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the player's data folder"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::components::{RenderPipeline, URP_ASSET, UnityObject};
use crate::read::mesh::SkinWeights;
use crate::read::serialized::SerializedFile;
use crate::read::tpk::TypeTreePackage;
use crate::read::typetree::TypeTree;
use crate::read::value::{Array, Value};
use crate::{Error, Result, invalid};

/// An object of the player: its file (lower-case name) and path id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    pub file: String,
    pub path_id: i64,
}

impl From<&ObjRef> for UnityObject {
    fn from(r: &ObjRef) -> Self {
        UnityObject {
            file: r.file.clone(),
            path_id: r.path_id,
        }
    }
}

/// A quality level of the player ([`Player::quality_levels`]): its name, the bone influences a vertex may have and
/// its shadow settings (`shadows`: 0 disabled, 1 hard shadows only, 2 all; the distance shadows are drawn to; the
/// built-in pipeline's cascade count, which a Universal Render Pipeline asset replaces with its own).
#[derive(Clone, Debug, PartialEq)]
pub struct QualityLevel {
    pub name: String,
    pub skin_weights: SkinWeights,
    pub shadows: i64,
    pub shadow_distance: f32,
    pub shadow_cascades: i64,
}

/// A scene of the build ([`Player::scenes`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildScene {
    /// Its place in the build's scene list.
    pub index: usize,
    /// The file it is stored in (`level<index>`).
    pub file: String,
    /// The path the editor knew it by (`Assets/Scenes/Main Mountain Scene.unity`), as stored.
    pub path: String,
    /// The scene's name: its path's file name without `.unity`.
    pub name: String,
}

#[derive(Debug)]
pub struct Player {
    pub dir: PathBuf,
    /// By lower-case file name: `level1`, `sharedassets1.assets`, `unity default resources`, ...
    pub files: BTreeMap<String, SerializedFile>,
    /// The folder each file is in (its streamed resources are beside it).
    pub folders: BTreeMap<String, PathBuf>,
    package: TypeTreePackage,
    trees: Mutex<BTreeMap<(String, i32), std::sync::Arc<TypeTree>>>,
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
            trees: Mutex::new(BTreeMap::new()),
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

    /// The field `name` of the object's root struct alone ([`TypeTree::decode_field`]): `None` when its class has no
    /// such field. The object need not decode whole, and a field stored early (`m_Name`) is read without the rest.
    pub fn field(&self, r: &ObjRef, name: &str) -> Result<Option<Value>> {
        let f = self.file(&r.file)?;
        let o = f
            .object(r.path_id)
            .ok_or_else(|| invalid(format!("{}: no object {}", r.file, r.path_id)))?;
        self.tree(&r.file, f, o.class_id)?
            .decode_field(f.bytes(o), f.big_endian, name)
            .map_err(|e| invalid(format!("{}/{} (class {}): {e}", r.file, r.path_id, o.class_id)))
    }

    /// The scenes of the build, in `BuildSettings`' order: scene `i` is the file `level<i>`. `Ok(empty)` for a player
    /// without a `globalgamemanagers` or a `BuildSettings` (a player is never read for scenes it does not list).
    pub fn scenes(&self) -> Result<Vec<BuildScene>> {
        const FILE: &str = "globalgamemanagers";
        let Some(f) = self.files.get(FILE) else {
            return Ok(Vec::new());
        };
        let mut found = None;
        for o in &f.objects {
            let r = ObjRef {
                file: FILE.into(),
                path_id: o.path_id,
            };
            if self.class_name(&r) == Some("BuildSettings") {
                if found.is_some() {
                    return Err(invalid(format!("{FILE}: more than one BuildSettings")));
                }
                found = Some(r);
            }
        }
        let Some(r) = found else {
            return Ok(Vec::new());
        };
        let list = self
            .field(&r, "scenes")?
            .ok_or_else(|| invalid(format!("{FILE}: BuildSettings without a scene list")))?;
        let Value::Array(Array::Values(paths)) = list else {
            return Err(invalid(format!("{FILE}: BuildSettings' scenes is not a list")));
        };
        paths
            .iter()
            .enumerate()
            .map(|(index, p)| {
                let path = p
                    .as_str()
                    .ok_or_else(|| invalid(format!("{FILE}: scene {index} is not a path")))?
                    .to_owned();
                let name = path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&path)
                    .trim_end_matches(".unity")
                    .to_owned();
                Ok(BuildScene {
                    index,
                    file: format!("level{index}"),
                    path,
                    name,
                })
            })
            .collect()
    }

    /// A MonoBehaviour's base fields and its script's serialized bytes, which a stripped build has no type tree for.
    pub fn behaviour(&self, r: &ObjRef) -> Result<(Value, Vec<u8>)> {
        let f = self.file(&r.file)?;
        let o = f
            .object(r.path_id)
            .ok_or_else(|| invalid(format!("{}: no object {}", r.file, r.path_id)))?;
        if o.class_id != crate::read::serialized::MONO_BEHAVIOUR {
            return Err(invalid(format!("{}/{}: not a MonoBehaviour", r.file, r.path_id)));
        }
        let d = f.decode_with(o, &*self.tree(&r.file, f, o.class_id)?)?;
        let bytes = f.bytes(o);
        Ok((d.value, bytes[bytes.len() - d.rest..].to_vec()))
    }

    /// The pipeline asset each quality level draws with, each with the level's name: its own (`customRenderPipeline`),
    /// else GraphicsSettings' default (`m_CustomRenderPipeline`); one level without a name when the player has no
    /// QualitySettings levels. `None` when the player has no GraphicsSettings.
    #[allow(clippy::type_complexity)]
    fn pipeline_assets(&self) -> Result<Option<Vec<(Option<String>, Option<ObjRef>)>>> {
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
        let asset = |pptr: Option<&Value>| -> Result<Option<ObjRef>> {
            Ok(pptr.map(|p| self.resolve(FILE, p)).transpose()?.flatten())
        };
        let default = asset(graphics.get("m_CustomRenderPipeline"))?;
        let mut levels = Vec::new();
        if let Some(Value::Array(crate::read::value::Array::Values(qs))) = one("QualitySettings")?
            .as_ref()
            .and_then(|q| q.get("m_QualitySettings"))
        {
            for q in qs {
                let name = q.get("name").and_then(Value::as_str).map(str::to_owned);
                levels.push((
                    name,
                    asset(q.get("customRenderPipeline"))?.or_else(|| default.clone()),
                ));
            }
        }
        if levels.is_empty() {
            levels.push((None, default));
        }
        Ok(Some(levels))
    }

    /// A pipeline asset's script class (`namespace.class`).
    fn pipeline_class(&self, asset: &ObjRef) -> Result<String> {
        let (base, _) = self.behaviour(asset)?;
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
        Ok(match field("m_Namespace") {
            "" => field("m_ClassName").to_owned(),
            ns => format!("{ns}.{}", field("m_ClassName")),
        })
    }

    /// The player's [`RenderPipeline`], from `globalgamemanagers`' GraphicsSettings and QualitySettings; `None` when
    /// the player has no GraphicsSettings.
    pub fn render_pipeline(&self) -> Result<Option<RenderPipeline>> {
        let Some(levels) = self.pipeline_assets()? else {
            return Ok(None);
        };
        let levels: Vec<Option<String>> = levels
            .iter()
            .map(|(_, asset)| asset.as_ref().map(|a| self.pipeline_class(a)).transpose())
            .collect::<Result<_>>()?;
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

    /// The Universal Render Pipeline asset the quality level `quality` draws with (every level's when none is named
    /// and they all draw with one); `Ok(None)` when the level (or the levels) draw with no URP asset, or with more
    /// than one where no level is named. An unknown level name is an error.
    pub fn urp_asset(&self, quality: Option<&str>) -> Result<Option<ObjRef>> {
        let Some(levels) = self.pipeline_assets()? else {
            return Ok(None);
        };
        let chosen: Vec<&Option<ObjRef>> = match quality {
            Some(name) => vec![
                levels
                    .iter()
                    .find(|(n, _)| n.as_deref() == Some(name))
                    .map(|(_, a)| a)
                    .ok_or_else(|| invalid(format!("no quality level named {name}")))?,
            ],
            None => levels.iter().map(|(_, a)| a).collect(),
        };
        let mut found: Option<&ObjRef> = None;
        for asset in chosen {
            let Some(asset) = asset else { return Ok(None) };
            if self.pipeline_class(asset)? != URP_ASSET {
                return Ok(None);
            }
            match found {
                Some(f) if f != asset => return Ok(None),
                _ => found = Some(asset),
            }
        }
        Ok(found.cloned())
    }

    /// The quality levels of `globalgamemanagers`' QualitySettings, in order, each with its name and the skin weights
    /// it allows; none when the player has no QualitySettings. Which level is current at run time is not data a
    /// consumer can read (see [`Player::build_quality`]), so a consumer states the level it wants where levels differ.
    pub fn quality_levels(&self) -> Result<Vec<QualityLevel>> {
        match self.quality_settings()? {
            Some(settings) => levels_of(&settings),
            None => Ok(Vec::new()),
        }
    }

    /// The name of the level the build was made with: the level `m_CurrentQuality` of the player's QualitySettings
    /// indexes (the three players read so far hold the top or only level). It is what the build states, not what the
    /// game runs at: a game may change its quality level at run time. `None` when the player has no QualitySettings or
    /// states no `m_CurrentQuality`; an index outside the levels is an error. (The per-platform default
    /// `m_PerPlatformDefaultQuality` is not in a player: SkateBIRD, Sledding Game and PEAK carry none.)
    pub fn build_quality(&self) -> Result<Option<String>> {
        let Some(settings) = self.quality_settings()? else {
            return Ok(None);
        };
        current_level(&settings, &levels_of(&settings)?)
    }

    /// The decoded QualitySettings of `globalgamemanagers`, `None` when the player has none.
    fn quality_settings(&self) -> Result<Option<Value>> {
        const FILE: &str = "globalgamemanagers";
        let Some(f) = self.files.get(FILE) else {
            return Ok(None);
        };
        let mut found = None;
        for o in &f.objects {
            let r = ObjRef {
                file: FILE.into(),
                path_id: o.path_id,
            };
            if self.class_name(&r) == Some("QualitySettings") {
                if found.is_some() {
                    return Err(invalid(format!("{FILE}: more than one QualitySettings")));
                }
                found = Some(self.decode(&r)?);
            }
        }
        Ok(found)
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

/// The quality levels a decoded QualitySettings lists, in order.
fn levels_of(settings: &Value) -> Result<Vec<QualityLevel>> {
    const FILE: &str = "globalgamemanagers";
    let levels = match settings.get("m_QualitySettings") {
        Some(Value::Array(Array::Values(levels))) => levels.as_slice(),
        Some(Value::Array(a)) if a.is_empty() => &[],
        _ => {
            return Err(invalid(format!(
                "{FILE}: QualitySettings without m_QualitySettings"
            )));
        }
    };
    levels
        .iter()
        .map(|q| {
            let name = q
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("a quality level without a name"))?
                .to_owned();
            let weights = q
                .get("skinWeights")
                .and_then(Value::as_i64)
                .ok_or_else(|| invalid(format!("quality level {name}: no skinWeights")))?;
            let field = |k: &str| -> Result<&Value> {
                q.get(k)
                    .ok_or_else(|| invalid(format!("quality level {name}: no {k}")))
            };
            Ok(QualityLevel {
                skin_weights: SkinWeights::from_setting(weights)?,
                shadows: field("shadows")?
                    .as_i64()
                    .ok_or_else(|| invalid(format!("quality level {name}: shadows is not a number")))?,
                shadow_distance: field("shadowDistance")?.as_f32().ok_or_else(|| {
                    invalid(format!("quality level {name}: shadowDistance is not a number"))
                })?,
                shadow_cascades: field("shadowCascades")?.as_i64().ok_or_else(|| {
                    invalid(format!("quality level {name}: shadowCascades is not a number"))
                })?,
                name,
            })
        })
        .collect()
}

/// The name of the level a decoded QualitySettings' `m_CurrentQuality` indexes among `levels`, `None` where it states none.
fn current_level(settings: &Value, levels: &[QualityLevel]) -> Result<Option<String>> {
    let Some(current) = settings.get("m_CurrentQuality") else {
        return Ok(None);
    };
    let index = current
        .as_i64()
        .ok_or_else(|| invalid("QualitySettings: m_CurrentQuality is not a number"))?;
    usize::try_from(index)
        .ok()
        .and_then(|i| levels.get(i))
        .map(|level| Some(level.name.clone()))
        .ok_or_else(|| {
            invalid(format!(
                "QualitySettings: m_CurrentQuality {index} is not one of its {} levels",
                levels.len()
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn level(name: &str) -> QualityLevel {
        QualityLevel {
            name: name.to_owned(),
            skin_weights: SkinWeights::from_setting(4).expect("a skin weights setting"),
            shadows: 2,
            shadow_distance: 50.0,
            shadow_cascades: 4,
        }
    }

    fn settings(current: Option<Value>) -> Value {
        Value::Struct(
            current
                .map(|v| (Arc::<str>::from("m_CurrentQuality"), v))
                .into_iter()
                .collect(),
        )
    }

    /// The level the build was made with is the one `m_CurrentQuality` indexes; a player that states none has none, and
    /// an index outside the levels is refused, not clamped.
    #[test]
    fn the_build_quality_is_the_level_m_current_quality_indexes() {
        let levels = [level("Low"), level("Mid"), level("High")];
        let name = |v| current_level(&settings(Some(v)), &levels);
        assert_eq!(name(Value::Int(2)).unwrap().as_deref(), Some("High"));
        assert_eq!(name(Value::UInt(0)).unwrap().as_deref(), Some("Low"));
        assert_eq!(current_level(&settings(None), &levels).unwrap(), None);
        for out in [Value::Int(3), Value::Int(-1), Value::UInt(99)] {
            let error = name(out).unwrap_err().to_string();
            assert!(error.contains("m_CurrentQuality"), "{error}");
        }
        assert!(name(Value::String("2".into())).is_err());
        assert!(current_level(&settings(Some(Value::Int(0))), &[]).is_err());
    }
}
