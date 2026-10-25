//! Map models, part placements and map-local offsets of Sekiro (MSBS) and Elden Ring (MSBE).
//!
//! Part coordinates are already in the map's frame. Sekiro's MapOffset event
//! instead places map-local resources (for example collision); never apply it a
//! second time to parts. Euler angles are degrees, applied X, then Z, then Y.
//! The generations share the section chain and the record envelopes; the [`Generation`] the file's section chain
//! names supplies what differs.
use crate::{Result, bytes::View, invalid};
use primase_core::components::Transform;

use crate::pose::FRAME;

/// How a part's point-light shadow byte is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagByte {
    /// 0 or 1.
    Bool,
    /// 0 (off) or 0xff (-1, on).
    ZeroOrMinusOne,
}
/// One MSB generation: its section chain, record layouts and kind codes. The section chain (its names, in order)
/// identifies the generation. The number each section stores is a revision (Elden Ring's maps store several with
/// one layout; Sekiro's sections within one file differ): it is kept in [`Section::version`] and does not choose a
/// layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation {
    /// The generation's common name (`MSBS`, `MSBE`).
    pub name: &'static str,
    /// Section names in file order.
    pub sections: &'static [&'static str],
    /// Indices of sections that must have no records.
    pub empty: &'static [usize],
    /// Accepted model kind codes.
    pub model_kinds: &'static [u32],
    /// Part kind codes.
    pub part_kinds: &'static [(u32, PartKind)],
    /// Part record header size in bytes, and where it keeps the kind and the model index (u32 each).
    pub part_header: usize,
    pub part_kind: usize,
    pub part_model: usize,
    /// Header bytes with fixed contents: (offset, bytes).
    pub part_fixed: &'static [(usize, &'static [u8])],
    /// The header's payload offset slots: offset of the first u64 slot and the slot count. The entity block (96) and
    /// type data (104) slots are among them in every generation and must be set.
    pub part_payloads: (usize, usize),
    /// Entity block size, and a range of it that must be zero.
    pub entity_block: usize,
    pub entity_zero: Option<(usize, usize)>,
    pub point_light_shadow: FlagByte,
    /// Event type code of MapOffset records, in generations that have them.
    pub map_offset_event: Option<u32>,
    /// Region type code of checkpoint/warp spawn points, where known.
    pub spawn_region: Option<u32>,
}
impl Generation {
    /// Sekiro (MSBS): eight sections, ending with part poses and bone names.
    pub const SEKIRO: Self = Self {
        name: "MSBS",
        sections: &[
            "MODEL_PARAM_ST",
            "EVENT_PARAM_ST",
            "POINT_PARAM_ST",
            "ROUTE_PARAM_ST",
            "LAYER_PARAM_ST",
            "PARTS_PARAM_ST",
            "MAPSTUDIO_PARTS_POSE_ST",
            "MAPSTUDIO_BONE_NAME_STRING",
        ],
        empty: &[4, 6, 7],
        model_kinds: &[0, 1, 2, 4, 5],
        part_kinds: &[
            (0, PartKind::MapPiece),
            (1, PartKind::Object),
            (2, PartKind::Enemy),
            (4, PartKind::Player),
            (5, PartKind::Collision),
            (9, PartKind::DummyObject),
            (10, PartKind::DummyEnemy),
            (11, PartKind::ConnectCollision),
        ],
        part_header: 160,
        part_kind: 8,
        part_model: 16,
        part_fixed: &[(20, &[0; 4]), (68, &[255; 8]), (76, &[0; 4]), (136, &[0; 24])],
        part_payloads: (80, 7),
        entity_block: 0x54,
        entity_zero: Some((0x44, 16)),
        point_light_shadow: FlagByte::Bool,
        map_offset_event: Some(9),
        spawn_region: Some(8),
    };
    /// Elden Ring (MSBE): six sections ending with parts; assets take the place of objects. It has no MapOffset
    /// events; its spawn points are region type 8, as Sekiro's (the region header is laid out alike: name offset,
    /// type at 8, position at 20, rotation at 32).
    pub const ELDEN_RING: Self = Self {
        name: "MSBE",
        sections: &[
            "MODEL_PARAM_ST",
            "EVENT_PARAM_ST",
            "POINT_PARAM_ST",
            "ROUTE_PARAM_ST",
            "LAYER_PARAM_ST",
            "PARTS_PARAM_ST",
        ],
        empty: &[4],
        model_kinds: &[0, 2, 4, 5, 10],
        part_kinds: &[
            (0, PartKind::MapPiece),
            (2, PartKind::Enemy),
            (4, PartKind::Player),
            (5, PartKind::Collision),
            (9, PartKind::DummyAsset),
            (10, PartKind::DummyEnemy),
            (11, PartKind::ConnectCollision),
            (13, PartKind::Asset),
        ],
        part_header: 192,
        part_kind: 12,
        part_model: 20,
        part_fixed: &[(76, &[0; 4]), (168, &[0; 24])],
        part_payloads: (80, 11),
        entity_block: 0x40,
        entity_zero: None,
        point_light_shadow: FlagByte::ZeroOrMinusOne,
        map_offset_event: None,
        spawn_region: Some(8),
    };
    pub const ALL: [&'static Self; 2] = [&Self::SEKIRO, &Self::ELDEN_RING];
    /// The generation whose section chain is `names`.
    pub fn find<S: AsRef<str>>(names: &[S]) -> Option<&'static Self> {
        Self::ALL.into_iter().find(|g| {
            g.sections.len() == names.len() && g.sections.iter().zip(names).all(|(a, b)| *a == b.as_ref())
        })
    }
}
/// The section names, read before choosing a generation: each section's name pointer sits at +8 and its last
/// offset points at the next section (zero after the last). At most 16 sections.
fn section_chain(v: &View<'_>) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut start = 16;
    loop {
        if names.len() == 16 {
            return Err(invalid(v.format, start, "section chain too long"));
        }
        let n = v.u32(start + 4)? as usize;
        if n == 0 {
            return Err(invalid(v.format, start + 4, "section has no next pointer"));
        }
        let table = n
            .checked_mul(8)
            .ok_or_else(|| invalid(v.format, start, "section size overflow"))?;
        v.slice(start + 16, table)?;
        names.push(text(v, v.size(start + 8)?)?);
        let next = v.size(start + 16 + (n - 1) * 8)?;
        if next == 0 {
            return Ok(names);
        }
        if next <= start {
            return Err(invalid(v.format, start, "section chain goes backwards"));
        }
        start = next;
    }
}

