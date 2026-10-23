//! The permutation selector of a program record ([`crate::lookup::ProgramRecord`]): the arrays of the 168-byte record
//! that say which of the program's permutations a set of option values runs. Five of the record's twelve
//! `(u32 count, u64 pointer)` items (at 0x18, 12 bytes each; the pointers are offsets in the program section) are read:
//!
//! ```text
//! 0x30 options   count x 16 bytes: u64 name hash, u32 type, u16 count, u16 extra
//! 0x3c nodes     count x 6 bytes:  u16 k, u16 start, u16 length
//! 0x48 values    one u64 value hash per entry (as many as the entries)
//! 0x54 entries   one u16 per entry: a child node, or 0x8000 + a permutation index
//! 0x60 ids       the permutation ids, u64 each (their count is the number of permutations)
//! ```
//!
//! The nodes tile the entries in order (a node's entries are `start..start + length`) and form a decision tree: node 0
//! is the root, a node tests the option `k` (an index into the options, increasing along every path, 0 at the root),
//! each of its entries is one value of that option (the value hash) and leads to a child node (an entry below 0x8000
//! is the child's index) or ends the walk at a permutation (an entry of 0x8000 and over). A node may mix both kinds of
//! entry: for such a value the options after `k` do not matter. A node may name only some of its option's values.
//! The permutation indices of the leaves are every index below the permutation count, each once. A record with no
//! options, nodes or entries has one permutation, which every walk ends at.
//!
//! Measured on the game's data (one read-only dump of 130 sampled records that have a tree, `tests/selector_dump.rs`):
//! all of those tile, are such trees, and test one option per level; the 6 that mix child and leaf entries are the
//! ones whose leaves lie above the last option. Every other record is unmeasured until a census reads them all.
//! The options are the engine's: 43 of 1,345 option names appear in a material's depot block, so the values a walk
//! takes come from the caller, never from the material.
//!
//! Not decoded, kept as stored: the name hashes and the value hashes (no name is known for either, the hash function is
//! not known), the options' type, count and extra words, and the record's other seven items (0x18, 0x24 and 0x6c to
//! 0x9c: counts of 0 or 1 to 24 in the sample, whose meaning is unknown). A record this reader cannot place (an array
//! outside the section, entries that do not tile, a value named twice in a node, a node or a permutation reached twice
//! or not at all, an option out of range or not increasing) is refused, never guessed at.

use crate::lookup::RECORD_LEN;
use crate::{Rd, Result, bad};

const W: &str = "program selector";
/// The record items read: the options, the nodes, the value hashes, the entries and the permutation ids.
const OPTIONS: usize = 0x30;
const NODES: usize = 0x3c;
const VALUES: usize = 0x48;
const ENTRIES: usize = 0x54;
const IDS: usize = 0x60;
/// An entry at or over this ends the walk at the permutation `entry - LEAF`.
const LEAF: u16 = 0x8000;

/// An option of a program, as stored: the engine's name for it, hashed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectorOption {
    /// The hash of the option's name (the name is not known).
    pub name: u64,
    pub kind: u32,
    /// The record's count word of this option (1 in the sample; its meaning is not known).
    pub count: u16,
    pub extra: u16,
}

/// Where an entry leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Another node of the tree (an index in [`Selector::nodes`]).
    Node(usize),
    /// The walk ends: this index of the record's permutations.
    Permutation(usize),
}

/// One value of a node's option and where it leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    /// The hash of the option value.
    pub value: u64,
    pub to: Target,
}

/// A node: the option it tests (an index in [`Selector::options`]) and its entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub option: usize,
    pub edges: Vec<Edge>,
}

/// One end of the tree: the option values on the way (option index and value hash, in the order tested) and the
/// permutation it ends at. An option not on the way may take any value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub path: Vec<(usize, u64)>,
    pub permutation: usize,
}

/// Why a walk did not end at a permutation.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum WalkError {
    /// A value is needed for each option of the record, in the record's order.
    #[error("{given} option values for {options} options")]
    Count { given: usize, options: usize },
    /// The node tests the option, and the value is not one of its entries.
    #[error("option {option} has no entry for the value {value:#018x} at node {node}")]
    NoEdge { node: usize, option: usize, value: u64 },
}

