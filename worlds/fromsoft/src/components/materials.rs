//! A source material as the reader resolved it from its FLVER entry and definition (an MTD or a material binary:
//! `fromsoft.material`), its layered definition (`fromsoft.layered_material`) and its bindings: what the source states,
//! no choice made.
use fromsoft_formats::{
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
    /// SpeedTree billboard cards: far stand-ins whose program turns them to the camera (no position of their own).
    #[serde(rename = "billboard card")]
    Billboard,
}
impl SkipClass {
    pub const ALL: [SkipClass; 4] = [
        SkipClass::Sky,
        SkipClass::LightVolume,
        SkipClass::Shadow,
        SkipClass::Billboard,
    ];
    /// The word a pass option names it by (`sky`, `light-volume`, `shadow`, `billboard`).
    pub fn word(self) -> &'static str {
        match self {
            SkipClass::Sky => "sky",
            SkipClass::LightVolume => "light-volume",
            SkipClass::Shadow => "shadow",
            SkipClass::Billboard => "billboard",
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
    Matbin(fromsoft_formats::matbin::Material),
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
                fromsoft_formats::matbin::Value::Int(x) => Some(*x),
                fromsoft_formats::matbin::Value::Int2(x) => Some(x[0]),
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
    pub address: [Option<iridia_interface::AddressMode>; 2],
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
    /// The material as the binding rules read it ([`MaterialView`]).
    pub fn view(&self) -> MaterialView<'_> {
        MaterialView {
            mtd: &self.mtd,
            definition: DefinitionRef::Mtd(&self.definition),
            bindings: &self.bindings,
        }
    }
    /// [`MaterialView::alpha_reference`].
    pub fn alpha_reference(&self) -> i32 {
        self.view().alpha_reference()
    }
    /// `g_BlendMode` ([`MaterialView::blend_mode`]), 0 where the MTD states none.
    pub fn blend_mode(&self) -> i32 {
        self.view().blend_mode().unwrap_or(0)
    }
    /// [`MaterialView::depth_write_disabled`].
    pub fn depth_write_disabled(&self) -> bool {
        self.view().depth_write_disabled()
    }
    /// [`MaterialView::program_name`].
    pub fn program_name(&self) -> fromsoft_formats::Result<String> {
        self.view().program_name()
    }
    /// [`MaterialView::is_forward`].
    pub fn is_forward(&self) -> bool {
        self.view().is_forward()
    }
    /// [`MaterialView::pass`].
    pub fn pass(&self) -> &'static str {
        self.view().pass()
    }
    /// [`MaterialView::gbuffer_program`].
    pub fn gbuffer_program(&self) -> String {
        self.view().gbuffer_program()
    }
    /// [`MaterialView::pass_program`].
    pub fn pass_program(&self) -> String {
        self.view().pass_program()
    }
}

/// A definition's parameter value in one vocabulary, whichever record states it: an MTD's booleans, integer and float
/// vectors, a material binary's typed scalars and vectors (a float3 colour's two further stored floats are not part of
/// its value).
#[derive(Clone, Debug, PartialEq)]
pub enum ParameterValue {
    Bool(bool),
    Int(Vec<i32>),
    Float(Vec<f32>),
}

