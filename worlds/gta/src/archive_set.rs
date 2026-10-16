//! Which archives the game mounts, and for each file name the live copy.
//!
//! The rules follow the game's mount data (`mount`): the base images (`x64*.rpf`, `common.rpf`) and the title
//! update (`update\update.rpf`) first; then the packs of the pack list in setup order (`order`, then `minorOrder`,
//! then list position). A pack mounts its enabled `RPF_FILE` data files and runs every change set of its change-set
//! groups except `GROUP_ON_DEMAND` (loaded by scripts, not by the world): change sets enable and disable data files
//! and, per map, invalidate archives (which also unmounts the archives overlaying them) and enable others. Change
//! sets conditioned on another level than gta5's (`$level=`, see [`LEVEL`]) and items for other platforms are
//! skipped. Title-update mounts
//! (`update:/dlc_patch/<pack>/`) replace a pack's files; subpacks (`dlc1.rpf`, ...) hold what the pack's own archive
//! does not.
//!
//! Map data (ymap, ybn, ynv) is looked up in the map archives (the base images' archives and the archives packs
//! mount, keyed by their mounted path, later replacing earlier); everything else in every mounted archive in mount
//! order. A later copy of a name replaces an earlier one.
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::keys::Keys;
use crate::mount::{self, Content, Setup};
use crate::rpf::{Archive, EntryKind};
use crate::{Result, invalid};

/// A file of the install: archive index and entry index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileRef {
    pub archive: usize,
    pub entry: usize,
}

/// Every archive of an install (top-level files and the archives inside them, depth first in path order) and a
/// path index over every file entry.
pub struct Install {
    pub dir: PathBuf,
    pub archives: Vec<Archive>,
    by_path: BTreeMap<String, usize>,
    files: BTreeMap<String, FileRef>,
}

fn rpf_files(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
    let mut items: Vec<_> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    items.sort();
    for p in items {
        let rel = p
            .strip_prefix(root)
            .map_err(|_| invalid("install", 0, "path outside the install"))?
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase();
        // mod managers' folders are not the game's data
        if rel == "mods" {
            continue;
        }
        if p.is_dir() {
            rpf_files(&p, root, out)?;
        } else if rel.ends_with(".rpf") {
            out.push((p, rel));
        }
    }
    Ok(())
}

impl Install {
    /// Opens every archive under `dir` (except a `mods` folder) with `keys`.
    pub fn open(dir: &Path, keys: &Keys) -> Result<Self> {
        let mut files = Vec::new();
        rpf_files(dir, dir, &mut files)?;
        let mut archives = Vec::new();
        for (path, rel) in files {
            let a = Archive::open(&path, &rel, keys)?;
            let mut f = File::open(&path)?;
            let mut stack = vec![a];
            while let Some(a) = stack.pop() {
                let nested: Vec<usize> = a
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| matches!(e.kind, EntryKind::Binary { .. }) && e.path.ends_with(".rpf"))
                    .map(|(i, _)| i)
                    .collect();
                let children: Vec<Archive> = nested
                    .iter()
                    .map(|&i| a.nested(&mut f, i, keys))
                    .collect::<Result<_>>()?;
                archives.push(a);
                stack.extend(children.into_iter().rev());
            }
        }
        let mut by_path = BTreeMap::new();
        let mut index = BTreeMap::new();
        for (ai, a) in archives.iter().enumerate() {
            by_path.insert(a.path.clone(), ai);
            for (ei, e) in a.entries.iter().enumerate() {
                if !matches!(e.kind, EntryKind::Directory { .. }) {
                    index.insert(
                        e.path.clone(),
                        FileRef {
                            archive: ai,
                            entry: ei,
                        },
                    );
                }
            }
        }
        Ok(Install {
            dir: dir.to_path_buf(),
            archives,
            by_path,
            files: index,
        })
    }

    /// The archive at this path (lowercase, `\`).
    pub fn archive(&self, path: &str) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    /// The file entry at this path.
    pub fn file(&self, path: &str) -> Option<FileRef> {
        self.files.get(path).copied()
    }

    /// A binary file's contents by path; `None` when there is no such file.
    pub fn read(&self, path: &str, keys: &Keys) -> Result<Option<Vec<u8>>> {
        let Some(r) = self.file(path) else { return Ok(None) };
        let a = &self.archives[r.archive];
        let mut f = File::open(&a.file)?;
        a.binary(&mut f, r.entry, keys).map(Some)
    }
}

