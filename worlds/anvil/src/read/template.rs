//! A MaterialTemplate's descriptor run: its counted property descriptors, each with three strings and its metadata
//! lists, read as stored. The rest of the template is not read, and the run is found by search, not by parsing the
//! body from its start: the template's whole layout is not established.
//!
//! A descriptor (little-endian): eight zero bytes; 0xD933DB50; a byte (2 or 5 seen); a u64; a u32; three strings;
//! a u32 count and that many strings; a u32 count and that many strings. A string is a u32 byte length, that many
//! bytes, and one zero byte when the length isn't 0. The run is a u32 count n directly followed by n descriptors,
//! each starting where the one before ends. [`descriptors`] takes the one place in the body where such a run reads
//! whole; a body with none, or with more than one, is refused.
//!
//! Established by the census (worlds/anvil/tests/census.rs) on every MaterialTemplate of Riders Republic, and by the
//! join it measures: a Material's texture-property key equals the CRC-32 of the second string of a descriptor in its
//! own template. What the strings, the byte, the u64 and the u32 mean is not read (the second string reads as a
//! label: "2 - Normal Map").
//!
//! Its binding records ([`bindings`]), found the same way, by their marker: each 0x92D220DD, a u32 key (a property's,
//! as a Material's records carry it), four u32 f1 to f4, a reference (or a tag-0 form, kept), then 8 bytes kept as
//! stored. By (f3, f4) the
//! Riders Republic notes read a record as a texture at shader slot t(75 + f1), a scalar in the material constant
//! buffer's vec4 f2, a vec4 there, or a UV transform's two rows from vec4 f2 (docs/material-spec.md §2: executing the
//! game's pixel shaders on values so placed reproduces their outputs); how far the run extends isn't established, so
//! every record found is returned, none inferred.
//!
//! Each binding record is a ShaderParameter whose 10 bytes link an Operator of the template's material graph
//! ([`operator`]; registry-named-fields §12, read by the class registry on every template body): a ConstantFloatValue,
//! ConstantVectorValue or ConstantIntegerValue holding the property's default `Value`, a UVTransformOperator holding
//! its default UVTransform, a TextureObject holding its TextureSelector (the TextureParameter record of material-spec
//! §27.0, [`texture_parameter`]: marker 0x7D08460D, a byte 0, its `TextureSpecificationMethod` (the channel) and
//! `MapType` (the TextureSet slot), a u32 0, and the default set and texture, each null where the template names none)
//! and its `SamplerMode`. A texture binding may instead link a ShaderCustomTextureOperator (36 in the game's templates,
//! 14 templates): the TextureObject definition instanced through the custom-operator mechanism, with the same
//! TextureSelector (docs/material-spec.md §27.0), read by walking its stored pins ([`custom_texture`]). The census
//! (worlds/anvil/tests/template_defaults.rs) reads every binding's Operator on every MaterialTemplate, against the
//! registry's worked examples.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// A record type: equals the CRC-32 of the ASCII string `MaterialTemplate` (the name a label).
pub use crate::read::material::MATERIAL_TEMPLATE;

/// A descriptor's marker, after its eight zero bytes.
pub const DESCRIPTOR: u32 = 0xD933_DB50;
/// A binding record's marker.
pub const BINDING: u32 = 0x92D2_20DD;
/// The type of a template's shader-state node (the notes, docs/material-spec.md §21: one in every template body, 428 of
/// 428), whose id follows the descriptor run directly.
pub const SHADER_STATE: u32 = 0xB4FD_3A4C;

/// One binding record as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// Where its marker starts in the body.
    pub at: usize,
    /// Its property's key.
    pub key: u32,
    pub f: [u32; 4],
    pub target: Target,
    /// The 8 bytes after the reference, as stored (fewer where the body ends first).
    pub after: Vec<u8>,
}

/// A binding record's 10 bytes after its four fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A reference (tag 1) or none (tag 3).
    Reference(Option<crate::read::reference::Reference>),
    /// Another form, as stored: tag 0 (then a byte and a u64; the generic `Texture` bindings hold it), not read.
    Stored([u8; 10]),
}

/// What a binding record binds, by its (f3, f4), as the Riders Republic notes read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    /// (0, 3): a texture, at shader slot t(75 + f1).
    Texture { slot: u32 },
    /// (1, 0): a scalar, the x of the constant buffer's vec4 f2.
    Scalar { vec4: u32 },
    /// (1, 2): a vec4, the constant buffer's vec4 f2.
    Vector { vec4: u32 },
    /// (3, 6): a UV transform, its two rows the constant buffer's vec4s f2 and f2 + 1.
    UvTransform { vec4: u32 },
    /// Another (f3, f4).
    Other,
}

impl Binding {
    pub fn kind(&self) -> BindingKind {
        let [f1, f2, f3, f4] = self.f;
        match (f3, f4) {
            (0, 3) => BindingKind::Texture { slot: f1 },
            (1, 0) => BindingKind::Scalar { vec4: f2 },
            (1, 2) => BindingKind::Vector { vec4: f2 },
            (3, 6) => BindingKind::UvTransform { vec4: f2 },
            _ => BindingKind::Other,
        }
    }
}

