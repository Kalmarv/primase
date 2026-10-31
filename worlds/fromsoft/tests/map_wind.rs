//! Stormveil Castle's wind (`sekiro::wind`) against the force field the running game built from it.
//!
//! The oracle: in a capture of Elden Ring 1.17.3 (offline, the stock game, `m10_00_00_00`), the force field blowing on
//! the player's cloth while it stood at (-93.57, 22.43, 19.74) in the map's frame, read from the game's memory. Its
//! bytes at the places its force reads them held for all 1,499 steps of the capture, and the cloth's forces computed from
//! them equal the game's bit for bit. The world block's offset in that session was (88, -16, -16), read from the block
//! the same moment (the stock game's render origin, (-88, 16, 16) in `elden_light_tables.rs`, is its negation). The
//! layout's twelve wind effect regions, their effect, shape and rotation are an independent script's over the layout's
//! bytes (kept with the private oracle tools).
//!
//! Needs `installs.elden`, `keys.elden` and `tools.oodle6`.
#![allow(clippy::unwrap_used, clippy::float_cmp)]
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::wind::{self, BlockGrid, BlockOffset, Refusal};
mod common;

/// The cloth's position in the map's frame at the capture's first step.
const CLOTH: [f64; 3] = [-93.57, 22.43, 19.74];

/// The field's rotation rows, its translation and its origin, as stored.
const ROWS: [[u32; 3]; 3] = [
    [0x3f4e_0ace, 0, 0xbf17_edfc],
    [0, 0x3f80_0000, 0],
    [0x3f17_edfc, 0, 0x3f4e_0ace],
];
const TRANSLATION: [u32; 3] = [0x4179_d2f0, 0xc1e1_0a3e, 0xc1dd_7efa];
const ORIGIN: [u32; 3] = [0x4179_d2f2, 0x4101_9168, 0xc1dd_7efa];

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// Native instruction-range oracles from 1.17.3's block creation (0x635475..0x635551)
/// and manager shift (0x634970..0x634bb3). The frame values distinguish the arithmetic;
/// they are not a claim about the player's runtime frame in this installed tile.
#[test]
fn installed_grid_tiles_use_the_native_block_placement_and_shift() -> Result<(), Box<dyn std::error::Error>> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let archive = install.open()?;
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let frame = BlockGrid {
        cell: [49, 49],
        offset: [88., -16., -16.],
    };
    for (map, cell, level, expected) in [
        (
            "m60_42_36_00",
            [42, 36],
            0,
            [0xc4d5_0000, 0xc180_0000, 0xc551_0000],
        ),
        (
            "m60_21_18_01",
            [21, 18],
            1,
            [0xc4c5_0000, 0xc180_0000, 0xc549_0000],
        ),
    ] {
        let layout = fromsoft_formats::msb::MapLayout::read(
            &archive.read(&format!("/map/mapstudio/{map}.msb.dcx"), &decoder)?,
        )?;
        assert!(!layout.parts.is_empty(), "{map} must be an installed grid tile");
        assert_eq!(bits(frame.block(cell, level)?.offset), expected, "{map}");
    }
    let frame = BlockGrid {
        cell: [49, 49],
        offset: [0.125, 16.5, -0.25],
    };
    assert_eq!(
        bits(frame.block([10, 9], 2)?.offset),
        [0xc4ef_fc00, 0x4184_0000, 0xc538_0400]
    );
    for (offset, delta, cell, expected) in [
        (
            [0., 0., 0.],
            [128., -16., -128.],
            [49, 49],
            [0x4300_0000, 0xc180_0000, 0xc300_0000],
        ),
        (
            [120., 4., -120.],
            [20., 3., -20.],
            [48, 50],
            [0xc2e8_0000, 0x40e0_0000, 0x42e8_0000],
        ),
        (
            [0.125, 0., -0.25],
            [1024., 0.5, -1024.],
            [45, 53],
            [0x3e00_0000, 0x3f00_0000, 0xbe80_0000],
        ),
    ] {
        let shifted = BlockGrid {
            cell: [49, 49],
            offset,
        }
        .shifted(delta)?;
        assert_eq!(shifted.cell, cell);
        assert_eq!(bits(shifted.offset), expected);
    }
    assert_eq!(frame.block([0, 0], 16), Err(Refusal::InvalidBlockGrid));
    assert_eq!(frame.shifted([f32::NAN, 0., 0.]), Err(Refusal::InvalidBlockGrid));
    Ok(())
}

#[test]
fn stormveils_wind_is_the_field_the_game_built() -> Result<(), Box<dyn std::error::Error>> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let archive = install.open()?;
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };

    // every wind effect region of the map resolves: each is its own area, a box, turned about y alone, effect 808003
    let all = wind::sources(&archive, &decoder, "m10_00_00_00")?;
    let names: Vec<String> = all
        .iter()
        .map(|r| match r {
            Ok(s) => s.region.clone(),
            Err((name, refusal)) => panic!("{name} refused: {refusal}"),
        })
        .collect();
    let expected: Vec<String> = (1000..1012)
        .map(|n| format!("【GD_環境風_突風12-5m】_808003_{n}"))
        .collect();
    assert_eq!(names, expected);
    for s in all.iter().flatten() {
        assert_eq!(s.area, s.region);
        assert_eq!(s.effect, 808_003);
    }

    // the one whose box holds the cloth is the region the field came from
    let here = wind::sources_at(&archive, &decoder, "m10_00_00_00", CLOTH)?;
    let [Ok(source)] = here.as_slice() else {
        panic!("{} sources at the cloth, the game had one", here.len());
    };
    assert_eq!(source.region, "【GD_環境風_突風12-5m】_808003_1001");
    assert_eq!(source.rows.map(bits), ROWS);
    assert_eq!(bits(source.direction()), ROWS[2]);

    let block = BlockOffset {
        offset: [88., -16., -16.],
        provenance: "the capture's world block, read at its first step".to_owned(),
    };
    assert_eq!(bits(source.translation(&block)), TRANSLATION);
    assert_eq!(
        bits(source.area_centre_at(&block))[1],
        ORIGIN[1],
        "the origin's y is the box centre's"
    );
    assert_eq!(
        source.origin(&block),
        Err(Refusal::OriginCreationArithmetic),
        "the origin's x is {:#x} and the centre's {:#x}: refused until the difference is traced",
        ORIGIN[0],
        bits(source.area_centre_at(&block))[0],
    );

    // the field's parameters, as its force reads them
    let w = &source.force;
    assert_eq!(w.unk_ds3_f1_31, 0, "the field's kind");
    assert_eq!(w.local, 0);
    assert_eq!(w.distance_falloff, 0);
    assert_eq!(
        (w.force_random_multiplier_min, w.force_random_multiplier_max),
        (12., 15.)
    );
    assert_eq!((w.wave_length, w.wave_offset, w.unk_ds3_f1_42), (1., 1., 0.1));
    assert_eq!(w.weight, 1.);
    assert_eq!(
        [w.gust, w.gust_length, w.gust_scale, w.gust_offset],
        [3., 1., 1., 1.]
    );
    assert_eq!([w.gust_rise, w.gust_fall, w.gust_rest], [5., 10., 60.]);
    Ok(())
}
