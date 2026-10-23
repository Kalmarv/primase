//! RBF ("RBF0"): RAGE's binary form of an XML data file, read into the same element tree as [`crate::read::xml`].
//!
//! After the magic, records follow until the root closes: a descriptor index byte and a type byte. A descriptor
//! index equal to the number of descriptors so far introduces a new one (a 16-bit name length and the name); its
//! type travels with every record. Types: 0 opens an element (three 16-bit words; the third is how many of the values
//! that follow are its attributes), 0x10 a u32, 0x20 true, 0x30 false, 0x40 an f32, 0x50 three f32s, 0x60 a string
//! (16-bit length). `FF FF` closes the open element; `FD FF` and a 32-bit length give the open element's text. A value
//! that is not an attribute becomes a child element carrying it in its `value` attribute (`x`, `y`, `z` for three
//! floats), as the XML form writes them. The stream must end exactly where the root closes.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::read::xml::Element;
use crate::{Result, invalid};

const MAGIC: &[u8; 4] = b"RBF0";

fn err(at: usize, detail: impl ToString) -> crate::Error {
    invalid("RBF", at as u64, detail)
}

struct Cursor<'a> {
    d: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .at
            .checked_add(n)
            .ok_or_else(|| err(self.at, "a length past the address space"))?;
        let s = self
            .d
            .get(self.at..end)
            .ok_or_else(|| err(self.at, "ends early"))?;
        self.at = end;
        Ok(s)
    }
    /// The next `N` bytes, as an array.
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let at = self.at;
        <[u8; N]>::try_from(self.take(N)?).map_err(|_| err(at, "ends early"))
    }
    fn u8(&mut self) -> Result<u8> {
        self.array().map(u8::from_le_bytes)
    }
    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_le_bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }
    fn f32(&mut self) -> Result<f32> {
        self.u32().map(f32::from_bits)
    }
    fn text(&mut self, n: usize) -> Result<String> {
        let at = self.at;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|e| err(at, e))
    }
}

/// An element being read: the element and how many attributes it still expects.
struct Open {
    el: Element,
    pending_attributes: u16,
}