#[derive(Clone, Debug)]
pub struct Model {
    pub name: String,
    pub kind: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartKind {
    MapPiece,
    Object,
    Enemy,
    Player,
    Collision,
    DummyObject,
    DummyEnemy,
    ConnectCollision,
    /// Elden Ring's placed asset geometry (its counterpart of Sekiro's objects).
    Asset,
    DummyAsset,
}
#[derive(Clone, Debug)]
pub struct Part {
    /// Original file order, stable even when names repeat.
    pub index: usize,
    pub name: String,
    pub kind: PartKind,
    pub model: Option<usize>,
    /// Collision-part hit filter; absent on other part kinds.
    pub hit_filter: Option<u8>,
    /// An enemy's or dummy enemy's NPC param row (`NPCParamID`, type data +12 in both generations: two zero words,
    /// the think param row, then the NPC row, as SoulsFormats' `MSBS`/`MSBE` `EnemyBase.ReadTypeData` read them);
    /// absent on other part kinds.
    pub npc_param: Option<i32>,
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub scale: [f32; 3],
    /// The part's own entity ID as stored (event scripts address it; -1 or 0: none).
    pub entity_id: i32,
    /// The entity groups it belongs to, as stored (unused slots -1 or 0); a script addressing a group addresses
    /// every member.
    pub entity_group_ids: [i32; 8],
    /// Source shadow controls, independent of material and placement state.
    pub shadows: ShadowFlags,
}
/// Raw entity-block shadow controls. Renderer-specific bytes are retained
/// uninterpreted. `only && !cast` is contradictory: retain and count it rather
/// than guessing a consumer's shadow mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ShadowFlags {
    /// Boolean byte at entity-block offset 0x0c; explicit false is retained.
    pub cast: bool,
    /// Boolean byte at entity-block offset 0x11; explicit false is retained.
    pub receive: bool,
    /// Boolean byte at entity-block offset 0x12, retained independently of `cast`.
    pub only: bool,
    /// Byte at entity-block offset 0x0a for point-light shadow casting (0/1 in MSBS, 0/0xff in MSBE).
    pub point_light_cast: bool,
    /// Uninterpreted byte at entity-block offset 0x0d; all byte values are retained.
    pub static_cast: u8,
    /// Uninterpreted byte at entity-block offset 0x0e; all byte values are retained.
    pub cascade3_cast: u8,
}
impl primase_core::Component for ShadowFlags {
    const NAME: &'static str = "fromsoft.shadow_flags";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
#[derive(Clone, Debug)]
pub struct MapOffset {
    pub name: String,
    pub position: [f32; 3],
    pub degrees: f32,
}
/// Record envelopes are validated; unrelated gameplay payloads remain opaque.
#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub version: u32,
    pub records: Vec<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct MapLayout {
    /// The generation the section chain names.
    pub generation: &'static Generation,
    pub models: Vec<Model>,
    pub parts: Vec<Part>,
    pub map_offsets: Vec<MapOffset>,
    pub sections: Vec<Section>,
}
fn text(v: &View<'_>, mut o: usize) -> Result<String> {
    let start = o;
    let mut units = Vec::new();
    loop {
        let x = v.u16(o)?;
        o += 2;
        if x == 0 {
            break;
        }
        units.push(x);
    }
    String::from_utf16(&units).map_err(|e| invalid(v.format, start, e))
}
fn f32_at(v: &View<'_>, o: usize) -> Result<f32> {
    let f = f32::from_bits(v.u32(o)?);
    if !f.is_finite() {
        return Err(invalid(v.format, o, "non-finite value"));
    }
    Ok(f)
}
fn vector(v: &View<'_>, o: usize) -> Result<[f32; 3]> {
    Ok([f32_at(v, o)?, f32_at(v, o + 4)?, f32_at(v, o + 8)?])
}
fn pointer(v: &View<'_>, o: usize, min: usize) -> Result<usize> {
    let p = v.size(o)?;
    if p < min {
        return Err(invalid(v.format, o, "pointer into record header"));
    }
    v.slice(p, 1)?;
    Ok(p)
}

impl MapLayout {
    /// Parse section envelopes and the fields used by placement import, in the generation the section chain names.
    /// Other gameplay records are retained, without claiming to validate their schemas.
    pub fn read(data: &[u8]) -> Result<Self> {
        let v = View {
            data,
            format: "MSB",
            be: false,
        };
        v.expect(0, b"MSB \x01\0\0\0\x10\0\0\0\0\0\x01\xff")?;
        let names = section_chain(&v)?;
        let generation = Generation::find(&names)
            .ok_or_else(|| invalid(v.format, 16, format!("unknown section chain {names:?}")))?;
        let last = generation.sections.len() - 1;
        let mut next = 16;
        let mut sections = Vec::new();
        for (section_index, &name) in generation.sections.iter().enumerate() {
            let start = next;
            let version = v.u32(start)?;
            let n = v.u32(start + 4)? as usize;
            if n == 0 {
                return Err(invalid(v.format, start + 4, "section has no next pointer"));
            }
            let bytes = n
                .checked_mul(8)
                .ok_or_else(|| invalid(v.format, start, "section size overflow"))?;
            v.slice(start + 16, bytes)?;
            next = v.size(start + 16 + (n - 1) * 8)?;
            let end = if next == 0 { data.len() } else { next };
            if (section_index == last) != (next == 0) || end <= start || end > data.len() {
                return Err(invalid(v.format, start, "invalid section chain"));
            }
            let header_end = start + 16 + bytes;
            if header_end > end {
                return Err(invalid(v.format, start, "section table overlaps next section"));
            }
            let section = View {
                data: v.slice(0, end)?,
                format: v.format,
                be: false,
            };
            let name_offset = v.size(start + 8)?;
            if name_offset < header_end || text(&section, name_offset)? != name {
                return Err(invalid(v.format, start + 8, "section name mismatch"));
            }
            let mut offsets = Vec::new();
            for i in 0..n - 1 {
                let p = v.size(start + 16 + i * 8)?;
                if p < header_end || p >= end || offsets.last().is_some_and(|&last| last >= p) {
                    return Err(invalid(v.format, start + 16 + i * 8, "invalid entry offset"));
                }
                offsets.push(p);
            }
            offsets.push(end);
            let records = offsets.windows(2).map(|r| data[r[0]..r[1]].to_vec()).collect();
            sections.push(Section {
                name: name.to_owned(),
                version,
                records,
            });
        }
        for &i in generation.empty {
            if !sections[i].records.is_empty() {
                return Err(invalid("MSB", 0, "nonempty unsupported section"));
            }
        }
        let mut models = Vec::new();
        for data in &sections[0].records {
            let r = View {
                data,
                format: "MSB model",
                be: false,
            };
            r.slice(0, 40)?;
            let name = text(&r, pointer(&r, 0, 40)?)?;
            text(&r, pointer(&r, 16, 40)?)?;
            let type_offset = r.size(32)?;
            if type_offset != 0 {
                r.slice(type_offset, 1)?;
            }
            let kind = r.u32(8)?;
            if !generation.model_kinds.contains(&kind) {
                return Err(invalid(r.format, 8, "model kind"));
            }
            models.push(Model { name, kind });
        }
        let mut parts = Vec::new();
        let header = generation.part_header;
        for (index, data) in sections[5].records.iter().enumerate() {
            let r = View {
                data,
                format: "MSB part",
                be: false,
            };
            r.slice(0, header)?;
            let name = text(&r, pointer(&r, 0, header)?)?;
            text(&r, pointer(&r, 24, header)?)?;
            let code = r.u32(generation.part_kind)?;
            let kind = generation
                .part_kinds
                .iter()
                .find(|(c, _)| *c == code)
                .map(|(_, k)| *k)
                .ok_or_else(|| invalid(r.format, generation.part_kind, "unknown part kind"))?;
            for &(offset, bytes) in generation.part_fixed {
                r.expect(offset, bytes)?;
            }
            let model = r.u32(generation.part_model)?;
            let model = if model == u32::MAX {
                None
            } else if (model as usize) < models.len() {
                Some(model as usize)
            } else {
                return Err(invalid(r.format, generation.part_model, "model index"));
            };
            let (first, count) = generation.part_payloads;
            for field in (0..count).map(|i| first + i * 8) {
                let p = r.size(field)?;
                if p == 0 {
                    if matches!(field, 96 | 104) {
                        return Err(invalid(r.format, field, "missing part payload"));
                    }
                } else {
                    pointer(&r, field, header)?;
                }
            }
            // entity block: ID at +0, shadow controls from +0x0a, eight group IDs at +0x1c
            let entity = r.size(96)?;
            r.slice(entity, generation.entity_block)?;
            if let Some((offset, len)) = generation.entity_zero {
                r.expect(entity + offset, &vec![0; len])?;
            }
            let i32_at = |o: usize| r.u32(o).map(|v| v as i32);
            let entity_id = i32_at(entity)?;
            let mut entity_group_ids = [0i32; 8];
            for (k, g) in entity_group_ids.iter_mut().enumerate() {
                *g = i32_at(entity + 0x1c + 4 * k)?;
            }
            parts.push(Part {
                index,
                name,
                kind,
                model,
                entity_id,
                entity_group_ids,
                shadows: ShadowFlags {
                    cast: r.boolean(entity + 0x0c)?,
                    receive: r.boolean(entity + 0x11)?,
                    only: r.boolean(entity + 0x12)?,
                    point_light_cast: match generation.point_light_shadow {
                        FlagByte::Bool => r.boolean(entity + 0x0a)?,
                        FlagByte::ZeroOrMinusOne => match r.u8(entity + 0x0a)? {
                            0 => false,
                            0xff => true,
                            _ => return Err(invalid(r.format, entity + 0x0a, "point-light shadow byte")),
                        },
                    },
                    static_cast: r.u8(entity + 0x0d)?,
                    cascade3_cast: r.u8(entity + 0x0e)?,
                },
                hit_filter: if kind == PartKind::Collision {
                    Some(r.slice(r.size(104)?, 1)?[0])
                } else {
                    None
                },
                npc_param: if matches!(kind, PartKind::Enemy | PartKind::DummyEnemy) {
                    let data = r.size(104)?;
                    r.expect(data, &[0; 8])?;
                    Some(i32_at(data + 12)?)
                } else {
                    None
                },
                position: vector(&r, 32)?,
                rotation_degrees: vector(&r, 44)?,
                scale: vector(&r, 56)?,
            });
        }
        let mut map_offsets = Vec::new();
        for data in &sections[1].records {
            let r = View {
                data,
                format: "MSB event",
                be: false,
            };
            r.slice(0, 40)?;
            if Some(r.u32(12)?) == generation.map_offset_event {
                let name = text(&r, pointer(&r, 0, 40)?)?;
                let base = pointer(&r, 24, 40)?;
                r.slice(base, 16)?;
                let p = pointer(&r, 32, 40)?;
                map_offsets.push(MapOffset {
                    name,
                    position: vector(&r, p)?,
                    degrees: f32_at(&r, p + 12)?,
                });
            }
        }
        Ok(Self {
            generation,
            models,
            parts,
            map_offsets,
            sections,
        })
    }
}
impl Part {
    /// See [`euler_transform`].
    pub fn transform(&self) -> Result<Transform> {
        euler_transform(self.position, self.rotation_degrees, self.scale, self.index)
    }