/// A program record's selector, read and checked ([`Selector::read`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub options: Vec<SelectorOption>,
    /// The tree: node 0 is the root. Empty when the record has no options (then one permutation).
    pub nodes: Vec<Node>,
    /// How many permutations the record lists.
    pub permutations: usize,
}

impl Selector {
    /// The selector of `record` (a program record as stored), its arrays read from `section` (the program section of
    /// the lookup resource, the payload before its rows). Refused as the module documentation lists.
    pub fn read(section: &[u8], record: &[u8; RECORD_LEN]) -> Result<Self> {
        let head = Rd::new(record, "program record");
        let place = |at: usize| -> Result<(usize, usize)> {
            let ptr = usize::try_from(head.u64le(at + 4)?).map_err(|_| bad(W, at, "pointer out of range"))?;
            Ok((head.u32le(at)? as usize, ptr))
        };
        let array = |at: usize, size: usize| -> Result<Rd<'_>> {
            let (n, ptr) = place(at)?;
            let bytes = n.checked_mul(size).ok_or_else(|| bad(W, at, "array size"))?;
            Ok(Rd::new(Rd::new(section, W).bytes(ptr, bytes)?, W))
        };
        let (n_options, _) = place(OPTIONS)?;
        let (n_nodes, _) = place(NODES)?;
        let (n_values, _) = place(VALUES)?;
        let (n_entries, _) = place(ENTRIES)?;
        let (permutations, _) = place(IDS)?;
        array(IDS, 8)?;
        if n_options == 0 && n_nodes == 0 && n_values == 0 && n_entries == 0 {
            if permutations != 1 {
                return Err(bad(W, IDS, format!("{permutations} permutations and no options")));
            }
            return Ok(Self {
                options: Vec::new(),
                nodes: Vec::new(),
                permutations,
            });
        }
        if n_nodes == 0 || n_options == 0 {
            return Err(bad(W, NODES, format!("{n_nodes} nodes and {n_options} options")));
        }
        if n_values != n_entries {
            return Err(bad(
                W,
                VALUES,
                format!("{n_values} values for {n_entries} entries"),
            ));
        }
        let (opts, nodes, values, entries) = (
            array(OPTIONS, 16)?,
            array(NODES, 6)?,
            array(VALUES, 8)?,
            array(ENTRIES, 2)?,
        );
        let options = (0..n_options)
            .map(|i| {
                let at = i * 16;
                Ok(SelectorOption {
                    name: opts.u64le(at)?,
                    kind: opts.u32le(at + 8)?,
                    count: opts.u16le(at + 12)?,
                    extra: opts.u16le(at + 14)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // the nodes tile the entries in order
        let mut tree = Vec::with_capacity(n_nodes);
        let mut next = 0usize;
        for i in 0..n_nodes {
            let at = i * 6;
            let (k, start, len) = (
                nodes.u16le(at)? as usize,
                nodes.u16le(at + 2)? as usize,
                nodes.u16le(at + 4)? as usize,
            );
            if start != next || len == 0 {
                return Err(bad(
                    W,
                    at,
                    format!("node {i} starts at {start} with {len} entries, {next} expected"),
                ));
            }
            next = start
                .checked_add(len)
                .filter(|&end| end <= n_entries)
                .ok_or_else(|| bad(W, at, format!("node {i} runs past the {n_entries} entries")))?;
            if k >= n_options {
                return Err(bad(W, at, format!("node {i} tests option {k} of {n_options}")));
            }
            let mut edges: Vec<Edge> = Vec::with_capacity(len);
            for j in start..next {
                let value = values.u64le(j * 8)?;
                let entry = entries.u16le(j * 2)?;
                if edges.iter().any(|e| e.value == value) {
                    return Err(bad(
                        W,
                        at,
                        format!("node {i} names the value {value:#018x} twice"),
                    ));
                }
                let to = match entry.checked_sub(LEAF) {
                    Some(p) => Target::Permutation(usize::from(p)),
                    None => Target::Node(usize::from(entry)),
                };
                edges.push(Edge { value, to });
            }
            tree.push(Node { option: k, edges });
        }
        if next != n_entries {
            return Err(bad(
                W,
                NODES,
                format!("the nodes cover {next} of {n_entries} entries"),
            ));
        }
        if tree[0].option != 0 {
            return Err(bad(W, NODES, format!("the root tests option {}", tree[0].option)));
        }
        // a tree: every node but the root is some earlier option's child once; each permutation is a leaf once
        let mut named = vec![0usize; n_nodes];
        let mut ended = vec![0usize; permutations];
        for (i, node) in tree.iter().enumerate() {
            for e in &node.edges {
                match e.to {
                    Target::Node(c) => {
                        let child = tree.get(c).filter(|_| c != 0).ok_or_else(|| {
                            bad(W, NODES, format!("node {i} leads to node {c} of {n_nodes}"))
                        })?;
                        if child.option <= node.option {
                            return Err(bad(
                                W,
                                NODES,
                                format!(
                                    "node {i} (option {}) leads to node {c} (option {})",
                                    node.option, child.option
                                ),
                            ));
                        }
                        named[c] += 1;
                    }
                    Target::Permutation(p) => match ended.get_mut(p) {
                        Some(n) => *n += 1,
                        None => {
                            return Err(bad(
                                W,
                                NODES,
                                format!("node {i} ends at permutation {p} of {permutations}"),
                            ));
                        }
                    },
                }
            }
        }
        if let Some(i) = (1..n_nodes).find(|&i| named[i] != 1) {
            return Err(bad(W, NODES, format!("node {i} is led to {} times", named[i])));
        }
        if let Some(p) = ended.iter().position(|&n| n != 1) {
            return Err(bad(
                W,
                NODES,
                format!("permutation {p} is ended at {} times", ended[p]),
            ));
        }
        Ok(Self {
            options,
            nodes: tree,
            permutations,
        })
    }

    /// The permutation (an index in the record's permutations) the option values select: `values[i]` is the hash of
    /// the value of option `i`, one for each option in the record's order. An option the walk does not test is not
    /// looked at.
    pub fn walk(&self, values: &[u64]) -> std::result::Result<usize, WalkError> {
        if values.len() != self.options.len() {
            return Err(WalkError::Count {
                given: values.len(),
                options: self.options.len(),
            });
        }
        if self.nodes.is_empty() {
            return Ok(0);
        }
        let mut at = 0;
        loop {
            let node = &self.nodes[at];
            let value = values[node.option];
            match node.edges.iter().find(|e| e.value == value).map(|e| e.to) {
                Some(Target::Permutation(p)) => return Ok(p),
                Some(Target::Node(next)) => at = next,
                None => {
                    return Err(WalkError::NoEdge {
                        node: at,
                        option: node.option,
                        value,
                    });
                }
            }
        }
    }

    /// Every end of the tree with the option values on the way to it, in depth-first order of the entries.
    pub fn leaves(&self) -> Vec<Leaf> {
        fn go(s: &Selector, at: usize, path: &mut Vec<(usize, u64)>, out: &mut Vec<Leaf>) {
            let node = &s.nodes[at];
            for e in &node.edges {
                path.push((node.option, e.value));
                match e.to {
                    Target::Permutation(permutation) => out.push(Leaf {
                        path: path.clone(),
                        permutation,
                    }),
                    Target::Node(c) => go(s, c, path, out),
                }
                path.pop();
            }
        }
        let mut out = Vec::new();
        if self.nodes.is_empty() {
            out.push(Leaf {
                path: Vec::new(),
                permutation: 0,
            });
        } else {
            go(self, 0, &mut Vec::new(), &mut out);
        }
        out
    }

    /// The value hashes the tree names for option `option`, ascending (every node that tests it, joined).
    pub fn values_of(&self, option: usize) -> Vec<u64> {
        let mut v: Vec<u64> = (self.nodes.iter())
            .filter(|n| n.option == option)
            .flat_map(|n| n.edges.iter().map(|e| e.value))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record and the section its arrays are in, built by hand in the layout the module documents (made-up hashes).
    #[derive(Default)]
    struct Build {
        options: Vec<(u64, u32)>,
        /// (k, start, length)
        nodes: Vec<(u16, u16, u16)>,
        values: Vec<u64>,
        entries: Vec<u16>,
        ids: usize,
    }

    impl Build {
        fn finish(&self) -> (Vec<u8>, [u8; RECORD_LEN]) {
            let mut section = vec![0xEEu8; 16];
            let mut record = [0u8; RECORD_LEN];
            let mut put = |at: usize, count: usize, bytes: Vec<u8>| {
                // the arrays follow one another, each at an offset of 4
                while !section.len().is_multiple_of(4) {
                    section.push(0);
                }
                let ptr = section.len() as u64;
                section.extend(bytes);
                record[at..at + 4].copy_from_slice(&(count as u32).to_le_bytes());
                record[at + 4..at + 12].copy_from_slice(&ptr.to_le_bytes());
            };
            put(
                OPTIONS,
                self.options.len(),
                (self.options.iter())
                    .flat_map(|&(name, kind)| {
                        let mut o = name.to_le_bytes().to_vec();
                        o.extend(kind.to_le_bytes());
                        o.extend(1u16.to_le_bytes());
                        o.extend(0u16.to_le_bytes());
                        o
                    })
                    .collect(),
            );
            put(
                NODES,
                self.nodes.len(),
                (self.nodes.iter())
                    .flat_map(|&(k, s, l)| [k, s, l].into_iter().flat_map(u16::to_le_bytes))
                    .collect(),
            );
            put(
                VALUES,
                self.values.len(),
                self.values.iter().flat_map(|v| v.to_le_bytes()).collect(),
            );
            put(
                ENTRIES,
                self.entries.len(),
                self.entries.iter().flat_map(|e| e.to_le_bytes()).collect(),
            );
            put(
                IDS,
                self.ids,
                (0..self.ids as u64)
                    .flat_map(|i| (0x1000 + i).to_le_bytes())
                    .collect(),
            );
            (section, record)
        }

        fn read(&self) -> Result<Selector> {
            let (section, record) = self.finish();
            Selector::read(&section, &record)
        }
    }

    // option 0 has three values (10, 11, 12), option 1 two (20, 21)
    const A: u64 = 0xA0A0_0000_0000_0001;
    const B: u64 = 0xB0B0_0000_0000_0002;

    /// The full tree: the root tests option 0 and each of its three values leads to a node that tests option 1.
    fn full() -> Build {
        Build {
            options: vec![(A, 7), (B, 8)],
            nodes: vec![(0, 0, 3), (1, 3, 2), (1, 5, 2), (1, 7, 2)],
            values: vec![10, 11, 12, 20, 21, 20, 21, 20, 21],
            entries: vec![1, 2, 3, LEAF, LEAF + 1, LEAF + 2, LEAF + 3, LEAF + 4, LEAF + 5],
            ids: 6,
        }
    }

    /// The root names the value 10 as a leaf (the second option does not matter for it) and 11 as a node; the value 12
    /// of the first option is not named (a pruned subset), and the node under 11 names only one value of option 1.
    fn mixed() -> Build {
        Build {
            options: vec![(A, 7), (B, 8)],
            nodes: vec![(0, 0, 2), (1, 2, 1)],
            values: vec![10, 11, 20],
            entries: vec![LEAF, 1, LEAF + 1],
            ids: 2,
        }
    }

    #[test]
    fn a_full_tree_walks_each_assignment_to_its_permutation() {
        let s = full().read().unwrap();
        assert_eq!((s.options.len(), s.nodes.len(), s.permutations), (2, 4, 6));
        assert_eq!(
            s.options[0],
            SelectorOption {
                name: A,
                kind: 7,
                count: 1,
                extra: 0
            }
        );
        let mut seen = Vec::new();
        for (a, first) in [(10, 0), (11, 2), (12, 4)] {
            for (b, add) in [(20, 0), (21, 1)] {
                assert_eq!(s.walk(&[a, b]), Ok(first + add), "{a} {b}");
                seen.push(first + add);
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3, 4, 5]);
        assert_eq!(s.values_of(0), [10, 11, 12]);
        assert_eq!(s.values_of(1), [20, 21]);
        let leaves = s.leaves();
        assert_eq!(leaves.len(), 6);
        assert_eq!(
            leaves[3],
            Leaf {
                path: vec![(0, 11), (1, 21)],
                permutation: 3
            }
        );
    }

    #[test]
    fn a_leaf_above_the_last_option_ignores_the_options_after_it() {
        let s = mixed().read().unwrap();
        // under the value 10 the second option is not looked at, whatever it is
        for b in [20, 21, 0xDEAD] {
            assert_eq!(s.walk(&[10, b]), Ok(0));
        }
        assert_eq!(s.walk(&[11, 20]), Ok(1));
        assert_eq!(s.leaves().len(), 2);
        assert_eq!(s.leaves()[0].path, [(0, 10)]);
    }

    #[test]
    fn a_walk_that_leaves_the_tree_is_refused_with_where() {
        let s = mixed().read().unwrap();
        // a value of option 0 the root does not name (a pruned subset)
        assert_eq!(
            s.walk(&[12, 20]),
            Err(WalkError::NoEdge {
                node: 0,
                option: 0,
                value: 12
            })
        );
        // a value of option 1 the node under 11 does not name
        assert_eq!(
            s.walk(&[11, 21]),
            Err(WalkError::NoEdge {
                node: 1,
                option: 1,
                value: 21
            })
        );
        assert_eq!(s.walk(&[10]), Err(WalkError::Count { given: 1, options: 2 }));
        assert_eq!(
            s.walk(&[10, 20, 30]),
            Err(WalkError::Count { given: 3, options: 2 })
        );
    }

    #[test]
    fn a_record_with_no_options_has_one_permutation() {
        let s = Build {
            ids: 1,
            ..Build::default()
        }
        .read()
        .unwrap();
        assert_eq!(s.walk(&[]), Ok(0));
        assert_eq!(
            s.leaves(),
            [Leaf {
                path: Vec::new(),
                permutation: 0
            }]
        );
        assert!(s.nodes.is_empty());
        // two permutations and nothing to choose between them is not placed
        assert!(
            Build {
                ids: 2,
                ..Build::default()
            }
            .read()
            .is_err()
        );
    }

    #[test]
    fn what_cannot_be_placed_is_refused() {
        let broken: Vec<(&str, Build)> = vec![
            (
                "gap between nodes",
                Build {
                    nodes: vec![(0, 0, 1), (1, 2, 1)],
                    ..mixed()
                },
            ),
            (
                "nodes short of the entries",
                Build {
                    nodes: vec![(0, 0, 2)],
                    entries: vec![LEAF, LEAF + 1, LEAF + 1],
                    values: vec![10, 11, 12],
                    ..mixed()
                },
            ),
            (
                "empty node",
                Build {
                    nodes: vec![(0, 0, 0), (1, 0, 3)],
                    ..mixed()
                },
            ),
            (
                "option out of range",
                Build {
                    nodes: vec![(0, 0, 2), (2, 2, 1)],
                    ..mixed()
                },
            ),
            (
                "root not at option 0",
                Build {
                    nodes: vec![(1, 0, 2), (1, 2, 1)],
                    ..mixed()
                },
            ),
            (
                "child at the same option",
                Build {
                    nodes: vec![(0, 0, 2), (0, 2, 1)],
                    ..mixed()
                },
            ),
            (
                "value named twice",
                Build {
                    values: vec![10, 10, 20],
                    ..mixed()
                },
            ),
            (
                "child past the nodes",
                Build {
                    entries: vec![LEAF, 5, LEAF + 1],
                    ..mixed()
                },
            ),
            (
                "root named as a child",
                Build {
                    entries: vec![0, 1, LEAF + 1],
                    ..mixed()
                },
            ),
            (
                "node led to twice",
                Build {
                    entries: vec![1, 1, LEAF],
                    ..mixed()
                },
            ),
            (
                "node never led to",
                Build {
                    entries: vec![LEAF, LEAF + 1, LEAF + 1],
                    ..mixed()
                },
            ),
            (
                "permutation twice",
                Build {
                    entries: vec![LEAF, 1, LEAF],
                    ..mixed()
                },
            ),
            ("permutation never", Build { ids: 3, ..mixed() }),
            (
                "permutation out of range",
                Build {
                    entries: vec![LEAF, 1, LEAF + 2],
                    ..mixed()
                },
            ),
            (
                "values and entries differ in count",
                Build {
                    values: vec![10, 11],
                    ..mixed()
                },
            ),
            (
                "nodes without options",
                Build {
                    options: vec![],
                    ..mixed()
                },
            ),
        ];
        for (what, b) in broken {
            assert!(b.read().is_err(), "{what} was read");
        }
        // an array that lies outside the section
        let (section, record) = mixed().finish();
        assert!(Selector::read(&section[..section.len() - 1], &record).is_err());
        assert!(Selector::read(&[], &record).is_err());
        let mut far = record;
        far[NODES + 4..NODES + 12].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Selector::read(&section, &far).is_err());
    }
}
