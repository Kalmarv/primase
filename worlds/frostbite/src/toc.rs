//! Superbundle TOC (`*.toc`): a 0x22C-byte envelope, then a big-endian header, the bundles (name, inline region)
//! and the chunk table, each found through a perfect hash over its keys (lowercase bundle names, chunk GUIDs).
//! Base-game TOCs store names Huffman-coded (flag 4); Patch TOCs as plain strings, which is what [`write`] writes.

use crate::{Rd, Result, align, bad, latin1};

pub const ENVELOPE: usize = 0x22C;
pub const MAGIC: [u8; 4] = [0x00, 0xd1, 0xce, 0x01];
const FNV_SEED: u32 = 0x811C_9DC5;
const W: &str = "toc";

/// Where a file lives: (patch folder or base game, install chunk, archive number).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Loc {
    pub patch: bool,
    pub chunk: u32,
    pub archive: u16,
}

impl Loc {
    pub fn decode(v: u64) -> Self {
        Self {
            patch: v & (0xFF << 48) != 0,
            chunk: (v >> 16) as u32,
            archive: v as u16,
        }
    }
    pub fn encode(self) -> u64 {
        (u64::from(self.patch) << 48) | (u64::from(self.chunk) << 16) | u64::from(self.archive)
    }
}

/// A chunk row of a TOC: GUID (as stored in bundles: little endian) and its file, or removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TocChunk {
    pub guid: [u8; 16],
    /// `None` for a removed chunk.
    pub file: Option<(Loc, u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toc {
    pub flags: u32,
    /// (name, inline bundle region bytes), in file order.
    pub bundles: Vec<(String, Vec<u8>)>,
    pub chunks: Vec<TocChunk>,
    pub bundle_hash: Vec<i32>,
    pub chunk_hash: Vec<i32>,
}

/// Huffman-coded names: leaves are `!char` values, internal nodes counters, read in pairs (left, right).
struct Huffman {
    words: Vec<u32>,
    nodes: Vec<(u32, Option<(usize, usize)>)>,
    /// `None` for an empty table (a TOC without names).
    root: Option<usize>,
}

impl Huffman {
    fn read(r: &Rd, names_at: usize, names_n: usize, table_at: usize, table_n: usize) -> Result<Self> {
        let words = (0..names_n)
            .map(|i| r.u32be(names_at + 4 * i))
            .collect::<Result<Vec<_>>>()?;
        let mut nodes: Vec<(u32, Option<(usize, usize)>)> = Vec::new();
        let mut by_value = std::collections::BTreeMap::new();
        let (mut left, mut counter, mut root) = (None, 0u32, None);
        for i in 0..table_n {
            let v = r.u32be(table_at + 4 * i)?;
            let node = *by_value.entry(v).or_insert_with(|| {
                nodes.push((v, None));
                nodes.len() - 1
            });
            let Some(l) = left.take() else {
                left = Some(node);
                continue;
            };
            nodes.push((counter, Some((l, node))));
            let parent = nodes.len() - 1;
            by_value.entry(counter).or_insert(parent);
            root = Some(parent);
            counter += 1;
        }
        Ok(Self { words, nodes, root })
    }

    fn decode(&self, mut bit: usize) -> Result<String> {
        let nbits = self.words.len() * 32;
        let mut out = String::new();
        let root = self
            .root
            .ok_or_else(|| bad(W, bit / 8, "a name in a TOC with an empty Huffman table"))?;
        while bit < nbits {
            let mut n = root;
            while let Some((l, r)) = self.nodes[n].1 {
                if bit >= nbits {
                    return Err(bad(W, bit / 8, "Huffman name runs past the data"));
                }
                let one = (self.words[bit >> 5] >> (bit & 31)) & 1 != 0;
                n = if one { r } else { l };
                bit += 1;
            }
            let ch = !self.nodes[n].0 & 0xFFFF;
            if ch == 0 {
                return Ok(out);
            }
            out.push(char::from_u32(ch).ok_or_else(|| bad(W, bit / 8, "Huffman character is not a char"))?);
        }
        Err(bad(W, bit / 8, "Huffman name is unterminated"))
    }
}

pub fn read(data: &[u8]) -> Result<Toc> {
    if data.len() < ENVELOPE + 48 || data[..4] != MAGIC {
        return Err(bad(W, 0, "envelope is invalid"));
    }
    let r = Rd::new(&data[ENVELOPE..], W);
    let u = |i: usize| r.u32be(4 * i);
    let (bho, bdo, cho, cgo, nameso, cdo, flags) = (
        u(0)? as usize,
        u(1)? as usize,
        u(3)? as usize,
        u(4)? as usize,
        u(8)? as usize,
        u(9)? as usize,
        u(11)?,
    );
    let count = |i: usize| -> Result<usize> {
        usize::try_from(r.i32be(4 * i)?).map_err(|_| bad(W, 4 * i, "negative count"))
    };
    let (bcount, ccount, dcount) = (count(2)?, count(5)?, count(10)?);
    let huffman = if flags & 4 != 0 {
        let (n, t, at) = (
            r.u32be(48)? as usize,
            r.u32be(52)? as usize,
            r.u32be(56)? as usize,
        );
        Some(Huffman::read(&r, nameso, n, at, t)?)
    } else {
        None
    };
    let mut toc = Toc {
        flags,
        bundles: Vec::new(),
        chunks: Vec::new(),
        bundle_hash: Vec::new(),
        chunk_hash: Vec::new(),
    };
    toc.bundle_hash = (0..bcount).map(|i| r.i32be(bho + 4 * i)).collect::<Result<_>>()?;
    for i in 0..bcount {
        let at = bdo + 16 * i;
        let (no, sf, ro) = (r.u32be(at)? as usize, r.u32be(at + 4)?, r.u64be(at + 8)?);
        if sf >> 30 != 1 {
            return Err(bad(W, at, "bundle region is not inline"));
        }
        let name = match &huffman {
            Some(h) => h.decode(no)?,
            None => r.cstr(nameso + no)?,
        };
        let ro = usize::try_from(ro).map_err(|_| bad(W, at, "region offset"))?;
        toc.bundles
            .push((name, r.bytes(ro, (sf & 0x3FFF_FFFF) as usize)?.to_vec()));
    }
    if ccount != 0 {
        toc.chunk_hash = (0..ccount).map(|i| r.i32be(cho + 4 * i)).collect::<Result<_>>()?;
        let words = (0..dcount)
            .map(|i| r.u32be(cdo + 4 * i))
            .collect::<Result<Vec<_>>>()?;
        for i in 0..ccount {
            let at = cgo + 20 * i;
            let mut guid: [u8; 16] = r.arr(at)?;
            guid.reverse();
            let enc = r.i32be(at + 16)?;
            if enc == -1 {
                toc.chunks.push(TocChunk { guid, file: None });
                continue;
            }
            let (flag, w) = ((enc as u32) >> 24, (enc as u32 & 0xFF_FFFF) as usize);
            if flag != 0x80 || w + 4 > words.len() {
                return Err(bad(W, at, "chunk data reference is invalid"));
            }
            let id = (u64::from(words[w]) << 32) | u64::from(words[w + 1]);
            toc.chunks.push(TocChunk {
                guid,
                file: Some((Loc::decode(id), words[w + 2], words[w + 3])),
            });
        }
    }
    Ok(toc)
}

/// The TOC's FNV-1: multiply by 0x01000193, xor the sign-extended byte, wrapping; reduced modulo 0x01000193 once at
/// the end. The tables' default seed is 0x811C9DC5.
fn hash_with(key: &[u8], seed: u32) -> u32 {
    key.iter()
        .fold(seed, |h, &b| h.wrapping_mul(0x01000193) ^ (b as i8 as i32 as u32))
        % 0x01000193
}

/// The perfect hash the TOC stores over `keys`: (hash map, key index per slot). Mirrors the game tools: buckets by
/// first hash, the multi-key buckets largest first (stable) get a seed that separates them, single keys fill free
/// slots in order, encoded as `-slot - 1`.
pub fn perfect_hash(keys: &[Vec<u8>]) -> Result<(Vec<i32>, Vec<usize>)> {
    let size = keys.len();
    let mut map = vec![-1i32; size];
    let mut order = vec![usize::MAX; size];
    if size == 0 {
        return Ok((map, order));
    }
    let mut seen = keys.to_vec();
    seen.sort();
    seen.dedup();
    if seen.len() != size {
        return Err(bad(W, 0, "perfect-hash keys are not unique"));
    }
    let first: Vec<usize> = keys
        .iter()
        .map(|k| hash_with(k, FNV_SEED) as usize % size)
        .collect();
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); size];
    for (i, &s) in first.iter().enumerate() {
        buckets[s].push(i);
    }
    buckets.sort_by_key(|b| std::cmp::Reverse(b.len()));
    let mut used = vec![false; size];
    let mut at = 0;
    while at < size && buckets[at].len() > 1 {
        let bucket = &buckets[at];
        let mut seed = 1u32;
        let slots = loop {
            let mut slots = Vec::with_capacity(bucket.len());
            let ok = bucket.iter().all(|&m| {
                let s = hash_with(&keys[m], seed) as usize % size;
                let free = !used[s] && !slots.contains(&s);
                slots.push(s);
                free
            });
            if ok {
                break slots;
            }
            seed += 1;
        };
        map[first[bucket[0]]] = seed as i32;
        for (i, &m) in bucket.iter().enumerate() {
            order[slots[i]] = m;
            used[slots[i]] = true;
        }
        at += 1;
    }
    let mut free = 0;
    while at < size && !buckets[at].is_empty() {
        while used[free] {
            free = (free + 1) % size;
        }
        let m = buckets[at][0];
        map[first[m]] = -(free as i32) - 1;
        order[free] = m;
        used[free] = true;
        free = (free + 1) % size;
        at += 1;
    }
    Ok((map, order))
}