    /// The IDs an event script can address this part by: its entity ID and its groups, the positive ones only
    /// (stored sentinels never match).
    pub fn entity_ids(&self) -> impl Iterator<Item = i32> + '_ {
        std::iter::once(self.entity_id)
            .chain(self.entity_group_ids)
            .filter(|&id| id > 0)
    }
}

impl MapLayout {
    /// A human enemy's equipment selector, distinct from NPCParamID. SoulsFormats MSBS/MSBE
    /// EnemyBase.ReadTypeData both read CharaInitID at type-data +0x18, after think/NPC/talk and platoon.
    /// A negative stored ID means no initializer; non-enemies have no such field.
    pub fn character_init(&self, index: usize) -> Result<Option<i32>> {
        let part = self.parts.get(index).ok_or_else(|| invalid("MSB part", index, "no part"))?;
        if !matches!(part.kind, PartKind::Enemy | PartKind::DummyEnemy) { return Ok(None); }
        let data = self.sections.get(5).and_then(|s| s.records.get(index))
            .ok_or_else(|| invalid("MSB part", index, "no source record"))?;
        let r = View { data, format: "MSB character initializer", be: false };
        let at = r.size(104)?;
        r.expect(at, &[0; 8])?;
        let id = r.u32(at + 0x18)? as i32;
        Ok((id >= 0).then_some(id))
    }
}

