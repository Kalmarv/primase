//! What a rider can be put together from, as the game's customization BuildTables list it ([`rider_catalog`]):
//! the slots, each slot's part tables, and each part's variants. Nothing is picked: a slot's weights are kept
//! as stored, and a default is named only where a table states one ([`stated`]).
//!
//! - **Slots.** `MAD_CHR_SLOTS_DEFAULT` lists the slot tables of a character (`MAD_CHR_SLOT_UBODY`, ...: one row with
//!   weight 1.0 that holds the upper body, lower body, footwear, headwear, neckwear, eyewear and backpack slots, and
//!   one with weight 0.0 that holds the outfits instead of the two body slots); `MAD_CHR_BODY_U` lists the body's own
//!   tables (`MAD_CHR_BODY_HEADS` and the eyes, hairs, skin and inner mouth). A slot is a table those list whose rows
//!   link part tables.
//! - **Parts.** A slot table's rows link part tables (a BuildTable one of whose rows links a Mesh) and others (the
//!   accessories tables). Each part's name is its entry's name.
//! - **Variants.** A part's materials table (the one of the tables its row links whose rows link variant tables, the
//!   BuildTables with a column naming a generic Material) lists the part's variants, one per variant table, with the
//!   row's weight and condition as stored. [`stated`] names the one row of a list whose weight is exactly 1.0, the
//!   data's own default: the heads' table states `MAD_CHR_HEAD_000_U` so, a part's materials table its first variant;
//!   the slot tables' weights (0.048, ...) are population odds and state none.
use std::collections::HashSet;

use crate::components::StoredBuildTable;
use crate::read::archive::{self, Archive};
use crate::read::build_table::{self, BUILD_TABLE};
use crate::read::material::TEXTURE_MAP;
use crate::read::mesh::{MATERIAL, MESH};
use crate::read::textures::read_texture_map;

/// The table listing a character's slot tables.
pub const SLOTS_DEFAULT: &str = "MAD_CHR_SLOTS_DEFAULT";
/// The table listing a character's body tables (the heads among them).
pub const BODY: &str = "MAD_CHR_BODY_U";

