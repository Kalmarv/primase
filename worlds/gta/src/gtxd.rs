//! Texture dictionary parents: when a texture is not in an archetype's dictionary, the game looks in that
//! dictionary's parent, then the parent's parent.
//!
//! The pairs (`CMapParentTxds`: `txdRelationships` of `Item`s with a `parent` and a `child` name) come from the live
//! `gtxd.ymt` (RBF, in `levels/gta5/generic/gtxd.rpf`, which patch packs overlay), the live `mph4_gtxd.ymt`, and the
//! packs' enabled `GTXD_PARENTING_DATA` files (XML), in that order. A child named again with another parent keeps its
//! first parent; such conflicts are listed, not hidden.
use std::collections::BTreeMap;

use crate::archive_set::{ArchiveSet, FileRef};
use crate::keys::Keys;
use crate::xml::Element;
use crate::{Result, invalid};

/// A child named twice with different parents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub child: String,
    pub kept: String,
    pub other: String,
    /// The source naming `other`.
    pub source: String,
}

/// The parent of each texture dictionary that has one, by the joaat of the lowercase names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextureParents {
    pub parents: BTreeMap<u32, u32>,
    /// Names by hash, for the dictionaries named in the pairs.
    pub names: BTreeMap<u32, String>,
    /// The files read, in order.
    pub sources: Vec<String>,
    pub conflicts: Vec<Conflict>,
}

/// The pairs of one `CMapParentTxds` tree: `(child, parent)` names as written.
fn pairs(root: &Element, what: &str) -> Result<Vec<(String, String)>> {
    let bad = |d: String| invalid("gtxd", 0, format!("{what}: {d}"));
    if !root.name.eq_ignore_ascii_case("CMapParentTxds") {
        return Err(bad(format!("root <{}>", root.name)));
    }
    root.only(what, &["txdRelationships"])?;
    let mut out = Vec::new();
    for rels in &root.children {
        rels.only(what, &["Item"])?;
        for item in rels.items() {
            item.only(what, &["parent", "child"])?;
            let (Some(parent), Some(child)) = (item.child_text("parent"), item.child_text("child")) else {
                return Err(bad("an item without parent or child".into()));
            };
            if parent.is_empty() || child.is_empty() {
                return Err(bad("an empty name".into()));
            }
            out.push((child.to_string(), parent.to_string()));
        }
    }
    Ok(out)
}

impl TextureParents {
    /// The parents the game uses for the archive set's world.
    pub fn read(set: &ArchiveSet, keys: &Keys) -> Result<TextureParents> {
        let mut files: Vec<(FileRef, bool)> = Vec::new();
        for name in ["gtxd", "mph4_gtxd"] {
            if let Some(f) = set.live("ymt", name) {
                files.push((f, true));
            }
        }
        for d in set
            .data_files
            .iter()
            .filter(|d| d.file_type == "GTXD_PARENTING_DATA")
        {
            let f = d
                .file
                .ok_or_else(|| invalid("gtxd", 0, format!("{}: enabled but not in its pack", d.path)))?;
            files.push((f, false));
        }
        let mut t = TextureParents::default();
        for (f, rbf) in files {
            let path = set.path(f).to_string();
            let a = &set.install.archives[f.archive];
            let bytes = a.binary(&mut std::fs::File::open(&a.file)?, f.entry, keys)?;
            let root = if rbf {
                crate::rbf::parse(&bytes)?
            } else {
                crate::xml::parse(&bytes, &path)?
            };
            for (child, parent) in pairs(&root, &path)? {
                let (c, p) = (
                    primase_hash::joaat(child.to_lowercase().as_bytes()),
                    primase_hash::joaat(parent.to_lowercase().as_bytes()),
                );
                match t.parents.get(&c) {
                    Some(&kept) if kept != p => t.conflicts.push(Conflict {
                        child: child.clone(),
                        kept: t.names[&kept].clone(),
                        other: parent.clone(),
                        source: path.clone(),
                    }),
                    Some(_) => {}
                    None => {
                        t.parents.insert(c, p);
                    }
                }
                t.names.entry(c).or_insert(child);
                t.names.entry(p).or_insert(parent);
            }
            t.sources.push(path);
        }
        Ok(t)
    }

    /// The dictionaries searched for `txd`'s textures after itself: its parent, the parent's parent, ... (a cycle
    /// stops the walk at its first repeat).
    pub fn chain(&self, txd: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut at = txd;
        while let Some(&p) = self.parents.get(&at) {
            if p == txd || out.contains(&p) {
                break;
            }
            out.push(p);
            at = p;
        }
        out
    }
}