/// Scale, X-Z-Y Euler rotation, translation in the source frame, then the
/// core's handedness conversion. Trigonometry/matrix products use f64 and
/// round once to each stored f32 basis component; translations remain exact.
/// `index` names the record in errors.
pub fn euler_transform(
    position: [f32; 3],
    rotation_degrees: [f32; 3],
    scale: [f32; 3],
    index: usize,
) -> Result<Transform> {
    if !position
        .iter()
        .chain(&rotation_degrees)
        .chain(&scale)
        .all(|x| x.is_finite())
    {
        return Err(invalid("MSB transform", index, "non-finite input"));
    }
    let [x, y, z] = rotation_degrees.map(|a| f64::from(a).to_radians());
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    // Columns of Ry * Rz * Rx, each multiplied by its local-axis scale.
    let basis = [
        [cy * cz, sz, -sy * cz],
        [sy * sx - cy * sz * cx, cz * cx, cy * sx + sy * sz * cx],
        [cy * sz * sx + sy * cx, -cz * sx, cy * cx - sy * sz * sx],
    ];
    let mut transform = Transform { cols: [[0.; 3]; 4] };
    for (i, col) in basis.into_iter().enumerate() {
        transform.cols[i] = col.map(|x| (x * f64::from(scale[i])) as f32);
    }
    transform.cols[3] = position;
    if !transform.cols.iter().flatten().all(|x| x.is_finite()) {
        return Err(invalid("MSB transform", index, "basis overflow"));
    }
    Ok(FRAME.transform(&transform))
}
impl MapOffset {
    /// Map-local data to the common world frame, never compounded onto parts.
    pub fn transform(&self) -> Result<Transform> {
        euler_transform(self.position, [0., self.degrees, 0.], [1.; 3], 0)
    }
}
/// The graphics config a part stores (SoulsFormats' `GparamConfig`, the part header's slot at +112): the value IDs it
/// takes from the groups of the draw-param file in force there. `light_set` selects the value set of the `LightSet
/// ParamEditor` group, `fog` of `FogParamEditor`, `light_scattering` of `LightScattering : ParamEditor` and `env_map` of
/// `Env Map:Editor` ([`crate::gparam`]). A collision part also stores a scene config (the slot at +120): four signed
/// bytes `scene` that choose draw-param files by the hundreds digit of their ID in Elden Ring (SoulsFormats'
/// `GparamSubID_Base` and three overrides) and that Sekiro's SoulsFormats names `EventIDs` and does not explain; they
/// are kept as stored, -1 meaning none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartGparam {
    /// The part's index in the layout's parts ([`Part::index`]).
    pub part: usize,
    pub light_set: i32,
    pub fog: i32,
    pub light_scattering: i32,
    pub env_map: i32,
    pub scene: Option<[i8; 4]>,
}

impl Generation {
    /// Where the four scene bytes sit in a part's scene config: Sekiro's block is 0x3c zero bytes, the four, a float and
    /// three zero words; Elden Ring's is 24 bytes (zeros, a transition time, a zero word), the four, and the rest.
    fn scene_ids_at(&self) -> usize {
        if self.name == Self::SEKIRO.name { 60 } else { 24 }
    }
}

impl MapLayout {
    /// The graphics config of every part that stores one, in part order. Slots are read where the generation puts them
    /// (the header's +112 and +120 in both), bounds-checked.
    pub fn gparam_configs(&self) -> Result<Vec<PartGparam>> {
        let mut out = Vec::new();
        let scene_ids = self.generation.scene_ids_at();
        for part in &self.parts {
            let data = self
                .sections
                .get(5)
                .and_then(|s| s.records.get(part.index))
                .ok_or_else(|| invalid("MSB part", part.index, "no record"))?;
            let r = View {
                data,
                format: "MSB part",
                be: false,
            };
            let config = r.size(112)?;
            if config == 0 {
                continue;
            }
            let word = |o: usize| r.u32(o).map(|v| v as i32);
            let scene = r.size(120)?;
            out.push(PartGparam {
                part: part.index,
                light_set: word(config)?,
                fog: word(config + 4)?,
                light_scattering: word(config + 8)?,
                env_map: word(config + 12)?,
                scene: if scene == 0 {
                    None
                } else {
                    let b = r.slice(scene + scene_ids, 4)?;
                    Some([b[0] as i8, b[1] as i8, b[2] as i8, b[3] as i8])
                },
            });
        }
        Ok(out)
    }
}

