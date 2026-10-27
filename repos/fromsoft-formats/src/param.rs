//! PARAM files (`NpcParam.param` and the other params of a game's param binder): the table of rows as stored, and the
//! typed reading of the NPC param's display masks.
//!
//! [`Param::read`] is strict about the layout Sekiro's and Elden Ring's params share (little-endian, 64-bit offsets,
//! the param type named by an offset, UTF-16 row names): the header, the row table, the rows' data, the param type
//! string and the row names account for every byte of the file, apart from short zero padding. It keeps each row's
//! data as stored; what the bytes mean is the field layout of the param type's definition, which the file does not
//! carry. The only layout this module reads is [`NpcParam`]'s, for the one group of fields it reads (the 32
//! `modelDispMask` bits), at the offsets each game states ([`NpcLayout`], [`crate::Game::npc_param`]); the other fields
//! of a row are not read.
use crate::{Result, bytes::View, bytes::coverage, invalid};

const FORMAT: &str = "PARAM";
/// The header, up to the row table.
const HEADER: usize = 0x40;
/// A row's entry in the row table: id, zero, data offset, name offset.
const ROW_ENTRY: usize = 24;
/// Format flags 1 of Sekiro's and Elden Ring's params: the param type string by offset (0x80), 64-bit offsets (0x04) and
/// the unexplained low flag (0x01).
const FORMAT_2D: u8 = 0x85;
/// Format flags 2 of Sekiro's and Elden Ring's params: UTF-16 row names (0x01) and two flags whose meaning is not
/// established.
const FORMAT_2E: u8 = 0x07;
/// The zero padding allowed between the parts of the file.
const PADDING: usize = 4;

/// One row as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: i32,
    /// The row's name; `None` when the row has no name offset. The games' params carry empty names (the names people
    /// use come from community lists, not the game).
    pub name: Option<String>,
    /// The row's bytes, `row_size` of them.
    pub data: Vec<u8>,
}

/// A param file: its header facts and its rows as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    /// The param type's name, `NPC_PARAM_ST`.
    pub param_type: String,
    /// The revision of the row layout (the definition's data version).
    pub data_version: u16,
    /// The header's unexplained field at 0x06 (1 in Sekiro's and Elden Ring's NPC params).
    pub unk06: u16,
    /// The bytes of the row layout every row takes.
    pub row_size: usize,
    pub rows: Vec<Row>,
}

fn utf16(v: &View<'_>, offset: usize) -> Result<(String, usize)> {
    let mut units = Vec::new();
    let mut p = offset;
    loop {
        let c = v.u16(p)?;
        p += 2;
        if c == 0 {
            break;
        }
        units.push(c);
    }
    let text = String::from_utf16(&units).map_err(|e| invalid(FORMAT, offset, e))?;
    Ok((text, p - offset))
}

