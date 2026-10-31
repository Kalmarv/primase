//! The engine's light records as `core` light tables ([`BufferSource::Lights`]): the `PointLight` (208 bytes) and
//! `SpotLight` (192 bytes) structured buffers the light-organize, tile-lighting, per-light and forward programs read,
//! each field by what the programs' code does with it. Sekiro's from fxc's disassembly of every program of the install
//! that declares them (79 renderer and effect programs, 852 forward materials) and the recorded frames' records of the
//! Ashina bank's lights. Elden Ring declares the same two structures at the same offsets (dxc's reflection of its 99
//! programs that declare them), its light-organize program bounds its loops over them by the same two counts (dxc's
//! disassembly of `GXLightOrganize8`), and its point records, read back from the running game at Stormveil (four
//! dispatches of 16 point lights, five of them BTL lights the reader lowers), hold each field below as Sekiro's do: the
//! view position, `1 / range^2`, the range, the colour and the specular colour exactly, `LightType` 0. No Elden Ring
//! spot record was read back: its spot table is the declaration's alone. A byte no program reads (`SpecColor.w`, which
//! Elden Ring's records hold as the specular colour's luma, `TransLightParam.z`, the pads, the spot's `ShadingMode`)
//! has no field.
//!
//! Both games' per-tile light lists ([`BufferSource::LightLists`]): `g_LightLookupBuffer` (12-byte records, one per tile)
//! and `g_LightIndices` (4-byte entries), which the light-organize program writes and the tile-lighting pass and every
//! forward material walk (fxc's disassembly of `GXLightOrganize16.cpo`, `GXRend_TileBasedLighting.ppo` and the forward
//! materials; Elden Ring's `GXLightOrganize8.cpo` and `GXRend_TileBasedLighting.ppo` inspected with dxc), and the frame
//! constants that place a pixel in them. Elden Ring's tile pass additionally discards when word 2 counts no eligible
//! lights, even outside its debug view. Its conservative lists repeat the counts there because every entry is eligible.

use fromsoft_formats::Game;
use primase_core::shader::{
    BufferSource, FrameParameter, LightField, LightIndices, LightListPart, LightLookup, LightTable,
    LightTableKind, LightValue, ListBits, ListConstant, ListUnknown,
};

/// Records each buffer holds: the frames bind both with 1024 elements.
const CAPACITY: u32 = 1024;

const TRANSLUCENCY: &str = "the translucency term's strength (x) and exponent over 32 (y), which the tile-lighting pass \
    and the translucent (`Tr`) forward materials add to a light's diffuse; core.light has no translucency";
const LIGHT_TYPE: &str = "0 in every record of the recorded frames; the organize program marks a light for the \
    tile-lighting pass only when it is 0";
const SHADOW: &str = "a shadowed light's place in the shadow atlas, which only the shadowed (`Sdw`) program variants \
    read; the recorded frames hold no shadowed light (the fields are 0)";
const ELDEN_LIGHT_TYPE: &str = "0 in every point record read back at Stormveil (16 lights in each of four \
    dispatches); the light-organize program reads it, as Sekiro's does";
const ELDEN_SHADOW: &str = "a shadowed light's place in the shadow atlas, which only the shadowed (`Sdw`) program \
    variants read; the records read back at Stormveil hold no shadowed light, and an unshadowed light's record keeps \
    there what the buffer held before (the game writes no value)";
const ELDEN_SPOT: &str = "Elden Ring's spot record by its programs' declaration alone (no spot light was in the frames \
    read back): ";

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
/// pixels at 1920 by 1080, 64 entries each) in Sekiro. Also the conservative consumer budget for Elden Ring, not a
/// claim about its native allocation; a consumer may use larger tiles to stay inside this budget.
const LOOKUP_CAPACITY: u32 = 2040;
const INDEX_CAPACITY: u32 = 130_560;

fn bits(offset: u32, shift: u8, bits: u8) -> ListBits {
    ListBits { offset, shift, bits }
}