/// The record that defines a material, borrowed: an MTD (Sekiro) or a material binary (Elden Ring).
#[derive(Clone, Copy, Debug)]
pub enum DefinitionRef<'a> {
    Mtd(&'a MaterialDefinition),
    Matbin(&'a fromsoft_formats::matbin::Material),
}
impl<'a> DefinitionRef<'a> {
    /// The shader's path as stored.
    pub fn shader(self) -> &'a str {
        match self {
            DefinitionRef::Mtd(d) => &d.shader,
            DefinitionRef::Matbin(m) => &m.shader,
        }
    }
    /// The shader's name: its path's last component without its extension (`M[AMSN_V]`).
    pub fn shader_name(self) -> &'a str {
        let leaf = self.shader().rsplit(['/', '\\']).next().unwrap_or("");
        leaf.rsplit_once('.').map_or(leaf, |x| x.0)
    }
    /// Parameter `name`'s value, where the definition states it (a material binary's value stored shorter than its type
    /// is not one).
    pub fn parameter(self, name: &str) -> Option<ParameterValue> {
        use fromsoft_formats::matbin::Value as B;
        match self {
            DefinitionRef::Mtd(d) => d
                .parameters
                .iter()
                .find(|p| p.name == name)
                .map(|p| match &p.value {
                    Value::Bool(b) => ParameterValue::Bool(*b),
                    Value::Int(v) => ParameterValue::Int(v.clone()),
                    Value::Float(v) => ParameterValue::Float(v.clone()),
                }),
            DefinitionRef::Matbin(m) => Some(match m.param(name)? {
                B::Bool(b) => ParameterValue::Bool(*b),
                B::Int(x) => ParameterValue::Int(vec![*x]),
                B::Int2(v) => ParameterValue::Int(v.to_vec()),
                B::Float(x) => ParameterValue::Float(vec![*x]),
                B::Float2(v) => ParameterValue::Float(v.to_vec()),
                B::Float3(v, _) => ParameterValue::Float(v.to_vec()),
                B::Float4(v) => ParameterValue::Float(v.to_vec()),
                B::Float5(v) => ParameterValue::Float(v.to_vec()),
                B::Truncated { .. } => return None,
            }),
        }
    }
    /// The texture slots the definition declares, by name: an MTD's texture kinds, a material binary's sampler names.
    pub fn texture_kinds(self) -> Vec<&'a str> {
        match self {
            DefinitionRef::Mtd(d) => d.textures.iter().map(|t| t.kind.as_str()).collect(),
            DefinitionRef::Matbin(m) => m.samplers.iter().map(|s| s.kind.as_str()).collect(),
        }
    }
    /// Whether it declares a texture slot named `kind`.
    pub fn declares(self, kind: &str) -> bool {
        self.texture_kinds().contains(&kind)
    }
    /// The tiling scale the definition stores for its texture slot of `Texture2D_N` number `n`
    /// ([`fromsoft_formats::metaparam::texture_number`]), where it stores one that is not zero: the two floats after an MTD
    /// texture's path (Sekiro), the two after a material binary sampler's key (Elden Ring).
    pub fn stored_scale(self, n: u32) -> Option<[f32; 2]> {
        let number = fromsoft_formats::metaparam::texture_number;
        let scale = match self {
            DefinitionRef::Mtd(d) => d
                .textures
                .iter()
                .find(|t| number(&t.kind) == Some(n))
                .and_then(|t| Some([*t.extra.first()?, *t.extra.get(1)?])),
            DefinitionRef::Matbin(m) => m
                .samplers
                .iter()
                .find(|s| number(&s.kind) == Some(n))
                .map(|s| s.unknown),
        };
        scale.filter(|s| *s != [0.; 2])
    }
}
impl Definition {
    /// The definition, borrowed ([`DefinitionRef`]).
    pub fn as_ref(&self) -> DefinitionRef<'_> {
        match self {
            Definition::Mtd(d) => DefinitionRef::Mtd(d),
            Definition::Matbin(m) => DefinitionRef::Matbin(m),
        }
    }
}