impl Param {
    /// Read the file `data`, a param of the layout Sekiro and Elden Ring share.
    pub fn read(data: &[u8]) -> Result<Param> {
        let v = View {
            data,
            format: FORMAT,
            be: false,
        };
        // The strings offset at 0 is not trusted (the format's other readers do not use it either): the param type
        // and the names are found by their own offsets.
        if v.u8(0x2c)? != 0 {
            return Err(invalid(FORMAT, 0x2c, "big-endian params are not read"));
        }
        let (f2d, f2e) = (v.u8(0x2d)?, v.u8(0x2e)?);
        if f2d != FORMAT_2D || f2e != FORMAT_2E {
            return Err(invalid(
                FORMAT,
                0x2d,
                format!(
                    "format flags {f2d:#04x} {f2e:#04x}, the ones read are {FORMAT_2D:#04x} {FORMAT_2E:#04x}"
                ),
            ));
        }
        v.expect(0x04, &[0, 0])?;
        let unk06 = v.u16(0x06)?;
        let data_version = v.u16(0x08)?;
        let count = usize::from(v.u16(0x0a)?);
        v.expect(0x0c, &[0; 4])?;
        let type_offset =
            usize::try_from(v.u64(0x10)?).map_err(|_| invalid(FORMAT, 0x10, "size overflow"))?;
        v.expect(0x18, &[0; 0x14])?;
        let data_start = usize::try_from(v.u64(0x30)?).map_err(|_| invalid(FORMAT, 0x30, "size overflow"))?;
        v.expect(0x38, &[0; 8])?;
        let (param_type, type_len) = {
            let rest = data
                .get(type_offset..)
                .ok_or_else(|| invalid(FORMAT, type_offset, "param type beyond the file"))?;
            let end = rest
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| invalid(FORMAT, type_offset, "param type not terminated"))?;
            let text = std::str::from_utf8(&rest[..end])
                .ok()
                .filter(|t| t.is_ascii())
                .ok_or_else(|| invalid(FORMAT, type_offset, "param type is not ASCII"))?;
            (text.to_owned(), end + 1)
        };

        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let at = HEADER + i * ROW_ENTRY;
            let id = v.u32(at)? as i32;
            v.expect(at + 4, &[0; 4])?;
            let offset =
                usize::try_from(v.u64(at + 8)?).map_err(|_| invalid(FORMAT, at + 8, "size overflow"))?;
            let name =
                usize::try_from(v.u64(at + 16)?).map_err(|_| invalid(FORMAT, at + 16, "size overflow"))?;
            entries.push((at, id, offset, name));
        }

        // The rows' data runs from the first row to the strings that follow it, in equal strides.
        let strings = entries
            .iter()
            .map(|e| e.3)
            .filter(|&n| n != 0)
            .chain(std::iter::once(type_offset))
            .min()
            .unwrap_or(type_offset);
        let span = strings
            .checked_sub(data_start)
            .ok_or_else(|| invalid(FORMAT, 0x30, "row data starts after the strings"))?;
        let row_size = if count == 0 {
            0
        } else if span % count == 0 && span / count > 0 {
            span / count
        } else {
            return Err(invalid(
                FORMAT,
                data_start,
                format!("{span} bytes of row data do not divide into {count} rows"),
            ));
        };
        let mut rows = Vec::with_capacity(count);
        let mut spans = vec![(0, HEADER), (HEADER, count * ROW_ENTRY), (type_offset, type_len)];
        if count > 0 {
            spans.push((data_start, span));
        }
        let mut names: Vec<(usize, usize)> = Vec::new();
        for (i, &(at, id, offset, name)) in entries.iter().enumerate() {
            if offset != data_start + i * row_size {
                return Err(invalid(
                    FORMAT,
                    at + 8,
                    format!("row {id} data at {offset:#x}, not in stride after the first"),
                ));
            }
            let name = if name == 0 {
                None
            } else {
                let (text, len) = utf16(&v, name)?;
                names.push((name, len));
                Some(text)
            };
            rows.push(Row {
                id,
                name,
                data: v.slice(offset, row_size)?.to_vec(),
            });
        }
        names.sort_unstable();
        names.dedup();
        spans.extend(names);
        coverage(&v, spans, data.len(), PADDING)?;
        Ok(Param {
            param_type,
            data_version,
            unk06,
            row_size,
            rows,
        })
    }
}

/// One row of the NPC param (`NPC_PARAM_ST`): its id and the 32 display mask bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct NpcRow {
    pub id: i32,
    /// `modelDispMask0` to `modelDispMask31`: bit `k` is `modelDispMask<k>`. The game shows the meshes of a model
    /// whose materials are named `#<k>#` when it is set.
    pub display_mask: u32,
}

/// The param type [`NpcParam::from_param`] reads.
pub const NPC_PARAM_TYPE: &str = "NPC_PARAM_ST";

