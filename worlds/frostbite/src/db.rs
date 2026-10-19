//! Frostbite's native DB (binary object tree): `layout.toc` (behind the TOC envelope) and the chunk metadata of a
//! bundle manifest. A node is a type tag (bit 7: unnamed), a name, then a container (1 list, 2 object: varint size,
//! children, optional terminator), a sized blob (7 string, 19 blob) or a fixed-size value.

use crate::toc::{ENVELOPE, MAGIC};
use crate::{Rd, Result, bad, latin1};

const W: &str = "native db";

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Node {
    pub ty: u8,
    pub named: bool,
    pub terminated: bool,
    pub name: String,
    pub payload: Vec<u8>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn field(&self, key: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.named && c.name == key)
    }
    /// A string node's text without its terminator.
    pub fn text(&self) -> String {
        let t = self.payload.strip_suffix(&[0]).unwrap_or(&self.payload);
        t.iter().map(|&b| char::from(b)).collect()
    }
    pub fn u32(&self) -> Option<u32> {
        (self.ty == 8 && self.payload.len() == 4)
            .then(|| u32::from_le_bytes([self.payload[0], self.payload[1], self.payload[2], self.payload[3]]))
    }
}

fn fixed_size(ty: u8) -> Option<usize> {
    Some(match ty {
        6 => 1,
        8 | 11 => 4,
        9 | 12 => 8,
        15 => 16,
        16 => 20,
        _ => return None,
    })
}

fn varint(r: &Rd, mut pos: usize) -> Result<(usize, usize)> {
    let (mut v, mut shift) = (0usize, 0u32);
    loop {
        let b = r.u8(pos)?;
        pos += 1;
        if shift > 56 {
            return Err(bad(W, pos, "varint too long"));
        }
        v |= usize::from(b & 127) << shift;
        if b & 128 == 0 {
            return Ok((v, pos));
        }
        shift += 7;
    }
}

/// One node at `pos`; the position after it.
pub fn read_node(data: &[u8], pos: usize) -> Result<(Node, usize)> {
    let r = Rd::new(data, W);
    let tag = r.u8(pos)?;
    let mut pos = pos + 1;
    let mut n = Node {
        ty: tag & 31,
        ..Node::default()
    };
    if n.ty == 0 {
        return Ok((n, pos));
    }
    n.named = tag & 128 == 0;
    if n.named {
        n.name = r.cstr(pos)?;
        pos += n.name.len() + 1;
    }
    match n.ty {
        1 | 2 => {
            let (size, p) = varint(&r, pos)?;
            let end = p + size;
            pos = p;
            while pos < end {
                let (child, p) = read_node(data, pos)?;
                pos = p;
                if child.ty == 0 {
                    n.terminated = true;
                    break;
                }
                n.children.push(child);
            }
            if pos != end {
                return Err(bad(W, pos, "container length mismatch"));
            }
        }
        7 | 19 => {
            let (size, p) = varint(&r, pos)?;
            n.payload = r.bytes(p, size)?.to_vec();
            pos = p + size;
        }
        t => {
            let size = fixed_size(t).ok_or_else(|| bad(W, pos, format!("unsupported value type {t}")))?;
            n.payload = r.bytes(pos, size)?.to_vec();
            pos += size;
        }
    }
    Ok((n, pos))
}

fn put_varint(out: &mut Vec<u8>, mut v: usize) {
    loop {
        let b = (v & 127) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 128);
    }
}

pub fn write_node(n: &Node, out: &mut Vec<u8>) -> Result<()> {
    if n.ty == 0 {
        out.push(0);
        return Ok(());
    }
    out.push(n.ty | if n.named { 0 } else { 128 });
    if n.named {
        out.extend(latin1(&n.name, W)?);
        out.push(0);
    }
    match n.ty {
        1 | 2 => {
            let mut body = Vec::new();
            for c in &n.children {
                write_node(c, &mut body)?;
            }
            if n.terminated {
                body.push(0);
            }
            put_varint(out, body.len());
            out.extend(body);
        }
        7 | 19 => {
            put_varint(out, n.payload.len());
            out.extend(&n.payload);
        }
        t => {
            if fixed_size(t) != Some(n.payload.len()) {
                return Err(bad(
                    W,
                    out.len(),
                    format!("value of type {t} has {} bytes", n.payload.len()),
                ));
            }
            out.extend(&n.payload);
        }
    }
    Ok(())
}

/// A DB file behind the TOC envelope: (envelope bytes, root, trailing bytes).
pub fn read_file(data: &[u8]) -> Result<(Vec<u8>, Node, Vec<u8>)> {
    if data.len() <= ENVELOPE || data[..4] != MAGIC {
        return Err(bad(W, 0, "unrecognised envelope"));
    }
    let (root, pos) = read_node(data, ENVELOPE)?;
    Ok((data[..ENVELOPE].to_vec(), root, data[pos..].to_vec()))
}

pub fn write_file(envelope: &[u8], root: &Node, trailer: &[u8]) -> Result<Vec<u8>> {
    let mut out = envelope.to_vec();
    write_node(root, &mut out)?;
    out.extend(trailer);
    Ok(out)
}
