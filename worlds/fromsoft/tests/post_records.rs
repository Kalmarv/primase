//! The install is the oracle: default, MapBase and the Castle file retain distinct tone settings and fog time keys.
mod common;
mod inputs;

use fromsoft_formats::{Game, dcx::Decoder};
use primase_fromsoft::sekiro::post;

#[test]
fn installed_post_records_keep_layers_and_missing_fields_distinct() -> Result<(), Box<dyn std::error::Error>>
{
    let Some(archive) = common::archive(Game::Sekiro)? else {
        return Ok(());
    };
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 28,
    };
    let default = post::read(&archive, &decoder, "default")?;
    let base = post::read(&archive, &decoder, "MapBase")?;
    let castle = post::read(&archive, &decoder, "m11_01_0000")?;
    assert_eq!(
        default.tone.reinhard_contrast,
        vec![post::Sample {
            id: 0,
            hour: Some(0.),
            value: 1.6
        }]
    );
    assert_eq!(
        base.tone.reinhard_contrast,
        vec![post::Sample {
            id: 0,
            hour: Some(0.),
            value: 1.45
        }]
    );
    for record in [&default, &base] {
        assert_eq!(record.tone.reinhard_white_balance[0].value, 1.7);
    }
    assert!(castle.tone.reinhard_contrast.is_empty());
    assert!(castle.tone.reinhard_white_balance.is_empty());
    assert_eq!(
        castle
            .tone
            .lut_source_id
            .iter()
            .map(|s| (s.id, s.hour, s.value))
            .collect::<Vec<_>>(),
        [
            (0, Some(0.), 0),
            (0, Some(2.), 20),
            (0, Some(6.), 0),
            (0, Some(12.), 30),
            (0, Some(18.), 0),
            (0, Some(22.), 20)
        ]
    );
    assert_eq!(
        castle
            .fog
            .far_depth
            .iter()
            .find(|s| s.hour == Some(18.) && s.id == 0)
            .unwrap()
            .value,
        300.
    );
    assert_eq!(
        castle
            .fog
            .fixed_density
            .iter()
            .find(|s| s.hour == Some(18.) && s.id == 0)
            .unwrap()
            .value,
        0.0015
    );
    println!("3 installed GPARAM records: distinct Reinhard layers, 6 LUT keys, Castle fog at 18:00");
    Ok(())
}