/// Where a game's NPC param keeps the display masks: the layout revision and row size the offsets are for, and the bytes
/// of the four groups of eight one-bit fields `modelDispMask0..7`, `8..15`, `16..23`, `24..31`, each byte holding its
/// fields low bit first ([`crate::Game::npc_param`] states each game's, from Paramdex's `NpcParam.xml`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NpcLayout {
    pub data_version: u16,
    pub row_size: usize,
    pub mask_bytes: [usize; 4],
}

/// The NPC param's rows, as far as this reader reads them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpcParam {
    pub rows: Vec<NpcRow>,
}

impl NpcParam {
    /// The display masks of every row of `param`, which must be `NPC_PARAM_ST` of `layout`'s data version and row size:
    /// the one layout whose field offsets are known for the game. Any other is refused.
    pub fn from_param(param: &Param, layout: &NpcLayout) -> Result<NpcParam> {
        if param.param_type != NPC_PARAM_TYPE
            || param.data_version != layout.data_version
            || param.row_size != layout.row_size
        {
            return Err(invalid(
                FORMAT,
                0,
                format!(
                    "{} version {} with rows of {} bytes, the NPC param read is {NPC_PARAM_TYPE} version {} with rows                      of {}",
                    param.param_type,
                    param.data_version,
                    param.row_size,
                    layout.data_version,
                    layout.row_size
                ),
            ));
        }
        let rows = param
            .rows
            .iter()
            .map(|r| {
                let display_mask = layout
                    .mask_bytes
                    .iter()
                    .enumerate()
                    .fold(0u32, |m, (g, &b)| m | u32::from(r.data[b]) << (8 * g));
                NpcRow {
                    id: r.id,
                    display_mask,
                }
            })
            .collect();
        Ok(NpcParam { rows })
    }

    /// Read `NpcParam.param`'s bytes, of `layout`.
    pub fn read(data: &[u8], layout: &NpcLayout) -> Result<NpcParam> {
        NpcParam::from_param(&Param::read(data)?, layout)
    }

    /// The row with `id`.
    pub fn row(&self, id: i32) -> Option<&NpcRow> {
        self.rows.iter().find(|r| r.id == id)
    }
}

/// Where a game's NPC param keeps the first fields of a row ([`NpcStats`]): the layout revision and row size the offsets
/// are for, and the byte of each field in [`NpcStats::NAMES`] order (Paramdex's `NpcParam.xml` as Smithbox ships it: the
/// fields are four bytes each from the start of the row, Sekiro's after `deadMoveForceRate`, Elden Ring's after a
/// four-byte header of `disableParam_NT` and its reserved bytes and `resistCorrectId_poison`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NpcStatLayout {
    pub data_version: u16,
    pub row_size: usize,
    pub offsets: [usize; 10],
}

/// The fields of an NPC param row that describe the character (its body and standing), not its fights: what
/// `NpcParam.xml` calls `behaviorVariationId`, `nameId`, `turnVellocity` (sic), `hitHeight`, `hitRadius`, `weight`,
/// `hitYOffset`, `hp`, `mp` and `getSoul`, with the units the definition's display names give. The row's other fields
/// (several hundred: resistances, attack rates, special effects, drops) are not read.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct NpcStats {
    pub id: i32,
    pub behavior_variation_id: i32,
    pub name_id: i32,
    /// Degrees a second.
    pub turn_velocity: f32,
    /// The collision capsule's height, metres.
    pub hit_height: f32,
    /// The collision capsule's radius, metres.
    pub hit_radius: f32,
    /// Kilograms.
    pub weight: u32,
    /// How far the model is drawn above the capsule's position, metres.
    pub hit_y_offset: f32,
    pub hp: u32,
    pub mp: u32,
    pub get_soul: u32,
}

impl NpcStats {
    /// The definition's names of the fields, in the order of [`NpcStatLayout::offsets`] and [`NpcStats::fields`].
    pub const NAMES: [&'static str; 10] = [
        "behaviorVariationId",
        "nameId",
        "turnVellocity",
        "hitHeight",
        "hitRadius",
        "weight",
        "hitYOffset",
        "hp",
        "mp",
        "getSoul",
    ];

