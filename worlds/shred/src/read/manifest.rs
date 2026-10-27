//! Which archives hold a level, from the game's database and the archives' own names and contents.
//!
//! `LEVELS/<LEVEL>.PAK` is a level's base (its quartet `levels\<level>\<level>`). `LEVELS/LAYERS/<DSZZ>.PAK` are
//! layers, the name's eight hex digits `0xDS00ZZ`: `D` the difficulty nibble (0 none, 8 casual, 9 confident, A
//! hardcore), `S` the session type (`session_types.id` of MAIN.DB), `ZZ` the map id. A layer archive holds quartets
//! `levels\<level>\<layer>\<layer>`; archives of difficulty 0 are the map's always-loaded layers (lighting, standard
//! session, nav splines, level-specific data), the others the session layers of one session type and difficulty.
//!
//! This module lists and filters. It chooses nothing: which map id a database level row loads, and which sessions and
//! difficulty a program wants, are the caller's. The row-to-map-id join is the level's Lua load script (`LoadData`
//! constants in the encrypted `SCRIPTS.SDAT`), which this reader does not read; [`Manifest::map_ids`] gives the
//! candidates (a level's maps) from the archives themselves.
use super::{level, pak::Pak};
use crate::read::database::{Database, Value};
use crate::{Result, invalid};

/// A layer archive's difficulty nibble.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Difficulty {
    Base,
    Casual,
    Confident,
    Hardcore,
}
impl Difficulty {
    pub fn from_nibble(n: u8) -> Option<Self> {
        match n {
            0 => Some(Self::Base),
            8 => Some(Self::Casual),
            9 => Some(Self::Confident),
            0xA => Some(Self::Hardcore),
            _ => None,
        }
    }
}

/// One layer archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerPak {
    /// The archive's file name as supplied, e.g. `8100000C.PAK`.
    pub file: String,
    pub difficulty: Difficulty,
    /// `session_types.id`; [`Manifest::session_type`] names it.
    pub session_type: u8,
    pub map_id: u8,
    /// The quartet stems it holds, e.g. `levels\charlie\trainingsession1\trainingsession1`.
    pub quartets: Vec<Vec<u8>>,
}

/// A level's base archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasePak {
    pub file: String,
    /// The level folder name of its quartets, lower-case as stored.
    pub level: String,
}

/// An archive that is neither (named and filled unlike a layer or a base): reported, not guessed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtherPak {
    pub file: String,
    pub reason: String,
}

/// A row of the database's `levels` table (the text columns the manifest needs, as stored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelRow {
    pub id: u32,
    pub name: String,
    pub display_name: String,
    pub path: String,
    pub board_type: String,
    /// `SESSIONS`: the `&`-separated session ids.
    pub sessions: Vec<u32>,
    /// `SCRIPT`: the Lua load script, e.g. `script\level_load_Charlie.lua`.
    pub script: String,
}

/// A row of `<Level>_dropspots`: where the player may start, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropSpot {
    pub id: u32,
    pub name: String,
    pub nav_set_start: String,
    pub nav_index_start: String,
    /// The spot chunk's name; `-1` or `na` where the row has none.
    pub spot_name: String,
    pub start_speed: String,
    pub start_state: String,
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub levels: Vec<LevelRow>,
    pub session_types: Vec<(u32, String)>,
    pub bases: Vec<BasePak>,
    pub layers: Vec<LayerPak>,
    pub others: Vec<OtherPak>,
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.clone()),
        Value::Int(i) => Some(i.to_string()),
        _ => None,
    }
}
fn number(v: &Value) -> Option<u32> {
    match v {
        Value::Int(i) => u32::try_from(*i).ok(),
        Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn cell<'a>(
    row: &'a crate::read::database::Row,
    t: &crate::read::database::Table,
    col: &str,
) -> Result<&'a Value> {
    let i = t
        .column(col)
        .ok_or_else(|| invalid(0, format!("database: table {:?} has no column {col}", t.name)))?;
    Ok(&row.values[i])
}

fn db_error(e: crate::read::database::Error) -> crate::Error {
    invalid(0, format!("database: {e}"))
}