/// Every binding record of a MaterialTemplate body, in body order: each place the marker starts a record. A record
/// whose 10 bytes are neither a reference nor the tag-0 form is refused, naming where.
pub fn bindings(body: &[u8]) -> Result<Vec<Binding>, BindingError> {
    let marker = BINDING.to_le_bytes();
    let mut out = Vec::new();
    for (at, w) in body.windows(4).enumerate() {
        if w != marker {
            continue;
        }
        let mut c = Cursor { b: body, at };
        let mut word = || c.take(4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
        let short = BindingError::Short(at);
        let (Some(_), Some(key), Some(f1), Some(f2), Some(f3), Some(f4)) =
            (word(), word(), word(), word(), word(), word())
        else {
            return Err(short);
        };
        let bytes: [u8; crate::read::reference::LENGTH] = c
            .take(crate::read::reference::LENGTH)
            .and_then(|b| b.try_into().ok())
            .ok_or(BindingError::Short(at))?;
        let target = match bytes {
            [0, ..] => Target::Stored(bytes),
            _ => Target::Reference(
                crate::read::reference::parse(bytes)
                    .map_err(|source| BindingError::Reference { at, source })?,
            ),
        };
        let after =
            c.b.get(c.at..c.at.saturating_add(8).min(c.b.len()))
                .unwrap_or_default()
                .to_vec();
        out.push(Binding {
            at,
            key,
            f: [f1, f2, f3, f4],
            target,
            after,
        });
    }
    Ok(out)
}

/// A TextureParameter record's marker.
pub const TEXTURE_PARAMETER: u32 = 0x7D08_460D;

/// A texture binding's TextureParameter record (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureParameter {
    /// Where its marker starts in the body.
    pub at: usize,
    pub channel: u32,
    pub slot: u32,
    /// The default set and texture the template names (none: null).
    pub set: Option<crate::read::reference::Reference>,
    pub texture: Option<crate::read::reference::Reference>,
}

impl TextureParameter {
    /// The record whose marker is at `at` of `body`: refused where its byte or third word isn't 0 or a reference
    /// doesn't read.
    pub fn parse(body: &[u8], at: usize) -> Result<TextureParameter, BindingError> {
        let mut c = Cursor { b: body, at };
        let short = BindingError::Short(at);
        let (Some(marker), Some(zero), Some(channel), Some(slot), Some(third)) = (
            c.u32(),
            c.take(1).and_then(|b| b.first().copied()),
            c.u32(),
            c.u32(),
            c.u32(),
        ) else {
            return Err(short);
        };
        if marker != TEXTURE_PARAMETER || zero != 0 || third != 0 {
            return Err(BindingError::Parameter(at));
        }
        let mut reference = || -> Result<Option<crate::read::reference::Reference>, BindingError> {
            let bytes: [u8; crate::read::reference::LENGTH] = c
                .take(crate::read::reference::LENGTH)
                .and_then(|b| b.try_into().ok())
                .ok_or(BindingError::Short(at))?;
            crate::read::reference::parse(bytes).map_err(|source| BindingError::Reference { at, source })
        };
        let set = reference()?;
        let texture = reference()?;
        Ok(TextureParameter {
            at,
            channel,
            slot,
            set,
            texture,
        })
    }
}

impl Binding {
    /// The u64 its 10 bytes link: a reference's id, or the tag-0 form's (none where the reference is null).
    pub fn link(&self) -> Option<u64> {
        match &self.target {
            Target::Reference(r) => r.map(|r| r.id),
            Target::Stored(bytes) => bytes
                .get(2..10)
                .and_then(|w| <[u8; 8]>::try_from(w).ok())
                .map(u64::from_le_bytes),
        }
    }
}

/// The class of each Operator a ShaderParameter links (registry-named-fields §12).
pub const CONSTANT_FLOAT: u32 = 0xA475_5364;
pub const CONSTANT_VECTOR: u32 = 0x6497_30E3;
pub const CONSTANT_INTEGER: u32 = 0xA026_AFF2;
pub const UV_TRANSFORM_OPERATOR: u32 = 0xEADF_9820;
/// The UVTransformOperator's owned parameters.
pub const UV_TRANSFORM: u32 = 0xC52E_2125;
pub const TEXTURE_OBJECT: u32 = 0x5EF9_EA65;
pub const TEXTURE_ARRAY: u32 = 0x4C6B_3C62;
/// A texture binding's custom operator (see [`custom_texture`]).
pub const SHADER_CUSTOM_TEXTURE_OPERATOR: u32 = 0x1181_BD58;
/// The pin classes of a ShaderCustomOperator's output and input lists: FloatOutput, VectorOutput, TextureOutput and
/// IntegerOutput; FloatInput, VectorInput and TextureInput (registry-named-fields §12).
const OUTPUT_PINS: [u32; 4] = [0x63AD_2CDD, 0x1603_ED4D, 0x3A8A_5517, 0x2C74_BF29];
const INPUT_PINS: [u32; 3] = [0xA10B_3D7E, 0x6569_31BC, 0x354F_E002];
/// The name of the definition a custom texture operator instances: the TextureObject's.
const TEXTURE_OBJECT_DEFINITION: &[u8] = b"TextureObject";

/// A binding's linked Operator's default (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OperatorDefault {
    /// A ConstantFloatValue's `Value`.
    Float(f32),
    /// A ConstantVectorValue's `Value`.
    Vector([f32; 4]),
    /// A ConstantIntegerValue's `Value`.
    Integer(i32),
    /// A UVTransformOperator's UVTransform: `ScaleU`, `ScaleV`, `TranslationU`, `TranslationV`, their speeds,
    /// `Rotation`, `RotationSpeed` (a Material's UV-transform property's order), then `ForceDynamic`,
    /// `AnimateTranslation` and `AnimateRotation`.
    Uv { floats: [f32; 8], flags: [u8; 3] },
    /// A TextureObject's (or TextureArray's) TextureSelector and `SamplerMode`.
    Texture {
        selector: TextureParameter,
        sampler_mode: u32,
    },
    /// A ShaderCustomTextureOperator's own TextureSelector and `SamplerMode`: it selects a texture as a TextureObject's
    /// selector does ([`custom_texture`]).
    CustomTexture {
        selector: TextureParameter,
        sampler_mode: u32,
    },
    /// Another Operator, its class: no default read.
    Other(u32),
}