/// What a program's structured buffer `name` of `stride`-byte elements holds in `game` when it is the scene's lights
/// ([`table`]) or one of the per-tile light lists ([`lists`]); `None` for any other buffer.
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
/// Sekiro tile-lighting pass's debug view reads; Elden Ring also rejects a tile when their sum is zero. An entry: the
/// light's record in the low 16 bits; bits 16 to 30 the
/// depth slices (a forward material skips the entry unless bit `16 + slice` is set, the slice from the pixel's depth
/// through `FC_FrustumNear`, `FC_FrustumSliceScale` and `FC_FrustumSliceBias`, at most 14; the organize program sets
/// the slices the light's depth range spans, a spot's all or none); bit 31 set where the light reaches the depth range of
/// the tile's opaque pixels, the only entries the tile-lighting pass visits (the organize program's tests of each light
/// against the tile's frustum and depth bounds).
pub(crate) fn lists(game: Game, name: &str, stride: u32) -> Option<LightListPart> {
    match (name, stride) {
        ("g_LightLookupBuffer", 12) => Some(LightListPart::Lookup(LightLookup {
            stride,
            capacity: LOOKUP_CAPACITY,
            first: bits(0, 0, 32),
            counts: vec![
                (LightTableKind::Point, bits(4, 0, 16)),
                (LightTableKind::Spot, bits(4, 16, 16)),
            ],
            count_copies: if game == Game::EldenRing {
                vec![
                    (LightTableKind::Point, bits(8, 0, 16)),
                    (LightTableKind::Spot, bits(8, 16, 16)),
                ]
            } else {
                Vec::new()
            },
            constants: Vec::new(),
            unknown: if game == Game::Sekiro {
                vec![ListUnknown {
                    bits: bits(8, 0, 32),
                    name: "lookup word 2".into(),
                    why: "point (low 16 bits) and spot (high 16 bits) counts with bit 31 set; Sekiro's tile pass \
                        reads these only in its debug view, showing their sum as a colour".into(),
                }]
            } else {
                Vec::new()
            },
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
/// buffer. Both games' records take the light's own specular ([`LightValue::SpecularColour`]): a BTL light's record
/// holds its specular bytes squared times its specular power, apart from its diffuse ones (every BTL light read back
/// from both games' frames).
/// Input meanings only. A registered source pass supplies the core attributes; the reader selects no defaults.
fn elden_translucency() -> [LightField; 2] {
    [
        field(48, LightValue::TranslucencyStrength),
        field(52, LightValue::TranslucencyExponentScale),
    ]
}

pub(crate) fn table(game: Game, name: &str, stride: u32) -> Option<LightTable> {
    let (light_type_why, shadow, spot_prefix) = match game {
        Game::Sekiro => (LIGHT_TYPE, SHADOW, ""),
        Game::EldenRing => (ELDEN_LIGHT_TYPE, ELDEN_SHADOW, ELDEN_SPOT),
    };
    let light_type = |why: String| LightValue::Constant { value: 0.0, why };
    let spot = |why: &str| format!("{spot_prefix}{why}");
    let mut table = match (name, stride) {
        ("g_LightBuffer", 208) => Some(LightTable {
            kind: LightTableKind::Point,
            stride,
            capacity: CAPACITY,
            fields: vec![
                field(0, LightValue::ViewPosition),
                field(12, LightValue::InverseRangeSquared),
                field(16, LightValue::Colour),
                field(28, LightValue::InverseFalloffDistance),
                field(32, LightValue::SpecularColour),
                field(48, unknown(2, "TransLightParam.xy", TRANSLUCENCY)),
                field(60, LightValue::Range),
                field(64, light_type(light_type_why.into())),
                field(80, unknown(4, "ProjParam", shadow)),
                field(96, unknown(4, "ShadowCenterPos", shadow)),
                field(112, unknown(24, "ViewportScaleOffset", shadow)),
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
                field(28, LightValue::InverseFalloffDistance),
                field(32, LightValue::SpecularColour),
                field(
                    48,
                    LightValue::Unknown {
                        floats: 2,
                        name: "TransLightParam.xy".into(),
                        why: spot(TRANSLUCENCY),
                    },
                ),
                field(64, LightValue::ViewToCone),
                field(128, light_type(spot(light_type_why))),
                field(132, LightValue::ViewDepthMin),
                field(136, LightValue::ViewDepthMax),
                field(144, LightValue::ViewBoundingSphere),
                field(
                    160,
                    LightValue::Unknown {
                        floats: 1,
                        name: "ShadowParam.x".into(),
                        why: spot(shadow),
                    },
                ),
                field(
                    176,
                    LightValue::Unknown {
                        floats: 4,
                        name: "ViewportScaleOffset".into(),
                        why: spot(shadow),
                    },
                ),
            ],
        }),
        _ => None,
    }?;
    if game == Game::EldenRing {
        table.fields.retain(|f| f.offset != 48);
        table.fields.extend(elden_translucency());
        table.fields.sort_by_key(|f| f.offset);
    }
    Some(table)
}

/// The frame parameter a 4-byte constant `name` is in `game`: `cbLightAccParam`'s `g_lightCount` and `g_spotLightCount`
/// (the bounds of the organize program's loops over the two light buffers), and `cbSceneParam`'s `FC_LightTile_Width`
/// and `FC_LightTile_NumCellX` (a pixel's tile is its position over the width, its record the tile's row times the
/// cells per row plus its column, in the tile-lighting pass and the forward materials); `None` otherwise. Elden Ring's
/// counts are the same (its light-organize program's loops over the two buffers end at them: dxc's disassembly of
/// `GXLightOrganize8`), and its tile-lighting and forward programs index the same lists through those tile constants.
pub(crate) fn frame_constant(game: Game, name: &str) -> Option<FrameParameter> {
    match (game, name) {
        (Game::Sekiro | Game::EldenRing, "g_lightCount") => {
            Some(FrameParameter::LightCount(LightTableKind::Point))
        }
        (Game::Sekiro | Game::EldenRing, "g_spotLightCount") => {
            Some(FrameParameter::LightCount(LightTableKind::Spot))
        }
        (Game::Sekiro | Game::EldenRing, "FC_LightTile_Width") => Some(FrameParameter::LightTileSize),
        (Game::Sekiro | Game::EldenRing, "FC_LightTile_NumCellX") => Some(FrameParameter::LightTilesPerRow),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elden_translucency_declares_roles_without_choosing_values() {
        let fields = elden_translucency();
        assert_eq!(fields[0].value, LightValue::TranslucencyStrength);
        assert_eq!(fields[1].value, LightValue::TranslucencyExponentScale);
    }

    /// Both games' tables agree with themselves and their buffers' element sizes, and lie at the same offsets; both take
    /// the light's own specular. Both games bind their counts and tile constants.
    #[test]
    fn the_light_tables_agree_with_their_buffers() {
        for (name, stride) in [("g_LightBuffer", 208), ("g_SpotLightBuffer", 192)] {
            let sekiro = table(Game::Sekiro, name, stride).expect(name);
            let elden = table(Game::EldenRing, name, stride).expect(name);
            for t in [&sekiro, &elden] {
                assert_eq!(t.verify(Some(stride)), Ok(()), "{name}");
                assert!(
                    t.fields
                        .iter()
                        .any(|f| f.offset == 32 && f.value == LightValue::SpecularColour)
                );
            }
            let offsets = |t: &LightTable| {
                t.fields
                    .iter()
                    .filter(|f| f.offset != 52)
                    .map(|f| f.offset)
                    .collect::<Vec<_>>()
            };
            assert_eq!(offsets(&sekiro), offsets(&elden), "{name}");
            assert!(table(Game::Sekiro, name, stride + 16).is_none());
        }
        for game in [Game::Sekiro, Game::EldenRing] {
            assert_eq!(
                frame_constant(game, "g_lightCount"),
                Some(FrameParameter::LightCount(LightTableKind::Point))
            );
        }
        for game in [Game::Sekiro, Game::EldenRing] {
            assert_eq!(
                frame_constant(game, "FC_LightTile_Width"),
                Some(FrameParameter::LightTileSize)
            );
            assert_eq!(
                frame_constant(game, "FC_LightTile_NumCellX"),
                Some(FrameParameter::LightTilesPerRow)
            );
        }
    }

    /// Both games' list parts agree with themselves and their buffers' element sizes.
    #[test]
    fn both_games_light_lists_agree_with_their_buffers() {
        for (name, stride) in [("g_LightLookupBuffer", 12), ("g_LightIndices", 4)] {
            for game in [Game::Sekiro, Game::EldenRing] {
                let part = lists(game, name, stride).expect(name);
                assert_eq!(part.verify(Some(stride)), Ok(()), "{name}");
                assert!(lists(game, name, stride + 4).is_none());
                assert!(matches!(
                    source(game, name, stride),
                    Some(BufferSource::LightLists(_))
                ));
            }
        }
    }
}
