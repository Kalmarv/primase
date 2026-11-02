//! A material entity's chain, read when its model is: the model's material instance (`MATI`), the parent material it
//! names and the parent's shader family, each as stored (`forzatech.material`), and the textures its resolved texture
//! parameters name (`forzatech.material_textures`, with a texture entity each, [`super::textures`]).
//!
//! The parent and the shader are entries of the track's `bin.zip`, found through its asset manifest
//! ([`forzatech_formats::asset_manifest`]): an instance's parent path `Game:\Media\<p>.materialbin` is the manifest
//! material whose source is `<p>.material`, a material's shader path the manifest shader whose source is
//! `<p>.shader`. The manifest names the base folder's entry (`materials\_0x....materialbin`); the zip's
//! `materials\spring`, `autumn` and `winter` folders hold an entry of each name, and the read's season takes its own
//! folder's (summer the base folder's: the zip has no summer folder, so that the base folder is summer's form is
//! inferred, not established). A parameter's name is the one the family's `PARM` table gives its hash.
//!
//! What a material shows is its parameters' nearest definitions ([`ForzaMaterial::resolved`]: the instance's, then
//! the material's, then the shader's defaults); the textures read are those of the resolved texture parameters. That
//! the nearest definition is the one the game uses is inferred from the chain's structure; no pass relies on it yet.
//!
//! A texture parameter with the empty path (whose hash is 0) binds no texture. A parent path the manifest does not
//! list, a listed entry the zip lacks, or a texture hash no archive of the table has is counted in the
//! [`MaterialReport`] by name and the chain kept as far as it reads; nothing stands in for it.
use super::open::{Archive, Track};
use super::textures::{TextureScope, TextureTable, TierRecord, load_texture};
use crate::components::{
    Declared, ForzaMaterial, InstanceLevel, MaterialLevel, MaterialTexture, MaterialTextures, Parameter,
    ShaderLevel, Slot, Value,
};
use crate::{Error, Result};
use forzatech_formats::asset_manifest::AssetManifest;
use forzatech_formats::bundle::Bundle;
use forzatech_formats::material::{self as fm, Instance, Material};
use forzatech_formats::shader::{MapEntry, Shader};
use forzatech_formats::world::Season;
use primase_core::{Entity, World};
use std::collections::{BTreeMap, HashMap};

/// The track's materials and shaders, read once per reader: its `bin.zip`, the zip's asset manifest, the GeoChunks'
/// texture table, and each parent and shader read so far.
#[derive(Debug)]
pub struct Library {
    bin: Archive,
    manifest: AssetManifest,
    textures: TextureTable,
    /// The tiers of each texture a read loads: the caller's scope.
    texture_scope: TextureScope,
    /// By zip entry (lower case): the material level, or `None` when the zip lacks the entry.
    materials: HashMap<String, Option<MaterialLevel>>,
    shaders: HashMap<String, Option<ShaderLevel>>,
}

/// What reading materials found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MaterialReport {
    pub instances: usize,
    /// Parent paths the manifest does not list, with how many instances name each.
    pub parent_unlisted: BTreeMap<String, usize>,
    /// Listed parent entries the zip lacks.
    pub parent_missing: BTreeMap<String, usize>,
    /// Shader paths the manifest does not list or the zip lacks, with how many materials name each.
    pub shader_missing: BTreeMap<String, usize>,
    /// Instances by shader family.
    pub families: BTreeMap<String, usize>,
    /// Texture entities made.
    pub textures: usize,
    /// Resolved texture parameters with the empty path (hash 0): no texture bound.
    pub unbound: usize,
    /// Texture hashes no GeoChunk or library zip entry has, with the path a parameter gives (empty when none) and how many
    /// materials name each.
    pub texture_missing: BTreeMap<u32, (String, usize)>,
    /// The tiers the scope left unloaded, and the textures given their largest as none reaches its size.
    pub tiers: TierRecord,
}

impl MaterialReport {
    pub fn lines(&self) -> Vec<(String, String)> {
        let sum = |m: &BTreeMap<String, usize>| m.values().sum::<usize>();
        let mut lines = vec![(
            "forzatech materials".into(),
            format!(
                "{} instances; parents not in the manifest {} ({} paths), not in the zip {}; shaders not found {}; {} \
                 families; {} textures read, {} texture parameters bound to none, {} texture hashes in no archive",
                self.instances,
                sum(&self.parent_unlisted),
                self.parent_unlisted.len(),
                sum(&self.parent_missing),
                sum(&self.shader_missing),
                self.families.len(),
                self.textures,
                self.unbound,
                self.texture_missing.len(),
            ),
        )];
        for (name, map) in [
            ("parent not in the manifest", &self.parent_unlisted),
            ("parent not in the zip", &self.parent_missing),
            ("shader not found", &self.shader_missing),
        ] {
            for (path, n) in map {
                lines.push((format!("forzatech material: {name}"), format!("{path} ({n})")));
            }
        }
        for (hash, (path, n)) in &self.texture_missing {
            lines.push((
                "forzatech material: texture in no archive".into(),
                format!("{hash:#010x} {path} ({n})"),
            ));
        }
        if !self.tiers.unloaded.is_empty() || self.tiers.short > 0 {
            lines.push((
                "forzatech textures".into(),
                format!(
                    "{} stored tiers left unloaded by the read scope; {} textures with no tier reaching its size, given their largest",
                    self.tiers.unloaded.len(),
                    self.tiers.short
                ),
            ));
        }
        lines
    }

