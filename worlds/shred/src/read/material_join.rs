//! Bank-qualified materials and every packaged texture copy, without shading guesses.
use super::{
    material,
    pak::Pak,
    pixel_program,
    texture::{self, Payload, Residency, TextureCatalog},
};
use crate::{
    Result,
    components::{
        BankEvidence, EncodedTexture, MaterialId, MaterialKey, MaterialNamespace, MaterialResolution,
        MaterialTextures, PixelProgram, SamplerTexture, ShredMaterial, SourceTexture, TextureCopy,
        TextureResidency,
    },
    invalid,
};
use primase_core::{ColorSpace, Entity, TexRole, World};
use std::collections::{BTreeMap, BTreeSet};

/// The caller explicitly requests the full chain, retaining prefetch as source
/// data and using it when no full copy exists. No copy is dropped by this choice.
#[derive(Clone, Copy, Debug)]
pub enum TexturePreference {
    FullChainThenPrefetch,
}

#[derive(Debug)]
pub struct MaterialInputs<'a> {
    pub banks: &'a Pak,
    /// Additional source archives; geometry PAKs are included by ShredReader.
    pub assets: Vec<(&'a str, &'a Pak)>,
    pub textures: TexturePreference,
}

#[derive(Clone, Debug, Default)]
pub struct MaterialReport {
    pub materials: usize,
    pub inferred_zero: usize,
    pub samplers_named: usize,
    pub samplers_unnamed: usize,
    pub empty_bindings: usize,
    pub texture_paths: usize,
    pub copies: usize,
    pub unresolved_materials: Vec<(u32, Vec<MaterialKey>)>,
    pub unavailable_textures: BTreeSet<String>,
    pub unavailable_pixels: BTreeSet<String>,
    pub unavailable_programs: BTreeSet<String>,
}

impl MaterialReport {
    pub fn lines(&self) -> primase_core::convert::Report {
        let mut lines = vec![(
            "shred materials".into(),
            format!(
                "{} materials; {} zero IDs inferred from quartet namespace; {} named / {} unnamed sampler slots; {} empty bindings; {} texture paths / {} packaged copies",
                self.materials,
                self.inferred_zero,
                self.samplers_named,
                self.samplers_unnamed,
                self.empty_bindings,
                self.texture_paths,
                self.copies
            ),
        )];
        for (id, candidates) in &self.unresolved_materials {
            lines.push((
                "shred unresolved material".into(),
                format!("id {id}: {candidates:?}"),
            ));
        }
        for path in &self.unavailable_textures {
            lines.push((
                "shred unavailable texture".into(),
                format!("{path}: not in supplied archives"),
            ));
        }
        for path in &self.unavailable_pixels {
            lines.push((
                "shred texture without pixels".into(),
                format!("{path}: headers present but no payload in supplied archives"),
            ));
        }
        for path in &self.unavailable_programs {
            lines.push((
                "shred unavailable fragment program".into(),
                format!("{path}: names unavailable in supplied archives"),
            ));
        }
        lines
    }
}

/// The namespace rule is exhaustive on all 34,469 nonzero references: levels ->
/// environments, characters -> characters, actors -> actors. Its application
/// to ID zero remains an inference and is reported on every affected entity.
pub fn namespace_bank(namespace: &[u8]) -> Option<&'static str> {
    match namespace {
        b"levels" => Some("environments"),
        b"characters" => Some("characters"),
        b"actors" => Some("actors"),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct PendingCopy {
    record: usize,
    payload: Payload,
    faces: Vec<Entity>,
}

#[derive(Debug)]
pub struct MaterialLibrary<'a> {
    materials: BTreeMap<u32, Vec<ShredMaterial>>,
    programs: BTreeSet<String>,
    classes: BTreeMap<String, PixelProgram>,
    textures: TextureCatalog<'a>,
    pending: Vec<PendingCopy>,
}