/// The length of the input pin at `at` (its u64 id, class and Tag, then its `Source`: `03`, or `02` or `05` and a u64).
fn input_pin(body: &[u8], at: usize) -> Result<usize, BindingError> {
    let tag_at = at.checked_add(20).ok_or(BindingError::Short(at))?;
    match body.get(tag_at) {
        Some(3) => Ok(21),
        Some(2 | 5) => Ok(29),
        Some(_) => Err(BindingError::Parameter(at)),
        None => Err(BindingError::Short(at)),
    }
}

/// The Operator the binding `b` of `body` links (registry-named-fields §12): its header `[tag][u64 id][u32 class]` where
/// the linked id first occurs, its body after the class (the offsets below from the class's first byte: a constant's
/// `Value` at 31, after its byte, `Active`, `ConstantID`, `Private` and its output pin; a UVTransformOperator's input
/// pin at 26 and its owned UVTransform after it; a TextureObject's eight output pins from 6, its TextureCoords input
/// pin at 166, its TextureSelector after it, then MipmapLODBias's input pin and `SamplerMode`). `None` where the binding
/// links nothing or its id occurs nowhere but in the binding itself (two character templates' generic textures).
pub fn operator(body: &[u8], b: &Binding) -> Result<Option<OperatorDefault>, BindingError> {
    let Some(link) = b.link() else {
        return Ok(None);
    };
    let id = link.to_le_bytes();
    let own = b.at.checked_add(26);
    let Some(header) = body
        .windows(8)
        .enumerate()
        .position(|(at, w)| w == id && Some(at) != own)
    else {
        return Ok(None);
    };
    let short = BindingError::Short(header);
    let class_at = header.checked_add(8).ok_or(BindingError::Short(header))?;
    let at = |offset: usize| class_at.checked_add(offset).ok_or(BindingError::Short(class_at));
    let word = |offset: usize| -> Result<[u8; 4], BindingError> {
        let from = at(offset)?;
        body.get(from..from.saturating_add(4))
            .and_then(|w| w.try_into().ok())
            .ok_or(BindingError::Short(from))
    };
    let f32_at = |offset: usize| word(offset).map(f32::from_le_bytes);
    let class = u32::from_le_bytes(word(0).map_err(|_| short)?);
    Ok(Some(match class {
        CONSTANT_FLOAT => OperatorDefault::Float(f32_at(31)?),
        CONSTANT_INTEGER => OperatorDefault::Integer(i32::from_le_bytes(word(31)?)),
        CONSTANT_VECTOR => OperatorDefault::Vector([f32_at(31)?, f32_at(35)?, f32_at(39)?, f32_at(43)?]),
        UV_TRANSFORM_OPERATOR => {
            let pin = input_pin(body, at(26)?)?;
            // the owned UVTransform: its u64 id, its class, then its fields
            let uv = 26usize.checked_add(pin).ok_or(BindingError::Short(class_at))?;
            let uv_class = u32::from_le_bytes(word(uv.saturating_add(8))?);
            if uv_class != UV_TRANSFORM {
                return Err(BindingError::Parameter(at(uv)?));
            }
            let first = uv.saturating_add(12);
            let mut floats = [0.0f32; 8];
            for (k, f) in floats.iter_mut().enumerate() {
                *f = f32_at(first.saturating_add(k.saturating_mul(4)))?;
            }
            let flags_at = at(first.saturating_add(32))?;
            let flags: [u8; 3] = body
                .get(flags_at..flags_at.saturating_add(3))
                .and_then(|w| w.try_into().ok())
                .ok_or(BindingError::Short(flags_at))?;
            OperatorDefault::Uv { floats, flags }
        }
        TEXTURE_OBJECT | TEXTURE_ARRAY => {
            let pin = input_pin(body, at(166)?)?;
            // the owned TextureSelector: its u64 id, then its class (the record's marker) and its fields
            let selector = 166usize.checked_add(pin).ok_or(BindingError::Short(class_at))?;
            let marker = at(selector.saturating_add(8))?;
            let parsed = TextureParameter::parse(body, marker)?;
            // after the selector's 37 bytes from its marker: MipmapLODBias's input pin, then SamplerMode
            let bias = selector.saturating_add(8).saturating_add(37);
            let bias_pin = input_pin(body, at(bias)?)?;
            let sampler_mode = u32::from_le_bytes(word(bias.saturating_add(bias_pin))?);
            OperatorDefault::Texture {
                selector: parsed,
                sampler_mode,
            }
        }
        SHADER_CUSTOM_TEXTURE_OPERATOR => {
            let (selector, sampler_mode) = custom_texture(body, class_at)?;
            OperatorDefault::CustomTexture {
                selector,
                sampler_mode,
            }
        }
        other => OperatorDefault::Other(other),
    }))
}