impl MapLayout {
    /// How many parts store a grass config: Elden Ring's header slot at +128 (SoulsFormats' `GrassConfig`, which its
    /// map pieces and assets may carry); Sekiro's layout has no such slot (its +128 slot is another record), so none.
    /// Only whether a part stores one is read; what the config names is [`MapLayout::grass_configs`].
    pub fn grass_parts(&self) -> Result<usize> {
        Ok(self.grass_configs()?.len())
    }

    /// The grass config of every part that stores one, in part order (Elden Ring's only): the six `GrassTypeParam` row
    /// ids SoulsFormats names `GrassParamId0..5` (0 or negative: none named), as stored.
    pub fn grass_configs(&self) -> Result<Vec<PartGrass>> {
        let mut out = Vec::new();
        if self.generation.name != Generation::ELDEN_RING.name {
            return Ok(out);
        }
        for part in &self.parts {
            let data = self
                .sections
                .get(5)
                .and_then(|s| s.records.get(part.index))
                .ok_or_else(|| invalid("MSB part", part.index, "no record"))?;
            let r = View {
                data,
                format: "MSB part",
                be: false,
            };
            let at = r.size(128)?;
            if at == 0 {
                continue;
            }
            let mut ids = [0i32; 6];
            for (i, id) in ids.iter_mut().enumerate() {
                *id = r.u32(at + 4 * i)? as i32;
            }
            out.push(PartGrass { part: part.index, ids });
        }
        Ok(out)
    }
}

/// The grass config a part stores (SoulsFormats' `GrassConfig`, the part header's slot at +128 in Elden Ring's layout):
/// the ids of up to six `GrassTypeParam` rows, whose model, textures and density the game grows on the part. The game
/// scatters the grass itself; the files hold no blade's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartGrass {
    /// The part's index in the layout's parts ([`Part::index`]).
    pub part: usize,
    pub ids: [i32; 6],
}

/// Checkpoint/warp spawn region stored in the common world frame. Its facing
/// convention and human-readable labels are lowering policy.
#[derive(Clone, Debug)]
pub struct SpawnPoint {
    pub name: String,
    /// Its index among the layout's regions, in file order ([`MapLayout::region_names`]).
    pub region: usize,
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
}
impl MapLayout {
    /// The regions' section (`POINT_PARAM_ST`).
    fn regions(&self) -> Result<&Section> {
        self.sections
            .iter()
            .find(|s| s.name == "POINT_PARAM_ST")
            .ok_or_else(|| invalid("MSB", 0, "missing region section"))
    }

    /// Every region's name as stored, in file order, of every region type. Names repeat: a reader that keys regions
    /// by name renames the repeats over this whole list (SoulsFormats: `"<name> {n}"`).
    pub fn region_names(&self) -> Result<Vec<String>> {
        self.regions()?
            .records
            .iter()
            .map(|data| {
                let v = View {
                    data,
                    format: "MSB region",
                    be: false,
                };
                text(&v, pointer(&v, 0, 96)?)
            })
            .collect()
    }

    /// The checkpoint/warp spawn regions, in generations whose region code is known; an error otherwise.
    pub fn spawn_points(&self) -> Result<Vec<SpawnPoint>> {
        let code = self.generation.spawn_region.ok_or_else(|| {
            invalid(
                "MSB",
                0,
                format!("{}: spawn region code not known", self.generation.name),
            )
        })?;
        let mut points = Vec::new();
        for (region, data) in self.regions()?.records.iter().enumerate() {
            let v = View {
                data,
                format: "MSB region",
                be: false,
            };
            if v.u32(8)? != code {
                continue;
            }
            v.slice(0, 96)?;
            points.push(SpawnPoint {
                name: text(&v, pointer(&v, 0, 96)?)?,
                region,
                position: vector(&v, 20)?,
                rotation_degrees: vector(&v, 32)?,
            });
        }
        Ok(points)
    }
}

/// The shape a region marks out, in the region's own space (SoulsFormats' `MSB.Shape`: the shape code at the record's
/// +16 and the shape's floats at the offset in +72, in both generations). The dimensions are metres as stored.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum RegionShape {
    /// Code 0xFFFFFFFF: the region has no shape.
    None,
    Point,
    Circle {
        radius: f32,
    },
    Sphere {
        radius: f32,
    },
    Cylinder {
        radius: f32,
        height: f32,
    },
    Rectangle {
        width: f32,
        depth: f32,
    },
    Box {
        width: f32,
        depth: f32,
        height: f32,
    },
    /// Eight references to other regions, by their index in the section and the second word of each as stored.
    Composite {
        children: Vec<(i32, i32)>,
    },
    /// A shape code SoulsFormats does not name; its data is not read.
    Unknown {
        code: u32,
    },
}

/// One region of the layout as stored (`POINT_PARAM_ST`, `fromsoft.region` once read into a world): what every type
/// shares, and the type's own data as bytes. SoulsFormats' `MSBS`/`MSBE` `Region` read the header alike: name offset at
/// +0, type at +8, the index at +12, the shape at +16, position at +20, rotation (degrees) at +32, the shape data
/// offset at +72, the entity block offset at +80 (an activation part index, then the entity ID) and the type data
/// offset at +88. What a type's data holds (a sound's id, an effect, a message) is per type and is not read: the
/// bytes are kept, from the type data offset to the record's end (a record that stores more after it, which Elden
/// Ring's does, keeps those bytes here too).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Region {
    /// Its index in the layout's regions, in file order.
    pub index: usize,
    pub name: String,
    /// The type code as stored, and its name where SoulsFormats names it ([`Generation::region_type`]).
    pub type_code: u32,
    pub type_name: Option<&'static str>,
    #[serde(flatten)]
    pub shape: RegionShape,
    /// In the map's frame, as stored: the transform of `euler_transform` places it in the world frame.
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    /// The region's own entity ID as stored (event scripts address it; 0 or -1: none).
    pub entity_id: i32,
    #[serde(serialize_with = "hex")]
    pub type_data: Vec<u8>,
}

