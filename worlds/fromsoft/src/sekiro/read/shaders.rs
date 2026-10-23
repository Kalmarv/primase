//! Shader bundles of the materials a read keeps: every program of the bundle of each shader a material's definition
//! names (`/shader/shaderbdle.shaderbdlebnd.dcx`, one `<shader>.shaderbdle` binder per shader), as
//! [`source::ShaderBundle`] on an entity of its own, linked from the materials by `fromsoft.material_shader`.
//! The `fromsoft.shader_programs` resource, the layered materials' G-buffer pixel programs the layer bake reads, is
//! taken from the same bundles.
use super::super::source::{self, ShaderBundle, ShaderProgram, ShaderStage};
use crate::materials::LayeredMaterial;
use crate::metaparam::Metaparam;
use crate::{Archive, Binder, Result, dcx::Decoder, invalid};
use primase_core::{Entity, World};
use std::collections::{BTreeMap, BTreeSet};

/// The last path component, either separator.
fn leaf(name: Option<&str>) -> &str {
    name.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("")
}

/// The bundles the install holds of some shaders, and what it did not.
#[derive(Debug, Default)]
pub(super) struct Bundles {
    pub bundles: BTreeMap<String, ShaderBundle>,
    /// Shaders a material names whose bundle the install does not hold.
    pub missing: BTreeSet<String>,
    /// Bundles whose metaparam is of a version this reader does not read (their `parameters` are `None`).
    pub unread_metaparams: BTreeSet<String>,
}

impl Bundles {
    /// Read the bundles of `shaders` that are not already in `self`.
    pub fn read(
        &mut self,
        archive: &Archive,
        decoder: &Decoder<'_>,
        shaders: &BTreeSet<String>,
    ) -> Result<()> {
        let wanted: BTreeSet<&String> = shaders
            .iter()
            .filter(|s| !self.bundles.contains_key(*s) && !self.missing.contains(*s))
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let outer = Binder::read(
            &archive.read("/shader/shaderbdle.shaderbdlebnd.dcx", decoder)?,
            decoder,
        )?;
        for file in outer.files {
            let name = leaf(file.name.as_deref());
            let Some(shader) = name.strip_suffix(".shaderbdle") else {
                continue;
            };
            if !wanted.contains(&shader.to_owned()) {
                continue;
            }
            let bundle = bundle(shader, &file.bytes, decoder, &mut self.unread_metaparams)?;
            self.bundles.insert(shader.to_owned(), bundle);
        }
        self.missing.extend(
            wanted
                .into_iter()
                .filter(|s| !self.bundles.contains_key(*s))
                .cloned(),
        );
        Ok(())
    }

    /// The G-buffer pixel programs (`<shader>_<switch>_Gbuf.ppo`) of `layered` materials, by file name, taken from
    /// the bundles of the layered shader families (`M[Multiple]`, `M[MultipleGround]`, `M[Multiple]_Emissive`).
    pub fn layered_programs<'a>(
        &self,
        layered: impl Iterator<Item = &'a LayeredMaterial>,
    ) -> Result<source::Payloads> {
        let wanted = layered
            .map(LayeredMaterial::program_name)
            .collect::<Result<BTreeSet<_>>>()?;
        let mut programs = BTreeMap::new();
        for family in ["M[Multiple]", "M[MultipleGround]", "M[Multiple]_Emissive"] {
            for p in self.bundles.get(family).into_iter().flat_map(|b| &b.programs) {
                if wanted.contains(&p.name) {
                    programs.insert(p.name.clone(), p.code.clone());
                }
            }
        }
        Ok(programs)
    }

    /// Put the bundles into `world` as entities named as their shaders, in name order, and link each of `materials`
    /// (a material entity and the shader its definition names) to its bundle.
    pub fn spawn(&self, world: &mut World, materials: &[(Entity, String)]) {
        let mut entities = BTreeMap::new();
        for (shader, bundle) in &self.bundles {
            let e = world.spawn();
            world.insert(e, primase_core::Name(shader.clone()));
            world.insert(e, bundle.clone());
            entities.insert(shader, e);
        }
        for (material, shader) in materials {
            if let Some(&bundle) = entities.get(shader) {
                world.insert(*material, source::MaterialShader(bundle));
            }
        }
    }
}

/// One shader's bundle from its binder's bytes.
fn bundle(
    shader: &str,
    bytes: &[u8],
    decoder: &Decoder<'_>,
    unread_metaparams: &mut BTreeSet<String>,
) -> Result<ShaderBundle> {
    let what = |why: String| invalid("Sekiro shader bundle", 0, format!("{shader}: {why}"));
    let mut programs = Vec::new();
    let mut parameters = None;
    let mut metaparams = 0;
    for file in Binder::read(bytes, decoder)?.files {
        let name = leaf(file.name.as_deref()).to_owned();
        let extension = name.rsplit_once('.').map_or("", |x| x.1);
        if extension == "metaparam" {
            metaparams += 1;
            match Metaparam::read(&file.bytes) {
                Ok(m) => parameters = Some(m),
                Err(_) => {
                    unread_metaparams.insert(shader.to_owned());
                }
            }
            continue;
        }
        let stage = ShaderStage::from_extension(extension).ok_or_else(|| {
            what(format!(
                "{name}: a file that is neither a program nor a metaparam"
            ))
        })?;
        let shader_type = primase_dxbc::chunks(&file.bytes)
            .map_err(|e| what(format!("{name}: {e}")))?
            .into_iter()
            .find(|c| &c.kind == b"SHEX" || &c.kind == b"SHDR")
            .and_then(|c| Some(u32::from_le_bytes(c.data.get(..4)?.try_into().ok()?) >> 16));
        let expected = match stage {
            ShaderStage::Pixel => 0,
            ShaderStage::Vertex => 1,
            ShaderStage::Hull => 3,
            ShaderStage::Domain => 4,
        };
        if shader_type != Some(expected) {
            return Err(what(format!(
                "{name}: its code is of another shader type than its extension"
            )));
        }
        programs.push(ShaderProgram {
            name,
            stage,
            code: file.bytes.into(),
        });
    }
    if metaparams != 1 {
        return Err(what(format!("{metaparams} metaparams, not one")));
    }
    Ok(ShaderBundle {
        name: shader.to_owned(),
        programs,
        parameters,
    })
}
