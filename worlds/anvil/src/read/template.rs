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

/// Why a binding record can't be read.
#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error("MaterialTemplate binding at {0}: the body ends inside it")]
    Short(usize),
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
/// by that magic and proven by its own stored checksum ([`primase_dxbc::checksum`]): a `DXBC` whose stated size
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
            && primase_dxbc::checksum(container).as_ref().map(|c| c.as_slice()) == container.get(4..20)
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
            primase_dxbc::chunks(container).ok().and_then(|c| {
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
}