impl<'a> MaterialLibrary<'a> {
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
            pending: Vec::new(),
        })
    }

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

    /// Attach to the existing MaterialId entities. Planning creates texture
    /// identities and references only; pixel payloads are decoded in load().
    pub fn attach(&mut self, world: &mut World, preference: TexturePreference) -> Result<MaterialReport> {
        if world.iter::<MaterialTextures>().next().is_some() || world.iter::<SourceTexture>().next().is_some()
        {
            return Err(invalid(0, "material texture graph is already attached"));
        }
        self.pending.clear();
        let mut report = MaterialReport::default();
        let mut texture_entities = BTreeMap::<String, Option<Entity>>::new();
        let entities: Vec<_> = world.iter::<MaterialId>().map(|(e, id)| (e, id.0)).collect();
        for (entity, id) in entities {
            let namespace = world.get::<MaterialNamespace>(entity).map(|n| n.0.as_slice());
            let resolution = self.resolve(id, namespace);
            let selected = resolution.selected.clone();
            report.inferred_zero +=
                usize::from(resolution.evidence == Some(BankEvidence::InferredQuartetNamespace));
            world.insert(entity, resolution.clone());
            let Some(key) = selected else {
                report.unresolved_materials.push((id, resolution.candidates));
                continue;
            };
            let material = self.materials[&id]
                .iter()
                .find(|m| m.key == key)
                .ok_or_else(|| invalid(0, "resolved material missing"))?
                .clone();
            if !self.programs.contains(&material.fragment_shader) {
                report
                    .unavailable_programs
                    .insert(material.fragment_shader.clone());
            }
            let mut bindings = Vec::new();
            for sampler in &material.samplers {
                if sampler.name.is_some() {
                    report.samplers_named += 1;
                } else {
                    report.samplers_unnamed += 1;
                }
                let path = sampler.texture.replace('\\', "/");
                let texture = if path.is_empty() {
                    report.empty_bindings += 1;
                    None
                } else if let Some(existing) = texture_entities.get(&path) {
                    *existing
                } else {
                    let e = self.attach_texture(world, &path, preference, &mut report)?;
                    texture_entities.insert(path, e);
                    e
                };
                bindings.push(SamplerTexture {
                    slot: sampler.slot,
                    texture,
                });
            }
            if let Some(class) = self.classes.get(&material.fragment_shader) {
                world.insert(entity, class.clone());
            }
            world.insert(entity, material);
            world.insert(entity, MaterialTextures(bindings));
            report.materials += 1;
        }
        Ok(report)
    }

    fn attach_texture(
        &mut self,
        world: &mut World,
        path: &str,
        preference: TexturePreference,
        report: &mut MaterialReport,
    ) -> Result<Option<Entity>> {
        let records: Vec<_> = self.textures.resolve(path).map(|(i, _)| i).collect();
        if records.is_empty() {
            report.unavailable_textures.insert(path.into());
            return Ok(None);
        }
        let mut copies = Vec::new();
        for index in records {
            let record = &self.textures.records()[index];
            for (payload, pixels) in [
                (Payload::Primary, record.pixels),
                (Payload::CubeTail, record.cube_tail),
            ] {
                let faces = if pixels.is_some() {
                    (0..if record.header.gcm.cubemap == 1 { 6 } else { 1 })
                        .map(|_| world.spawn())
                        .collect()
                } else {
                    Vec::new()
                };
                // A header-only record is still source data. Do not manufacture an empty texture.
                if pixels.is_none() && matches!(payload, Payload::CubeTail) {
                    continue;
                }
                copies.push(TextureCopy {
                    archive: self
                        .textures
                        .archive_label(record.archive)
                        .ok_or_else(|| invalid(0, "texture archive index"))?
                        .into(),
                    section: self.textures.section_name(index).map(<[u8]>::to_vec),
                    header: record.header.raw.to_vec(),
                    residency: match record.residency {
                        Residency::Streamed => TextureResidency::Streamed,
                        Residency::Prefetch => TextureResidency::Prefetch,
                        Residency::Other => TextureResidency::Other,
                    },
                    first_mip: pixels.map_or(0, |p| p.first_mip),
                    cube_tail: matches!(payload, Payload::CubeTail),
                    faces,
                });
                if pixels.is_some() {
                    self.pending.push(PendingCopy {
                        record: index,
                        payload,
                        faces: copies
                            .last()
                            .ok_or_else(|| invalid(0, "missing texture copy"))?
                            .faces
                            .clone(),
                    });
                }
            }
        }
        let preferred = match preference {
            TexturePreference::FullChainThenPrefetch => copies
                .iter()
                .enumerate()
                .filter(|(_, c)| !c.faces.is_empty())
                .min_by_key(|(i, c)| (c.first_mip, c.cube_tail, *i))
                .map(|(i, _)| i),
        };
        let entity = world.spawn();
        if preferred.is_none() {
            report.unavailable_pixels.insert(path.into());
        }
        report.texture_paths += 1;
        report.copies += copies.len();
        world.insert(
            entity,
            SourceTexture {
                path: path.into(),
                copies,
                preferred,
            },
        );
        Ok(Some(entity))
    }

    /// Decode exactly the copies reached by the part. All mip chains are raw
    /// linear sample values with role Other; shader colour interpretation is a pass.
    pub fn load(&self, world: &mut World) -> Result<primase_core::convert::Report> {
        let (mut decoded, mut encoded) = (0, 0);
        let mut notices = Vec::new();
        for copy in &self.pending {
            if !copy.faces.iter().any(|&e| world.is_alive(e)) {
                continue;
            }
            match self
                .textures
                .decode(copy.record, copy.payload, ColorSpace::Linear, TexRole::Other)
            {
                Ok(textures) => {
                    if textures.len() != copy.faces.len() {
                        return Err(invalid(0, "decoded texture face count"));
                    }
                    for (e, t) in copy.faces.iter().copied().zip(textures) {
                        if world.is_alive(e) {
                            world.insert(e, t);
                            decoded += 1;
                        }
                    }
                }
                Err(texture::Error::Gcm(primase_texcodec::gcm::Error::Dimension { .. })) => {
                    let bytes = self
                        .textures
                        .encoded(copy.record, copy.payload)
                        .map_err(|e| invalid(0, e))?
                        .into_owned();
                    let reason = "3D texture outside 2D material API; source payload retained".to_owned();
                    for &e in &copy.faces {
                        if world.is_alive(e) {
                            world.insert(
                                e,
                                EncodedTexture {
                                    bytes: bytes.clone(),
                                    reason: reason.clone(),
                                },
                            );
                            encoded += 1;
                        }
                    }
                    notices.push((
                        "shred unsupported texture".into(),
                        String::from_utf8_lossy(&self.textures.records()[copy.record].path).into_owned(),
                    ));
                }
                Err(e) => return Err(invalid(0, e)),
            }
        }
        notices.insert(
            0,
            (
                "shred texture load".into(),
                format!("{decoded} decoded faces; {encoded} encoded unsupported copies"),
            ),
        );
        Ok(notices)
    }
}
