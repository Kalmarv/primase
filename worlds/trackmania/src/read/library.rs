//! The game's documents ([`Library`]): each GameBox file of [`GameFiles`] read once, and a node reference followed to
//! the node it names, in its own file or in the file its reference table names (resolved from the naming file's path,
//! [`gbx_formats::RefTable::file_path`]).
use std::collections::BTreeMap;
use std::sync::Arc;

use gbx_formats::{Gbx, Node, NodeIndex, Nodes, RefTable};

use crate::Result;
use crate::read::files::GameFiles;

/// One GameBox file's nodes.
#[derive(Debug)]
pub struct Doc {
    /// Its path in the game's file system.
    pub path: String,
    pub nodes: Nodes,
    pub refs: RefTable,
}

impl Doc {
    /// The node at `index` (the main node at 0), when this file holds it.
    pub fn node(&self, index: NodeIndex) -> Option<&Node> {
        self.nodes.get(index)
    }
}

/// A node of a document: where a reference leads.
#[derive(Clone, Debug)]
pub struct At {
    pub doc: Arc<Doc>,
    pub index: NodeIndex,
}

impl At {
    pub fn node(&self) -> &Node {
        // an `At` is only made for a node its document holds
        self.doc.node(self.index).unwrap_or_else(|| self.doc.nodes.main())
    }

    /// A key naming the node across documents.
    pub fn key(&self) -> (String, u32) {
        (self.doc.path.to_lowercase(), self.index.0)
    }
}

/// The documents read so far, and what could not be read.
#[derive(Debug)]
pub struct Library<'f> {
    files: &'f GameFiles,
    docs: BTreeMap<String, Option<Arc<Doc>>>,
    /// Each file the game names that could not be read, with why (no such file, not GameBox, or the reader's error).
    pub unread: BTreeMap<String, String>,
}

impl<'f> Library<'f> {
    pub fn new(files: &'f GameFiles) -> Library<'f> {
        Library {
            files,
            docs: BTreeMap::new(),
            unread: BTreeMap::new(),
        }
    }

    /// The document at `path`, `None` when it can't be read (recorded in [`Library::unread`]). An error only for the
    /// files themselves (I/O, a pack's decryption).
    pub fn load(&mut self, path: &str) -> Result<Option<Arc<Doc>>> {
        let key = path.to_lowercase();
        if let Some(doc) = self.docs.get(&key) {
            return Ok(doc.clone());
        }
        let doc = match self.files.read(path)? {
            None => {
                self.unread.insert(path.to_owned(), "no such file".into());
                None
            }
            Some(bytes) if !bytes.starts_with(b"GBX") => {
                self.unread.insert(path.to_owned(), "not a GameBox file".into());
                None
            }
            Some(bytes) => match Gbx::parse(&bytes).and_then(|g| Ok((g.nodes()?, g.ref_table))) {
                Ok((nodes, refs)) => Some(Arc::new(Doc {
                    path: path.to_owned(),
                    nodes,
                    refs,
                })),
                Err(e) => {
                    self.unread.insert(path.to_owned(), e.to_string());
                    None
                }
            },
        };
        self.docs.insert(key, doc.clone());
        Ok(doc)
    }

    /// The main node of the document at `path`.
    pub fn main(&mut self, path: &str) -> Result<Option<At>> {
        Ok(self.load(path)?.map(|doc| At {
            doc,
            index: NodeIndex(0),
        }))
    }

    /// The node `reference` (made in `doc`) names: a node of `doc`, or the main node of the file its reference table
    /// names. `None` for a null reference, a resource, or a file that can't be read.
    pub fn follow(&mut self, doc: &Arc<Doc>, reference: Option<NodeIndex>) -> Result<Option<At>> {
        let Some(index) = reference else {
            return Ok(None);
        };
        if doc.node(index).is_some() {
            return Ok(Some(At {
                doc: doc.clone(),
                index,
            }));
        }
        let Some(path) = doc
            .refs
            .external(index)
            .and_then(|e| doc.refs.file_path(e, &doc.path))
        else {
            return Ok(None);
        };
        self.main(&path)
    }

    /// The path of the file `reference` (made in `doc`) names, when it names one.
    pub fn file_of(doc: &Doc, reference: Option<NodeIndex>) -> Option<String> {
        let index = reference?;
        if doc.node(index).is_some() {
            return None;
        }
        doc.refs
            .external(index)
            .and_then(|e| doc.refs.file_path(e, &doc.path))
    }
}