    /// Adds `other`'s counts to this one's.
    pub fn add(&mut self, other: &MaterialReport) {
        self.instances += other.instances;
        self.textures += other.textures;
        self.unbound += other.unbound;
        for (mine, theirs) in [
            (&mut self.parent_unlisted, &other.parent_unlisted),
            (&mut self.parent_missing, &other.parent_missing),
            (&mut self.shader_missing, &other.shader_missing),
            (&mut self.families, &other.families),
        ] {
            for (k, n) in theirs {
                *mine.entry(k.clone()).or_default() += n;
            }
        }
        for (hash, (path, n)) in &other.texture_missing {
            self.texture_missing
                .entry(*hash)
                .or_insert_with(|| (path.clone(), 0))
                .1 += n;
        }
        self.tiers.short += other.tiers.short;
        self.tiers.unloaded.extend(other.tiers.unloaded.iter().cloned());
    }
}

/// The zip entry of a manifest material's name for `season`: its season folder's (`materials\<season>\...`), the base
/// folder's for summer.
fn season_entry(name: &str, season: Season) -> String {
    let low = name.to_ascii_lowercase();
    match (season, low.strip_prefix("materials\\")) {
        (Season::Summer, _) | (_, None) => low,
        (s, Some(rest)) => format!("materials\\{}\\{rest}", s.folder()),
    }
}

fn value(v: &fm::Value<'_>) -> Value {
    match *v {
        fm::Value::Vector(x) => Value::Vector(x),
        fm::Value::Colour(x) => Value::Colour(x),
        fm::Value::Float(x) => Value::Float(x),
        fm::Value::Bool(x) => Value::Bool(x),
        fm::Value::Int(x) => Value::Int(x),
        fm::Value::Swizzle(x) => Value::Swizzle(x),
        fm::Value::Texture { path, path_hash } => Value::Texture {
            path: path.to_owned(),
            path_hash,
        },
        fm::Value::Sampler(x) => Value::Sampler(x),
        fm::Value::Gradient(x) => Value::Gradient(x.to_vec()),
        fm::Value::Vector2(x) => Value::Vector2(x),
        fm::Value::Type12(x) => Value::Type12(x),
    }
}

/// A list's parameters, unnamed; [`name`] names them once the family is known.
fn parameters(list: &fm::Parameters<'_>) -> Vec<Parameter> {
    list.list
        .iter()
        .map(|p| Parameter {
            hash: p.hash,
            flag_hash: p.flag_hash,
            name: None,
            value: value(&p.value),
        })
        .collect()
}

/// Gives each parameter the name the family declares its hash with (the first, when the table repeats a hash).
fn name(list: &mut [Parameter], declared: &[Declared]) {
    for p in list {
        p.name = declared.iter().find(|d| d.hash == p.hash).map(|d| d.name.clone());
    }
}

fn slots(map: &[MapEntry]) -> Vec<Slot> {
    map.iter()
        .map(|m| Slot {
            hash: m.hash,
            value: m.value,
            guid: m.guid,
        })
        .collect()
}

impl Library {
    /// The track's library: its `bin.zip` and asset manifest, and the texture table of its GeoChunks.
    pub fn open(track: &Track, texture_scope: TextureScope) -> Result<Library> {
        let bin = track.bin()?;
        let bytes = bin
            .read("AssetManifest.xml")?
            .ok_or_else(|| Error::Track(format!("{}: no AssetManifest.xml", bin.path().display())))?;
        Ok(Library {
            manifest: AssetManifest::parse(&bytes)?,
            bin,
            textures: TextureTable::of(track)?,
            texture_scope,
            materials: HashMap::new(),
            shaders: HashMap::new(),
        })
    }