    /// Each field as the definition names it, its value and its unit (empty where the definition's display name states
    /// none), in the order of [`NpcStats::NAMES`].
    pub fn fields(&self) -> [(&'static str, f64, &'static str); 10] {
        let values = [
            f64::from(self.behavior_variation_id),
            f64::from(self.name_id),
            f64::from(self.turn_velocity),
            f64::from(self.hit_height),
            f64::from(self.hit_radius),
            f64::from(self.weight),
            f64::from(self.hit_y_offset),
            f64::from(self.hp),
            f64::from(self.mp),
            f64::from(self.get_soul),
        ];
        let units = ["", "", "deg/sec", "m", "m", "kg", "m", "", "", ""];
        std::array::from_fn(|i| (Self::NAMES[i], values[i], units[i]))
    }

    /// The stats of every row of `param`, which must be `NPC_PARAM_ST` of `layout`'s data version and row size. A row
    /// whose float fields are not finite is refused, naming it.
    pub fn from_param(param: &Param, layout: &NpcStatLayout) -> Result<Vec<NpcStats>> {
        if param.param_type != NPC_PARAM_TYPE
            || param.data_version != layout.data_version
            || param.row_size != layout.row_size
        {
            return Err(invalid(
                FORMAT,
                0,
                format!(
                    "{} version {} with rows of {} bytes, the NPC stats read are {NPC_PARAM_TYPE} version {} with rows of {}",
                    param.param_type,
                    param.data_version,
                    param.row_size,
                    layout.data_version,
                    layout.row_size
                ),
            ));
        }
        let [
            behavior,
            name,
            turn,
            height,
            radius,
            weight,
            y_offset,
            hp,
            mp,
            soul,
        ] = layout.offsets;
        let mut out = Vec::with_capacity(param.rows.len());
        for r in &param.rows {
            let word = |at: usize| -> Result<[u8; 4]> {
                r.data
                    .get(at..at + 4)
                    .and_then(|b| <[u8; 4]>::try_from(b).ok())
                    .ok_or_else(|| invalid(FORMAT, at, format!("row {} ends before byte {at}", r.id)))
            };
            let float = |at: usize| -> Result<f32> {
                let n = f32::from_le_bytes(word(at)?);
                if n.is_finite() {
                    Ok(n)
                } else {
                    Err(invalid(
                        FORMAT,
                        at,
                        format!("row {}: a field that is not finite", r.id),
                    ))
                }
            };
            out.push(NpcStats {
                id: r.id,
                behavior_variation_id: i32::from_le_bytes(word(behavior)?),
                name_id: i32::from_le_bytes(word(name)?),
                turn_velocity: float(turn)?,
                hit_height: float(height)?,
                hit_radius: float(radius)?,
                weight: u32::from_le_bytes(word(weight)?),
                hit_y_offset: float(y_offset)?,
                hp: u32::from_le_bytes(word(hp)?),
                mp: u32::from_le_bytes(word(mp)?),
                get_soul: u32::from_le_bytes(word(soul)?),
            });
        }
        Ok(out)
    }
}

/// The param type [`GrassParam::from_param`] reads.
pub const GRASS_PARAM_TYPE: &str = "GRASS_TYPE_PARAM_ST";

/// The bytes of a `GRASS_TYPE_PARAM_ST` row up to the end of its last field (`model1Name`), in Paramdex's
/// `ER/Defs/GrassTypeParam.xml` as Smithbox ships it (FormatVersion 203; fields laid out as SoulsFormats' `PARAMDEF`
/// packs them). A row of fewer bytes is not that layout; a longer one appends fields, which are not read.
const GRASS_ROW: usize = 276;

