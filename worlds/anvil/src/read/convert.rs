//! The Anvil reader of [`primase_core::convert`]: a section of a world read from an archive of forges, as the game's
//! own listing gives it. The World's GridDescriptions say where each grid lies, the named grids' GridPartitions their
//! cells, and each cell's GridCellDataBlock the Entities and EntityGroups standing in it; an EntityGroup's members come
//! with it. Each is placed through [`Placer`] (meshes, collision shapes, lakes, decals), and the terrain tiles under a
//! stated rectangle are read too.
//!
//! What the reader decides, it is told: the forges to search and their edition, the grids and which cells of each
//! ([`CellSelection`]), what an instanced visual or its collision places ([`InstancedLevels`], [`InstancedCollision`])
//! and the terrain read, if any ([`TerrainSelection`]). It keeps what the game lists and marks where each object was
//! listed ([`AnvilListed`]); which of it a program takes is a pass ([`crate::passes::SelectAnvilListed`]), as are the
//! world state, hidden and phantom entities and every lowering.
//!
//! [`Reader::read`] places everything except what is heavy and not needed to plan from: Materials, their textures and
//! shader permutations. [`Reader::load`] gives the part its roots reach and reads those: a prototype's Materials are
//! read once for the part, linked from its meshes' ranges. So a program's selection runs before a texture is read.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use primase_core::convert::{Notice, NoticeKind, Reader, Report, Result};
use primase_core::frame::SourceFrame;
use primase_core::{Entity, World};

use crate::components::{AnvilListed, AnvilMesh, AnvilTerrain, ListedBy};
use crate::read::component_types::component_name;
use crate::read::overview::{self, Counts, GridOverview};
use crate::read::placements::{InstancedCollision, InstancedLevels, Placed, Placer};
use crate::read::terrain::{TerrainRead, read_terrain};
use anvil_formats::archive::{Archive, At};
use anvil_formats::entity::{ENTITY, ENTITY_GROUP, StoredEntityGroup};
use anvil_formats::grid::{
    GridDescription, GridLayout, StoredGridCellDataBlock, StoredGridPartition, grid_descriptions,
};
use anvil_formats::listing::Kind;
use anvil_formats::material::TEXTURE_SET;
use anvil_formats::tnl::TERRAIN_NODE_LAYER;

/// Whether a forge is a base forge or its patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edition {
    Base,
    Patch,
}

impl Edition {
    /// The length of the u16 run before the five numbers of a World's GridLayout: 5 in a base forge's World, 4 in a
    /// patch's. Nothing stored tells them apart (the run's meaning is unknown, docs/placement.md §10): the edition of
    /// the forge the World comes from does.
    pub const fn grid_layout_words(self) -> usize {
        match self {
            Edition::Base => 5,
            Edition::Patch => 4,
        }
    }
}

/// A forge of the archive and which edition it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeFile {
    pub path: PathBuf,
    pub edition: Edition,
}

/// Which levels of a grid's cells are read over a rectangle. The finest level (0) has the grid's smallest squares;
/// each coarser level has squares twice the side. The coarsest level's one cell covers the whole world and lists world
/// volumes and far content, not what stands in a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Levels {
    /// All authored levels, including global far-view lists. The caller must spatially select their contents.
    All,
    /// Level 0 only.
    Finest,
    /// Every level but the coarsest. An object larger than a level-0 cell is listed only at a coarser level.
    AllButCoarsest,
}

impl Levels {
    fn reads(self, level: u32, levels: u32) -> bool {
        match self {
            Levels::All => true,
            Levels::Finest => level == 0,
            Levels::AllButCoarsest => level.saturating_add(1) < levels,
        }
    }
}

/// Which cells of a grid are read.
#[derive(Clone, Debug, PartialEq)]
pub enum CellSelection {
    /// These cells, by number (the GridPartition's record index).
    Numbered(Vec<usize>),
    /// The cells whose squares overlap `rect` (x0, y0, x1, y1, in the grid's units, metres: the game's x east and y
    /// north) in a positive area, at `levels`.
    Over { rect: [f64; 4], levels: Levels },
}

/// One grid of the World and which of its cells are read.
#[derive(Clone, Debug, PartialEq)]
pub struct GridSelection {
    /// The GridPartition: its entry's name, or the part after its last `" - "`.
    pub grid: String,
    pub cells: CellSelection,
}