impl std::fmt::Debug for Install {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Install")
            .field("archives", &self.archives.len())
            .field("files", &self.files.len())
            .finish()
    }
}

/// A pack: where its archive is, its setup and content.
#[derive(Clone, Debug)]
pub struct Pack {
    /// The pack's archive (`...\dlcpacks\<name>\dlc.rpf`).
    pub archive: String,
    pub setup: Setup,
    pub content: Content,
    /// Subpack archives (`dlc1.rpf`, ...) that exist.
    pub subpacks: Vec<String>,
}

/// The mounted archives and the live file tables.
pub struct ArchiveSet {
    pub install: Install,
    /// Packs in mount order.
    pub packs: Vec<Pack>,
    /// Map archives in mount order, by mounted path (`x64/levels/gta5/...rpf`, `dlcmpheist:/x64/...rpf`).
    pub map_archives: IndexMap<String, usize>,
    /// Every mounted archive, in mount order (an archive mounted twice counts at its last mount).
    pub mounted: Vec<usize>,
    /// Live files by extension (`ymap`, `ydr`, ...) and the joaat of the lowercase name without extension.
    pub tables: BTreeMap<String, BTreeMap<u32, FileRef>>,
}

impl std::fmt::Debug for ArchiveSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveSet")
            .field("packs", &self.packs.len())
            .field("map_archives", &self.map_archives.len())
            .field("mounted", &self.mounted.len())
            .finish()
    }
}

/// `%PLATFORM%` expanded, `\` as `/`, lowercase.
fn platform_path(p: &str) -> String {
    let p = p
        .replace('\\', "/")
        .to_lowercase()
        .replace("%platform%", mount::PLATFORM);
    // `platform:/levels/...` names the base images' `x64/levels/...`
    match p.strip_prefix("platform:/") {
        Some(rest) => format!("{}/{rest}", mount::PLATFORM),
        None => p,
    }
}

/// A device name without its `:/` (`dlcMPBeach:/` -> `dlcmpbeach`).
fn bare_device(d: &str) -> String {
    d.trim()
        .trim_end_matches('/')
        .trim_end_matches(':')
        .to_lowercase()
}

/// A mounted path with its device removed (`dlcmpheist:/x64/a.rpf` -> `x64/a.rpf`): the base path it overlays.
fn overlaid(path: &str) -> String {
    match path.split_once(":/") {
        Some((_, rest)) => rest.to_string(),
        None => path.to_string(),
    }
}

const MAP_KINDS: [&str; 3] = ["ymap", "ybn", "ynv"];

/// The level whose world is read, by the name change-set conditions use (`$level=...`): the level list
/// (`x64a.rpf\data\levels.ymt`) gives level `gta5` the friendly name `MO_JIM_L11`. Change sets conditioned on
/// another level are skipped.
pub const LEVEL: &str = "MO_JIM_L11";