/// A ShaderCustomTextureOperator whose class id starts at `class_at`: its body walked field by field (the
/// registry's order, base classes first; verified on all 36 in the game's templates, docs/material-spec
/// §27.0). After the class: a byte (0 or 1), the Operator's `Active` byte (1), a string (the `ShaderCustomOperator`'s
/// definition name, which must be `TextureObject`: this operator is that definition instanced, so its selector
/// selects as a TextureObject's does), a u32 count and that many output pins (a tag byte 0 or 4, then a pin of 20
/// bytes: a u64 id, its class and a u64 tag), a u32 count and that many input pins (a tag byte, a pin of 20 bytes and
/// its `Source`: `03`, or `02` or `05` and a u64), a string, a u32 (the `SamplerMode`), then the owned
/// TextureSelector: its u64 id and the TextureParameter record. Anything else is refused, naming where: a pin of
/// another class, another definition, a longer list than the body holds.
pub fn custom_texture(body: &[u8], class_at: usize) -> Result<(TextureParameter, u32), BindingError> {
    let mut at = class_at.checked_add(4).ok_or(BindingError::Short(class_at))?;
    let take = |at: &mut usize, n: usize| -> Result<&[u8], BindingError> {
        let from = *at;
        let end = from.checked_add(n).ok_or(BindingError::Short(from))?;
        let s = body.get(from..end).ok_or(BindingError::Short(from))?;
        *at = end;
        Ok(s)
    };
    let u32_at = |at: &mut usize| -> Result<u32, BindingError> {
        let s: [u8; 4] = take(at, 4)?.try_into().map_err(|_| BindingError::Short(*at))?;
        Ok(u32::from_le_bytes(s))
    };
    // a string: a u32 length, that many bytes, and a zero byte when it isn't empty
    let string = |at: &mut usize| -> Result<&[u8], BindingError> {
        let n = usize::try_from(u32_at(at)?).map_err(|_| BindingError::Short(*at))?;
        let s = take(at, n)?;
        if n > 0 && take(at, 1)? != [0] {
            return Err(BindingError::Parameter(*at));
        }
        Ok(s)
    };
    let first = *take(&mut at, 1)?.first().ok_or(BindingError::Short(at))?;
    let active = *take(&mut at, 1)?.first().ok_or(BindingError::Short(at))?;
    if first > 1 || active != 1 {
        return Err(BindingError::Parameter(class_at));
    }
    if string(&mut at)? != TEXTURE_OBJECT_DEFINITION {
        return Err(BindingError::Parameter(at));
    }
    let outputs = u32_at(&mut at)?;
    for _ in 0..outputs {
        let pin = take(&mut at, 21)?;
        let class = pin
            .get(9..13)
            .and_then(|w| <[u8; 4]>::try_from(w).ok())
            .map(u32::from_le_bytes);
        let tag = pin.first().copied();
        if !matches!(tag, Some(0 | 4)) || !class.is_some_and(|c| OUTPUT_PINS.contains(&c)) {
            return Err(BindingError::Parameter(at.saturating_sub(21)));
        }
    }
    let inputs = u32_at(&mut at)?;
    for _ in 0..inputs {
        let start = at;
        let head = take(&mut at, 21)?;
        let class = head
            .get(9..13)
            .and_then(|w| <[u8; 4]>::try_from(w).ok())
            .map(u32::from_le_bytes);
        if !matches!(head.first(), Some(0 | 4)) || !class.is_some_and(|c| INPUT_PINS.contains(&c)) {
            return Err(BindingError::Parameter(start));
        }
        // the pin after its tag byte: its Source is at 20 from there
        let pin = input_pin(body, start.checked_add(1).ok_or(BindingError::Short(start))?)?;
        at = start
            .checked_add(1)
            .and_then(|s| s.checked_add(pin))
            .ok_or(BindingError::Short(start))?;
    }
    string(&mut at)?;
    let sampler_mode = u32_at(&mut at)?;
    let marker = at.checked_add(8).ok_or(BindingError::Short(at))?;
    Ok((TextureParameter::parse(body, marker)?, sampler_mode))
}

/// The TextureParameter (TextureSelector) record of the texture binding `b` of `body`: its linked TextureObject's,
/// TextureArray's or ShaderCustomTextureOperator's ([`operator`]); `None` where it isn't a texture binding or links another Operator or none.
pub fn texture_parameter(body: &[u8], b: &Binding) -> Result<Option<TextureParameter>, BindingError> {
    if !matches!(b.kind(), BindingKind::Texture { .. }) {
        return Ok(None);
    }
    Ok(match operator(body, b)? {
        Some(OperatorDefault::Texture { selector, .. } | OperatorDefault::CustomTexture { selector, .. }) => {
            Some(selector)
        }
        _ => None,
    })
}

/// Why a binding record can't be read.
#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error("MaterialTemplate binding at {0}: the body ends inside it")]
    Short(usize),
    #[error("MaterialTemplate TextureParameter at {0}: its byte or third word isn't 0")]
    Parameter(usize),
    #[error("MaterialTemplate binding at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: crate::read::reference::Error,
    },
}

/// One descriptor as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    /// Where it starts in the body.
    pub at: usize,
    /// The byte after the marker.
    pub byte: u8,
    /// The u64 after it.
    pub link: u64,
    /// The u32 after that.
    pub word: u32,
    /// The three strings, as stored bytes.
    pub strings: [Vec<u8>; 3],
    /// The first counted list of strings.
    pub keys: Vec<Vec<u8>>,
    /// The second counted list of strings.
    pub values: Vec<Vec<u8>>,
}

impl Descriptor {
    /// The CRC-32 of its second string (equal to a Material texture-property key in the census's join).
    pub fn key(&self) -> u32 {
        let [_, label, _] = &self.strings;
        primase_hash::crc32(label)
    }
}

/// Why a template's descriptors can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("MaterialTemplate: no descriptor run reads whole")]
    None,
    #[error(
        "MaterialTemplate: {0} descriptor runs read whole, at {1:?}: which is the template's isn't established"
    )]
    Several(usize, Vec<usize>),
}

