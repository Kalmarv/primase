//! The material banks of a disc joined to the fragment programs and texture headers around them: every bank material
//! by key, the fragment programs' uniform names and pixel-program classes, and a catalog of the packaged textures.
//! Nothing here picks a bank for a mesh or a texture copy for a sampler beyond the explicit rules documented on each
//! function.
use crate::material::{self, MaterialKey, ShredMaterial};
use crate::pak::Pak;
use crate::pixel_program::{self, PixelProgram};
use crate::texture::TextureCatalog;
use crate::{Result, invalid};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Why a bank was selected. Namespace inference is supported by every nonzero
/// material reference on the disc, but is not a native-loader observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum BankEvidence {
    UniqueId,
    InferredQuartetNamespace,
}

/// The banks that hold a material id and the one chosen for it, with why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialResolution {
    pub candidates: Vec<MaterialKey>,
    pub selected: Option<MaterialKey>,
    pub evidence: Option<BankEvidence>,
}

/// The namespace rule is exhaustive on all 34,469 nonzero references: levels ->
/// environments, characters -> characters, actors -> actors. Its application
/// to ID zero remains an inference.
pub fn namespace_bank(namespace: &[u8]) -> Option<&'static str> {
    match namespace {
        b"levels" => Some("environments"),
        b"characters" => Some("characters"),
        b"actors" => Some("actors"),
        _ => None,
    }
}

/// Bank materials by id, the names and classes of the fragment programs (`.psp` entries) of the supplied archives,
/// and the texture catalog of the same archives.
#[derive(Debug)]
pub struct MaterialLibrary<'a> {
    materials: BTreeMap<u32, Vec<ShredMaterial>>,
    programs: BTreeSet<String>,
    classes: BTreeMap<String, PixelProgram>,
    textures: TextureCatalog<'a>,
}

impl<'a> MaterialLibrary<'a> {
    /// Read the three material banks of `banks` (actors, characters, environments), the fragment programs of
    /// `assets` and index the textures of `assets`.
    pub fn new(banks: &Pak, assets: &[(&'a str, &'a Pak)]) -> Result<Self> {
        let mut names = BTreeMap::new();
        let mut classes = BTreeMap::new();
        for (_, pak) in assets {
            for (i, e) in pak.entries().iter().enumerate() {
                let Some(stem) = e.name.strip_suffix(b".psp") else {
                    continue;
                };
                let stem = std::str::from_utf8(stem).map_err(|e| invalid(0, e))?.to_owned();
                let payload = pak.payload(i)?;
                let reflected = material::fragment_uniform_names(&payload)?;
                let class = pixel_program::classify(&payload)?;
                if let Some(previous) = classes.insert(stem.clone(), class.clone())
                    && previous != class
                {
                    return Err(invalid(
                        e.offset,
                        format!("conflicting pixel program class for {stem}"),
                    ));
                }
                if let Some(previous) = names.insert(stem.clone(), reflected.clone())
                    && previous != reflected
                {
                    return Err(invalid(
                        e.offset,
                        format!("conflicting fragment reflection for {stem}"),
                    ));
                }
            }
        }
        let mut materials = Vec::new();
        for (bank, prefix) in [
            ("actors", "actor"),
            ("characters", "char"),
            ("environments", "env"),
        ] {
            let stem = format!("database\\materials\\{bank}\\{prefix}_materials");
            let payload = |ext: &str| -> Result<_> {
                let name = format!("{stem}.{ext}");
                let mut entries = banks
                    .entries()
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.name == name.as_bytes());
                let (i, _) = entries
                    .next()
                    .ok_or_else(|| invalid(0, format!("missing {name}")))?;
                if entries.next().is_some() {
                    return Err(invalid(0, format!("duplicate {name}")));
                }
                banks.payload(i)
            };
            materials.extend(material::bank(
                bank,
                &payload("mbp")?,
                &payload("mdp")?,
                |stem| Ok(names.get(stem).cloned().unwrap_or_default()),
            )?);
        }
        let mut library = Self::from_materials(materials, assets)?;
        library.programs = names.into_keys().collect();
        library.classes = classes;
        Ok(library)
    }

    /// Source records can also be supplied directly, including synthetic records.
    pub fn from_materials(materials: Vec<ShredMaterial>, assets: &[(&'a str, &'a Pak)]) -> Result<Self> {
        let mut by_id = BTreeMap::<u32, Vec<ShredMaterial>>::new();
        let mut keys = BTreeSet::new();
        for m in materials {
            if !keys.insert(m.key.clone()) {
                return Err(invalid(0, "duplicate bank-qualified material"));
            }
            by_id.entry(m.key.id).or_default().push(m);
        }
        for values in by_id.values_mut() {
            values.sort_by(|a, b| a.key.cmp(&b.key));
        }
        Ok(Self {
            materials: by_id,
            programs: BTreeSet::new(),
            classes: BTreeMap::new(),
            textures: TextureCatalog::new(assets).map_err(|e| invalid(0, e))?,
        })
    }

    /// The banks that hold `id` and the one chosen: the only holder, or for id zero (which every bank holds) the bank
    /// the quartet `namespace` names, an inference reported as such.
    pub fn resolve(&self, id: u32, namespace: Option<&[u8]>) -> MaterialResolution {
        let candidates: Vec<_> = self
            .materials
            .get(&id)
            .into_iter()
            .flatten()
            .map(|m| m.key.clone())
            .collect();
        let (selected, evidence) = if candidates.len() == 1 {
            (candidates.first().cloned(), Some(BankEvidence::UniqueId))
        } else if id == 0 {
            let inferred = namespace
                .and_then(namespace_bank)
                .and_then(|bank| candidates.iter().find(|k| k.bank == bank))
                .cloned();
            let evidence = inferred.as_ref().map(|_| BankEvidence::InferredQuartetNamespace);
            (inferred, evidence)
        } else {
            (None, None)
        };
        MaterialResolution {
            candidates,
            selected,
            evidence,
        }
    }

    /// The material with `key`.
    pub fn material(&self, key: &MaterialKey) -> Option<&ShredMaterial> {
        self.materials.get(&key.id)?.iter().find(|m| &m.key == key)
    }

    /// Whether a fragment program named `stem` (the `.psp` entry name without its extension) is in the archives.
    pub fn has_program(&self, stem: &str) -> bool {
        self.programs.contains(stem)
    }

    /// The class of the fragment program named `stem`.
    pub fn pixel_class(&self, stem: &str) -> Option<&PixelProgram> {
        self.classes.get(stem)
    }

    /// The textures of the supplied archives.
    pub fn textures(&self) -> &TextureCatalog<'a> {
        &self.textures
    }
}
