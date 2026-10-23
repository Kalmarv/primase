//! Shader bundles of the materials a read keeps: every program of the bundle of each shader a material's definition
//! names (one `<shader>.shaderbdle` binder per shader, in one of the game's bundle binders:
//! [`crate::Game::shader_binders`]), as [`source::ShaderBundle`] on an entity of its own, linked from the materials by
//! `fromsoft.material_shader`. Each program's container is of the game's format ([`crate::Game::shader_format`]:
//! DXBC or DXIL) and its shader kind agrees with its file's extension.
//! The `fromsoft.shader_programs` resource, the layered materials' G-buffer pixel programs the layer bake reads, is
//! taken from the same bundles.
use super::super::source::{self, ShaderBundle, ShaderProgram, ShaderStage};
use crate::materials::LayeredMaterial;
use crate::metaparam::Metaparam;
use crate::{Archive, Binder, Game, Result, dcx::Decoder, invalid};
use primase_core::shader::ShaderFormat;
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
    /// Read the bundles of `shaders` that are not already in `self`, from the game's bundle binders in order: a later
    /// binder is read only while a wanted shader is still not found.
    pub fn read(
        &mut self,
        archive: &Archive,
        decoder: &Decoder<'_>,
        game: Game,
        shaders: &BTreeSet<String>,
    ) -> Result<()> {
        let mut wanted: BTreeSet<&String> = shaders
            .iter()
            .filter(|s| !self.bundles.contains_key(*s) && !self.missing.contains(*s))
            .collect();
        for path in game.shader_binders() {
            if wanted.is_empty() {
                break;
            }
            if !archive.contains(path) {
                continue;
            }
            for file in Binder::read(&archive.read(path, decoder)?, decoder)?.files {
                let name = leaf(file.name.as_deref());
                let Some(shader) = name.strip_suffix(".shaderbdle") else {
                    continue;
                };
                let Some(&key) = wanted.iter().find(|w| w.as_str() == shader) else {
                    continue;
                };
                let bundle = bundle(game, shader, &file.bytes, decoder, &mut self.unread_metaparams)?;
                self.bundles.insert(shader.to_owned(), bundle);
                wanted.remove(key);
            }
        }
        // a shader without a bundle may be a fixed-function one, whose programs are in one binder by name
        if let Some(path) = game.fixed_function_binder()
            && archive.contains(path)
            && wanted.iter().any(|w| fixed_function(w))
        {
            let binder = Binder::read(&archive.read(path, decoder)?, decoder)?;
            for shader in wanted
                .iter()
                .filter(|w| fixed_function(w))
                .copied()
                .collect::<Vec<_>>()
            {
                let programs = fixed_function_programs(game, shader, &binder)?;
                if !programs.is_empty() {
                    self.bundles.insert(
                        shader.clone(),
                        ShaderBundle {
                            name: shader.clone(),
                            programs,
                            parameters: None,
                        },
                    );
                    wanted.remove(shader);
                }
            }
        }
        self.missing.extend(wanted.into_iter().cloned());
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

/// Whether a shader is of the fixed-function family: the `GXFlver_` shaders have programs but no bundle.
fn fixed_function(shader: &str) -> bool {
    shader.starts_with("GXFlver_")
}

/// The G-buffer programs of a fixed-function shader in its binder, by exact name: the pixel programs
/// `<shader>_Gbuf.ppo` and `<shader>_Gbuf_[A].ppo` and the vertex programs `<shader>_GBuf.vpo`, `<shader>_GBuf_Skin.vpo` and
/// their `@[cl]` permutations. The permutations of an effect the engine selects per instance (`_Bloody`, `_Burn`, `_Cm`,
/// `_Df`, `_Cloth`, `_Phantom`, `_ZFeather`, `_Decal[A]`: the same shader with that effect's code) are not taken: nothing in
/// a material says which an instance draws with. Each program's container states the shader kind its extension names.
fn fixed_function_programs(game: Game, shader: &str, binder: &Binder) -> Result<Vec<ShaderProgram>> {
    let format = game.shader_format();
    let names = [
        (format!("{shader}_Gbuf.ppo"), ShaderStage::Pixel, 0),
        (format!("{shader}_Gbuf_[A].ppo"), ShaderStage::Pixel, 0),
        (format!("{shader}_GBuf.vpo"), ShaderStage::Vertex, 1),
        (format!("{shader}_GBuf@[cl].vpo"), ShaderStage::Vertex, 1),
        (format!("{shader}_GBuf_Skin.vpo"), ShaderStage::Vertex, 1),
        (format!("{shader}_GBuf_Skin@[cl].vpo"), ShaderStage::Vertex, 1),
    ];
    let mut programs = Vec::new();
    for (name, stage, kind) in names {
        let Some(file) = binder.files.iter().find(|f| leaf(f.name.as_deref()) == name) else {
            continue;
        };
        let found = shader_kind(format, &file.bytes)
            .map_err(|e| invalid("shader binder", 0, format!("{shader}: {name}: {e}")))?;
        if found != Some(kind) {
            return Err(invalid(
                "shader binder",
                0,
                format!("{shader}: {name}: its code is of another shader type than its extension"),
            ));
        }
        programs.push(ShaderProgram {
            name,
            stage,
            format,
            code: file.bytes.clone().into(),
        });
    }
    Ok(programs)
}

/// The shader kind (0 pixel, 1 vertex, 3 hull, 4 domain) a program's container states, read as the game's `format`.
fn shader_kind(format: ShaderFormat, code: &[u8]) -> std::result::Result<Option<u32>, String> {
    match format {
        ShaderFormat::Dxbc => Ok(primase_dxcontainer::chunks(code)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|c| &c.kind == b"SHEX" || &c.kind == b"SHDR")
            .and_then(|c| Some(u32::from_le_bytes(c.data.get(..4)?.try_into().ok()?) >> 16))),
        ShaderFormat::Dxil => Ok(Some(
            primase_dxil::part(code)
                .map_err(|e| e.to_string())?
                .program_version
                >> 16,
        )),
        other => Err(format!("no shader bundle stores {other:?} programs")),
    }
}

/// One shader's bundle from its binder's bytes.
fn bundle(
    game: Game,
    shader: &str,
    bytes: &[u8],
    decoder: &Decoder<'_>,
    unread_metaparams: &mut BTreeSet<String>,
) -> Result<ShaderBundle> {
    let what = |why: String| invalid("shader bundle", 0, format!("{shader}: {why}"));
    let format = game.shader_format();
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
        let shader_type = shader_kind(format, &file.bytes).map_err(|e| what(format!("{name}: {e}")))?;
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
            format,
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