/// One event of the layout as stored (`EVENT_PARAM_ST`, `fromsoft.event` once read into a world): what every type
/// shares, and the type's own data as bytes. SoulsFormats' `MSBS`/`MSBE` `Event` read the header alike: the event ID at
/// +8, the type at +12, the index at +16, the base data offset at +24 (a part index, a region index, the entity ID) and
/// the type data offset at +32. The part and region an event names are resolved to their names where the index is in
/// range (-1: none).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Event {
    /// Its index in the layout's events, in file order.
    pub index: usize,
    pub name: String,
    /// The event ID as stored (`EventID`).
    pub event_id: i32,
    pub type_code: u32,
    pub type_name: Option<&'static str>,
    pub part: Option<String>,
    pub region: Option<String>,
    pub entity_id: i32,
    #[serde(serialize_with = "hex")]
    pub type_data: Vec<u8>,
}

impl primase_core::Component for Region {
    const NAME: &'static str = "fromsoft.region";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// The stored place is the map's frame as stored: the entity's transform places the region in the world.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
impl primase_core::Component for Event {
    const NAME: &'static str = "fromsoft.event";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

impl PartKind {
    /// The word for the kind (a lower-case word, as a marker's kind is).
    pub fn word(self) -> &'static str {
        match self {
            PartKind::MapPiece => "map_piece",
            PartKind::Object => "object",
            PartKind::Enemy => "enemy",
            PartKind::Player => "player_start",
            PartKind::Collision => "collision",
            PartKind::DummyObject => "dummy_object",
            PartKind::DummyEnemy => "dummy_enemy",
            PartKind::ConnectCollision => "connect_collision",
            PartKind::Asset => "asset",
            PartKind::DummyAsset => "dummy_asset",
        }
    }
}

/// A part of a kind the area read does not place as a model (`fromsoft.unplaced_part`): an enemy, a player start, a
/// dummy object, asset or enemy, a connect collision. What the layout stores of it, as stored: its place in the map's
/// frame, its model's name, its entity ID and groups, and an enemy's NPC param row. What an enemy does (its think
/// param, patrol, talk) lives in other parts of the record and in the event scripts and is not read.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct UnplacedPart {
    /// Its index in the layout's parts ([`Part::index`]).
    pub index: usize,
    pub name: String,
    pub kind: &'static str,
    pub model: Option<String>,
    pub npc_param: Option<i32>,
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
    pub scale: [f32; 3],
    pub entity_id: i32,
    pub entity_group_ids: [i32; 8],
}
impl primase_core::Component for UnplacedPart {
    const NAME: &'static str = "fromsoft.unplaced_part";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
impl MapLayout {
    /// The parts of the kinds the area read does not place as models, in part order.
    pub fn unplaced_parts(&self) -> Vec<UnplacedPart> {
        self.parts
            .iter()
            .filter(|p| {
                matches!(
                    p.kind,
                    PartKind::Enemy
                        | PartKind::Player
                        | PartKind::DummyObject
                        | PartKind::DummyAsset
                        | PartKind::DummyEnemy
                        | PartKind::ConnectCollision
                )
            })
            .map(|p| UnplacedPart {
                index: p.index,
                name: p.name.clone(),
                kind: p.kind.word(),
                model: p.model.and_then(|i| self.models.get(i)).map(|m| m.name.clone()),
                npc_param: p.npc_param,
                position: p.position,
                rotation_degrees: p.rotation_degrees,
                scale: p.scale,
                entity_id: p.entity_id,
                entity_group_ids: p.entity_group_ids,
            })
            .collect()
    }
}

fn hex<S: serde::Serializer>(bytes: &[u8], s: S) -> std::result::Result<S::Ok, S::Error> {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(text, "{b:02x}");
    }
    s.serialize_str(&text)
}

impl Generation {
    /// The word for a region type code, where SoulsFormats' `RegionType` names it (a lower-case word of `a-z`, `0-9` and
    /// `_`, as a marker's kind is). Sekiro's and Elden Ring's codes differ: 21 is Sekiro's muffling portal and Elden
    /// Ring's connection.
    pub fn region_type(&self, code: u32) -> Option<&'static str> {
        if self.name == Self::SEKIRO.name {
            Some(match code {
                0 => "logic",
                1 => "invasion_point",
                2 => "environment_map_point",
                4 => "sound",
                5 => "sfx",
                6 => "wind_sfx",
                8 => "spawn_point",
                11 => "patrol_route",
                13 => "warp_point",
                14 => "activation_area",
                15 => "event",
                17 => "environment_map_effect_box",
                18 => "wind_area",
                20 => "muffling_box",
                21 => "muffling_portal",
                23 => "sound_space_override",
                24 => "muffling_plane",
                25 => "parts_group_area",
                26 => "auto_draw_group_point",
                _ => return None,
            })
        } else {
            Some(match code {
                1 => "invasion_point",
                2 => "environment_map_point",
                4 => "sound",
                5 => "sfx",
                6 => "wind_sfx",
                8 => "spawn_point",
                9 => "message",
                17 => "environment_map_effect_box",
                18 => "wind_area",
                21 => "connection",
                22 => "patrol_route_22",
                26 => "buddy_summon_point",
                27 => "disable_tumbleweed",
                28 => "muffling_box",
                29 => "muffling_portal",
                30 => "sound_region",
                31 => "muffling_plane",
                32 => "patrol_route",
                33 => "map_point",
                35 => "weather_override",
                36 => "auto_draw_group_point",
                37 => "group_defeat_reward",
                38 => "map_point_discovery_override",
                39 => "map_point_participation_override",
                40 => "hitset",
                41 => "fast_travel_restriction",
                42 => "weather_create_asset_point",
                43 => "play_area",
                44 => "environment_map_output",
                46 => "mount_jump",
                48 => "dummy",
                49 => "fall_prevention_removal",
                50 => "navmesh_cutting",
                51 => "map_name_override",
                52 => "mount_jump_fall",
                53 => "horse_ride_override",
                54 => "locked_mount_jump",
                55 => "locked_mount_jump_fall",
                _ => return None,
            })
        }
    }

    /// The word for an event type code, where SoulsFormats' `EventType` names it (as [`Generation::region_type`]).
    pub fn event_type(&self, code: u32) -> Option<&'static str> {
        if self.name == Self::SEKIRO.name {
            Some(match code {
                4 => "treasure",
                5 => "generator",
                7 => "obj_act",
                9 => "map_offset",
                14 => "patrol_info",
                15 => "platoon_info",
                17 => "resource_item_info",
                18 => "grass_lod_param",
                20 => "skit_info",
                21 => "placement_group",
                22 => "parts_group",
                23 => "talk",
                24 => "auto_draw_group_collision",
                _ => return None,
            })
        } else {
            Some(match code {
                4 => "treasure",
                5 => "generator",
                7 => "obj_act",
                10 => "navmesh",
                12 => "pseudo_multiplayer",
                15 => "platoon_info",
                20 => "patrol_info",
                21 => "mount",
                23 => "sign_pool",
                24 => "retry_point",
                25 => "area_team",
                _ => return None,
            })
        }
    }
}

