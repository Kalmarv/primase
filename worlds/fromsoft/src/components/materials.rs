//! A source material as the reader resolved it from its FLVER entry and MTD (`fromsoft.material`), its layered
//! definition (`fromsoft.layered_material`) and its bindings: what the source states, no choice made.
use crate::{
    flver,
    mtd::{MaterialDefinition, Value},
};
use primase_core::world::Component;
use std::collections::BTreeMap;

/// A source material class [`resolve`] names in [`Material::skip`]: one the game draws as something other than a
/// surface. Serialized as its description.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum SkipClass {
    /// Sky, cloud and fog cards.
    #[serde(rename = "sky / cloud / fog card")]
    Sky,
    /// Additive light volumes.
    #[serde(rename = "additive light volume")]
    LightVolume,
    /// Shadow-only proxies.
    #[serde(rename = "shadow proxy")]
    Shadow,
}
impl SkipClass {
    pub const ALL: [SkipClass; 3] = [SkipClass::Sky, SkipClass::LightVolume, SkipClass::Shadow];
    /// The word a pass option names it by (`sky`, `light-volume`, `shadow`).
    pub fn word(self) -> &'static str {
        match self {
            SkipClass::Sky => "sky",
            SkipClass::LightVolume => "light-volume",
            SkipClass::Shadow => "shadow",
        }
    }
    /// The class an option word names.
    pub fn from_word(word: &str) -> Option<SkipClass> {
        SkipClass::ALL.into_iter().find(|c| c.word() == word)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Binding {
    pub texture: String,
    pub scale: [f32; 2],
    pub uv_number: i32,
    /// Whether the area's texture catalog holds the texture (the reader's fact; a texture it lacks is not loaded).
    pub available: bool,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct LayeredMaterial {
    pub mtd: String,
    pub definition: MaterialDefinition,
    pub bindings: BTreeMap<String, Binding>,
}
impl LayeredMaterial {
    /// The G-buffer program file its shader and static switch select (`<shader>_<switch>_Gbuf.ppo`).
    pub fn program_name(&self) -> crate::Result<String> {
        let leaf = self.definition.shader.rsplit(['/', '\\']).next().unwrap_or("");
        let shader = leaf.rsplit_once('.').map_or(leaf, |x| x.0);
        let switch = self
            .definition
            .parameters
            .iter()
            .find(|p| p.name == "GXFT_SATStaticSwitch")
            .and_then(|p| match &p.value {
                Value::Int(v) => v.first().copied(),
                _ => None,
            })
            .ok_or_else(|| crate::invalid("layer shader", 0, "missing static switch"))?;
        Ok(format!("{shader}_{switch}_Gbuf.ppo"))
    }
}
impl Component for LayeredMaterial {
    const NAME: &'static str = "fromsoft.layered_material";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Material {
    pub mtd: String,
    /// Original definition, including non-layered shader parameters. Absence is
    /// explicit; runtime shader constants must not be guessed from slot names.
    pub definition: Option<MaterialDefinition>,
    /// Raw, uninterpreted source GX records, included in material identity.
    pub gx: Vec<flver::GxItem>,
    pub shader: String,
    pub class: String,
    /// The source class of a material the game draws as something other than a surface ([`SkipClass`]); a
    /// program chooses whether to leave such meshes out.
    pub skip: Option<SkipClass>,
    pub cutoff: f32,
    pub bindings: BTreeMap<String, Binding>,
    pub layered: Option<LayeredMaterial>,
}
impl Material {
    /// The material's identity record: the JSON of every field. The reader keeps one entity per record, so two
    /// source materials are one when their records are equal.
    pub fn identity(&self) -> crate::Result<String> {
        primase_core::json::to_value(self)
            .map(|v| v.to_string())
            .map_err(|e| crate::invalid("Sekiro material", 0, e))
    }
    /// A short name for [`Material::identity`], for what is derived from the material ([`identity_digest`]).
    pub fn identity_digest(&self) -> crate::Result<String> {
        Ok(identity_digest(&self.identity()?))
    }
}
/// A short name for an identity record ([`Material::identity`]): the first 8 bytes of its SHA-256, lowercase hex. It
/// follows the source, never the order entities were spawned in.
pub fn identity_digest(identity: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(identity.as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
impl Component for Material {
    const NAME: &'static str = "fromsoft.material";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 4: no `two_sided` (sidedness is the face sets' `fromsoft.face_cull`). 5: the source's facts only (each
    /// binding's availability); the preview textures, the tiling and the name-based surface are the passes'
    /// ([`crate::preview`]).
    const VERSION: u32 = 5;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
