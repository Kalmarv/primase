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
/// Checkpoint/warp spawn region stored in the common world frame. Its facing
/// convention and human-readable labels are lowering policy.
#[derive(Clone, Debug)]
pub struct SpawnPoint {
    pub name: String,
    pub position: [f32; 3],
    pub rotation_degrees: [f32; 3],
}
impl MapLayout {
    /// The checkpoint/warp spawn regions, in generations whose region code is known; an error otherwise.
    pub fn spawn_points(&self) -> Result<Vec<SpawnPoint>> {
        let code = self.generation.spawn_region.ok_or_else(|| {
            invalid(
                "MSB",
                0,
                format!("{}: spawn region code not known", self.generation.name),
            )
        })?;
        let section = self
            .sections
            .iter()
            .find(|s| s.name == "POINT_PARAM_ST")
            .ok_or_else(|| invalid("MSB", 0, "missing region section"))?;
        let mut points = Vec::new();
        for data in &section.records {
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
                position: vector(&v, 20)?,
                rotation_degrees: vector(&v, 32)?,
            });
        }
        Ok(points)
    }
}