/// `levels`, `session_types` from the game's database.
type DbRows = (Vec<LevelRow>, Vec<(u32, String)>);

fn db_tables(db: &Database<'_>) -> Result<DbRows> {
    let levels = db.table("levels").map_err(db_error)?;
    let mut rows = Vec::new();
    for r in &levels.rows {
        let get = |c: &str| -> Result<String> {
            text(cell(r, &levels, c)?)
                .ok_or_else(|| invalid(0, format!("database: levels row {} column {c}", r.rowid)))
        };
        let sessions = get("SESSIONS")?
            .split('&')
            .map(|s| s.trim().parse().ok())
            .collect::<Option<Vec<u32>>>()
            .ok_or_else(|| invalid(0, format!("database: levels row {} SESSIONS", r.rowid)))?;
        rows.push(LevelRow {
            id: number(cell(r, &levels, "id")?)
                .ok_or_else(|| invalid(0, format!("database: levels row {} id", r.rowid)))?,
            name: get("NAME")?,
            display_name: get("DISPLAY_NAME")?,
            path: get("PATH")?,
            board_type: get("BOARD_TYPE")?,
            sessions,
            script: get("SCRIPT")?,
        });
    }
    let types = db.table("session_types").map_err(db_error)?;
    let mut session_types = Vec::new();
    for r in &types.rows {
        let id = number(cell(r, &types, "id")?)
            .ok_or_else(|| invalid(0, format!("database: session_types row {} id", r.rowid)))?;
        let name = text(cell(r, &types, "NAME")?)
            .ok_or_else(|| invalid(0, format!("database: session_types row {} NAME", r.rowid)))?;
        session_types.push((id, name));
    }
    Ok((rows, session_types))
}

/// A layer archive's difficulty, session type and map id from its file name (`8100000C.PAK`).
pub fn parse_layer_name(file: &str) -> Option<(Difficulty, u8, u8)> {
    let stem = file.strip_suffix(".PAK").or_else(|| file.strip_suffix(".pak"))?;
    if stem.len() != 8 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(stem, 16).ok()?;
    if v & 0x00ff_ff00 != 0 {
        return None;
    }
    Some((
        Difficulty::from_nibble((v >> 28) as u8)?,
        ((v >> 24) & 0xf) as u8,
        v as u8,
    ))
}

impl Manifest {
    /// The database's levels and session types, and the archives supplied as `(file name, archive)`: those of
    /// `LEVELS` and `LEVELS/LAYERS`. Archives that are neither a layer nor a base are listed in `others`.
    pub fn new(db: &Database<'_>, paks: &[(&str, &Pak)]) -> Result<Self> {
        let (levels, session_types) = db_tables(db)?;
        let mut m = Manifest {
            levels,
            session_types,
            ..Manifest::default()
        };
        for &(file, pak) in paks {
            let quartets: Vec<Vec<u8>> = level::quartets(pak)?.into_keys().collect();
            if let Some((difficulty, session_type, map_id)) = parse_layer_name(file) {
                m.layers.push(LayerPak {
                    file: file.into(),
                    difficulty,
                    session_type,
                    map_id,
                    quartets,
                });
                continue;
            }
            let stem = file.rsplit(['/', '\\']).next().unwrap_or(file);
            let level = stem
                .strip_suffix(".PAK")
                .or_else(|| stem.strip_suffix(".pak"))
                .map(str::to_ascii_lowercase);
            let own = level.as_ref().map(|l| format!("levels\\{l}\\{l}").into_bytes());
            match (level, own) {
                (Some(level), Some(own)) if quartets.len() == 1 && quartets[0] == own => {
                    m.bases.push(BasePak {
                        file: file.into(),
                        level,
                    });
                }
                _ => m.others.push(OtherPak {
                    file: file.into(),
                    reason: "neither a layer archive name nor a level base quartet".into(),
                }),
            }
        }
        Ok(m)
    }

    /// The name of a session type id.
    pub fn session_type(&self, id: u32) -> Option<&str> {
        self.session_types
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
    }

    /// A level's base archive (`level` as the database names it, any case).
    pub fn base(&self, level: &str) -> Option<&BasePak> {
        self.bases.iter().find(|b| b.level.eq_ignore_ascii_case(level))
    }

