//! Which archives the game mounts, and for each file name the live copy.
//!
//! The rules follow the game's mount data (`mount`): the base images (`x64*.rpf`, `common.rpf`) and the title
//! update (`update\update.rpf`) first; then the packs of the pack list in setup order (`order`, then `minorOrder`,
//! then list position). A pack mounts its enabled `RPF_FILE` data files and runs the change sets of its startup
//! groups (all but `GROUP_ON_DEMAND`, loaded by scripts, and `GROUP_MAP`): change sets enable and disable data
//! files and, per map, invalidate archives (which also unmounts the archives overlaying them) and enable others.
//! For GTA Online's map ([`MapState::Online`]) every pack's `GROUP_MAP` change sets then run in pack order, on top of
//! the started world, as the game runs them on entering Online; the story-mode map ([`MapState::Story`]) leaves them
//! out. Run pack by pack instead, the multiplayer packs' map changes (mpheist: invalidate story's
//! `downtown_01_metadata.rpf`, enable `hei_` maps) would come before the later story patches (patchday27ng's overlay
//! of the same archive) and both maps would be live at once. Change sets conditioned on another level than gta5's
//! (`$level=`, see [`LEVEL`]) and items for other platforms are skipped. Title-update mounts
//! (`update:/dlc_patch/<pack>/`) replace a pack's files; subpacks (`dlc1.rpf`, ...) hold what the pack's own archive
//! does not.
//!
//! Map data (ymap, ybn, ynv) is looked up in the map archives (the base images' archives and the archives packs
//! mount, keyed by their mounted path, later replacing earlier); everything else in every mounted archive. Archives
//! whose data file is an overlay (`<overlay value="true"/>`) replace files of the same name whenever they are
//! mounted; among plain archives, and among overlays, a later mount replaces an earlier one. The overlay rule is
//! read from the data, not from the game's code: patch packs ship fixed copies of other packs' files in overlay
//! archives while mounting before those packs (patchday21ng, order 21, and the vehicles of mpchristmas2018), and
//! overlay archives exist at paths no other archive has (`levels/patchday26ng/vehiclemods/...`), so the flag is
//! about names, not about one archive overlaying another.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the install's archives"
)]
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::archive::keys::Keys;
use crate::archive::rpf::{Archive, EntryKind};
use crate::mount::{self, Content, Setup};
use crate::{Result, invalid};

/// A file of the install: archive index and entry index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileRef {
    pub archive: usize,
    pub entry: usize,
}

/// Every archive of an install (top-level files and the archives inside them, depth first in path order) and a
/// path index over every file entry.
#[derive(Clone)]
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
        self.binary(r, keys).map(Some)
    }

    /// A binary file's contents.
    pub fn binary(&self, f: FileRef, keys: &Keys) -> Result<Vec<u8>> {
        let a = &self.archives[f.archive];
        a.binary(&mut File::open(&a.file)?, f.entry, keys)
    }

    /// A binary file as its archive stores it, decrypted: a compressed file's raw deflate stream with the size it
    /// inflates to, or an uncompressed file's contents with `None` ([`Archive::stored`](crate::archive::rpf::Archive::stored)).
    pub fn stored(&self, f: FileRef, keys: &Keys) -> Result<(Vec<u8>, Option<usize>)> {
        let a = &self.archives[f.archive];
        a.stored(&mut File::open(&a.file)?, f.entry, keys)
    }

    /// A resource file (RSC7), decoded.
    pub fn resource(&self, f: FileRef, keys: &Keys) -> Result<crate::resource::Resource> {
        let a = &self.archives[f.archive];
        a.resource(&mut File::open(&a.file)?, f.entry, keys)
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
    /// The mounted archives whose data file is an overlay (`<overlay value="true"/>`).
    pub overlay: BTreeSet<usize>,
    /// Live files by extension (`ymap`, `ydr`, ...) and the joaat of the lowercase name without extension.
    pub tables: BTreeMap<String, BTreeMap<u32, FileRef>>,
    /// The packs' enabled data files other than archives, in the order enabled.
    pub data_files: Vec<DataFileRef>,
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

/// Which map the world is: story mode's, or GTA Online's (story mode's plus the packs' `GROUP_MAP` change sets,
/// which the game runs on entering Online, after every pack's startup groups; every such change set that changes
/// anything belongs to a multiplayer pack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapState {
    Story,
    Online,
}

/// The archive of a pack's mounted path: its title-update patch folder, its own archive, then its subpacks.
fn physical(install: &Install, pack: &Pack, patch: Option<&str>, path: &str) -> Option<usize> {
    let device = pack.setup.device_name.to_lowercase();
    let rel = path.strip_prefix(&format!("{device}:/"))?.replace('/', "\\");
    patch
        .into_iter()
        .chain(std::iter::once(pack.archive.as_str()))
        .chain(pack.subpacks.iter().map(String::as_str))
        .find_map(|r| install.archive(&format!("{r}\\{rel}")))
}

