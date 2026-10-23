//! A source material as the reader resolved it from its FLVER entry and definition (an MTD or a material binary:
//! `fromsoft.material`), its layered definition (`fromsoft.layered_material`) and its bindings: what the source states,
//! no choice made.
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

/// A material's definition as its game stores it: an MTD (Sekiro) or a material binary (MATBIN, Elden Ring).
/// Serialized as the stored record itself (untagged), so an MTD material's identity record is what it always was.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(untagged)]
pub enum Definition {
    Mtd(MaterialDefinition),
    Matbin(crate::matbin::Material),
}
impl Definition {
    /// The shader's path as stored (`.spx` / `.fx`).
    pub fn shader(&self) -> &str {
        match self {
            Definition::Mtd(d) => &d.shader,
            Definition::Matbin(m) => &m.shader,
        }
    }
    /// The MTD, for what only an MTD states (its layered shaders' switches and SAT parameters).
    pub fn mtd(&self) -> Option<&MaterialDefinition> {
        match self {
            Definition::Mtd(d) => Some(d),
            Definition::Matbin(_) => None,
        }
    }
    pub fn mtd_mut(&mut self) -> Option<&mut MaterialDefinition> {
        match self {
            Definition::Mtd(d) => Some(d),
            Definition::Matbin(_) => None,
        }
    }
    /// The first integer of parameter `name`, where the definition stores it as integers.
    pub fn int(&self, name: &str) -> Option<i32> {
        match self {
            Definition::Mtd(d) => d
                .parameters
                .iter()
                .find(|p| p.name == name)
                .and_then(|p| match &p.value {
                    Value::Int(x) => x.first().copied(),
                    _ => None,
                }),
            Definition::Matbin(m) => match m.param(name)? {
                crate::matbin::Value::Int(x) => Some(*x),
                crate::matbin::Value::Int2(x) => Some(x[0]),
                _ => None,
            },
        }
    }
    /// Every texture the definition declares: its slot name (an MTD's texture kind, a material binary's sampler type),
    /// its path as stored and its UV set (an MTD's UV number; a material binary states none: 1).
    pub fn textures(&self) -> Vec<(&str, &str, i32)> {
        match self {
            Definition::Mtd(d) => d
                .textures
                .iter()
                .map(|t| (t.kind.as_str(), t.path.as_str(), t.uv_number))
                .collect(),
            Definition::Matbin(m) => m
                .samplers
                .iter()
                .map(|s| (s.kind.as_str(), s.path.as_str(), 1))
                .collect(),
        }
    }
    /// Whether the definition declares a texture slot named `kind`.
    pub fn declares(&self, kind: &str) -> bool {
        self.textures().iter().any(|t| t.0 == kind)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Binding {
    pub texture: String,
    pub scale: [f32; 2],
    pub uv_number: i32,
    /// The addressing along u and v the FLVER texture record stores (`None` where it states none, and for a binding
    /// only the MTD states).
    pub address: [Option<primase_core::shader::AddressMode>; 2],
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
    /// The alpha test's reference, the first word of `g_AlphaRef` (an int pair; 0 where the material states none: no
    /// alpha test).
    pub fn alpha_reference(&self) -> i32 {
        self.definition
            .parameters
            .iter()
            .find(|p| p.name == "g_AlphaRef")
            .and_then(|p| match &p.value {
                Value::Int(v) => v.first().copied(),
                _ => None,
            })
            .unwrap_or(0)
    }
    /// The G-buffer program file its shader and static switch select (`<shader>_<switch>_Gbuf.ppo`), the alpha-tested
    /// permutation (`<shader>_<switch>_Gbuf_[A].ppo`, the plain program with a discard against `FC_AlphaRef`) when the
    /// material has an alpha reference. Read from the bundles: all 34 switches of each of the three layered shaders have both
    /// (no plain program discards); in a capture of the running game the contents of the pixel stage's material buffer
    /// for a plain program have `FC_AlphaRef.x` = -100 and the alpha-tested one's 0.502 = 128 / 255 of `g_AlphaRef`.
    pub fn program_name(&self) -> crate::Result<String> {
        if !self
            .definition
            .parameters
            .iter()
            .any(|p| p.name == "GXFT_SATStaticSwitch")
        {
            return Err(crate::invalid("layer shader", 0, "missing static switch"));
        }
        Ok(self.gbuffer_program())
    }
    /// The G-buffer pixel program file the material's shader, its static switch where it states one and its alpha
    /// reference select: `<shader>[_<switch>]_Gbuf[_[A]].ppo`. A layered shader has a switch (all 34 of each of the three
    /// have their programs); the other SAT shaders of Sekiro state none and have one program (`<shader>_Gbuf.ppo`).
    pub fn gbuffer_program(&self) -> String {
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
            .map_or(String::new(), |s| format!("_{s}"));
        let alpha = if self.alpha_reference() > 0 { "_[A]" } else { "" };
        format!("{shader}{switch}_Gbuf{alpha}.ppo")
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
    /// Original definition (an MTD or a material binary), including non-layered shader parameters. Absence is
    /// explicit; runtime shader constants must not be guessed from slot names.
    pub definition: Option<Definition>,
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
    /// The material with every binding's addressing as the engine samples it: an addressing the source does not state
    /// is repeat, the same as a stated repeat.
    pub fn with_effective_addressing(&self) -> Material {
        use primase_core::shader::AddressMode::Repeat;
        let mut effective = self.clone();
        let layered = effective.layered.iter_mut().flat_map(|l| l.bindings.values_mut());
        for binding in effective.bindings.values_mut().chain(layered) {
            binding.address = binding.address.map(|a| a.or(Some(Repeat)));
        }
        effective
    }
    /// The material's identity record: the JSON of every field, the addressing as the engine samples it
    /// ([`Material::with_effective_addressing`]: a stated repeat and an unstated addressing are one). The reader keeps
    /// one entity per record, so two source materials are one when their records are equal.
    pub fn identity(&self) -> crate::Result<String> {
        primase_core::json::to_value(&self.with_effective_addressing())
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
    /// ([`crate::preview`]). 6: the definition may be a material binary ([`Definition`]; an MTD serializes as before).
    const VERSION: u32 = 6;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
