//! A small strict element tree over quick-xml's pull parser, for the game's XML data files (DLC lists, setup and
//! content files). A UTF-8 byte order mark is skipped; the five predefined entities and character references are
//! resolved; comments and the declaration are skipped; anything else unusual (a DOCTYPE, processing instructions,
//! unknown entities, text beside the root) is an error.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::{Result, invalid};

/// One element: its name, attributes, child elements in order, and its text (concatenated, untrimmed).
#[derive(Clone, Debug, Default)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    /// An attribute by name.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The first child of this name (element names compare without case, as the game's parser hashes them).
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The trimmed text of the child of this name.
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }

    /// The `value` attribute of the child of this name.
    pub fn child_value(&self, name: &str) -> Option<&str> {
        self.child(name).and_then(|c| c.attr("value"))
    }

    /// The `Item` children (any case).
    pub fn items(&self) -> impl Iterator<Item = &Element> {
        self.children
            .iter()
            .filter(|c| c.name.eq_ignore_ascii_case("item"))
    }

    /// Fails naming the first child whose name is not in `known` (compared without case).
    pub fn only(&self, what: &str, known: &[&str]) -> Result<()> {
        match self
            .children
            .iter()
            .find(|c| !known.iter().any(|k| c.name.eq_ignore_ascii_case(k)))
        {
            Some(c) => Err(invalid(
                "XML",
                0,
                format!("{what}: unknown element <{}> in <{}>", c.name, self.name),
            )),
            None => Ok(()),
        }
    }
}

/// The deepest element nesting read. The tree is built iteratively but dropped and walked by recursion, which a document
/// of a few hundred thousand nested elements overflows the stack of; the game's files nest a dozen deep.
pub(crate) const MAX_DEPTH: usize = 64;

fn err(what: &str, e: impl std::fmt::Display) -> crate::Error {
    invalid("XML", 0, format!("{what}: {e}"))
}

/// Parses `bytes` (UTF-8, optionally with a byte order mark) into its root element.
pub(crate) fn parse(bytes: &[u8], what: &str) -> Result<Element> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| err(what, e))?;
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    loop {
        let ev = reader.read_event().map_err(|e| err(what, e))?;
        let empty = matches!(ev, Event::Empty(_));
        match ev {
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Start(s) | Event::Empty(s) => {
                if root.is_some() {
                    return Err(err(what, "a second root element"));
                }
                let name = s.name().as_ref().to_string();
                let mut attrs = Vec::new();
                for a in s.attributes() {
                    let a = a.map_err(|e| err(what, e))?;
                    let k = a.key.as_ref().to_string();
                    let v = a
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|e| err(what, e))?
                        .into_owned();
                    attrs.push((k, v));
                }
                let el = Element {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                };
                if empty {
                    match stack.last_mut() {
                        Some(p) => p.children.push(el),
                        None => root = Some(el),
                    }
                } else {
                    if stack.len() >= MAX_DEPTH {
                        return Err(err(what, format!("elements nest deeper than {MAX_DEPTH}")));
                    }
                    stack.push(el);
                }
            }
            Event::End(e) => {
                let el = stack
                    .pop()
                    .ok_or_else(|| err(what, "an end tag without a start"))?;
                if e.name().as_ref() != el.name {
                    return Err(err(
                        what,
                        format!("</{}> closes <{}>", e.name().as_ref(), el.name),
                    ));
                }
                match stack.last_mut() {
                    Some(p) => p.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => {
                let s = t.xml10_content();
                match stack.last_mut() {
                    Some(p) => p.text.push_str(&s),
                    None if s.trim().is_empty() => {}
                    None => return Err(err(what, "text outside the root element")),
                }
            }
            Event::CData(c) => {
                let s = c.as_ref().to_string();
                stack
                    .last_mut()
                    .ok_or_else(|| err(what, "CDATA outside the root element"))?
                    .text
                    .push_str(&s);
            }
            Event::GeneralRef(r) => {
                let c = match r.resolve_char_ref().map_err(|e| err(what, e))? {
                    Some(c) => c,
                    None => match &*r {
                        "lt" => '<',
                        "gt" => '>',
                        "amp" => '&',
                        "apos" => '\'',
                        "quot" => '"',
                        other => return Err(err(what, format!("unknown entity &{other};"))),
                    },
                };
                stack
                    .last_mut()
                    .ok_or_else(|| err(what, "an entity outside the root element"))?
                    .text
                    .push(c);
            }
            Event::PI(_) | Event::DocType(_) => return Err(err(what, "processing instruction or DOCTYPE")),
            Event::Eof => break,
        }
    }
    if let Some(open) = stack.last() {
        return Err(err(what, format!("<{}> is not closed", open.name)));
    }
    root.ok_or_else(|| err(what, "no root element"))
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

    /// Nesting is capped, and a document of hundreds of thousands of nested elements is refused, not a stack overflow.
    #[test]
    fn nesting_is_capped() {
        let nested = |depth: usize| format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        assert!(parse(nested(MAX_DEPTH).as_bytes(), "t").is_ok());
        let e = parse(nested(MAX_DEPTH + 1).as_bytes(), "t")
            .unwrap_err()
            .to_string();
        assert!(e.contains("nest deeper than 64"), "{e}");
        assert!(parse(nested(400_000).as_bytes(), "t").is_err());
    }

    #[test]
    fn tree_entities_bom_and_strictness() {
        let doc = b"\xEF\xBB\xBF<?xml version=\"1.0\"?>\n<!-- c --><Root a=\"x&amp;y\"><Item>p&lt;q&#x41;</Item><item v=\"1\"/></Root>";
        let r = parse(doc, "t").expect("parses");
        assert_eq!(r.attr("a"), Some("x&y"));
        let items: Vec<_> = r.items().collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "p<qA");
        assert_eq!(items[1].attr("v"), Some("1"));
        assert!(r.only("t", &["item"]).is_ok());
        assert!(r.only("t", &["other"]).is_err());
        assert!(parse(b"<a>&nope;</a>", "t").is_err());
        assert!(parse(b"<a></b>", "t").is_err());
        assert!(parse(b"<a/><b/>", "t").is_err());
        assert!(parse(b"<!DOCTYPE a><a/>", "t").is_err());
        assert!(parse(b"<a>", "t").is_err());
    }
}