/// A pack's enabled data file other than an archive (`GTXD_PARENTING_DATA`, metadata files, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataFileRef {
    pub file_type: String,
    /// The mounted path (`dlcmpheist:/common/data/gtxd.meta`).
    pub path: String,
    /// The file, when it exists in the pack's title-update patch folder, archive or subpacks.
    pub file: Option<FileRef>,
}

/// The file of a pack's mounted path: its title-update patch folder, its own archive, then its subpacks.
fn physical_file(install: &Install, pack: &Pack, patch: Option<&str>, path: &str) -> Option<FileRef> {
    let device = pack.setup.device_name.to_lowercase();
    let rel = path.strip_prefix(&format!("{device}:/"))?.replace('/', "\\");
    patch
        .into_iter()
        .chain(std::iter::once(pack.archive.as_str()))
        .chain(pack.subpacks.iter().map(String::as_str))
        .find_map(|r| install.file(&format!("{r}\\{rel}")))
}

/// The mount state while packs are applied.
struct Mounts {
    /// Map archives by mounted path, in mount order.
    map: IndexMap<String, usize>,
    /// Every mount, in order (an archive may appear more than once).
    mounted: Vec<usize>,
    /// Base path -> the mounted paths overlaying it (removed with it when it is invalidated).
    overlays: BTreeMap<String, Vec<String>>,
    /// Whether each archive was last mounted as an overlay.
    overlay: BTreeMap<usize, bool>,
    /// Enabled data files other than archives, by mounted path, in the order enabled.
    data: IndexMap<String, DataFileRef>,
}

impl Mounts {
    fn enable_data(&mut self, install: &Install, pack: &Pack, patch: Option<&str>, d: &mount::DataFile) {
        let path = platform_path(&d.filename);
        let file = physical_file(install, pack, patch, &path);
        self.data.shift_remove(&path);
        self.data.insert(
            path.clone(),
            DataFileRef {
                file_type: d.file_type.clone(),
                path,
                file,
            },
        );
    }

    fn mount(&mut self, name: &str, archive: Option<usize>, overlay: bool) {
        let Some(i) = archive else { return };
        self.map.shift_remove(name);
        self.map.insert(name.to_string(), i);
        self.mounted.push(i);
        self.overlay.insert(i, overlay);
        let o = overlaid(name);
        if o != name {
            self.overlays.entry(o).or_default().push(name.to_string());
        }
    }

    /// Runs a pack's change sets of the groups `groups` accepts, in group order.
    fn run_groups(
        &mut self,
        install: &Install,
        pack: &Pack,
        patch: Option<&str>,
        groups: impl Fn(&str) -> bool,
    ) {
        let data_file = |name: &str| {
            pack.content
                .data_files
                .iter()
                .find(|d| platform_path(&d.filename) == name)
        };
        for (group, sets) in &pack.setup.groups {
            if !groups(group) {
                continue;
            }
            // a name may be defined more than once, each copy for its own level: run the ones for this level
            let named = sets.iter().flat_map(|name| {
                pack.content
                    .change_sets
                    .iter()
                    .filter(move |c| c.name.eq_ignore_ascii_case(name))
            });
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
                    match data_file(&f) {
                        Some(d) if d.file_type == "RPF_FILE" => {
                            self.mount(&f, physical(install, pack, patch, &f), d.overlay)
                        }
                        Some(d) => self.enable_data(install, pack, patch, d),
                        None => {}
                    }
                }
                for f in &cs.files_to_disable {
                    let f = platform_path(f);
                    self.map.shift_remove(&f);
                    self.data.shift_remove(&f);
                }
                for mc in &cs.map_changes {
                    for f in &mc.files_to_invalidate {
                        let f = platform_path(f);
                        if !f.ends_with(".rpf") {
                            continue;
                        }
                        for o in self.overlays.remove(&f).unwrap_or_default() {
                            self.map.shift_remove(&o);
                        }
                        self.map.shift_remove(&f);
                    }
                    for f in &mc.files_to_enable {
                        let f = platform_path(f);
                        match data_file(&f) {
                            Some(d) if d.file_type != "RPF_FILE" => self.enable_data(install, pack, patch, d),
                            d => self.mount(
                                &f,
                                physical(install, pack, patch, &f),
                                d.is_some_and(|d| d.overlay),
                            ),
                        }
                    }
                }
            }
        }
    }
}