    /// The layer archives holding at least one quartet under `levels\<level>\`, in file-name order.
    pub fn layers_naming(&self, level: &str) -> Vec<&LayerPak> {
        let prefix = format!("levels\\{}\\", level.to_ascii_lowercase()).into_bytes();
        let mut v: Vec<_> = self
            .layers
            .iter()
            .filter(|l| l.quartets.iter().any(|q| q.starts_with(&prefix)))
            .collect();
        v.sort_by(|a, b| a.file.cmp(&b.file));
        v
    }

    /// The map ids of the layer archives naming `level`: the candidates a level row's load script picks among.
    pub fn map_ids(&self, level: &str) -> Vec<u8> {
        let mut ids: Vec<u8> = self.layers_naming(level).iter().map(|l| l.map_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// A map's always-loaded layer archives (difficulty none), in file-name order.
    pub fn common(&self, map_id: u8) -> Vec<&LayerPak> {
        self.session(map_id, Difficulty::Base, None)
    }

    /// A map's layer archives of one difficulty and, when given, session type, in file-name order.
    pub fn session(&self, map_id: u8, difficulty: Difficulty, session_type: Option<u8>) -> Vec<&LayerPak> {
        let mut v: Vec<_> = self
            .layers
            .iter()
            .filter(|l| {
                l.map_id == map_id
                    && l.difficulty == difficulty
                    && session_type.is_none_or(|s| l.session_type == s)
            })
            .collect();
        v.sort_by(|a, b| a.file.cmp(&b.file));
        v
    }
}

/// Every drop spot row of `<level>_dropspots` (`level` as the database names it); `None` when the database has no
/// such table.
pub fn drop_spots(db: &Database<'_>, level: &str) -> Result<Option<Vec<DropSpot>>> {
    let t = match db.table(&format!("{level}_dropspots")) {
        Ok(t) => t,
        Err(crate::read::database::Error::NoTable(_)) => return Ok(None),
        Err(e) => return Err(db_error(e)),
    };
    let mut out = Vec::new();
    for r in &t.rows {
        let get = |c: &str| -> Result<String> {
            text(cell(r, &t, c)?)
                .ok_or_else(|| invalid(0, format!("database: {} row {} column {c}", t.name, r.rowid)))
        };
        out.push(DropSpot {
            id: number(cell(r, &t, "id")?)
                .ok_or_else(|| invalid(0, format!("database: {} row {} id", t.name, r.rowid)))?,
            name: get("NAME")?,
            nav_set_start: get("NAV_SET_START")?,
            nav_index_start: get("NAV_INDEX_START")?,
            spot_name: get("SPOT_NAME")?,
            start_speed: get("START_SPEED")?,
            start_state: get("START_STATE")?,
        });
    }
    Ok(Some(out))
}

/// The spot chunks of `world` (a level's `read_level`) that a drop spot names: those whose name hash is sdbm of the
/// row's `SPOT_NAME`, as written or lower-cased (the game hashes either). Rows with no `SPOT_NAME` (`-1`, `na`) or no
/// matching chunk have an empty list. The chunks are all classes; the caller checks `shred.spot` class.
pub fn resolve_drop_spots<'a>(
    world: &primase_core::World,
    rows: &'a [DropSpot],
) -> Vec<(&'a DropSpot, Vec<primase_core::Entity>)> {
    let mut by_name: std::collections::BTreeMap<u32, Vec<primase_core::Entity>> =
        std::collections::BTreeMap::new();
    for (e, s) in world.iter::<crate::components::Spot>() {
        by_name.entry(s.name).or_default().push(e);
    }
    rows.iter()
        .map(|r| {
            let mut hits = Vec::new();
            for variant in [r.spot_name.clone(), r.spot_name.to_ascii_lowercase()] {
                for e in by_name
                    .get(&primase_foundations::hash::sdbm(variant.as_bytes()))
                    .into_iter()
                    .flatten()
                {
                    if !hits.contains(e) {
                        hits.push(*e);
                    }
                }
            }
            (r, hits)
        })
        .collect()
}