impl ArchiveSet {
    pub fn build(install: Install, keys: &Keys) -> Result<Self> {
        let read = |path: &str| -> Result<Vec<u8>> {
            install
                .read(path, keys)?
                .ok_or_else(|| invalid("install", 0, format!("{path} not found")))
        };
        let mut map: IndexMap<String, usize> = IndexMap::new();
        let mut mounted: Vec<usize> = Vec::new();
        let base_top = |p: &str| !p.starts_with("update\\") && !p.contains("\\dlcpacks\\");
        // the base images: every archive not under update\ and not inside a pack
        for (i, a) in install.archives.iter().enumerate() {
            if !base_top(&a.path) {
                continue;
            }
            mounted.push(i);
            let p = a.path.replace('\\', "/");
            let key = if p == "common.rpf" {
                "common".to_string()
            } else {
                match p.find('/') {
                    Some(s) => format!("x64{}", &p[s..]),
                    None => p,
                }
            };
            map.insert(key, i);
        }
        // the title update and the other files under update\ that are not packs
        for (i, a) in install.archives.iter().enumerate() {
            if a.path.starts_with("update\\") && !a.path.contains("\\dlcpacks\\") {
                mounted.push(i);
                if a.path == "update\\update.rpf" {
                    map.insert("update/update.rpf".to_string(), i);
                }
            }
        }
        // title-update mounts: device -> patch folder in update.rpf
        let tu = mount::title_update_mounts(
            &read("update\\update.rpf\\common\\data\\extratitleupdatedata.meta")?,
            "extratitleupdatedata.meta",
        )?;
        let patch_for = |device: &str| -> Option<String> {
            tu.iter()
                .find(|(d, _)| bare_device(d) == bare_device(device))
                .map(|(_, p)| {
                    let p = p.replace('\\', "/").to_lowercase();
                    format!(
                        "update\\update.rpf\\{}",
                        p.trim_start_matches("update:/")
                            .trim_end_matches('/')
                            .replace('/', "\\")
                    )
                })
        };
        // packs
        let list = mount::pack_list(&read("update\\update.rpf\\common\\data\\dlclist.xml")?)?;
        let mut packs = Vec::new();
        for (pos, item) in list.iter().enumerate() {
            let archive = if let Some(rest) = item.strip_prefix("dlcpacks:/") {
                format!(
                    "update\\x64\\dlcpacks\\{}\\dlc.rpf",
                    rest.trim_end_matches('/').replace('/', "\\")
                )
            } else if let Some(rest) = item.strip_prefix("platform:/") {
                let rel = format!("{}\\dlc.rpf", rest.trim_end_matches('/').replace('/', "\\"));
                // in one of the base images (x64?.rpf\dlcpacks\...)
                match install.archives.iter().find(|a| {
                    a.path
                        .split_once('\\')
                        .is_some_and(|(top, p)| top.starts_with("x64") && p == rel)
                }) {
                    Some(a) => a.path.clone(),
                    None => continue,
                }
            } else {
                return Err(invalid("dlclist.xml", 0, format!("pack path '{item}'")));
            };
            if install.archive(&archive).is_none() {
                continue; // listed but not installed
            }
            // the setup file may itself be replaced by the title update; the device name it would be keyed by is only
            // known from the file, so the patch folder is found by the pack's folder name
            let folder_name = archive
                .trim_end_matches("\\dlc.rpf")
                .rsplit('\\')
                .next()
                .unwrap_or_default()
                .to_string();
            let setup_path = tu
                .iter()
                .map(|(_, p)| p.replace('\\', "/").to_lowercase())
                .find(|p| p.trim_end_matches('/').rsplit('/').next() == Some(folder_name.as_str()))
                .map(|p| {
                    format!(
                        "update\\update.rpf\\{}\\setup2.xml",
                        p.trim_start_matches("update:/")
                            .trim_end_matches('/')
                            .replace('/', "\\")
                    )
                })
                .filter(|p| install.file(p).is_some())
                .unwrap_or_else(|| format!("{archive}\\setup2.xml"));
            let setup = mount::setup(&read(&setup_path)?, &setup_path)?;
            let content_path = format!("{archive}\\{}", setup.dat_file.to_lowercase());
            let content_path = patch_for(&setup.device_name)
                .map(|p| format!("{p}\\{}", setup.dat_file.to_lowercase()))
                .filter(|p| install.file(p).is_some())
                .unwrap_or(content_path);
            let content = mount::content(&read(&content_path)?, &content_path)?;
            let folder = archive.trim_end_matches("dlc.rpf");
            let subpacks = (1..=setup.sub_pack_count)
                .map(|i| format!("{folder}dlc{i}.rpf"))
                .filter(|p| install.archive(p).is_some())
                .collect();
            packs.push((
                setup.order,
                setup.minor_order,
                pos,
                Pack {
                    archive,
                    setup,
                    content,
                    subpacks,
                },
            ));
        }
        packs.sort_by_key(|(o, m, p, _)| (*o, *m, *p));
        let packs: Vec<Pack> = packs.into_iter().map(|(_, _, _, p)| p).collect();

        let mut overlays: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for pack in &packs {
            if let Some(i) = install.archive(&pack.archive) {
                mounted.push(i);
            }
            for s in &pack.subpacks {
                if let Some(i) = install.archive(s) {
                    mounted.push(i);
                }
            }
            let device = pack.setup.device_name.to_lowercase();
            let patch = patch_for(&pack.setup.device_name);
            // a mounted path of this pack -> the archive: its title-update patch, its own archive, then its subpacks
            let physical = |path: &str| -> Option<usize> {
                let rest = path.strip_prefix(&format!("{device}:/"))?;
                let rel = rest.replace('/', "\\");
                let mut roots = Vec::new();
                if let Some(p) = &patch {
                    roots.push(p.clone());
                }
                roots.push(pack.archive.clone());
                roots.extend(pack.subpacks.iter().cloned());
                roots.iter().find_map(|r| install.archive(&format!("{r}\\{rel}")))
            };
            let data_file = |name: &str| {
                pack.content
                    .data_files
                    .iter()
                    .find(|d| platform_path(&d.filename) == name)
            };
            let mount_file = |map: &mut IndexMap<String, usize>,
                              mounted: &mut Vec<usize>,
                              overlays: &mut BTreeMap<String, Vec<String>>,
                              name: &str| {
                if let Some(i) = physical(name) {
                    map.shift_remove(name);
                    map.insert(name.to_string(), i);
                    mounted.push(i);
                    let o = overlaid(name);
                    if o != name {
                        overlays.entry(o).or_default().push(name.to_string());
                    }
                }
            };
            for d in &pack.content.data_files {
                if d.file_type == "RPF_FILE" && !d.disabled {
                    mount_file(&mut map, &mut mounted, &mut overlays, &platform_path(&d.filename));
                }
            }
            for (group, sets) in &pack.setup.groups {
                if group.eq_ignore_ascii_case("GROUP_ON_DEMAND") {
                    continue;
                }
                // a name may be defined more than once, each copy for its own level: run the ones for this level
                let named: Vec<&mount::ChangeSet> = sets
                    .iter()
                    .flat_map(|name| {
                        pack.content
                            .change_sets
                            .iter()
                            .filter(move |c| c.name.eq_ignore_ascii_case(name))
                    })
                    .collect();
                for cs in named {
                    if cs
                        .level
                        .as_deref()
                        .is_some_and(|l| !l.eq_ignore_ascii_case(LEVEL))
                    {
                        continue;
                    }
                    for f in &cs.files_to_enable {
                        let f = platform_path(f);
                        if data_file(&f).is_some_and(|d| d.file_type == "RPF_FILE") {
                            mount_file(&mut map, &mut mounted, &mut overlays, &f);
                        }
                    }
                    for f in &cs.files_to_disable {
                        map.shift_remove(&platform_path(f));
                    }
                    for mc in &cs.map_changes {
                        for f in &mc.files_to_invalidate {
                            let f = platform_path(f);
                            if !f.ends_with(".rpf") {
                                continue;
                            }
                            for o in overlays.remove(&f).unwrap_or_default() {
                                map.shift_remove(&o);
                            }
                            map.shift_remove(&f);
                        }
                        for f in &mc.files_to_enable {
                            mount_file(&mut map, &mut mounted, &mut overlays, &platform_path(f));
                        }
                    }
                }
            }
        }
        // an archive mounted more than once counts at its last mount
        let mut last = BTreeMap::new();
        for (pos, &i) in mounted.iter().enumerate() {
            last.insert(i, pos);
        }
        let mut order: Vec<(usize, usize)> = last.into_iter().map(|(i, p)| (p, i)).collect();
        order.sort();
        let mounted: Vec<usize> = order.into_iter().map(|(_, i)| i).collect();

        let mut tables: BTreeMap<String, BTreeMap<u32, FileRef>> = BTreeMap::new();
        let add = |tables: &mut BTreeMap<String, BTreeMap<u32, FileRef>>, ai: usize, map_kinds: bool| {
            for (ei, e) in install.archives[ai].entries.iter().enumerate() {
                if matches!(e.kind, EntryKind::Directory { .. }) {
                    continue;
                }
                let name = e.path.rsplit('\\').next().unwrap_or_default();
                let Some((stem, ext)) = name.rsplit_once('.') else {
                    continue;
                };
                if MAP_KINDS.contains(&ext) != map_kinds || ext == "rpf" {
                    continue;
                }
                tables.entry(ext.to_string()).or_default().insert(
                    primase_hash::joaat(stem.as_bytes()),
                    FileRef {
                        archive: ai,
                        entry: ei,
                    },
                );
            }
        };
        for &ai in map.values() {
            add(&mut tables, ai, true);
        }
        for &ai in &mounted {
            add(&mut tables, ai, false);
        }
        Ok(ArchiveSet {
            install,
            packs,
            map_archives: map,
            mounted,
            tables,
        })
    }

    /// The live file of this kind and name (`("ydr", "dt1_13_build2")`).
    pub fn live(&self, ext: &str, name: &str) -> Option<FileRef> {
        self.tables
            .get(ext)?
            .get(&primase_hash::joaat(name.as_bytes()))
            .copied()
    }

    /// A file's path.
    pub fn path(&self, f: FileRef) -> &str {
        &self.install.archives[f.archive].entries[f.entry].path
    }
}