/// A material as the binding rules read it, whichever record defines it: the name its FLVER gives its definition, the
/// definition, and its texture bindings. A layered material's ([`LayeredMaterial::view`]) and any defined material's
/// ([`Material::view`]).
#[derive(Clone, Copy, Debug)]
pub struct MaterialView<'a> {
    pub mtd: &'a str,
    pub definition: DefinitionRef<'a>,
    pub bindings: &'a BTreeMap<String, Binding>,
}
impl MaterialView<'_> {
    /// The first integer of parameter `name`, where the definition states it as integers.
    fn int_parameter(&self, name: &str) -> Option<i32> {
        match self.definition.parameter(name)? {
            ParameterValue::Int(v) => v.first().copied(),
            _ => None,
        }
    }
    /// The alpha test's reference, the first word of `g_AlphaRef` (an int pair in both games' definitions; 0 where the
    /// material states none: no alpha test).
    pub fn alpha_reference(&self) -> i32 {
        self.int_parameter("g_AlphaRef").unwrap_or(0)
    }
    /// `g_BlendMode` where the definition states it. For Sekiro's MTDs: 0 opaque, 2 alpha blended (the two the captured
    /// game's draws tell apart: the others, 3, 4 and 5, are stated by a handful of materials and no draw of one was
    /// matched).
    pub fn blend_mode(&self) -> Option<i32> {
        self.int_parameter("g_BlendMode")
    }
    /// `g_GX_bDisableDepthWrite`: whether the material is drawn without writing depth (false where it states none).
    pub fn depth_write_disabled(&self) -> bool {
        matches!(
            self.definition.parameter("g_GX_bDisableDepthWrite"),
            Some(ParameterValue::Bool(true))
        )
    }
    /// The G-buffer program file its shader and static switch select (`<shader>_<switch>_Gbuf.ppo`), the alpha-tested
    /// permutation (`<shader>_<switch>_Gbuf_[A].ppo`, the plain program with a discard against `FC_AlphaRef`) when the
    /// material has an alpha reference; refused without a switch. Read from the bundles: all 34 switches of each of the
    /// three layered shaders have both (no plain program discards); in a capture of the running game the contents of the
    /// pixel stage's material buffer for a plain program have `FC_AlphaRef.x` = -100 and the alpha-tested one's 0.502 =
    /// 128 / 255 of `g_AlphaRef`.
    pub fn program_name(&self) -> fromsoft_formats::Result<String> {
        if self.definition.parameter("GXFT_SATStaticSwitch").is_none() {
            return Err(crate::invalid("layer shader", 0, "missing static switch"));
        }
        Ok(self.gbuffer_program())
    }
    /// Whether the game draws the material in its forward pass (after the lighting, its target 0 the finished colour)
    /// rather than the G-buffer pass: a definition (an MTD or a material binary) that states `g_GX_ForwardRendering`
    /// true, or `g_BlendMode` 2 (alpha blended). Checked against every draw of recorded frames, by the shader family of
    /// the program each drew with (a draw names no material, so a family's MTDs or binaries stand for it):
    /// - Sekiro, 9 frames of 4 sessions: every family drawn only forward has every MTD this rule calls forward (the sky
    ///   dome's 10 and the far mountains' 13, `g_BlendMode` 0 with `g_GX_ForwardRendering`; the moon, the ground fog's 8;
    ///   the cloud fog's 7 of 9, whose other 2 are `g_BlendMode` 4 and drawn by no frame), and of the families drawn only
    ///   through the G-buffer one MTD is called forward (a character's hair, `g_BlendMode` 2, in no frame).
    /// - Elden Ring, 7 frames: every family drawn only forward has every binary this rule calls forward (the cloud shell
    ///   `M[Clouds9]` 13, `M[A]_NoLight` 37, the lakes, ocean, bonfire, Erdtree light shaft and gargoyle walls), but for
    ///   the character creator's hair planes (`P[ChrCustomize]`, `g_BlendMode` 0, drawn in several passes of their own),
    ///   and no family drawn only through the G-buffer has a binary stating `g_GX_ForwardRendering`; a family drawn both
    ///   ways mixes binaries of `g_BlendMode` 0 and 2.
    ///
    /// The other blend modes (3, 4, 5) without `g_GX_ForwardRendering` were drawn by no frame and keep the G-buffer
    /// program.
    pub fn is_forward(&self) -> bool {
        self.blend_mode() == Some(2)
            || matches!(
                self.definition.parameter("g_GX_ForwardRendering"),
                Some(ParameterValue::Bool(true))
            )
    }
    /// The pass the material is drawn in, as its programs' file names spell it: `Fwd` for a forward material, `Gbuf`.
    pub fn pass(&self) -> &'static str {
        if self.is_forward() { "Fwd" } else { "Gbuf" }
    }
    /// The G-buffer pixel program file the material's shader, its static switch where it states one and its alpha
    /// reference select: `<shader>[_<switch>]_Gbuf[_[A]].ppo`. A layered shader has a switch (all 34 of each of Sekiro's
    /// three have their programs); the other SAT shaders of Sekiro state none and have one program (`<shader>_Gbuf.ppo`).
    /// Elden Ring's names are of the same form: the switch partitions its 20,193 bundled materials exactly (13,342 state
    /// none, 6,851 a numbered program).
    pub fn gbuffer_program(&self) -> String {
        self.program_in("Gbuf")
    }
    /// The pixel program the material is drawn with ([`MaterialView::pass`]'s): `<shader>[_<switch>]_<pass>[_[A]].ppo`.
    /// Of a pass the engine also draws with shadow permutations (`FwdSdw`, `FwdSSdw`) the plain one: which of them a draw
    /// takes is chosen per instance, not by the material. The recorded frames draw one family both ways (Elden Ring's
    /// lake and ocean, `_Fwd` and `_FwdSdw` alike) and two families of the same forward parameters differently (Sekiro's
    /// far mountains `_Fwd_[A]`, its sky dome `_FwdSdw`).
    pub fn pass_program(&self) -> String {
        self.program_in(self.pass())
    }
    fn program_in(&self, pass: &str) -> String {
        let switch = self
            .int_parameter("GXFT_SATStaticSwitch")
            .map_or(String::new(), |s| format!("_{s}"));
        let alpha = if self.alpha_reference() > 0 { "_[A]" } else { "" };
        format!("{}{switch}_{pass}{alpha}.ppo", self.definition.shader_name())
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
    /// The surface class: `opaque`, `cutout` (alpha tested), `leaves`, `alpha` (blended, `g_BlendMode` 2), `additive`
    /// (Elden Ring's `g_BlendMode` 4: added to what is behind it), `decal`, `water`.
    pub class: String,
    /// The source class of a material the game draws as something other than a surface ([`SkipClass`]); a
    /// program chooses whether to leave such meshes out.
    pub skip: Option<SkipClass>,
    pub cutoff: f32,
    pub bindings: BTreeMap<String, Binding>,
    pub layered: Option<LayeredMaterial>,
}
impl Material {
    /// The material as the binding rules read it ([`MaterialView`]), where a definition defines it.
    pub fn view(&self) -> Option<MaterialView<'_>> {
        Some(MaterialView {
            mtd: &self.mtd,
            definition: self.definition.as_ref()?.as_ref(),
            bindings: &self.bindings,
        })
    }
    /// The material with every binding's addressing as the engine samples it: an addressing the source does not state
    /// is repeat, the same as a stated repeat.
    pub fn with_effective_addressing(&self) -> Material {
        use iridia_interface::AddressMode::Repeat;
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
    pub fn identity(&self) -> fromsoft_formats::Result<String> {
        primase_core::json::to_value(&self.with_effective_addressing())
            .map(|v| v.to_string())
            .map_err(|e| crate::invalid("Sekiro material", 0, e))
    }
    /// A short name for [`Material::identity`], for what is derived from the material ([`identity_digest`]).
    pub fn identity_digest(&self) -> fromsoft_formats::Result<String> {
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
