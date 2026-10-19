//! The cell spool's folder: a file of share records per collision cell ([`super::codec`]), appended to as each part's
//! shares are taken and read back, then removed, as each cell is given.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: the cell spool writes the shares in the folder the caller names, reads and removes them"
)]

use std::collections::{BTreeSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

use primase_core::World;
use primase_core::convert::Spool;

use crate::components::{CellKey, SkateCellLayout, SkateCellShare};

/// What the spool did: the parts it took shares from, the shares, their triangles and the cells, the bytes it wrote,
/// and the most share payload ([`SkateCellShare::payload_bytes`]) it held in memory at once (a part's shares as it
/// takes them, a cell's as it gives it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpoolReport {
    pub parts: usize,
    pub shares: usize,
    pub triangles: usize,
    pub cells: usize,
    pub bytes_written: u64,
    pub peak_payload_bytes: usize,
}

/// See the module documentation and [`super`].
#[derive(Debug)]
pub struct CellSpool {
    dir: PathBuf,
    layout: Option<SkateCellLayout>,
    keys: BTreeSet<CellKey>,
    /// The cells still to give, once giving began.
    giving: Option<VecDeque<CellKey>>,
    report: SpoolReport,
}

fn io(path: &Path, e: std::io::Error) -> primase_core::convert::Error {
    format!("the cell spool, {}: {e}", path.display()).into()
}

impl CellSpool {
    /// A spool keeping its files in `dir`, made now; an error when `dir` exists and is not empty (the spool removes
    /// what it writes, and nothing else).
    pub fn new(dir: &Path) -> primase_core::convert::Result<CellSpool> {
        if dir.exists() {
            let mut entries = std::fs::read_dir(dir).map_err(|e| io(dir, e))?;
            if entries.next().is_some() {
                return Err(format!(
                    "the cell spool's folder {} is not empty: give it a folder of its own",
                    dir.display()
                )
                .into());
            }
        }
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        Ok(CellSpool {
            dir: dir.to_owned(),
            layout: None,
            keys: BTreeSet::new(),
            giving: None,
            report: SpoolReport::default(),
        })
    }

    /// What it did so far.
    pub fn report(&self) -> SpoolReport {
        self.report
    }

    fn file(&self, (water, x, y): CellKey) -> PathBuf {
        self.dir
            .join(format!("{}_{x}_{y}.bin", if water { "w" } else { "c" }))
    }
}

impl Spool for CellSpool {
    /// The part's shares (`skate.cell_share`, and the `skate.cell_layout` beside them) taken out of it and appended to
    /// their cells' files. A part with no layout shared nothing (`share-reskate-collision` not in the gather passes) and
    /// is refused, as is one whose layout differs from the parts' before it.
    fn take(&mut self, part: &mut World) -> primase_core::convert::Result<()> {
        if self.giving.is_some() {
            return Err("the cell spool was given a part after it began giving cells".into());
        }
        let layout = part.remove_resource::<SkateCellLayout>().ok_or(
            "the cell spool was given a part that shared no collision: share-reskate-collision in the gather passes",
        )?;
        match &self.layout {
            Some(l) if *l != layout => {
                return Err(
                    "the parts of one map share their collision for different layouts (skate.cell_layout)"
                        .into(),
                );
            }
            Some(_) => {}
            None => self.layout = Some(layout),
        }
        let held: Vec<_> = part.iter::<SkateCellShare>().map(|(e, _)| e).collect();
        let shares: Vec<SkateCellShare> = held
            .into_iter()
            .map(|e| {
                let s = part.remove::<SkateCellShare>(e).expect("listed");
                part.despawn(e);
                s
            })
            .collect();
        let live: usize = shares.iter().map(SkateCellShare::payload_bytes).sum();
        self.report.peak_payload_bytes = self.report.peak_payload_bytes.max(live);
        self.report.triangles += shares.iter().map(SkateCellShare::triangles).sum::<usize>();
        let mut by_cell: std::collections::BTreeMap<CellKey, Vec<u8>> = std::collections::BTreeMap::new();
        for s in &shares {
            super::codec::encode(s, by_cell.entry(s.cell).or_default());
        }
        drop(shares);
        for (key, bytes) in by_cell {
            let path = self.file(key);
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| io(&path, e))?;
            f.write_all(&bytes).map_err(|e| io(&path, e))?;
            self.report.bytes_written += bytes.len() as u64;
            self.report.shares += 1;
            self.keys.insert(key);
        }
        self.report.parts += 1;
        Ok(())
    }

    /// The next cell, in cell order (water last, then column, then row), whatever order the parts came in: a world with
    /// every part's share of it and the cell layout. Its file is removed once read; the folder, once every cell was
    /// given.
    fn next(&mut self) -> primase_core::convert::Result<Option<World>> {
        let keys = &self.keys;
        let giving = self.giving.get_or_insert_with(|| keys.iter().copied().collect());
        let Some(key) = giving.pop_front() else {
            if self.dir.exists() {
                std::fs::remove_dir(&self.dir).map_err(|e| io(&self.dir, e))?;
            }
            return Ok(None);
        };
        let path = self.file(key);
        let bytes = std::fs::read(&path).map_err(|e| io(&path, e))?;
        let shares =
            super::codec::decode(&bytes).map_err(|e| format!("the cell spool, {}: {e}", path.display()))?;
        std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
        drop(bytes);
        if let Some(s) = shares.iter().find(|s| s.cell != key) {
            return Err(format!(
                "the cell spool's file for the cell {key:?} holds a share of {:?}",
                s.cell
            )
            .into());
        }
        let live: usize = shares.iter().map(SkateCellShare::payload_bytes).sum();
        self.report.peak_payload_bytes = self.report.peak_payload_bytes.max(live);
        let mut w = World::new();
        w.insert_resource(self.layout.clone().expect("a cell's layout came with its shares"));
        for s in shares {
            let e = w.spawn();
            w.insert(e, s);
        }
        self.report.cells += 1;
        Ok(Some(w))
    }
}
