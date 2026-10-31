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
    for (file, record) in [
        ("default", &default),
        ("MapBase", &base),
        ("m11_01_0000", &castle),
    ] {
        println!(
            "POST_EXTRAS {file}: tone={:?}, grading={:?}, fog={:?}",
            record.tone_extra, record.grading_extra, record.fog_extra
        );
    }
    // Independent header/extra-table census of the Castle file: value ID 0 is explicitly listed in all three groups.
    for (extra, group_index, word) in [
        (&castle.tone_extra, 4, 1),
        (&castle.grading_extra, 10, 0),
        (&castle.fog_extra, 22, 1),
    ] {
        let extra = extra.as_ref().expect("the group stores an extra record");
        assert_eq!(extra.group_index, group_index);
        assert_eq!(extra.ids, [0]);
        assert_eq!(extra.unknown_0c, Some(word));
    }
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
    // Native loader evidence: the Castle extra's low byte is nonzero, so a missing contrast field leaves its
    // fallback row inactive (140319d40). The other two files explicitly author and list value ID zero.
    // MapBase's controller activation is deliberately an input here, not a conclusion drawn from this oracle.
    use post::composition::{Activation, ScalarBank, ScalarRow, scalar};
    let full_weight = Some(Activation::Active { weight: 1.0 });
    let default_row = ScalarRow::Active(default.tone.reinhard_contrast[0].value);
    let base_row = ScalarRow::Active(base.tone.reinhard_contrast[0].value);
    let banks = [
        ScalarBank {
            index: 5,
            activation: full_weight,
            row: ScalarRow::Inactive,
        },
        ScalarBank {
            index: 4,
            activation: None,
            row: base_row,
        },
        ScalarBank {
            index: 1,
            activation: full_weight,
            row: default_row,
        },
    ];
    assert!(
        scalar(1.6, &banks)
            .unwrap_err()
            .to_string()
            .contains("MapBase activation/weight not traced")
    );
    let mut explicit_banks = banks;
    explicit_banks[1].activation = full_weight;
    // The captured table-writer contrast is an expected result only; no captured value enters the composition.
    assert_eq!(scalar(1.6, &explicit_banks)?, 1.45);
    explicit_banks[1].activation = Some(Activation::Inactive);
    assert_eq!(scalar(1.6, &explicit_banks)?, 1.6);
    // The steady native map-context path supplies MapBase activation; the loader's extra flags supply field activity.
    let resolved = post::reinhard::resolve(&default, &base, &castle)?;
    assert_eq!(resolved.contrast, 1.45);
    assert_eq!(resolved.white_balance, 1.7);
    println!(
        "POST_REINHARD m11_01_0000: contrast={} white_balance={}",
        resolved.contrast, resolved.white_balance
    );
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