/// The terrain tiles to read: where the quadtree lies and which of it, and the entries it needs, by name.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainSelection {
    /// The quadtree square's corner with the least x and y, and its side (the World's GridLayout square).
    pub origin: [f64; 2],
    pub side: f64,
    /// The level whose nodes are read (8: the leaves, at 0.5 m).
    pub level: u32,
    /// x0, y0, x1, y1: the nodes whose squares meet it are read.
    pub rect: [f64; 4],
    /// The TerrainNodeLayer by entry name; `None`: the first one in the forges' order.
    pub layer: Option<String>,
    /// The TerrainMaterialBank (`TMB_MAD`) the nodes' material rasters index, by entry name.
    pub bank: Option<String>,
    /// The TextureSet of the splatting noise (`TerrainSplattingNoise_Set`), by entry name.
    pub noise: Option<String>,
    /// Whether each node's grass rasters (slots 5 and 4) are read too ([`crate::components::AnvilTerrainGroups`]).
    pub groups: bool,
}

/// What to read. Nothing here has a default: each is a choice the program makes.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilOptions {
    /// The forges, searched in this order: the first holding an id answers. A patch before its base is the program's
    /// stated choice (the Riders Republic notes lead to "the patch wins"; not established from the game).
    pub forges: Vec<ForgeFile>,
    /// The frame the source's numbers are converted through (the notes: x east, y north, z up, metres).
    pub frame: SourceFrame,
    /// The grids and their cells; every listed object is placed once, in the order the listing gives them.
    pub grids: Vec<GridSelection>,
    /// What an instanced visual places at each instance.
    pub instanced_levels: InstancedLevels,
    /// What an instanced Entity's RigidBody places.
    pub instanced_collision: InstancedCollision,
    /// The terrain to read under the section, if any.
    pub terrain: Option<TerrainSelection>,
}

/// The stage the reader's notices name.
const STAGE: &str = "anvil reader";

/// The Anvil reader (see the module documentation).
pub struct AnvilReader {
    options: AnvilOptions,
    archive: Archive,
    pending: Vec<Notice>,
}

impl std::fmt::Debug for AnvilReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnvilReader")
            .field("forges", &self.options.forges)
            .field("grids", &self.options.grids)
            .finish_non_exhaustive()
    }
}

/// One cell's listing: its place and what it lists.
struct CellList {
    grid: String,
    cell: usize,
    level: u32,
    ids: Vec<u64>,
}

impl AnvilReader {
    /// Opens the forges' tables.
    pub fn open(options: &AnvilOptions) -> Result<AnvilReader> {
        if options.forges.is_empty() {
            return Err("anvil: no forges".into());
        }
        if options.grids.is_empty() {
            return Err("anvil: no grids selected".into());
        }
        let paths: Vec<&PathBuf> = options.forges.iter().map(|f| &f.path).collect();
        Ok(AnvilReader {
            options: options.clone(),
            archive: Archive::open(&paths)?,
            pending: Vec::new(),
        })
    }

    /// The World's grids with every cell of each, from the grid tables ([`overview::grids`]): what to select from.
    pub fn grids(&mut self, counts: Counts) -> Result<Vec<GridOverview>> {
        overview::grids(&mut self.archive, &self.options.forges, counts)
    }

    /// The first entry, in the forges' order, of `kind` whose name `want` accepts.
    fn find(&self, kind: Kind, want: impl Fn(&str) -> bool) -> Option<At> {
        self.archive
            .listing()
            .find(|(_, l)| l.kind == Some(kind) && want(l.name))
            .map(|(forge, l)| At {
                forge,
                entry: l.entry,
            })
    }

    fn name(&self, at: At) -> String {
        entry_name(&self.archive, at)
    }