/// One row of the grass type param: what the game grows where a map part names the row. Only the fields that say which
/// models and textures: the placement of the blades is the game's (a density over the part's surface and a spacing).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GrassType {
    pub id: i32,
    /// `lodRange`: the distance the LODs change at.
    pub lod_range: u16,
    /// `baseDensity` and `spacing`.
    pub base_density: f32,
    pub spacing: f32,
    /// `model0Name`, `model1Name` and `simpleModelName` (empty: none).
    pub model0: String,
    pub model1: String,
    pub simple_model: String,
    /// `flatTextureName` and `billboardTextureName` (empty: none).
    pub flat_texture: String,
    pub billboard_texture: String,
}

/// The grass type param's rows, as far as this reader reads them.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GrassParam {
    pub data_version: u16,
    pub row_size: usize,
    pub rows: Vec<GrassType>,
}

/// A fixed-length UTF-16 string field of `chars` characters at `at`: up to its first NUL; every character printable
/// ASCII, or the row is not this layout.
fn fixed_string(row: &[u8], at: usize, chars: usize) -> Result<String> {
    let bytes = row
        .get(at..at + 2 * chars)
        .ok_or_else(|| invalid(FORMAT, at, "string beyond the row"))?;
    let mut text = String::new();
    for pair in bytes.as_chunks::<2>().0 {
        let c = u16::from_le_bytes(*pair);
        if c == 0 {
            break;
        }
        match u8::try_from(c) {
            Ok(b) if (0x20..0x7f).contains(&b) => text.push(char::from(b)),
            _ => {
                return Err(invalid(
                    FORMAT,
                    at,
                    format!("character {c:#06x} in a name field: the row is not the layout read"),
                ));
            }
        }
    }
    Ok(text)
}