impl ArchiveSet {
    pub fn build(install: Install, keys: &Keys, state: MapState) -> Result<Self> {
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

        let mut m = Mounts {
            map,
            mounted,
            overlays: BTreeMap::new(),
            overlay: BTreeMap::new(),
            data: IndexMap::new(),
        };
        // startup: each pack mounts its archives and enabled data files and runs its startup change-set groups
        for pack in &packs {
            for a in std::iter::once(&pack.archive).chain(&pack.subpacks) {
                if let Some(i) = install.archive(a) {
                    m.mounted.push(i);
                }
            }
            let patch = patch_for(&pack.setup.device_name);
            for d in pack.content.data_files.iter().filter(|d| !d.disabled) {
                if d.file_type == "RPF_FILE" {
                    let name = platform_path(&d.filename);
                    m.mount(
                        &name,
                        physical(&install, pack, patch.as_deref(), &name),
                        d.overlay,
                    );
                } else {
                    m.enable_data(&install, pack, patch.as_deref(), d);
                }
            }
            m.run_groups(&install, pack, patch.as_deref(), |g| {
                !g.eq_ignore_ascii_case("GROUP_ON_DEMAND") && !g.eq_ignore_ascii_case("GROUP_MAP")
            });
        }
        // GTA Online's map: the packs' map groups, on top of the started world
        if state == MapState::Online {
            for pack in &packs {
                let patch = patch_for(&pack.setup.device_name);
                m.run_groups(&install, pack, patch.as_deref(), |g| {
                    g.eq_ignore_ascii_case("GROUP_MAP")
                });
            }
        }
        let Mounts {
            map,
            mounted,
            overlay,
            data,
            ..
        } = m;
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
                    primase_foundations::hash::joaat(stem.as_bytes()),
                    FileRef {
                        archive: ai,
                        entry: ei,
                    },
                );
            }
        };
        let overlay: BTreeSet<usize> = overlay.into_iter().filter(|&(_, o)| o).map(|(i, _)| i).collect();
        // overlay archives replace files of the same name whenever they are mounted: plain archives first, then
        // overlays, each in mount order
        for overlays_pass in [false, true] {
            for &ai in map.values().filter(|ai| overlay.contains(ai) == overlays_pass) {
                add(&mut tables, ai, true);
            }
            for &ai in mounted.iter().filter(|ai| overlay.contains(ai) == overlays_pass) {
                add(&mut tables, ai, false);
            }
        }
        Ok(ArchiveSet {
            install,
            packs,
            map_archives: map,
            mounted,
            overlay,
            tables,
            data_files: data.into_values().collect(),
        })
    }

    /// The live file of this kind and name (`("ydr", "dt1_13_build2")`).
    pub fn live(&self, ext: &str, name: &str) -> Option<FileRef> {
        self.tables
            .get(ext)?
            .get(&primase_foundations::hash::joaat(name.as_bytes()))
            .copied()
    }

    /// A file's path.
    pub fn path(&self, f: FileRef) -> &str {
        &self.install.archives[f.archive].entries[f.entry].path
    }
}

impl ArchiveSet {
    /// An archive set that is not over an install, for code that consumes one: a single archive (`test.rpf`, never
    /// opened) holding a file at each of `paths`, and the live tables `live` naming them (extension, name hash, index
    /// into `paths`). Sets made from the same `paths` are over one install.
    pub fn synthetic(paths: &[&str], live: &[(&str, u32, usize)]) -> ArchiveSet {
        let entries: Vec<crate::archive::rpf::Entry> = paths
            .iter()
            .map(|p| crate::archive::rpf::Entry {
                name: p.rsplit('\\').next().unwrap_or(p).to_string(),
                path: p.to_string(),
                kind: EntryKind::Binary {
                    block: 0,
                    stored: 0,
                    size: 0,
                    encrypted: false,
                },
            })
            .collect();
        let archive = Archive {
            file: PathBuf::from("test.rpf"),
            start: 0,
            size: 0,
            name: "test.rpf".into(),
            path: "test.rpf".into(),
            encryption: crate::archive::rpf::Encryption::None,
            entries,
        };
        let mut tables: BTreeMap<String, BTreeMap<u32, FileRef>> = BTreeMap::new();
        for &(ext, hash, entry) in live {
            tables
                .entry(ext.to_string())
                .or_default()
                .insert(hash, FileRef { archive: 0, entry });
        }
        ArchiveSet {
            install: Install {
                dir: PathBuf::from("install"),
                archives: vec![archive],
                by_path: BTreeMap::new(),
                files: BTreeMap::new(),
            },
            packs: Vec::new(),
            map_archives: IndexMap::new(),
            mounted: vec![0],
            overlay: BTreeSet::new(),
            tables,
            data_files: Vec::new(),
        }
    }
}