impl MapLayout {
    /// Every region of the layout, of every type, in file order, with its shape and the bytes of its type data. A
    /// record that does not fit the layout above is an error naming it.
    pub fn map_regions(&self) -> Result<Vec<Region>> {
        let mut out = Vec::new();
        for (index, data) in self.regions()?.records.iter().enumerate() {
            let r = View {
                data,
                format: "MSB region",
                be: false,
            };
            r.slice(0, 96)?;
            let name = text(&r, pointer(&r, 0, 96)?)?;
            let type_code = r.u32(8)?;
            let shape_code = r.u32(16)?;
            let shape_data = r.size(72)?;
            let floats = |n: usize| -> Result<Vec<f32>> {
                if shape_data < 96 {
                    return Err(invalid(r.format, 72, "a shape without data"));
                }
                (0..n).map(|i| f32_at(&r, shape_data + 4 * i)).collect()
            };
            let shape = match shape_code {
                0xFFFF_FFFF => RegionShape::None,
                0 => RegionShape::Point,
                1 => RegionShape::Circle { radius: floats(1)?[0] },
                2 => RegionShape::Sphere { radius: floats(1)?[0] },
                3 => {
                    let f = floats(2)?;
                    RegionShape::Cylinder {
                        radius: f[0],
                        height: f[1],
                    }
                }
                4 => {
                    let f = floats(2)?;
                    RegionShape::Rectangle {
                        width: f[0],
                        depth: f[1],
                    }
                }
                5 => {
                    let f = floats(3)?;
                    RegionShape::Box {
                        width: f[0],
                        depth: f[1],
                        height: f[2],
                    }
                }
                6 => {
                    if shape_data < 96 {
                        return Err(invalid(r.format, 72, "a shape without data"));
                    }
                    RegionShape::Composite {
                        children: (0..8)
                            .map(|i| Ok((r.u32(shape_data + 8 * i)? as i32, r.u32(shape_data + 8 * i + 4)? as i32)))
                            .collect::<Result<_>>()?,
                    }
                }
                code => RegionShape::Unknown { code },
            };
            let entity = r.size(80)?;
            if entity < 96 {
                return Err(invalid(r.format, 80, "no entity block"));
            }
            let type_data = match r.size(88)? {
                0 => Vec::new(),
                at if at >= 96 => r.slice(at, r.data.len() - at.min(r.data.len()))?.to_vec(),
                _ => return Err(invalid(r.format, 88, "type data inside the header")),
            };
            out.push(Region {
                index,
                name,
                type_code,
                type_name: self.generation.region_type(type_code),
                shape,
                position: vector(&r, 20)?,
                rotation_degrees: vector(&r, 32)?,
                entity_id: r.u32(entity + 4)? as i32,
                type_data,
            });
        }
        Ok(out)
    }