    /// The shader family a material's shader path names, read on first use.
    fn shader(&mut self, path: &str, report: &mut MaterialReport) -> Result<Option<ShaderLevel>> {
        let Some(asset) = self.manifest.shader_for(path) else {
            *report.shader_missing.entry(path.to_owned()).or_default() += 1;
            return Ok(None);
        };
        let entry = asset.name.to_ascii_lowercase();
        if let Some(s) = self.shaders.get(&entry) {
            if s.is_none() {
                *report.shader_missing.entry(path.to_owned()).or_default() += 1;
            }
            return Ok(s.clone());
        }
        let level = match self.bin.read(&entry)? {
            None => {
                *report.shader_missing.entry(path.to_owned()).or_default() += 1;
                None
            }
            Some(bytes) => {
                let s = Shader::parse(&bytes).map_err(|e| Error::Model(format!("{entry}: {e}")))?;
                let file = entry.rsplit('\\').next().unwrap_or(&entry);
                let declared: Vec<Declared> = s
                    .names
                    .iter()
                    .map(|n| Declared {
                        name_hash: n.name_hash,
                        name: n.name.to_owned(),
                        hash: n.hash,
                        extra: n.extra.to_vec(),
                    })
                    .collect();
                let mut defaults = parameters(&s.defaults);
                name(&mut defaults, &declared);
                Some(ShaderLevel {
                    entry: asset.name.clone(),
                    source: asset.source.clone(),
                    family: file.strip_suffix(".shaderbin").unwrap_or(file).to_owned(),
                    declared,
                    constants: slots(&s.constants),
                    textures: slots(&s.textures),
                    samplers: slots(&s.samplers),
                    constants_size: s.constants_size,
                    defaults,
                    trailer: s.defaults.trailer,
                })
            }
        };
        self.shaders.insert(entry, level.clone());
        Ok(level)
    }

    /// The parent material an instance's parent path names, in `season`'s folder, read on first use.
    fn material(
        &mut self,
        path: &str,
        season: Season,
        report: &mut MaterialReport,
    ) -> Result<Option<MaterialLevel>> {
        let Some(asset) = self.manifest.material_for(path) else {
            *report.parent_unlisted.entry(path.to_owned()).or_default() += 1;
            return Ok(None);
        };
        let entry = season_entry(&asset.name, season);
        if let Some(m) = self.materials.get(&entry) {
            if m.is_none() {
                *report.parent_missing.entry(entry.clone()).or_default() += 1;
            }
            return Ok(m.clone());
        }
        let level = match self.bin.read(&entry)? {
            None => {
                *report.parent_missing.entry(entry.clone()).or_default() += 1;
                None
            }
            Some(bytes) => {
                let m = Material::parse(&bytes).map_err(|e| Error::Model(format!("{entry}: {e}")))?;
                Some(MaterialLevel {
                    entry: entry.clone(),
                    source: asset.source.clone(),
                    name: m.name.to_owned(),
                    atst: m.atst,
                    blst: m.blst,
                    xtra: m.xtra.map(<[u8]>::to_vec),
                    shader: m.shader.to_owned(),
                    technique: m.technique.to_owned(),
                    third: m.third.to_owned(),
                    user_data: m
                        .user_data
                        .iter()
                        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                        .collect(),
                    parameters: parameters(&m.parameters),
                    trailer: m.parameters.trailer,
                })
            }
        };
        self.materials.insert(entry, level.clone());
        Ok(level)
    }

    /// The chain of a material instance (the bundle of a model's `MatI` blob) in `season`.
    pub fn chain(
        &mut self,
        instance: &Bundle<'_>,
        season: Season,
        report: &mut MaterialReport,
    ) -> Result<ForzaMaterial> {
        let i = Instance::from_bundle(instance)?;
        report.instances += 1;
        let mut instance = InstanceLevel {
            name: i.name.to_owned(),
            atst: i.atst,
            xtra: i.xtra.to_vec(),
            parent: i.parent.to_owned(),
            parameters: parameters(&i.parameters),
            trailer: i.parameters.trailer,
        };
        let mut material = self.material(i.parent, season, report)?;
        let shader = match &material {
            Some(m) => self.shader(&m.shader, report)?,
            None => None,
        };
        if let Some(s) = &shader {
            name(&mut instance.parameters, &s.declared);
            if let Some(m) = &mut material {
                name(&mut m.parameters, &s.declared);
            }
            *report.families.entry(s.family.clone()).or_default() += 1;
        }
        Ok(ForzaMaterial {
            instance,
            material,
            shader,
        })
    }

    /// The texture entities of `material`'s resolved texture parameters, made on first use in `world`.
    pub fn textures(
        &self,
        track: &Track,
        material: &ForzaMaterial,
        made: &mut HashMap<u32, Entity>,
        world: &mut World,
        report: &mut MaterialReport,
    ) -> Result<MaterialTextures> {
        let mut out: Vec<MaterialTexture> = Vec::new();
        for (_, p) in material.resolved() {
            let Value::Texture { path, path_hash } = &p.value else {
                continue;
            };
            // the empty path (its hash is 0): no texture bound
            if path.is_empty() && *path_hash == 0 {
                report.unbound += 1;
                continue;
            }
            if out.iter().any(|t| t.path_hash == *path_hash) {
                continue;
            }
            let before = made.len();
            let texture = load_texture(
                track,
                &self.textures,
                self.texture_scope,
                made,
                world,
                *path_hash,
                &mut report.tiers,
            )?;
            report.textures += made.len() - before;
            if texture.is_none() {
                report
                    .texture_missing
                    .entry(*path_hash)
                    .or_insert_with(|| (path.clone(), 0))
                    .1 += 1;
            }
            out.push(MaterialTexture {
                path_hash: *path_hash,
                texture,
            });
        }
        Ok(MaterialTextures(out))
    }
}