impl GrassParam {
    /// The models and textures of every row of `param`, which must be `GRASS_TYPE_PARAM_ST` with rows of at least
    /// 276 bytes whose name fields hold printable ASCII (the one layout whose offsets are known; any other is refused).
    pub fn from_param(param: &Param) -> Result<GrassParam> {
        if param.param_type != GRASS_PARAM_TYPE || param.row_size < GRASS_ROW {
            return Err(invalid(
                FORMAT,
                0,
                format!(
                    "{} version {} with rows of {} bytes, the grass param read is {GRASS_PARAM_TYPE} with rows of at least {GRASS_ROW}",
                    param.param_type, param.data_version, param.row_size
                ),
            ));
        }
        let f32_at = |data: &[u8], at: usize| -> Result<f32> {
            data.get(at..at + 4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or_else(|| invalid(FORMAT, at, "float beyond the row"))
        };
        let rows = param
            .rows
            .iter()
            .map(|r| {
                Ok(GrassType {
                    id: r.id,
                    lod_range: u16::from_le_bytes([r.data[0], r.data[1]]),
                    base_density: f32_at(&r.data, 8)?,
                    spacing: f32_at(&r.data, 204)?,
                    model0: fixed_string(&r.data, 12, 16)?,
                    flat_texture: fixed_string(&r.data, 44, 32)?,
                    billboard_texture: fixed_string(&r.data, 108, 32)?,
                    simple_model: fixed_string(&r.data, 212, 16)?,
                    model1: fixed_string(&r.data, 244, 16)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GrassParam {
            data_version: param.data_version,
            row_size: param.row_size,
            rows,
        })
    }

    /// The row with `id`.
    pub fn row(&self, id: i32) -> Option<&GrassType> {
        self.rows.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A param of `rows` (id, data), all of `size` bytes, every row named the empty string.
    pub(crate) fn fixture(param_type: &str, version: u16, size: usize, rows: &[(i32, Vec<u8>)]) -> Vec<u8> {
        let n = rows.len();
        let data_start = HEADER + n * ROW_ENTRY;
        let type_offset = data_start + n * size;
        let names = type_offset + param_type.len() + 1;
        let mut b = vec![0u8; names + 2];
        b[0..4].copy_from_slice(&(type_offset as u32).to_le_bytes());
        b[6..8].copy_from_slice(&1u16.to_le_bytes());
        b[8..10].copy_from_slice(&version.to_le_bytes());
        b[10..12].copy_from_slice(&(n as u16).to_le_bytes());
        b[0x10..0x18].copy_from_slice(&(type_offset as u64).to_le_bytes());
        b[0x2d] = FORMAT_2D;
        b[0x2e] = FORMAT_2E;
        b[0x30..0x38].copy_from_slice(&(data_start as u64).to_le_bytes());
        for (i, (id, data)) in rows.iter().enumerate() {
            assert_eq!(data.len(), size);
            let at = HEADER + i * ROW_ENTRY;
            b[at..at + 4].copy_from_slice(&id.to_le_bytes());
            b[at + 8..at + 16].copy_from_slice(&((data_start + i * size) as u64).to_le_bytes());
            b[at + 16..at + 24].copy_from_slice(&(names as u64).to_le_bytes());
            b[data_start + i * size..][..size].copy_from_slice(data);
        }
        b[type_offset..][..param_type.len()].copy_from_slice(param_type.as_bytes());
        b
    }

    use crate::Game;

    fn npc_row(layout: &NpcLayout, bits: u32) -> Vec<u8> {
        let mut d = vec![0u8; layout.row_size];
        for (g, &b) in layout.mask_bytes.iter().enumerate() {
            d[b] = (bits >> (8 * g)) as u8;
        }
        d
    }

    #[test]
    fn rows_read_as_stored_with_each_games_layout() {
        for game in Game::ALL {
            let l = game.npc_param();
            let bytes = fixture(
                NPC_PARAM_TYPE,
                l.data_version,
                l.row_size,
                &[
                    (10100000, npc_row(&l, 0b1011)),
                    (10100001, npc_row(&l, 1 << 31 | 1 << 16 | 1 << 8)),
                ],
            );
            let p = Param::read(&bytes).unwrap();
            assert_eq!(
                (p.param_type.as_str(), p.data_version, p.row_size),
                (NPC_PARAM_TYPE, l.data_version, l.row_size)
            );
            assert_eq!(p.rows.len(), 2);
            assert_eq!(p.rows[0].name.as_deref(), Some(""));
            let npc = NpcParam::from_param(&p, &l).unwrap();
            assert_eq!(
                npc.rows,
                [
                    NpcRow {
                        id: 10100000,
                        display_mask: 0b1011
                    },
                    NpcRow {
                        id: 10100001,
                        display_mask: 1 << 31 | 1 << 16 | 1 << 8
                    }
                ]
            );
            assert_eq!(npc.row(10100001).unwrap().display_mask >> 31, 1);
            assert!(npc.row(7).is_none());
        }
    }

    #[test]
    fn a_layout_other_than_the_games_is_refused() {
        let (sekiro, elden) = (Game::Sekiro.npc_param(), Game::EldenRing.npc_param());
        // one game's NPC param is not read with the other's offsets
        let bytes = fixture(
            NPC_PARAM_TYPE,
            elden.data_version,
            elden.row_size,
            &[(1, npc_row(&elden, 1))],
        );
        let e = NpcParam::read(&bytes, &sekiro).unwrap_err().to_string();
        assert!(e.contains("version 9"), "{e}");
        let short = fixture(NPC_PARAM_TYPE, sekiro.data_version, 8, &[(1, vec![0; 8])]);
        assert!(NpcParam::read(&short, &sekiro).is_err());
        let other_type = fixture(
            "EQUIP_PARAM_GOODS_ST",
            sekiro.data_version,
            sekiro.row_size,
            &[(1, npc_row(&sekiro, 0))],
        );
        assert!(NpcParam::read(&other_type, &sekiro).is_err());
    }

    #[test]
    fn bytes_the_file_does_not_account_for_are_refused() {
        let l = Game::Sekiro.npc_param();
        let one = |bits| {
            fixture(
                NPC_PARAM_TYPE,
                l.data_version,
                l.row_size,
                &[(1, npc_row(&l, bits))],
            )
        };
        let mut bytes = one(1);
        bytes.extend_from_slice(&[0; 2]);
        Param::read(&bytes).unwrap();
        bytes.extend_from_slice(&[1; 8]);
        assert!(Param::read(&bytes).is_err());
        let mut flags = one(1);
        flags[0x2d] = 0x04;
        let e = Param::read(&flags).unwrap_err().to_string();
        assert!(e.contains("format flags"), "{e}");
        let mut stride = fixture(
            NPC_PARAM_TYPE,
            l.data_version,
            l.row_size,
            &[(1, npc_row(&l, 1)), (2, npc_row(&l, 2))],
        );
        let second = HEADER + ROW_ENTRY + 8;
        let at = u64::from_le_bytes(stride[second..second + 8].try_into().unwrap()) + 8;
        stride[second..second + 8].copy_from_slice(&at.to_le_bytes());
        assert!(Param::read(&stride).is_err());
        assert!(Param::read(&stride[..40]).is_err());
    }

    fn grass_row(model0: &str, billboard: &str, density: f32) -> Vec<u8> {
        let mut d = vec![0u8; GRASS_ROW];
        d[0..2].copy_from_slice(&40u16.to_le_bytes());
        d[8..12].copy_from_slice(&density.to_le_bytes());
        d[204..208].copy_from_slice(&0.5f32.to_le_bytes());
        for (at, text) in [(12, model0), (108, billboard), (244, "M1")] {
            for (i, c) in text.encode_utf16().enumerate() {
                d[at + 2 * i..at + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
            }
        }
        d
    }

    #[test]
    fn grass_types_name_their_models_and_textures_and_a_row_of_another_layout_is_refused() {
        let bytes = fixture(
            GRASS_PARAM_TYPE,
            1,
            GRASS_ROW,
            &[
                (10, grass_row("Grass_A", "bb_a", 1.5)),
                (11, grass_row("", "", 0.0)),
            ],
        );
        let grass = GrassParam::from_param(&Param::read(&bytes).unwrap()).unwrap();
        let a = grass.row(10).unwrap();
        assert_eq!(
            (
                a.lod_range,
                a.base_density,
                a.spacing,
                a.model0.as_str(),
                a.model1.as_str()
            ),
            (40, 1.5, 0.5, "Grass_A", "M1")
        );
        assert_eq!(
            (
                a.billboard_texture.as_str(),
                a.flat_texture.as_str(),
                a.simple_model.as_str()
            ),
            ("bb_a", "", "")
        );
        assert_eq!(grass.row(11).unwrap().model0, "");
        assert!(grass.row(12).is_none());
        // the NPC param is not a grass param, and a short row or a row of non-text name fields is another layout
        let npc = NpcLayout {
            data_version: 9,
            row_size: 736,
            mask_bytes: [330, 331, 334, 335],
        };
        let other = fixture(NPC_PARAM_TYPE, 9, npc.row_size, &[(1, npc_row(&npc, 1))]);
        assert!(GrassParam::from_param(&Param::read(&other).unwrap()).is_err());
        let short = fixture(GRASS_PARAM_TYPE, 1, GRASS_ROW - 4, &[(1, vec![0; GRASS_ROW - 4])]);
        assert!(GrassParam::from_param(&Param::read(&short).unwrap()).is_err());
        let mut binary = grass_row("x", "", 1.0);
        binary[12..14].copy_from_slice(&0x1234u16.to_le_bytes());
        let garbled = fixture(GRASS_PARAM_TYPE, 1, GRASS_ROW, &[(1, binary)]);
        let e = GrassParam::from_param(&Param::read(&garbled).unwrap())
            .unwrap_err()
            .to_string();
        assert!(e.contains("not the layout read"), "{e}");
    }
}