    /// The cells the selected grids list, in the selection's order.
    fn listings(&mut self) -> Result<Vec<CellList>> {
        let descriptions = world_descriptions(&mut self.archive, &self.options.forges)?;
        let mut out = Vec::new();
        for selection in self.options.grids.clone() {
            let mut found = false;
            for d in &descriptions {
                let Some(partition) = d.partition else {
                    continue;
                };
                let Some(at) = self.archive.find(partition.id) else {
                    continue;
                };
                let name = self.name(at);
                if name != selection.grid && name.rsplit(" - ").next() != Some(selection.grid.as_str()) {
                    continue;
                }
                let r = self.archive.resource(at)?;
                if r.type_id != Kind::GridPartition.type_id() {
                    return Err(format!("{name}: type {:#010x}, not a GridPartition", r.type_id).into());
                }
                let p = StoredGridPartition::parse(&r.body).map_err(|e| format!("{name}: {e}"))?;
                let cells = match &selection.cells {
                    CellSelection::Numbered(cells) => cells.clone(),
                    CellSelection::Over { rect, levels } => {
                        cells_over(&d.layout, p.levels, p.cells.len(), *rect, *levels)
                    }
                };
                for cell in cells {
                    let level = d
                        .layout
                        .cell(p.levels, cell)
                        .ok_or_else(|| format!("{name}: no cell {cell} of {}", p.cells.len()))?
                        .level;
                    let block = p.cells.get(cell).copied().unwrap_or(0);
                    let mut ids = Vec::new();
                    if block != 0 {
                        let bat = self
                            .archive
                            .find(block)
                            .ok_or_else(|| format!("{name}: block {block:#x} in no forge"))?;
                        let b = self.archive.resource(bat)?;
                        if b.type_id != Kind::GridCellDataBlock.type_id() {
                            return Err(format!("{name}: block {block:#x}: type {:#010x}", b.type_id).into());
                        }
                        let listing = StoredGridCellDataBlock::parse(&b.body)
                            .map_err(|e| format!("{name} cell {cell}: {e}"))?;
                        ids = listing.objects.iter().map(|o| o.id).collect();
                    }
                    out.push(CellList {
                        grid: selection.grid.clone(),
                        cell,
                        level,
                        ids,
                    });
                }
                found = true;
                break;
            }
            if !found {
                return Err(format!("anvil: no GridPartition named {} in the World", selection.grid).into());
            }
        }
        Ok(out)
    }

    /// Reads the terrain under the stated rectangle into `world`.
    /// Read exactly the caller's terrain extent. Programs that select placements first can use this after `load`,
    /// avoiding both discarded terrain and a guessed margin around a cell's rectangle.
    pub fn read_terrain(&mut self, world: &mut World, t: &TerrainSelection) -> Result<Entity> {
        let layer = self
            .find(Kind::TerrainNodeLayer, |n| {
                t.layer.as_deref().is_none_or(|l| l == n)
            })
            .ok_or("anvil: no TerrainNodeLayer in the forges")?;
        debug_assert_eq!(
            self.archive
                .forge(layer.forge)
                .map(|f| f.names[layer.entry].type_id),
            Some(TERRAIN_NODE_LAYER)
        );
        let bank = match &t.bank {
            Some(n) => Some(
                self.find(Kind::TerrainMaterialBank, |m| m == n)
                    .ok_or_else(|| format!("anvil: no TerrainMaterialBank {n} in the forges"))?,
            ),
            None => None,
        };
        let noise = match &t.noise {
            Some(n) => {
                let found = self
                    .archive
                    .listing()
                    .find(|(_, l)| l.type_id == Some(TEXTURE_SET) && l.name == n)
                    .map(|(forge, l)| At {
                        forge,
                        entry: l.entry,
                    });
                Some(found.ok_or_else(|| format!("anvil: no TextureSet {n} in the forges"))?)
            }
            None => None,
        };
        Ok(read_terrain(
            world,
            &mut self.archive,
            layer,
            TerrainRead {
                origin: t.origin,
                side: t.side,
                level: t.level,
                rect: t.rect,
                frame: self.options.frame,
                bank,
                noise,
                groups: t.groups,
            },
        )?)
    }
}

/// The World's GridDescriptions: the first World in the forges' order, its layouts read with the u16 run its forge's
/// edition states.
pub(crate) fn world_descriptions(
    archive: &mut Archive,
    forges: &[ForgeFile],
) -> Result<Vec<GridDescription>> {
    let world_at = archive
        .listing()
        .find(|(_, l)| l.kind == Some(Kind::World))
        .map(|(forge, l)| At {
            forge,
            entry: l.entry,
        })
        .ok_or("anvil: no World in the forges")?;
    let words = forges
        .get(world_at.forge)
        .ok_or("anvil: the World's forge is not among the forges")?
        .edition
        .grid_layout_words();
    let world = archive.resource(world_at)?;
    Ok(grid_descriptions(&world.body, words)?)
}