/// Parses an RBF file into its root element.
pub(crate) fn parse(bytes: &[u8]) -> Result<Element> {
    if bytes.get(..4) != Some(MAGIC) {
        return Err(err(0, "not RBF0"));
    }
    let mut c = Cursor { d: bytes, at: 4 };
    let mut names: Vec<(String, u8)> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    loop {
        let at = c.at;
        let index = c.u8()?;
        match index {
            0xFF => {
                if c.u8()? != 0xFF {
                    return Err(err(at, "close tag not FF FF"));
                }
                let done = stack
                    .pop()
                    .ok_or_else(|| err(at, "close without an open element"))?;
                if done.pending_attributes != 0 {
                    return Err(err(
                        at,
                        format!("<{}> closes with attributes missing", done.el.name),
                    ));
                }
                match stack.last_mut() {
                    Some(parent) => parent.el.children.push(done.el),
                    None => {
                        if c.at != bytes.len() {
                            return Err(err(c.at, "data after the root closes"));
                        }
                        return Ok(done.el);
                    }
                }
            }
            0xFD => {
                if c.u8()? != 0xFF {
                    return Err(err(at, "text tag not FD FF"));
                }
                let n = c.u32()? as usize;
                let raw = c.take(n)?;
                let text = String::from_utf8(raw.iter().copied().take_while(|&b| b != 0).collect())
                    .map_err(|e| err(at, e))?;
                if raw.iter().skip_while(|&&b| b != 0).any(|&b| b != 0) {
                    return Err(err(at, "text with data after its NUL"));
                }
                stack
                    .last_mut()
                    .ok_or_else(|| err(at, "text outside an element"))?
                    .el
                    .text
                    .push_str(&text);
            }
            _ => {
                let kind = c.u8()?;
                let i = usize::from(index);
                if i == names.len() {
                    let n = usize::from(c.u16()?);
                    names.push((c.text(n)?, kind));
                } else if i > names.len() {
                    return Err(err(at, format!("descriptor {i} of {}", names.len())));
                }
                let (name, declared) = names
                    .get(i)
                    .cloned()
                    .ok_or_else(|| err(at, format!("descriptor {i} of {}", names.len())))?;
                if kind != declared {
                    return Err(err(at, format!("{name}: type {kind:#x}, declared {declared:#x}")));
                }
                let value = match kind {
                    0x00 => {
                        let (_, _, attrs) = (c.u16()?, c.u16()?, c.u16()?);
                        if stack.is_empty() && at != 4 {
                            return Err(err(at, "a second root element"));
                        }
                        if stack.len() >= super::xml::MAX_DEPTH {
                            return Err(err(
                                at,
                                format!("elements nest deeper than {}", super::xml::MAX_DEPTH),
                            ));
                        }
                        stack.push(Open {
                            el: Element {
                                name,
                                ..Element::default()
                            },
                            pending_attributes: attrs,
                        });
                        continue;
                    }
                    0x10 => vec![("value".to_string(), c.u32()?.to_string())],
                    0x20 => vec![("value".to_string(), "true".to_string())],
                    0x30 => vec![("value".to_string(), "false".to_string())],
                    0x40 => vec![("value".to_string(), c.f32()?.to_string())],
                    0x50 => vec![
                        ("x".to_string(), c.f32()?.to_string()),
                        ("y".to_string(), c.f32()?.to_string()),
                        ("z".to_string(), c.f32()?.to_string()),
                    ],
                    0x60 => {
                        let n = usize::from(c.u16()?);
                        vec![("value".to_string(), c.text(n)?)]
                    }
                    k => return Err(err(at, format!("{name}: type {k:#x}"))),
                };
                let open = stack
                    .last_mut()
                    .ok_or_else(|| err(at, "a value outside an element"))?;
                if let Some(left) = open.pending_attributes.checked_sub(1) {
                    open.pending_attributes = left;
                    let v = value.into_iter().next().expect("one value").1;
                    open.el.attrs.push((name, v));
                } else {
                    open.el.children.push(Element {
                        name,
                        attrs: value,
                        ..Element::default()
                    });
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use super::*;

    fn name(out: &mut Vec<u8>, index: u8, kind: u8, n: &str) {
        out.extend([index, kind]);
        out.extend((n.len() as u16).to_le_bytes());
        out.extend(n.as_bytes());
    }

    /// Nesting is capped like the text form's.
    #[test]
    fn nesting_is_capped() {
        // the first element names its descriptor, the later ones refer to it by index and kind
        let build = |depth: usize| {
            let mut d = MAGIC.to_vec();
            name(&mut d, 0, 0, "a");
            d.extend([0, 0, 0, 0, 0, 0]);
            for _ in 1..depth {
                d.extend([0, 0]);
                d.extend([0, 0, 0, 0, 0, 0]);
            }
            d.extend([0xFF, 0xFF].repeat(depth));
            d
        };
        assert!(parse(&build(super::super::xml::MAX_DEPTH)).is_ok());
        let e = parse(&build(super::super::xml::MAX_DEPTH + 1))
            .unwrap_err()
            .to_string();
        assert!(e.contains("nest deeper than"), "{e}");
    }

    #[test]
    fn elements_attributes_values_and_text() {
        let mut d = MAGIC.to_vec();
        name(&mut d, 0, 0, "Root");
        d.extend([0, 0, 0, 0, 1, 0]); // one attribute follows
        name(&mut d, 1, 0x60, "kind");
        d.extend(2u16.to_le_bytes());
        d.extend(b"ab");
        name(&mut d, 2, 0, "item");
        d.extend([0, 0, 0, 0, 0, 0]);
        d.extend([0xFD, 0xFF]);
        d.extend(4u32.to_le_bytes());
        d.extend(b"txd\0");
        d.extend([0xFF, 0xFF]);
        d.extend([2, 0, 0, 0, 0, 0, 0, 0]); // a second <item>, by its descriptor
        name(&mut d, 3, 0x10, "n");
        d.extend(7u32.to_le_bytes());
        d.extend([0xFF, 0xFF, 0xFF, 0xFF]);
        let r = parse(&d).expect("parses");
        assert_eq!(r.attr("kind"), Some("ab"));
        let items: Vec<_> = r.items().collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "txd");
        assert_eq!(items[1].child_value("n"), Some("7"));
        // a descriptor index past the next new one, and trailing data
        let mut bad = d.clone();
        bad[4] = 5;
        assert!(parse(&bad).is_err());
        let mut long = d.clone();
        long.push(0);
        assert!(parse(&long).is_err());
    }
}