/// Index of `key` in `ordered` through the stored hash map, as the game looks it up.
pub fn lookup(map: &[i32], ordered: &[Vec<u8>], key: &[u8]) -> Option<usize> {
    if map.is_empty() || map.len() != ordered.len() {
        return None;
    }
    let v = map[hash_with(key, FNV_SEED) as usize % map.len()];
    let i = if v < 0 {
        (-v - 1) as usize
    } else {
        hash_with(key, v as u32) as usize % map.len()
    };
    (ordered.get(i)? == key).then_some(i)
}

fn bundle_key(name: &str) -> Result<Vec<u8>> {
    latin1(&name.to_lowercase(), W)
}

/// Every bundle and chunk resolves through the stored perfect hashes.
pub fn verify(toc: &Toc) -> Result<()> {
    let keys = toc
        .bundles
        .iter()
        .map(|(n, _)| bundle_key(n))
        .collect::<Result<Vec<_>>>()?;
    for (i, k) in keys.iter().enumerate() {
        if lookup(&toc.bundle_hash, &keys, k) != Some(i) {
            return Err(bad(W, i, "bundle perfect hash does not resolve"));
        }
    }
    let keys: Vec<Vec<u8>> = toc.chunks.iter().map(|c| c.guid.to_vec()).collect();
    for (i, k) in keys.iter().enumerate() {
        if lookup(&toc.chunk_hash, &keys, k) != Some(i) {
            return Err(bad(W, i, "chunk perfect hash does not resolve"));
        }
    }
    Ok(())
}