/// Fields read in order, `None` past the body.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)?.try_into().ok().map(u64::from_le_bytes)
    }
    fn string(&mut self) -> Option<Vec<u8>> {
        let n = usize::try_from(self.u32()?).ok()?;
        let s = self.take(n)?.to_vec();
        if n > 0 && self.take(1)? != [0] {
            return None;
        }
        Some(s)
    }
    fn strings(&mut self) -> Option<Vec<Vec<u8>>> {
        let n = usize::try_from(self.u32()?).ok()?;
        if n > self.b.len() / 4 {
            return None;
        }
        (0..n).map(|_| self.string()).collect()
    }
    fn descriptor(&mut self) -> Option<Descriptor> {
        let at = self.at;
        if self.u64()? != 0 || self.u32()? != DESCRIPTOR {
            return None;
        }
        let [byte] = self.take(1)? else {
            return None;
        };
        let byte = *byte;
        let link = self.u64()?;
        let word = self.u32()?;
        let strings = [self.string()?, self.string()?, self.string()?];
        let keys = self.strings()?;
        let values = self.strings()?;
        Some(Descriptor {
            at,
            byte,
            link,
            word,
            strings,
            keys,
            values,
        })
    }
}

/// The run of n descriptors whose count is the u32 before `first`, if it reads whole.
fn run(body: &[u8], first: usize) -> Option<Vec<Descriptor>> {
    let count_at = first.checked_sub(4)?;
    let mut c = Cursor {
        b: body,
        at: count_at,
    };
    let n = usize::try_from(c.u32()?).ok()?;
    if n == 0 || n > body.len() / 37 {
        return None;
    }
    (0..n).map(|_| c.descriptor()).collect()
}

/// The template body's descriptor run: the one place where a u32 count is followed by that many descriptors read
/// whole. A body holding no descriptor marker has an empty run where its count, 0, stands 12 bytes before the type
/// of its one shader-state node (the node's id between: §21, 16 of 428 templates). Refused when there is no run, or
/// more than one.
pub fn descriptors(body: &[u8]) -> Result<Vec<Descriptor>, Error> {
    let marker = DESCRIPTOR.to_le_bytes();
    if !body.windows(4).any(|w| w == marker) {
        let state = SHADER_STATE.to_le_bytes();
        let mut nodes = body
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == state)
            .map(|(at, _)| at);
        return match (nodes.next(), nodes.next()) {
            (Some(at), None)
                if at
                    .checked_sub(12)
                    .and_then(|count| body.get(count..count.saturating_add(4)))
                    == Some(&[0; 4][..]) =>
            {
                Ok(Vec::new())
            }
            _ => Err(Error::None),
        };
    }
    let mut runs: Vec<Vec<Descriptor>> = Vec::new();
    let mut inside_a_run_up_to = 0usize;
    for first in 12..body.len() {
        // a descriptor of a run already found isn't a second run's first
        if first < inside_a_run_up_to {
            continue;
        }
        let signature = body.get(first..first.saturating_add(12));
        if signature.is_none_or(|s| s.get(..8) != Some(&[0; 8][..]) || s.get(8..) != Some(&marker[..])) {
            continue;
        }
        if let Some(r) = run(body, first) {
            inside_a_run_up_to = r.last().map_or(first, |d| d.at.saturating_add(1));
            runs.push(r);
        }
    }
    match runs.len() {
        0 => Err(Error::None),
        1 => Ok(runs.pop().unwrap_or_default()),
        n => Err(Error::Several(
            n,
            runs.iter().filter_map(|r| r.first().map(|d| d.at)).collect(),
        )),
    }
}

/// The shader programs a template's body holds: each DXBC container, by the offset of its magic in the body, found
/// by that magic and proven by its own stored checksum ([`primase_dxcontainer::checksum`]): a `DXBC` whose stated size
/// runs past the body, or whose checksum isn't its bytes', isn't one (the bytes there are something else). The
/// Riders Republic notes read 24 bytes before each (a hash, a u32, 0x7C7AC9C7 and the size: docs/material-spec.md
/// §13.3); which material state selects which program isn't read.
pub fn shaders(body: &[u8]) -> Vec<(usize, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = body
        .get(at..)
        .and_then(|rest| rest.windows(4).position(|w| w == b"DXBC"))
    {
        let start = at.saturating_add(found);
        at = start.saturating_add(4);
        let size = body
            .get(start.saturating_add(24)..start.saturating_add(28))
            .and_then(|w| w.try_into().ok())
            .map(u32::from_le_bytes)
            .and_then(|s| usize::try_from(s).ok());
        let Some(container) = size.and_then(|s| body.get(start..start.checked_add(s)?)) else {
            continue;
        };
        if container.len() >= 32
            && primase_dxcontainer::checksum(container)
                .as_ref()
                .map(|c| c.as_slice())
                == container.get(4..20)
        {
            out.push((start, container));
            at = start.saturating_add(container.len());
        }
    }
    out
}

/// A record type: the CRC-32 of `ShaderPermutation`.
pub const SHADER_PERMUTATION: u32 = 0x9467_F2BB;

/// One stage of a shader program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Vertex,
    Pixel,
}

/// One of a template's shader permutations: the key the engine looks it up by, and its program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permutation<'b> {
    pub stage: Stage,
    /// The engine's ShaderKey (bits 22 to 29 its pass: docs/shaders.md §6.2).
    pub key: u32,
    /// Its program's UCodeHash (the u32 after the program, as stored).
    pub code_hash: u32,
    /// The program's offset in the body and its DXBC container.
    pub at: usize,
    pub container: &'b [u8],
}

/// Why a template's permutations can't be read.
#[derive(Debug, thiserror::Error)]
pub enum PermutationError {
    #[error("ShaderPermutation at {at}: {what} {found:#x}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error(
        "ShaderPermutation at {at} (key {key:#010x}): its UCodeHash {code_hash:#010x} names {programs} programs"
    )]
    Program {
        at: usize,
        key: u32,
        code_hash: u32,
        programs: usize,
    },
}