    /// Every event of the layout, of every type, in file order, with the bytes of its type data (a MapOffset event's
    /// among them: [`MapLayout::map_offsets`] reads those).
    pub fn map_events(&self) -> Result<Vec<Event>> {
        let regions = self.region_names()?;
        let mut out = Vec::new();
        for (index, data) in self.sections[1].records.iter().enumerate() {
            let r = View {
                data,
                format: "MSB event",
                be: false,
            };
            r.slice(0, 40)?;
            let name = text(&r, pointer(&r, 0, 40)?)?;
            let type_code = r.u32(12)?;
            let base = pointer(&r, 24, 40)?;
            let type_data = match r.size(32)? {
                0 => Vec::new(),
                at if at >= 40 => r.slice(at, r.data.len() - at.min(r.data.len()))?.to_vec(),
                _ => return Err(invalid(r.format, 32, "type data inside the header")),
            };
            let named = |i: i32, names: &dyn Fn(usize) -> Option<String>| usize::try_from(i).ok().and_then(names);
            out.push(Event {
                index,
                name,
                event_id: r.u32(8)? as i32,
                type_code,
                type_name: self.generation.event_type(type_code),
                part: named(r.u32(base)? as i32, &|i| self.parts.get(i).map(|p| p.name.clone())),
                region: named(r.u32(base + 4)? as i32, &|i| regions.get(i).cloned()),
                entity_id: r.u32(base + 8)? as i32,
                type_data,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod region_tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    /// A region record: a 96-byte header, then the name, the entity block, the shape data and the type data.
    fn region(name: &str, type_code: u32, shape: u32, floats: &[f32], entity: u32, type_data: &[u8]) -> Vec<u8> {
        let mut tail = utf16(name);
        let entity_at = 96 + tail.len();
        tail.extend([0u8; 4]);
        tail.extend(entity.to_le_bytes());
        let shape_at = 96 + tail.len();
        tail.extend(floats.iter().flat_map(|f| f.to_le_bytes()));
        let type_at = 96 + tail.len();
        tail.extend(type_data);
        let mut r = vec![0u8; 96];
        r[0..8].copy_from_slice(&96u64.to_le_bytes());
        r[8..12].copy_from_slice(&type_code.to_le_bytes());
        r[16..20].copy_from_slice(&shape.to_le_bytes());
        for (i, v) in [1.0f32, 2.0, 3.0, 10.0, 20.0, 30.0].iter().enumerate() {
            r[20 + 4 * i..24 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        let slot = |r: &mut [u8], at: usize, v: usize| r[at..at + 8].copy_from_slice(&(v as u64).to_le_bytes());
        slot(&mut r, 72, if floats.is_empty() { 0 } else { shape_at });
        slot(&mut r, 80, entity_at);
        slot(&mut r, 88, if type_data.is_empty() { 0 } else { type_at });
        r.extend(tail);
        r
    }

    fn layout(generation: &'static Generation, records: Vec<Vec<u8>>) -> MapLayout {
        let mut sections = generation
            .sections
            .iter()
            .map(|name| Section {
                name: (*name).to_owned(),
                version: 0,
                records: Vec::new(),
            })
            .collect::<Vec<_>>();
        sections[2].records = records;
        MapLayout {
            generation,
            models: Vec::new(),
            parts: Vec::new(),
            map_offsets: Vec::new(),
            sections,
        }
    }

    /// A region of any type is read with its shape's dimensions, its stored place and entity, and its type's bytes; the
    /// type is named by the generation that reads it.
    #[test]
    fn a_regions_shape_place_and_type_data_are_read_as_stored() {
        let layout = layout(
            &Generation::ELDEN_RING,
            vec![
                region("Wind", 6, 5, &[4.0, 5.0, 6.0], 1042, &[7, 8, 9]),
                region("Spot", 21, 0, &[], 0, &[]),
                region("Odd", 77, 3, &[2.0, 9.0], 5, &[]),
            ],
        );
        let regions = layout.map_regions().unwrap();
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].name, "Wind");
        assert_eq!(regions[0].type_name, Some("wind_sfx"));
        assert_eq!(
            regions[0].shape,
            RegionShape::Box {
                width: 4.0,
                depth: 5.0,
                height: 6.0
            }
        );
        assert_eq!(regions[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(regions[0].rotation_degrees, [10.0, 20.0, 30.0]);
        assert_eq!((regions[0].entity_id, regions[0].type_data.clone()), (1042, vec![7, 8, 9]));
        assert_eq!((regions[1].shape.clone(), regions[1].type_name), (RegionShape::Point, Some("connection")));
        assert!(regions[1].type_data.is_empty());
        // a type the generation does not name keeps its code
        assert_eq!((regions[2].type_code, regions[2].type_name), (77, None));
        assert_eq!(
            regions[2].shape,
            RegionShape::Cylinder {
                radius: 2.0,
                height: 9.0
            }
        );
    }

    #[test]
    fn enemy_initializer_is_separate_from_the_npc_row_in_both_generations() {
        for generation in [&Generation::SEKIRO, &Generation::ELDEN_RING] {
            let mut layout = layout(generation, Vec::new());
            let mut record = vec![0u8; 160];
            record[104..112].copy_from_slice(&128u64.to_le_bytes());
            record[140..144].copy_from_slice(&10002020i32.to_le_bytes());
            record[152..156].copy_from_slice(&9000i32.to_le_bytes());
            layout.sections[5].records.push(record);
            layout.parts.push(Part { index: 0, name: "human".into(), kind: PartKind::Enemy, model: None,
                hit_filter: None, npc_param: Some(10002020), position: [0.; 3], rotation_degrees: [0.; 3],
                scale: [1.; 3], entity_id: -1, entity_group_ids: [-1; 8], shadows: Default::default() });
            assert_eq!(layout.character_init(0).unwrap(), Some(9000));
            layout.sections[5].records[0][152..156].copy_from_slice(&(-1i32).to_le_bytes());
            assert_eq!(layout.character_init(0).unwrap(), None);
            layout.parts[0].kind = PartKind::MapPiece;
            assert_eq!(layout.character_init(0).unwrap(), None);
        }
    }

    /// The same code names another type in the other generation.
    #[test]
    fn a_type_code_is_named_by_the_generation_that_reads_it() {
        assert_eq!(Generation::SEKIRO.region_type(21), Some("muffling_portal"));
        assert_eq!(Generation::ELDEN_RING.region_type(21), Some("connection"));
        assert_eq!(Generation::SEKIRO.event_type(9), Some("map_offset"));
        assert_eq!(Generation::ELDEN_RING.event_type(9), None);
        // every word is a valid marker kind
        for generation in Generation::ALL {
            for code in 0..64 {
                for word in [generation.region_type(code), generation.event_type(code)].into_iter().flatten() {
                    assert!(
                        word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                        "{word}"
                    );
                }
            }
        }
    }

    /// A shape that needs data and has none is an error naming the field; a record shorter than its header is one too.
    #[test]
    fn a_region_that_does_not_fit_is_an_error() {
        let mut bad = region("Wind", 6, 5, &[4.0, 5.0, 6.0], 1, &[]);
        bad[72..80].copy_from_slice(&0u64.to_le_bytes());
        assert!(layout(&Generation::ELDEN_RING, vec![bad]).map_regions().is_err());
        assert!(layout(&Generation::ELDEN_RING, vec![vec![0; 40]]).map_regions().is_err());
    }

    /// Type data is serialised as hexadecimal text, not a list of numbers.
    #[test]
    fn type_data_serialises_as_hex() {
        let layout = layout(&Generation::SEKIRO, vec![region("Snd", 4, 2, &[3.0], 9, &[0xab, 0x01])]);
        let json = serde_json::to_value(&layout.map_regions().unwrap()[0]).unwrap();
        assert_eq!(json["type_data"], "ab01");
        assert_eq!(json["shape"], "sphere");
        assert_eq!(json["radius"], 3.0);
        assert_eq!(json["type_name"], "sound");
    }
}