/// A Patch TOC (plain names): bundles and chunks are laid out in perfect-hash order. `flags` must not ask for
/// Huffman names.
pub fn write(bundles: &[(String, Vec<u8>)], chunks: &[TocChunk], flags: u32) -> Result<Vec<u8>> {
    if flags & 4 != 0 {
        return Err(bad(W, 0, "the Patch TOC writer stores plain names"));
    }
    let bkeys = bundles
        .iter()
        .map(|(n, _)| bundle_key(n))
        .collect::<Result<Vec<_>>>()?;
    let (bmap, border) = perfect_hash(&bkeys)?;
    let ckeys: Vec<Vec<u8>> = chunks.iter().map(|c| c.guid.to_vec()).collect();
    let (cmap, corder) = perfect_hash(&ckeys)?;
    let mut names = Vec::new();
    let mut name_at = Vec::with_capacity(border.len());
    for &src in &border {
        name_at.push(names.len());
        names.extend(latin1(&bundles[src].0, W)?);
        names.push(0);
    }
    let mut words: Vec<u32> = Vec::new();
    let mut table = Vec::with_capacity(20 * chunks.len());
    for &src in &corder {
        let c = &chunks[src];
        table.extend(c.guid.iter().rev());
        match c.file {
            None => table.extend((-1i32).to_be_bytes()),
            Some((loc, off, size)) => {
                table.extend(((0x80u32 << 24) | words.len() as u32).to_be_bytes());
                let id = loc.encode();
                words.extend([(id >> 32) as u32, id as u32, off, size]);
            }
        }
    }
    let bho = 48;
    let bdo = align(bho + 4 * bundles.len(), 8);
    let cho = align(bdo + 16 * bundles.len(), 4);
    let cgo = align(cho + 4 * chunks.len(), 4);
    let cdo = align(cgo + table.len(), 4);
    let nameso = cdo + 4 * words.len();
    let mut pos = align(nameso + names.len(), 4);
    let mut region_at = Vec::with_capacity(border.len());
    for &src in &border {
        region_at.push(pos);
        pos = align(pos + bundles[src].1.len(), 4);
    }
    let mut out = vec![0u8; ENVELOPE + pos];
    out[..4].copy_from_slice(&MAGIC);
    let d = &mut out[ENVELOPE..];
    let put = |d: &mut [u8], at: usize, v: &[u8]| d[at..at + v.len()].copy_from_slice(v);
    let has_chunks = !chunks.is_empty();
    let head = [
        bho as u32,
        bdo as u32,
        bundles.len() as u32,
        if has_chunks { cho as u32 } else { 0 },
        if has_chunks { cgo as u32 } else { 0 },
        chunks.len() as u32,
        cdo as u32,
        cdo as u32,
        nameso as u32,
        cdo as u32,
        words.len() as u32,
        flags,
    ];
    for (i, v) in head.iter().enumerate() {
        put(d, 4 * i, &v.to_be_bytes());
    }
    for (i, v) in bmap.iter().enumerate() {
        put(d, bho + 4 * i, &v.to_be_bytes());
    }
    for (i, &src) in border.iter().enumerate() {
        let region = &bundles[src].1;
        if region.len() > 0x3FFF_FFFF {
            return Err(bad(W, i, "bundle region exceeds 30 bits"));
        }
        put(d, bdo + 16 * i, &(name_at[i] as u32).to_be_bytes());
        put(
            d,
            bdo + 16 * i + 4,
            &((1u32 << 30) | region.len() as u32).to_be_bytes(),
        );
        put(d, bdo + 16 * i + 8, &(region_at[i] as u64).to_be_bytes());
        put(d, region_at[i], region);
    }
    if has_chunks {
        for (i, v) in cmap.iter().enumerate() {
            put(d, cho + 4 * i, &v.to_be_bytes());
        }
        put(d, cgo, &table);
        for (i, w) in words.iter().enumerate() {
            put(d, cdo + 4 * i, &w.to_be_bytes());
        }
    }
    put(d, nameso, &names);
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// Reference values of the TOC hash (independent Python TOC hashing).
    #[test]
    fn toc_hash_reference_values() {
        assert_eq!(super::hash_with(&[0, 127, 128, 255], 0x811c9dc5), 0x529916);
        assert_eq!(super::hash_with(b"levels/test", 0x811c9dc5), 0xb97702);
    }
}