/// An entry's name in the forge's name table.
pub(crate) fn entry_name(archive: &Archive, at: At) -> String {
    archive
        .forge(at.forge)
        .and_then(|f| f.names.get(at.entry))
        .map(|n| n.name.clone())
        .unwrap_or_default()
}

/// The cells of a grid (of `levels` levels and `cells` cells) whose squares overlap `rect` in a positive area, at the
/// levels `read` reads, by cell number.
fn cells_over(layout: &GridLayout, levels: u32, cells: usize, rect: [f64; 4], read: Levels) -> Vec<usize> {
    let [x0, y0, x1, y1] = rect;
    (0..cells)
        .filter(|&k| {
            layout.cell(levels, k).is_some_and(|c| {
                let [cx, cy] = c.min.map(|v| v as f64);
                let size = c.size as f64;
                read.reads(c.level, levels) && cx < x1 && cx + size > x0 && cy < y1 && cy + size > y0
            })
        })
        .collect()
}

/// An object to place and where it was found.
struct Found {
    id: u64,
    cells: Vec<ListedBy>,
    /// The finest level of any cell reaching it, directly or through a group.
    reached_from: u32,
}

/// `id` reached from `level`: new, or from a finer level than before (its members are reached from it too): its
/// position in `found`, queued to have its members visited when it is new or finer.
fn reach(
    found: &mut Vec<Found>,
    index: &mut BTreeMap<u64, usize>,
    queue: &mut VecDeque<u64>,
    id: u64,
    level: u32,
) -> usize {
    let k = *index.entry(id).or_insert_with(|| {
        found.push(Found {
            id,
            cells: Vec::new(),
            reached_from: u32::MAX,
        });
        found.len() - 1
    });
    if level < found[k].reached_from {
        found[k].reached_from = level;
        queue.push_back(id);
    }
    k
}

/// The listed ids with every listed EntityGroup's members appended (recursively, each id once): each id once, in
/// order of first being reached (the cells' listing in order, then members breadth first), with the cells that list it
/// itself and the finest level that reaches it by any route.
fn with_members(archive: &mut Archive, lists: &[CellList]) -> Result<Vec<Found>> {
    let mut found: Vec<Found> = Vec::new();
    let mut index: BTreeMap<u64, usize> = BTreeMap::new();
    let mut queue: VecDeque<u64> = VecDeque::new();
    for l in lists {
        for &id in &l.ids {
            let k = reach(&mut found, &mut index, &mut queue, id, l.level);
            let by = ListedBy {
                grid: l.grid.clone(),
                cell: u32::try_from(l.cell).unwrap_or(u32::MAX),
                level: l.level,
            };
            if !found[k].cells.contains(&by) {
                found[k].cells.push(by);
            }
        }
    }
    while let Some(id) = queue.pop_front() {
        let Some(at) = archive.find(id) else { continue };
        let is_group = archive
            .forge(at.forge)
            .and_then(|f| f.names.get(at.entry))
            .is_some_and(|n| n.type_id == ENTITY_GROUP);
        if !is_group {
            continue;
        }
        let level = found[index[&id]].reached_from;
        let r = archive.resource(at)?;
        let Ok(g) = StoredEntityGroup::parse(&r.body, crate::components::component_types::is_component_type)
        else {
            continue;
        };
        for m in g.members {
            reach(&mut found, &mut index, &mut queue, m.id, level);
        }
    }
    Ok(found)
}

