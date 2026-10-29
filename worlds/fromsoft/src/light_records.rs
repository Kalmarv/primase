//! The engine's light records as `core` light tables ([`BufferSource::Lights`]): Sekiro's `PointLight` (208 bytes) and
//! `SpotLight` (192 bytes), the structured buffers its light-organize, tile-lighting, per-light and forward programs
//! read, each field by what the programs' code does with it (fxc's disassembly of every program of the install that
//! declares them: 79 renderer and effect programs, 852 forward materials). A byte no program reads (`SpecColor.w`,
//! `TransLightParam.z`, the pads, the spot's `ShadingMode`) has no field. Elden Ring declares the same two structures,
//! but what its programs read of them is not yet read: its buffers stay unknown.
//!
//! Sekiro's per-tile light lists ([`BufferSource::LightLists`]): `g_LightLookupBuffer` (12-byte records, one per tile)
//! and `g_LightIndices` (4-byte entries), which the light-organize program writes and the tile-lighting pass and every
//! forward material walk (fxc's disassembly of `GXLightOrganize16.cpo`, `GXRend_TileBasedLighting.ppo` and the forward
//! materials), and the frame constants that place a pixel in them.

use fromsoft_formats::Game;
use primase_core::shader::{
    BufferSource, FrameParameter, LightField, LightIndices, LightListPart, LightLookup, LightTable,
    LightTableKind, LightValue, ListBits, ListConstant, ListUnknown,
};

/// Records each buffer holds: the frames bind both with 1024 elements.
const CAPACITY: u32 = 1024;

const WRAP: &str = "1 / a size of the light in the engine's own model: the tile-lighting and forward programs soften \
    the diffuse wrap and widen the specular lobe by the distance times it; the recorded records of bank lights hold 1 / \
    the bank's word at offset 164; core.light's radius is not established to be that size";
const TRANSLUCENCY: &str = "the translucency term's strength (x) and exponent over 32 (y), which the tile-lighting pass \
    and the translucent (`Tr`) forward materials add to a light's diffuse; core.light has no translucency";
const LIGHT_TYPE: &str = "0 in every record of the recorded frames; the organize program marks a light for the \
    tile-lighting pass only when it is 0";
const SHADOW: &str = "a shadowed light's place in the shadow atlas, which only the shadowed (`Sdw`) program variants \
    read; the recorded frames hold no shadowed light (the fields are 0)";

fn field(offset: u32, value: LightValue) -> LightField {
    LightField { offset, value }
}

fn unknown(floats: u8, name: &str, why: &str) -> LightValue {
    LightValue::Unknown {
        floats,
        name: name.into(),
        why: why.into(),
    }
}

/// Tiles the lookup holds and entries the index list holds: the frames bind 2,040 and 130,560 (60 by 34 tiles of 32
/// pixels at 1920 by 1080, 64 entries each).
const LOOKUP_CAPACITY: u32 = 2040;
const INDEX_CAPACITY: u32 = 130_560;

fn bits(offset: u32, shift: u8, bits: u8) -> ListBits {
    ListBits { offset, shift, bits }
}

/// What a program's structured buffer `name` of `stride`-byte elements holds in `game` when it is the scene's lights
/// ([`table`]) or one of the per-tile light lists ([`lists`]); `None` for any other buffer, and for every Elden Ring
/// buffer.
pub(crate) fn source(game: Game, name: &str, stride: u32) -> Option<BufferSource> {
    table(game, name, stride)
        .map(BufferSource::Lights)
        .or_else(|| lists(game, name, stride).map(BufferSource::LightLists))
}

/// The light-list part a program's structured buffer `name` of `stride`-byte elements is, in `game`.
///
/// The lookup's record: word 0 the tile's first entry; word 1's low 16 bits its point lights, its high 16 its spot
/// lights, whose entries follow the points' (the forward materials and the tile-lighting pass walk the points from word
/// 0, then the spots from where the points end); word 2 how many of each kind's entries have bit 31 set, which only the
/// tile-lighting pass's debug view reads. An entry: the light's record in the low 16 bits; bits 16 to 30 the
/// depth slices (a forward material skips the entry unless bit `16 + slice` is set, the slice from the pixel's depth
/// through `FC_FrustumNear`, `FC_FrustumSliceScale` and `FC_FrustumSliceBias`, at most 14; the organize program sets
/// the slices the light's depth range spans, a spot's all or none); bit 31 set where the light reaches the depth range of
/// the tile's opaque pixels, the only entries the tile-lighting pass visits (the organize program's tests of each light
/// against the tile's frustum and depth bounds).
pub(crate) fn lists(game: Game, name: &str, stride: u32) -> Option<LightListPart> {
    if game != Game::Sekiro {
        return None;
    }
    match (name, stride) {
        ("g_LightLookupBuffer", 12) => Some(LightListPart::Lookup(LightLookup {
            stride,
            capacity: LOOKUP_CAPACITY,
            first: bits(0, 0, 32),
            counts: vec![
                (LightTableKind::Point, bits(4, 0, 16)),
                (LightTableKind::Spot, bits(4, 16, 16)),
            ],
            constants: Vec::new(),
            unknown: vec![ListUnknown {
                bits: bits(8, 0, 32),
                name: "lookup word 2".into(),
                why: "how many of the tile's point (low 16 bits) and spot (high 16 bits) entries have bit 31 set, which \
                    only the tile-lighting pass's debug view reads (it sums them and shows the sum as a colour); a \
                    packing with that bit on every entry would hold the counts again"
                    .into(),
            }],
        })),
        ("g_LightIndices", 4) => Some(LightListPart::Indices(LightIndices {
            stride,
            capacity: INDEX_CAPACITY,
            index: bits(0, 0, 16),
            slices: bits(0, 16, 15),
            constants: vec![ListConstant {
                bits: bits(0, 31, 1),
                value: 1,
                why: TILE_PASS_FLAG.into(),
            }],
            unknown: Vec::new(),
        })),
        _ => None,
    }
}