/// Every shader permutation a template body lists (CompiledMaterialTemplate's vertex and pixel ShaderPermutations:
/// docs/shaders.md §6, docs/material-spec.md §25, read from the engine's class registry): each record a u64 0, the
/// record type, the ShaderKey, a u32 parameter count (0 on all 177,578 the notes count: another is refused), the
/// UCodeHash and 16 bytes, found by its first 12 bytes (the byte after it, 0 between records, isn't read). Its program is the one container ([`shaders`])
/// whose framing stores that UCodeHash after it (each container: `0x7C7AC9C7`, its size, the container, then the
/// UCodeHash, a u32 and a u64); one whose hash names no container or several is refused. Its stage is its program's.
pub fn permutations(body: &[u8]) -> Result<Vec<Permutation<'_>>, PermutationError> {
    let u32_at = |at: usize| -> Option<u32> {
        body.get(at..at.checked_add(4)?)
            .and_then(|w| w.try_into().ok())
            .map(u32::from_le_bytes)
    };
    // each program by the hash after it
    let mut programs: Vec<(u32, usize, &[u8], Stage)> = Vec::new();
    for (at, container) in shaders(body) {
        let end = at.saturating_add(container.len());
        let (Some(hash), Some(kind)) = (
            u32_at(end),
            primase_dxcontainer::chunks(container).ok().and_then(|c| {
                c.into_iter()
                    .find(|k| &k.kind == b"SHEX" || &k.kind == b"SHDR")
                    .and_then(|k| {
                        k.data
                            .get(..4)
                            .and_then(|w| w.try_into().ok())
                            .map(u32::from_le_bytes)
                    })
            }),
        ) else {
            continue;
        };
        let stage = match kind >> 16 {
            0 => Stage::Pixel,
            1 => Stage::Vertex,
            _ => continue,
        };
        programs.push((hash, at, container, stage));
    }
    let marker = {
        let mut m = [0u8; 12];
        m[8..].copy_from_slice(&SHADER_PERMUTATION.to_le_bytes());
        m
    };
    let mut out = Vec::new();
    for at in 0..body.len().saturating_sub(39) {
        if at.checked_add(12).and_then(|end| body.get(at..end)) != Some(&marker[..]) {
            continue;
        }
        let field = |offset: usize| at.checked_add(offset).and_then(u32_at).unwrap_or(u32::MAX);
        let (key, parameters, code_hash) = (field(12), field(16), field(20));
        if parameters != 0 {
            return Err(PermutationError::Value {
                what: "parameter count",
                at,
                found: u64::from(parameters),
                read: "0",
            });
        }
        let named: Vec<_> = programs.iter().filter(|p| p.0 == code_hash).collect();
        let [(_, program_at, container, stage)] = named[..] else {
            return Err(PermutationError::Program {
                at,
                key,
                code_hash,
                programs: named.len(),
            });
        };
        out.push(Permutation {
            stage: *stage,
            key,
            code_hash,
            at: *program_at,
            container,
        });
    }
    Ok(out)
}

/// The class of a template's `CompiledMaterialTemplate` record, which follows its shader-state node.
pub const COMPILED_MATERIAL_TEMPLATE: u32 = 0x3B9B_7477;

/// Why a template's shader-state node can't be read.
#[derive(Debug, thiserror::Error)]
pub enum FeatureError {
    #[error("{0} shader-state nodes (class 0xB4FD3A4C), not one")]
    Nodes(usize),
    #[error("the shader-state node at {0} is not followed by a CompiledMaterialTemplate record")]
    Unterminated(usize),
}

/// The body of a template's shader-state node (class `0xB4FD3A4C`, the `FeaturePreset` node of the notes), as stored:
/// the bytes between its class word and the class word of the `CompiledMaterialTemplate` record that follows it (54
/// bytes on all 432 templates). The first is the object's byte, then one byte per template feature, 0 or 1: the
/// engine's feature word `T` of the key composer (docs/shaders.md §6.2) has feature n at byte n + 1. Established by
/// the census of `tests/template_features.rs`: on all 432 templates the bytes for T bits 3 and 4 (key bits 0x400,
/// 0x800 and 0x200), 9 (0x1000), 10 (LayerMaterial's 0x4 and 0x20), 11 (the forward pass's 0x8), 12 (0x40), 13 (0x2)
/// and 15 (0x80; its byte is 1 or 2) are set exactly where the template stores a permutation key with the bit they gate.
pub fn feature_node(body: &[u8]) -> Result<&[u8], FeatureError> {
    let class = SHADER_STATE.to_le_bytes();
    let mut nodes = body
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == class)
        .map(|(at, _)| at);
    let (Some(at), None) = (nodes.next(), nodes.next()) else {
        return Err(FeatureError::Nodes(
            body.windows(4).filter(|w| *w == class).count(),
        ));
    };
    let start = at.saturating_add(4);
    let next = COMPILED_MATERIAL_TEMPLATE.to_le_bytes();
    let end = body
        .get(start..)
        .and_then(|rest| rest.windows(4).position(|w| w == next))
        .map(|p| start.saturating_add(p))
        .ok_or(FeatureError::Unterminated(at))?;
    body.get(start..end).ok_or(FeatureError::Unterminated(at))
}

