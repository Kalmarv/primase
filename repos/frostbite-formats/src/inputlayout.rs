//! Input layouts (type [`RTYPE`], ExpressionShaderInputLayout, named `expressionshader/inputlayout<id>`): the vertex
//! elements a raster permutation's vertex program was compiled to read. A resource is a relocatable payload (the
//! 16-byte metadata gives its size, its relocation size and the resource's own id):
//!
//! ```text
//! 0x00 u32 element count N   0x04 u64 pointer to the elements (0x20)   0x0c u64 pointer to the usages (after them)
//! 0x14 twelve zero bytes
//! 0x20 N elements of 20 bytes: u32 first, u32 last, two words (kept), u32 format
//! then the usages (one byte each), the elements' 20 N bytes and the usages' N each padded to a multiple of 16, then
//! the relocations: the u32 offsets 4 and 0xc
//! ```
//!
//! The usage of element `i` is the `usage` of a MeshSet's vertex element ([`crate::meshset::usage`]: 1 position, 33
//! and 34 the texture coordinates, 52 the tangent space, 2 to 5 blend data). An element is a Direct3D input element
//! without its name: `first` the semantic index, `last` the input slot (the vertex buffer), the first of `words` the
//! byte offset in the slot, the second the slot class; the format word's second byte is a DXGI format (6
//! `R32G32B32_FLOAT`, 10 `R16G16B16A16_FLOAT`, 34 `R16G16_FLOAT`, 28 `R8G8B8A8_UNORM`, ...) and its first byte
//! (2, 3, 4 or 6) is kept as stored. A vertex program's input is an element with its semantic index; its semantic
//! name mostly follows the element's usage ([`semantic_name`]) and [`InputLayout::resolve`] settles the rest by
//! elimination. `tests/inputlayout.rs` checks it against the vertex programs' own input signatures: of the 15,383
//! inputs of the vertex programs of the game configuration's 2,931 raster permutations, 15,336 are one element by
//! name and fit their format, 6 are one element by elimination and fit, and 41 are one element by name that the
//! program reads wider than its format (`TEXCOORD0` as four components of a usage 2 element stored as `R16G16_UINT`,
//! two); none is without an element.

use crate::permutation::resource_id;
use crate::{Rd, Result, bad};

const W: &str = "input layout";
/// ExpressionShaderInputLayout.
pub const RTYPE: u32 = 0x7D98_F632;
const HEADER: usize = 0x20;
const ELEMENT: usize = 20;

/// One element of a layout, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Element {
    pub first: u32,
    pub last: u32,
    /// The two words after `last`, as stored.
    pub words: [u32; 2],
    /// The format word: byte 0 as stored, byte 1 a DXGI format ([`Element::dxgi`]).
    pub format: u32,
}

impl Element {
    /// The DXGI format number the element's data is read as.
    pub fn dxgi(&self) -> u8 {
        (self.format >> 8) as u8
    }
}

/// The semantic name a vertex program usually reads an element of `usage` under: `POSITION` for usage 1, `NORMAL` for 6,
/// `COLOR` for 151, `TEXCOORD` for every other usage. A program may read an element under another name (a position as
/// `TEXCOORD0`, usage 30 as `TEXCOORD`): [`InputLayout::resolve`] takes that into account.
pub fn semantic_name(usage: u8) -> &'static str {
    match usage {
        1 => "POSITION",
        6 => "NORMAL",
        151 => "COLOR",
        _ => "TEXCOORD",
    }
}

/// The number of components of a DXGI format a vertex element is read as (`DXGI_FORMAT`: the float, integer and
/// normalised formats of 1 to 4, 2, 3 or 1 channels), `None` for a format not listed.
pub fn dxgi_components(format: u8) -> Option<u8> {
    Some(match format {
        1..=4 | 9..=14 | 24 | 27..=32 => 4,
        5..=8 | 26 => 3,
        15..=18 | 33..=38 | 48..=52 => 2,
        41..=43 | 53..=58 | 61..=65 => 1,
        _ => return None,
    })
}