impl Reader for AnvilReader {
    fn read(&mut self) -> Result<(World, Report)> {
        let lists = self.listings()?;
        let found = with_members(&mut self.archive, &lists)?;
        let listed_ids = found.iter().filter(|f| !f.cells.is_empty()).count();
        // what each listed id is, by the forge's name table: only Entities and EntityGroups place
        let mut other: BTreeMap<u32, u64> = BTreeMap::new();
        let mut missing = 0u64;
        let mut to_place: Vec<(&Found, At)> = Vec::new();
        for f in &found {
            let Some(at) = self.archive.find(f.id) else {
                missing += 1;
                continue;
            };
            let type_id = self
                .archive
                .forge(at.forge)
                .and_then(|forge| forge.names.get(at.entry))
                .map_or(0, |n| n.type_id);
            if type_id == ENTITY || type_id == ENTITY_GROUP {
                to_place.push((f, at));
            } else {
                *other.entry(type_id).or_default() += 1;
            }
        }
        let mut world = World::new();
        let mut placed = Placed::default();
        let mut refused: Vec<(u64, String)> = Vec::new();
        let mut ranges: Vec<(usize, [usize; 5], usize)> = Vec::new();
        {
            let mut placer = Placer::new(
                &mut self.archive,
                &mut world,
                self.options.frame,
                self.options.instanced_levels,
            )
            .with_instanced_collision(self.options.instanced_collision);
            for (k, (f, at)) in to_place.iter().enumerate() {
                let before = [
                    placed.placements.len(),
                    placed.collisions.len(),
                    placed.lakes.len(),
                    placed.deferred_decals.len(),
                    placed.grass.len(),
                ];
                if let Err(e) = placer.place(*at, &mut placed) {
                    refused.push((f.id, e.to_string()));
                }
                ranges.push((k, before, 0));
            }
        }
        // each owner's marks, on everything made for it: the placements' ranges run to the next owner's start
        let ends = [
            placed.placements.len(),
            placed.collisions.len(),
            placed.lakes.len(),
            placed.deferred_decals.len(),
            placed.grass.len(),
        ];
        for (n, &(k, before, _)) in ranges.iter().enumerate() {
            let after = ranges.get(n + 1).map_or(ends, |r| r.1);
            let f = to_place[k].0;
            let mark = AnvilListed {
                owner: f.id,
                frame: self.options.frame,
                cells: f.cells.clone(),
                reached_from: f.reached_from,
            };
            let lists: [&[Entity]; 5] = [
                &placed.placements,
                &placed.collisions,
                &placed.lakes,
                &placed.deferred_decals,
                &placed.grass,
            ];
            for (list, (b, a)) in lists.iter().zip(before.iter().zip(after)) {
                for &e in &list[*b..a] {
                    world.insert(e, mark.clone());
                }
            }
        }
        let mut report: Report = vec![(
            "anvil cells".into(),
            lists
                .iter()
                .map(|l| {
                    format!(
                        "{} cell {} (level {}): {} objects",
                        l.grid,
                        l.cell,
                        l.level,
                        l.ids.len()
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        )];
        report.push((
            "anvil listing".into(),
            format!(
                "{listed_ids} objects listed, {} with group members, {} Entities or EntityGroups placed, \
                 {missing} in no forge, refused {}",
                found.len(),
                to_place.len(),
                refused.len()
            ),
        ));
        if let Some(t) = self.options.terrain.clone() {
            let e = self.read_terrain(&mut world, &t)?;
            let nodes = world.get::<AnvilTerrain>(e).map_or(0, |t| t.tiles.len());
            report.push((
                "anvil terrain".into(),
                format!("{nodes} level-{} nodes meeting {:?}", t.level, t.rect),
            ));
        }
        report.push(("anvil placed".into(), summary(&placed)));
        self.pending
            .extend(read_notices(&placed, &refused, &other, missing));
        Ok((world, report))
    }

    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        let keep = world.closure(roots.iter().copied());
        let mut part = world.subset(&keep);
        // the Materials of every mesh the part holds, read once each, linked from the ranges
        let wanted: Vec<(Entity, Vec<u64>)> = part
            .iter::<AnvilMesh>()
            .filter(|(_, m)| m.material_entities.is_empty())
            .map(|(e, m)| (e, m.materials.clone()))
            .collect();
        let mut placed = Placed::default();
        let linked: Vec<(Entity, Vec<Option<Entity>>)> = {
            let mut placer = Placer::new(
                &mut self.archive,
                &mut part,
                self.options.frame,
                self.options.instanced_levels,
            )
            .with_materials()
            .with_shaders();
            wanted
                .into_iter()
                .map(|(e, ids)| {
                    (
                        e,
                        ids.into_iter()
                            .map(|id| placer.material(id, &mut placed))
                            .collect(),
                    )
                })
                .collect()
        };
        for (e, entities) in linked {
            if let Some(mesh) = part.get_mut::<AnvilMesh>(e) {
                mesh.material_entities = entities;
            }
        }
        let mut notices = Vec::new();
        if !placed.refused.is_empty() {
            let mut n = Notice::new(
                STAGE,
                NoticeKind::Unsupported,
                "Materials and textures that couldn't be read, left unbound",
                placed.refused.len() as u64,
            );
            for (id, why) in &placed.refused {
                n = n.example(format!("{id:#x}: {why}"));
            }
            notices.push(n);
        }
        self.pending.extend(notices);
        Ok((
            part,
            vec![(
                "anvil materials".into(),
                format!("{} refused", placed.refused.len()),
            )],
        ))
    }

    fn emits(&self) -> &'static [&'static str] {
        &["core", "anvil"]
    }

    /// A prototype's box in core's frame: its mesh's positions (the read holds every mesh).
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        let mesh = world.get::<AnvilMesh>(prototype)?;
        let mut b: Option<[[f64; 3]; 2]> = None;
        for p in &mesh.positions {
            let p = p.map(f64::from);
            b = Some(b.map_or([p, p], |[lo, hi]| {
                [
                    std::array::from_fn(|k| lo[k].min(p[k])),
                    std::array::from_fn(|k| hi[k].max(p[k])),
                ]
            }));
        }
        b
    }