/// A template's permutations ([`permutations`]) as the tables of its [`AnvilShaders`]: each stage's keys, sorted, and
/// each program's container once, however many permutations name it; `feature_node` its shader-state node's body
/// ([`feature_node`]).
pub fn shader_tables(read: &[Permutation<'_>], feature_node: &[u8]) -> crate::components::AnvilShaders {
    let mut programs: Vec<(usize, Vec<u8>)> = Vec::new();
    let (mut pixel, mut vertex) = (Vec::new(), Vec::new());
    for p in read {
        let index = match programs.iter().position(|(at, _)| *at == p.at) {
            Some(i) => i,
            None => {
                programs.push((p.at, p.container.to_vec()));
                programs.len().saturating_sub(1)
            }
        };
        match p.stage {
            Stage::Pixel => pixel.push((p.key, index)),
            Stage::Vertex => vertex.push((p.key, index)),
        }
    }
    pixel.sort_unstable();
    vertex.sort_unstable();
    crate::components::AnvilShaders {
        pixel,
        vertex,
        programs: programs.into_iter().map(|(_, p)| p).collect(),
        feature_node: feature_node.to_vec(),
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "test fixtures built by hand")]
mod tests {
    use super::*;

    fn string(b: &mut Vec<u8>, s: &[u8]) {
        b.extend(u32::try_from(s.len()).expect("short").to_le_bytes());
        b.extend(s);
        if !s.is_empty() {
            b.push(0);
        }
    }

    fn descriptor(b: &mut Vec<u8>, label: &[u8]) {
        b.extend([0; 8]);
        b.extend(DESCRIPTOR.to_le_bytes());
        b.push(2);
        b.extend(9u64.to_le_bytes());
        b.extend(0x4DDB_34EEu32.to_le_bytes());
        string(b, b"");
        string(b, label);
        string(b, b"shown");
        b.extend(1u32.to_le_bytes());
        string(b, b"Category");
        b.extend(1u32.to_le_bytes());
        string(b, b"MAD");
    }

    #[test]
    fn the_shader_tables_are_sorted_by_key_and_share_a_program() {
        static A: [u8; 4] = [1; 4];
        static B: [u8; 4] = [2; 4];
        static C: [u8; 4] = [3; 4];
        let (a, b, c) = (&A[..], &B[..], &C[..]);
        let permutation = |stage, key, at: usize, container: &'static [u8]| Permutation {
            stage,
            key,
            code_hash: 0,
            at,
            container,
        };
        let read = [
            permutation(Stage::Vertex, 9, 100, a),
            permutation(Stage::Pixel, 9, 200, b),
            permutation(Stage::Vertex, 3, 100, a),
            permutation(Stage::Pixel, 4, 300, c),
        ];
        let t = shader_tables(&read, &[0, 1, 0, 1]);
        assert_eq!(
            (t.feature(0), t.feature(1), t.feature(2), t.feature(3)),
            (Some(true), Some(false), Some(true), None)
        );
        // a byte of 2 (feature 15 on some templates) is set too
        let t = shader_tables(&read, &[0, 0, 2]);
        assert_eq!(
            (t.feature_byte(1), t.feature(1), t.feature(0)),
            (Some(2), Some(true), Some(false))
        );
        assert_eq!(t.programs, [a.to_vec(), b.to_vec(), c.to_vec()]);
        assert_eq!(
            (t.vertex.as_slice(), t.pixel.as_slice()),
            (&[(3, 0), (9, 0)][..], &[(4, 2), (9, 1)][..])
        );
        // the same key in both stages names each stage's own program
        assert_eq!((t.vertex(9), t.pixel(9)), (Some(a), Some(b)));
        assert_eq!((t.vertex(4), t.pixel(3)), (None, None));
    }

    #[test]
    fn a_run_is_found_and_read_whole() {
        let mut b = vec![7; 40];
        b.extend(2u32.to_le_bytes());
        descriptor(&mut b, b"2 - Normal Map");
        descriptor(&mut b, b"1 - Diffuse Alpha");
        b.extend([5; 9]);
        let d = descriptors(&b).expect("run");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].key(), 0x443B_F980);
        assert_eq!(d[1].key(), 0xE4B6_CC48);
        assert_eq!(d[0].keys, [b"Category".to_vec()]);
    }

    /// A body without descriptors: its count 0, then its shader-state node's id and type.
    fn empty(count: u32, nodes: usize) -> Vec<u8> {
        let mut b = vec![7; 20];
        b.extend(count.to_le_bytes());
        for _ in 0..nodes {
            b.extend(5u64.to_le_bytes());
            b.extend(SHADER_STATE.to_le_bytes());
        }
        b.extend([9; 9]);
        b
    }

    #[test]
    fn an_empty_run_is_the_zero_count_before_the_shader_state_node() {
        assert_eq!(descriptors(&empty(0, 1)).expect("empty run"), []);
        assert!(matches!(descriptors(&empty(1, 1)), Err(Error::None)));
        assert!(matches!(descriptors(&empty(0, 2)), Err(Error::None)));
        assert!(matches!(descriptors(&empty(0, 0)), Err(Error::None)));
    }

    #[test]
    fn none_or_two_runs_are_refused() {
        assert!(matches!(descriptors(&[0; 64]), Err(Error::None)));
        let mut b = vec![7; 20];
        b.extend(1u32.to_le_bytes());
        descriptor(&mut b, b"a");
        b.extend([7; 20]);
        b.extend(1u32.to_le_bytes());
        descriptor(&mut b, b"b");
        assert!(matches!(descriptors(&b), Err(Error::Several(2, _))));
    }

    /// A ShaderCustomTextureOperator's header and body worked by hand: the tag, id and class, a byte, `Active`, the
    /// definition name, `outputs` output pins and `inputs` input pins (each with an optional `Source` id), an empty
    /// string, the sampler mode, then the selector with the default texture `texture` (an id) in channel `channel`,
    /// slot `slot`. Returns the body and where the class id starts.
    fn custom_operator(
        name: &[u8],
        outputs: &[u32],
        inputs: &[(u32, Option<u64>)],
        texture: Option<u64>,
    ) -> (Vec<u8>, usize) {
        let mut b = vec![9; 5];
        b.push(0);
        b.extend(0x77u64.to_le_bytes());
        let class_at = b.len();
        b.extend(SHADER_CUSTOM_TEXTURE_OPERATOR.to_le_bytes());
        b.extend([1, 1]);
        string(&mut b, name);
        b.extend(u32::try_from(outputs.len()).expect("short").to_le_bytes());
        for &class in outputs {
            b.push(0);
            b.extend(0x100u64.to_le_bytes());
            b.extend(class.to_le_bytes());
            b.extend(0x200u64.to_le_bytes());
        }
        b.extend(u32::try_from(inputs.len()).expect("short").to_le_bytes());
        for &(class, source) in inputs {
            b.push(4);
            b.extend(0x300u64.to_le_bytes());
            b.extend(class.to_le_bytes());
            b.extend(0x400u64.to_le_bytes());
            match source {
                None => b.push(3),
                Some(id) => {
                    b.push(2);
                    b.extend(id.to_le_bytes());
                }
            }
        }
        string(&mut b, b"");
        b.extend(2u32.to_le_bytes());
        // the selector: its id, then the record
        b.extend(0x500u64.to_le_bytes());
        b.extend(TEXTURE_PARAMETER.to_le_bytes());
        b.push(0);
        b.extend(2u32.to_le_bytes());
        b.extend(5u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend([3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        match texture {
            None => b.extend([3, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(id) => {
                b.extend([1, 1]);
                b.extend(id.to_le_bytes());
            }
        }
        (b, class_at)
    }

    const PINS: ([u32; 3], [(u32, Option<u64>); 3]) = (
        [0x1603_ED4D, 0x63AD_2CDD, 0x3A8A_5517],
        [
            (0x6569_31BC, Some(0x42)),
            (0xA10B_3D7E, None),
            (0x354F_E002, None),
        ],
    );

    #[test]
    fn a_custom_texture_operator_is_walked_pin_by_pin_to_its_selector() {
        let (b, class_at) = custom_operator(b"TextureObject", &PINS.0, &PINS.1, Some(0x1122));
        let (selector, mode) = custom_texture(&b, class_at).expect("walks");
        assert_eq!((selector.channel, selector.slot, mode), (2, 5, 2));
        assert_eq!(selector.set, None);
        assert_eq!(selector.texture.map(|r| r.id), Some(0x1122));
        assert_eq!(
            Some(selector.at),
            b.windows(4).position(|w| w == TEXTURE_PARAMETER.to_le_bytes())
        );
        // no pins at all walks too; a null default reads as none
        let (b, class_at) = custom_operator(b"TextureObject", &[], &[], None);
        assert_eq!(custom_texture(&b, class_at).expect("walks").0.texture, None);
    }

    #[test]
    fn a_binding_linking_one_reads_its_selector_as_a_texture_objects() {
        let (mut b, class_at) = custom_operator(b"TextureObject", &PINS.0, &PINS.1, Some(0x1122));
        // a binding record linking the operator's id 0x77, before the operator
        let mut binding = BINDING.to_le_bytes().to_vec();
        binding.extend(0xABu32.to_le_bytes());
        for f in [3u32, 0, 0, 3] {
            binding.extend(f.to_le_bytes());
        }
        binding.extend([1, 1]);
        binding.extend(0x77u64.to_le_bytes());
        binding.extend([0; 8]);
        let shift = binding.len();
        binding.extend(&b);
        b = binding;
        let all = bindings(&b).expect("binding");
        assert_eq!(all.len(), 1);
        let Some(OperatorDefault::CustomTexture {
            selector,
            sampler_mode,
        }) = operator(&b, &all[0]).expect("reads")
        else {
            panic!("a custom texture operator");
        };
        assert_eq!((selector.channel, selector.slot, sampler_mode), (2, 5, 2));
        assert_eq!(
            Some(selector.at),
            b.windows(4).position(|w| w == TEXTURE_PARAMETER.to_le_bytes())
        );
        assert!(selector.at > shift + class_at);
        assert_eq!(texture_parameter(&b, &all[0]).expect("reads"), Some(selector));
    }

    #[test]
    fn a_custom_texture_operator_that_does_not_walk_is_refused() {
        let ok = custom_operator(b"TextureObject", &PINS.0, &PINS.1, None);
        // another definition than the TextureObject's
        let (b, at) = custom_operator(b"Other", &PINS.0, &PINS.1, None);
        assert!(matches!(custom_texture(&b, at), Err(BindingError::Parameter(_))));
        // a pin of a class that isn't a pin's, in either list
        let (b, at) = custom_operator(b"TextureObject", &[0x1234_5678], &[], None);
        assert!(matches!(custom_texture(&b, at), Err(BindingError::Parameter(_))));
        let (b, at) = custom_operator(b"TextureObject", &[], &[(0x63AD_2CDD, None)], None);
        assert!(matches!(custom_texture(&b, at), Err(BindingError::Parameter(_))));
        // the Active byte not 1, the leading byte past 1
        for k in [4 + 1, 4] {
            let mut b = ok.0.clone();
            b[ok.1 + k] = 2;
            assert!(
                matches!(custom_texture(&b, ok.1), Err(BindingError::Parameter(_))),
                "byte {k}"
            );
        }
        // the selector's record marker overwritten
        let mut b = ok.0.clone();
        let marker = b.len() - 37;
        b[marker] ^= 0xFF;
        assert!(matches!(
            custom_texture(&b, ok.1),
            Err(BindingError::Parameter(_))
        ));
        // the body ending anywhere inside it
        for cut in ok.1..ok.0.len() {
            assert!(custom_texture(&ok.0[..cut], ok.1).is_err(), "cut at {cut}");
        }
    }
}