/// How [`InputLayout::resolve`] found the element of an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// The one unclaimed element of its semantic index whose usage names its semantic ([`semantic_name`]).
    ByName(usize),
    /// The one element of its semantic index left after the other inputs took theirs by name.
    ByElimination(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputLayout {
    /// The resource's own id.
    pub id: u64,
    pub elements: Vec<Element>,
    /// The MeshSet usage of each element.
    pub usages: Vec<u8>,
}

impl InputLayout {
    /// The elements the inputs `(semantic name, semantic index)` of a vertex program read, one entry per input. An
    /// input takes the element of its semantic index whose usage names its semantic ([`semantic_name`]) when exactly one
    /// does; an input left over takes the one element of its index that no other input took when exactly one is left
    /// ([`Found::ByElimination`]: a program reads a position as `TEXCOORD0`, or usage 30 as a `TEXCOORD`). `None`: no
    /// element, or several left.
    pub fn resolve(&self, inputs: &[(&str, u32)]) -> Vec<Option<Found>> {
        let of_index =
            |index: u32| (0..self.elements.len()).filter(move |&i| self.elements[i].first == index);
        let mut found: Vec<Option<Found>> = inputs
            .iter()
            .map(|&(name, index)| {
                let mut named = of_index(index).filter(|&i| semantic_name(self.usages[i]) == name);
                let one = named.next()?;
                named.next().is_none().then_some(Found::ByName(one))
            })
            .collect();
        let mut taken: Vec<usize> = found
            .iter()
            .filter_map(|f| match f {
                Some(Found::ByName(i) | Found::ByElimination(i)) => Some(*i),
                None => None,
            })
            .collect();
        for (k, &(_, index)) in inputs.iter().enumerate() {
            if found[k].is_none() {
                let mut left = of_index(index).filter(|i| !taken.contains(i));
                if let (Some(one), None) = (left.next(), left.next()) {
                    found[k] = Some(Found::ByElimination(one));
                    taken.push(one);
                }
            }
        }
        found
    }
}

/// Read an input layout from its payload and metadata. Refused: a payload that is not the metadata's size and
/// relocation list, pointers that are not the ones above, more than 32 elements, relocations other than the two
/// pointers.
pub fn read(d: &[u8], meta: &[u8; 16]) -> Result<InputLayout> {
    let m = Rd::new(meta, W);
    let (size, reloc) = (m.u32le(0)? as usize, m.u32le(4)? as usize);
    if size.checked_add(reloc) != Some(d.len()) {
        return Err(bad(
            W,
            0,
            format!("size {size} + relocations {reloc} != {}", d.len()),
        ));
    }
    let r = Rd::new(d, W);
    let n = r.u32le(0)? as usize;
    if n == 0 || n > 32 {
        return Err(bad(W, 0, format!("{n} elements")));
    }
    let usages_at = HEADER + (ELEMENT * n).next_multiple_of(16);
    if r.u64le(4)? != HEADER as u64
        || r.u64le(0xc)? != usages_at as u64
        || r.bytes(0x14, 12)?.iter().any(|&b| b != 0)
    {
        return Err(bad(
            W,
            4,
            "pointers are not the elements at 0x20 and the usages after them",
        ));
    }
    if size != usages_at + n.next_multiple_of(16)
        || reloc != 8
        || r.u32le(size)? != 4
        || r.u32le(size + 4)? != 0xc
    {
        return Err(bad(
            W,
            size,
            format!("{size} bytes for {n} elements, or relocations other than the two pointers"),
        ));
    }
    let elements = (0..n)
        .map(|i| {
            let at = HEADER + ELEMENT * i;
            Ok(Element {
                first: r.u32le(at)?,
                last: r.u32le(at + 4)?,
                words: [r.u32le(at + 8)?, r.u32le(at + 12)?],
                format: r.u32le(at + 16)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(InputLayout {
        id: resource_id(meta),
        elements,
        usages: r.bytes(usages_at, n)?.to_vec(),
    })
}