    fn notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.pending)
    }
}

/// What placing did, for the report.
fn summary(p: &Placed) -> String {
    format!(
        "{} placements, {} collisions, {} lakes, {} deferred decal sets, {} static decals, {} fakes, {} water bodies, \
         {} collision-only, {} hidden or phantom, {} grass setups",
        p.placements.len(),
        p.collisions.len(),
        p.lakes.len(),
        p.deferred_decals.len(),
        p.static_decals,
        p.fakes,
        p.water_bodies,
        p.collision_only,
        p.hidden_or_phantom,
        p.grass.len()
    )
}

/// What reading couldn't place, as notices.
fn read_notices(
    placed: &Placed,
    refused: &[(u64, String)],
    other: &BTreeMap<u32, u64>,
    missing: u64,
) -> Vec<Notice> {
    let mut out = Vec::new();
    let mut push = |kind, reason: &str, count: u64, examples: Vec<String>| {
        if count > 0 {
            let mut n = Notice::new(STAGE, kind, reason, count);
            for e in examples {
                n = n.example(e);
            }
            out.push(n);
        }
    };
    push(
        NoticeKind::Unsupported,
        "Entities and EntityGroups the reader refused to place",
        refused.len() as u64,
        refused
            .iter()
            .map(|(id, why)| format!("{id:#x}: {why}"))
            .collect(),
    );
    push(
        NoticeKind::Unsupported,
        "listed objects that are neither Entities nor EntityGroups, not placed",
        other.values().sum(),
        other
            .iter()
            .map(|(t, n)| format!("type {t:#010x} x {n}"))
            .collect(),
    );
    push(
        NoticeKind::Unsupported,
        "listed ids in no forge",
        missing,
        Vec::new(),
    );
    push(
        NoticeKind::Unsupported,
        "visuals whose object resolves to nothing the reader places",
        placed.visuals_skipped,
        Vec::new(),
    );
    push(
        NoticeKind::Unsupported,
        "levels referring to an Impostor, not placed",
        placed.impostor_levels,
        Vec::new(),
    );
    push(
        NoticeKind::Unsupported,
        "objects (meshes, selectors, shapes) the reader couldn't read",
        placed.refused.len() as u64,
        placed
            .refused
            .iter()
            .map(|(id, why)| format!("{id:#x}: {why}"))
            .collect(),
    );
    push(
        NoticeKind::Unsupported,
        "rigid bodies whose shape type isn't read",
        placed.shapes_unread.values().sum(),
        placed
            .shapes_unread
            .iter()
            .map(|(k, n)| format!("type {k:#010x} x {n}"))
            .collect(),
    );
    push(
        NoticeKind::Unsupported,
        "rigid bodies of scaled owners, whose placement isn't established",
        placed.scaled_collision,
        Vec::new(),
    );
    push(
        NoticeKind::Unsupported,
        "components of types with no length rule here, kept as stored bytes and not read",
        placed.unread_components.values().sum(),
        placed
            .unread_components
            .iter()
            .map(|(&kind, n)| {
                let name = component_name(kind).unwrap_or("a type outside the registry's components");
                format!("{name} ({kind:#010x}) x {n}")
            })
            .collect(),
    );
    push(
        NoticeKind::Unsupported,
        "components whose rule refused their bytes, kept as stored and not read",
        placed.refused_components.values().map(|r| r.count).sum(),
        placed
            .refused_components
            .iter()
            .map(|((kind, reason), r)| {
                let name = component_name(*kind).unwrap_or("a type outside the registry's components");
                format!(
                    "{name} ({kind:#010x}): {reason} x {} (the first: {})",
                    r.count, r.first
                )
            })
            .collect(),
    );
    out
}