const TILE_PASS_FLAG: &str = "the organize program sets it where the light's volume reaches the depth range of the \
    tile's opaque pixels (and its record's LightType is 0), and the tile-lighting pass visits only an entry with it set; \
    set on every entry, every light is visited, its own range still bounding what it lights";

/// The light table a program's structured buffer `name` of `stride`-byte elements is, in `game`; `None` for any other
/// buffer, and for every Elden Ring buffer.
pub(crate) fn table(game: Game, name: &str, stride: u32) -> Option<LightTable> {
    if game != Game::Sekiro {
        return None;
    }
    let light_type = || LightValue::Constant {
        value: 0.0,
        why: LIGHT_TYPE.into(),
    };
    match (name, stride) {
        ("g_LightBuffer", 208) => Some(LightTable {
            kind: LightTableKind::Point,
            stride,
            capacity: CAPACITY,
            fields: vec![
                field(0, LightValue::ViewPosition),
                field(12, LightValue::InverseRangeSquared),
                field(16, LightValue::Colour),
                field(28, unknown(1, "DiffColor.w", WRAP)),
                field(32, LightValue::Colour),
                field(48, unknown(2, "TransLightParam.xy", TRANSLUCENCY)),
                field(60, LightValue::Range),
                field(64, light_type()),
                field(80, unknown(4, "ProjParam", SHADOW)),
                field(96, unknown(4, "ShadowCenterPos", SHADOW)),
                field(112, unknown(24, "ViewportScaleOffset", SHADOW)),
            ],
        }),
        ("g_SpotLightBuffer", 192) => Some(LightTable {
            kind: LightTableKind::Spot,
            stride,
            capacity: CAPACITY,
            fields: vec![
                field(0, LightValue::ViewPosition),
                field(12, LightValue::Range),
                field(16, LightValue::Colour),
                field(28, unknown(1, "DiffColor.w", WRAP)),
                field(32, LightValue::Colour),
                field(48, unknown(2, "TransLightParam.xy", TRANSLUCENCY)),
                field(64, LightValue::ViewToCone),
                field(128, light_type()),
                field(132, LightValue::ViewDepthMin),
                field(136, LightValue::ViewDepthMax),
                field(144, LightValue::ViewBoundingSphere),
                field(160, unknown(1, "ShadowParam.x", SHADOW)),
                field(176, unknown(4, "ViewportScaleOffset", SHADOW)),
            ],
        }),
        _ => None,
    }
}

/// The frame parameter a 4-byte constant `name` is in `game`: `cbLightAccParam`'s `g_lightCount` and `g_spotLightCount`
/// (the bounds of the organize program's loops over the two light buffers), and `cbSceneParam`'s `FC_LightTile_Width`
/// and `FC_LightTile_NumCellX` (a pixel's tile is its position over the width, its record the tile's row times the
/// cells per row plus its column, in the tile-lighting pass and the forward materials); `None` otherwise, and for every
/// Elden Ring constant.
pub(crate) fn frame_constant(game: Game, name: &str) -> Option<FrameParameter> {
    match (game, name) {
        (Game::Sekiro, "g_lightCount") => Some(FrameParameter::LightCount(LightTableKind::Point)),
        (Game::Sekiro, "g_spotLightCount") => Some(FrameParameter::LightCount(LightTableKind::Spot)),
        (Game::Sekiro, "FC_LightTile_Width") => Some(FrameParameter::LightTileSize),
        (Game::Sekiro, "FC_LightTile_NumCellX") => Some(FrameParameter::LightTilesPerRow),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tables agree with themselves and their buffers' element sizes; Elden Ring has none yet.
    #[test]
    fn sekiros_light_tables_agree_with_their_buffers() {
        for (name, stride) in [("g_LightBuffer", 208), ("g_SpotLightBuffer", 192)] {
            let t = table(Game::Sekiro, name, stride).expect(name);
            assert_eq!(t.verify(Some(stride)), Ok(()), "{name}");
            assert!(table(Game::EldenRing, name, stride).is_none());
            assert!(table(Game::Sekiro, name, stride + 16).is_none());
        }
        assert_eq!(
            frame_constant(Game::Sekiro, "g_lightCount"),
            Some(FrameParameter::LightCount(LightTableKind::Point))
        );
        assert_eq!(frame_constant(Game::EldenRing, "g_lightCount"), None);
    }

    /// Both list parts agree with themselves and their buffers' element sizes; Elden Ring has none yet.
    #[test]
    fn sekiros_light_lists_agree_with_their_buffers() {
        for (name, stride) in [("g_LightLookupBuffer", 12), ("g_LightIndices", 4)] {
            let part = lists(Game::Sekiro, name, stride).expect(name);
            assert_eq!(part.verify(Some(stride)), Ok(()), "{name}");
            assert!(lists(Game::EldenRing, name, stride).is_none());
            assert!(matches!(
                source(Game::Sekiro, name, stride),
                Some(BufferSource::LightLists(_))
            ));
        }
    }
}