/// Why the rider's tables can't be listed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] archive::Error),
    #[error(transparent)]
    Table(#[from] build_table::Error),
    #[error("no BuildTable named {0} in the forges")]
    NoTable(&'static str),
    #[error("{0:#x} is not a BuildTable")]
    NotATable(u64),
    #[error(transparent)]
    Texture(#[from] crate::read::textures::Error),
    #[error("no TextureMap named {0} in the forges, or it holds no image")]
    NoTexture(&'static str),
    #[error("the skin palette is {width} x {height}, not 8 x 4")]
    Palette { width: u32, height: u32 },
}

/// A variant a part's materials table lists: the variant table, and the row's weight and condition as stored (the
/// weights are population odds except where a row states 1.0 among 0.0s: [`stated`]).
#[derive(Clone, Debug, PartialEq)]
pub struct VariantRow {
    pub table: u64,
    pub name: String,
    pub weight: f32,
    pub condition: u32,
}

/// A part table a slot lists, and the variants its materials table lists.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogPart {
    pub table: u64,
    pub name: String,
    pub variants: Vec<VariantRow>,
}

/// A slot of a rider: its table, and the parts it lists.
#[derive(Clone, Debug, PartialEq)]
pub struct RiderSlot {
    /// The table's entry name, and the key it gives: the name without its `MAD_CHR_SLOT_` or `MAD_CHR_BODY_`
    /// prefix, in lower case (`ubody`, `heads`).
    pub table_name: String,
    pub key: String,
    pub table: u64,
    /// The table is listed by the weight-1.0 row of [`SLOTS_DEFAULT`] or [`BODY`] (the default composition).
    pub in_default: bool,
    pub parts: Vec<CatalogPart>,
    /// The one part whose row has weight exactly 1.0 among the slot's rows, where the table states one.
    pub stated: Option<u64>,
}

/// Every slot with the parts and variants it lists.
#[derive(Clone, Debug, PartialEq)]
pub struct RiderCatalog {
    pub slots: Vec<RiderSlot>,
}

/// The ClassID of the first BuildTable named `name`, the forges searched in their order.
pub fn find_table(archive: &Archive, name: &str) -> Option<u64> {
    (0..).map_while(|forge| archive.forge(forge)).find_map(|f| {
        f.entries()
            .find(|(_, n)| n.type_id == BUILD_TABLE && n.name == name)
            .map(|(i, _)| i.id)
    })
}

fn table(archive: &mut Archive, id: u64) -> Result<(StoredBuildTable, String), Error> {
    let at = archive.find(id).ok_or(Error::NotATable(id))?;
    let name = archive
        .forge(at.forge)
        .and_then(|f| f.names.get(at.entry))
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let r = archive.resource(at)?;
    if r.type_id != BUILD_TABLE {
        return Err(Error::NotATable(id));
    }
    Ok((StoredBuildTable::parse(&r.body)?, name))
}

fn type_of(archive: &Archive, id: u64) -> Option<u32> {
    let at = archive.find(id)?;
    archive
        .forge(at.forge)
        .and_then(|f| f.names.get(at.entry))
        .map(|n| n.type_id)
}

/// The BuildTables a table's rows link, once each, in stored order, with the weight of the first row that links each.
fn linked_tables(archive: &Archive, t: &StoredBuildTable) -> Result<Vec<(u64, f32)>, Error> {
    let mut out: Vec<(u64, f32)> = Vec::new();
    for row in &t.rows {
        for (_, id) in row.links()? {
            if type_of(archive, id) == Some(BUILD_TABLE) && !out.iter().any(|(t, _)| *t == id) {
                out.push((id, row.weight));
            }
        }
    }
    Ok(out)
}

/// The part tables a slot table lists: the BuildTables its rows link that are parts, or, where it links none, those of
/// the tables it links in turn (the footwear slot lists activities tables, each listing parts), down to `NESTING`
/// levels. Once each, in stored order.
fn part_tables(archive: &mut Archive, slot: &StoredBuildTable) -> Result<Vec<u64>, Error> {
    let mut seen: Vec<u64> = Vec::new();
    let mut level = vec![slot.clone()];
    for _ in 0..NESTING {
        let mut parts: Vec<u64> = Vec::new();
        let mut next: Vec<StoredBuildTable> = Vec::new();
        for t in &level {
            for (id, _) in linked_tables(archive, t)? {
                if seen.contains(&id) {
                    continue;
                }
                seen.push(id);
                let (pt, _) = table(archive, id)?;
                if is_part(archive, &pt)? {
                    parts.push(id);
                } else {
                    next.push(pt);
                }
            }
        }
        if !parts.is_empty() {
            return Ok(parts);
        }
        level = next;
    }
    Ok(Vec::new())
}

/// How many levels of tables below a slot table are searched for its parts.
const NESTING: usize = 3;

/// Whether one of the table's rows links a Mesh: a part table.
fn is_part(archive: &Archive, t: &StoredBuildTable) -> Result<bool, Error> {
    for row in &t.rows {
        for (_, id) in row.links()? {
            if type_of(archive, id) == Some(MESH) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The variants listed by the one of `tables` (the BuildTables a part's row links) whose rows link variant tables, in
/// stored order, one per variant table; none where no table, or more than one, does.
pub fn part_variants(archive: &mut Archive, tables: &[u64]) -> Result<Vec<VariantRow>, Error> {
    let mut found: Vec<Vec<VariantRow>> = Vec::new();
    for &id in tables {
        let (t, _) = table(archive, id)?;
        let mut variants = Vec::new();
        let mut names_variants = false;
        for row in &t.rows {
            for (_, target) in row.links()? {
                if type_of(archive, target) != Some(BUILD_TABLE) {
                    continue;
                }
                if !names_variants {
                    let (v, _) = table(archive, target)?;
                    names_variants = v
                        .columns
                        .iter()
                        .any(|c| c.generic_material().ok().flatten().is_some());
                    if !names_variants {
                        continue;
                    }
                }
                let name = archive
                    .find(target)
                    .and_then(|at| archive.forge(at.forge).and_then(|f| f.names.get(at.entry)))
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                variants.push(VariantRow {
                    table: target,
                    name,
                    weight: row.weight,
                    condition: row.condition,
                });
            }
        }
        if names_variants {
            found.push(variants);
        }
    }
    Ok(match <[_; 1]>::try_from(found) {
        Ok([one]) => one,
        Err(_) => Vec::new(),
    })
}

/// The one variant of `variants` whose weight is exactly 1.0, where the list states one.
pub fn stated(variants: &[VariantRow]) -> Option<&VariantRow> {
    let mut one = variants.iter().filter(|v| v.weight == 1.0);
    match (one.next(), one.next()) {
        (Some(v), None) => Some(v),
        _ => None,
    }
}

/// The slot key of a table's name.
fn key_of(name: &str) -> String {
    let rest = name
        .strip_prefix("MAD_CHR_SLOT_")
        .or_else(|| name.strip_prefix("MAD_CHR_BODY_"))
        .unwrap_or(name);
    rest.to_ascii_lowercase()
}

/// The slots [`SLOTS_DEFAULT`] and [`BODY`] list, each with the parts it lists and their variants. A table those list
/// that lists no part table within [`NESTING`] levels is not a slot itself but may hold some: the tables it links are
/// tried as slots in their own right, one level only. The body's hairs table is such a table: it links the hair
/// activator (the haircuts, each a table of its `Full` and `Cut` styles, which are the parts), the beard activator (the
/// beards, each a head selector listing its table per head, which are the parts) and the hair materials (no part, so no
/// slot). A table whose links make no slot either is left out.
pub fn rider_catalog(archive: &mut Archive) -> Result<RiderCatalog, Error> {
    let mut listed: Vec<(u64, bool)> = Vec::new();
    for name in [SLOTS_DEFAULT, BODY] {
        let id = find_table(archive, name).ok_or(Error::NoTable(name))?;
        let (t, _) = table(archive, id)?;
        for row in &t.rows {
            for (_, link) in row.links()? {
                if type_of(archive, link) != Some(BUILD_TABLE) {
                    continue;
                }
                let default = row.weight == 1.0;
                match listed.iter_mut().find(|(t, _)| *t == link) {
                    Some((_, d)) => *d |= default,
                    None => listed.push((link, default)),
                }
            }
        }
    }
    let mut slots = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    for (id, in_default) in listed {
        if !seen.insert(id) {
            continue;
        }
        if let Some(slot) = slot_of(archive, id, in_default)? {
            slots.push(slot);
            continue;
        }
        // no part within reach: the tables this one links may be slots
        let (t, _) = table(archive, id)?;
        for (link, _) in linked_tables(archive, &t)? {
            if !seen.insert(link) {
                continue;
            }
            if let Some(slot) = slot_of(archive, link, in_default)? {
                slots.push(slot);
            }
        }
    }
    Ok(RiderCatalog { slots })
}

/// The slot the table `id` is: its parts and their variants; `None` where it lists no part within [`NESTING`] levels.
fn slot_of(archive: &mut Archive, id: u64, in_default: bool) -> Result<Option<RiderSlot>, Error> {
    let (t, table_name) = table(archive, id)?;
    let mut parts = Vec::new();
    for part in part_tables(archive, &t)? {
        let (pt, name) = table(archive, part)?;
        let mut links: Vec<u64> = Vec::new();
        for row in &pt.rows {
            for (_, l) in row.links()? {
                if type_of(archive, l) == Some(BUILD_TABLE) && !links.contains(&l) {
                    links.push(l);
                }
            }
        }
        let variants = part_variants(archive, &links)?;
        parts.push(CatalogPart {
            table: part,
            name,
            variants,
        });
    }
    if parts.is_empty() {
        return Ok(None);
    }
    // the one part linked by the slot's one row of weight 1.0
    let mut weighted = t.rows.iter().filter(|r| r.weight == 1.0);
    let stated_part = match (weighted.next(), weighted.next()) {
        (Some(row), None) => {
            let mut mine = Vec::new();
            for (_, l) in row.links()? {
                if parts.iter().any(|p| p.table == l) {
                    mine.push(l);
                }
            }
            match mine.as_slice() {
                [one] => Some(*one),
                _ => None,
            }
        }
        _ => None,
    };
    Ok(Some(RiderSlot {
        key: key_of(&table_name),
        table_name,
        table: id,
        in_default,
        parts,
        stated: stated_part,
    }))
}

/// The table listing the hair colours: one row per colour, the row's condition its tag.
pub const HAIR_COLOURS: &str = "MAD_CHR_BODY_HAIRCUTS_Materials";
/// The table listing the beard colours (the same colours' Materials, the beard's generic Materials replaced).
pub const BEARD_COLOURS: &str = "MAD_CHR_BODY_BEARDS_Materials";

/// A colour a colour table lists: its row's condition (the tag that names the row for `Variant::Row`), its weight as
/// stored (population odds, no default) and the name of the Material the row's first column links.
#[derive(Clone, Debug, PartialEq)]
pub struct Colour {
    pub table: u64,
    pub name: String,
    pub condition: u32,
    pub weight: f32,
}

/// The colours the table named `table_name` ([`HAIR_COLOURS`] or [`BEARD_COLOURS`]) lists, in stored order. A row that
/// links no Material is left out.
pub fn colours(archive: &mut Archive, table_name: &'static str) -> Result<Vec<Colour>, Error> {
    let id = find_table(archive, table_name).ok_or(Error::NoTable(table_name))?;
    let (t, _) = table(archive, id)?;
    let mut out = Vec::new();
    for row in &t.rows {
        let first = row
            .links()?
            .into_iter()
            .filter(|(_, link)| type_of(archive, *link) == Some(MATERIAL))
            .min_by_key(|(column, _)| *column);
        let Some((_, material)) = first else {
            continue;
        };
        let name = archive
            .find(material)
            .and_then(|at| archive.forge(at.forge).and_then(|f| f.names.get(at.entry)))
            .map(|n| n.name.clone())
            .unwrap_or_default();
        out.push(Colour {
            table: id,
            name,
            condition: row.condition,
            weight: row.weight,
        });
    }
    Ok(out)
}

/// The texture of the crowd's skin palette (`MAD_CHR_BULK`'s: the one skin palette the forges hold).
pub const SKIN_PALETTE: &str = "MAD_CHR_SkinColors_DiffuseMap_PC_Scarlett_Prospero";

/// The colours of the skin palette's four tones, R, G, B: the palette is 8 x 4 texels of 2 x 2 cells (four cells across
/// and two down, the cell of tone `n` at x = 2 n); the bottom row's cells are the tones (the top row's role is not
/// established, so it is not given). `MAD_CHR_SkinColors_DiffuseMap` is referenced by the crowd templates `MAD_CHR_BULK`
/// and `MAD_CHR_Bulk_Test` only; no rider table links it.
pub fn skin_palette(archive: &mut Archive) -> Result<Vec<[u8; 3]>, Error> {
    let id = (0..)
        .map_while(|forge| archive.forge(forge))
        .find_map(|f| {
            f.entries()
                .find(|(_, n)| n.type_id == TEXTURE_MAP && n.name == SKIN_PALETTE)
                .map(|(i, _)| i.id)
        })
        .ok_or(Error::NoTexture(SKIN_PALETTE))?;
    let mut world = primase_core::World::new();
    let e = read_texture_map(archive, &mut world, id)?.ok_or(Error::NoTexture(SKIN_PALETTE))?;
    let texture = world
        .get::<primase_core::Texture>(e)
        .ok_or(Error::NoTexture(SKIN_PALETTE))?;
    let levels = primase_image::sampling::Levels::of(texture).map_err(|_| Error::NoTexture(SKIN_PALETTE))?;
    if (levels.width, levels.height) != (8, 4) {
        return Err(Error::Palette {
            width: levels.width,
            height: levels.height,
        });
    }
    let at = |x: i64, y: i64| {
        let t = levels.texel(0, x, y, [primase_image::sampling::Address::Clamp; 2]);
        [t[0], t[1], t[2]]
    };
    Ok((0..4).map(|n| at(2 * n, 2)).collect())
}
