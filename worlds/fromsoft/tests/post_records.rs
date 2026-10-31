//! The install is the oracle: default, MapBase and the Castle file retain distinct tone settings and fog time keys.
mod common;
mod inputs;

use fromsoft_formats::{Game, dcx::Decoder};
use primase_fromsoft::sekiro::post;

#[test]
fn installed_stormveil_reinhard_follows_the_selected_files_clock_keys()
-> Result<(), Box<dyn std::error::Error>> {
    let (Some(archive), Some(library)) = (
        common::archive(Game::EldenRing)?,
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 28,
    };
    let raw = post::read(&archive, &decoder, "m10_00_0000")?;
    let (offset, period) = raw.time.expect("version5 native time axis").sampling();
    assert_eq!(period, 24.);
    let (draw, _) = primase_fromsoft::sekiro::read_area_draw_param(
        &archive,
        &decoder,
        "m10_00_00_00",
        &primase_fromsoft::sekiro::DrawParams::File("m10_00_0000".into()),
    )?;
    let draw = draw.expect("explicit installed file selected");
    // Independent installed-file census: these two authored keys deliberately have different contrast.
    // Their clock hours come from the file header, not an assumed noon or a captured camera exposure.
    for (key, expected) in [(0., 1.6), (11.5, 1.45)] {
        let authored = raw
            .tone
            .reinhard_contrast
            .iter()
            .find(|s| s.id == 0 && s.hour == Some(key))
            .unwrap();
        assert_eq!(authored.value, expected);
        let mut world = primase_core::World::new();
        world.insert_resource(draw.clone());
        world.insert_resource(primase_core::TimeOfDay {
            hour: (offset + key) % period,
        });
        use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new()).run(
            &mut world,
            &Pipeline {
                passes: vec![Box::new(
                    primase_fromsoft::sekiro::passes::post::elden_ring::LowerEldenRingReinhard,
                )],
            },
            &mut report,
        )?;
        let tone = world
            .resource::<primase_core::Atmosphere>()
            .unwrap()
            .tone_map
            .as_ref()
            .unwrap();
        assert_eq!(tone.curve.contrast, expected);
        assert_eq!(tone.curve.white_balance, 1.);
        assert_eq!(tone.curve.gamma, 1.);
        assert!(tone.colour_matrix.is_none() && tone.vignette.is_none() && tone.lut.is_none());
        assert_eq!(report.steps[0].log.omitted.len(), 3);
        assert!(world.verify_resources().is_empty());
        println!(
            "ER_POST selected m10_00_0000 clock={} contrast={} white=1; native-default table exponent1, later stages omitted",
            (offset + key) % period,
            expected
        );
    }
    Ok(())
}

#[test]
fn installed_ashina_colour_lookup_matches_the_captured_cube() -> Result<(), Box<dyn std::error::Error>> {
    let (Some(archive), Some(library), Some(frames)) = (
        common::archive(Game::Sekiro)?,
        inputs::oodle(inputs::Tool::Oodle6),
        inputs::oracle(inputs::OracleSet::Scripts, inputs::Game::Sekiro, "frames"),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 28,
    };
    let actual = post::lut::read(&archive, &decoder, "m11_01", 0)?;
    assert_eq!(actual.texture, "m11_01_CGrading0000");
    assert_eq!(actual.side, 16);
    assert_eq!(actual.values.len(), 4096);
    let frame = frames.join("sek2/cap/frame_13604");
    let events: Vec<serde_json::Value> = std::fs::read_to_string(frame.join("events.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let readback = events
        .iter()
        .find(|event| event["seq"] == 994)
        .expect("native LUT readback");
    let expected = std::fs::read(frame.join(readback["file"].as_str().expect("readback file")))?;
    assert_eq!(expected.len(), 4096 * 16);
    for (index, (actual, expected)) in actual.values.iter().zip(expected.as_chunks::<16>().0).enumerate() {
        for channel in 0..3 {
            let word = u32::from_le_bytes(expected[channel * 4..channel * 4 + 4].try_into()?);
            assert_eq!(
                actual[channel].to_bits(),
                word,
                "LUT texel {index} RGB channel {channel}"
            );
        }
    }
    println!(
        "POST_LUT m11_01 SourceID0: 4096/4096 RGB texels bit-exact to native cube; red fastest, no flips"
    );
    Ok(())
}

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
    // Exercise the public area read and declared partial pass, not a fabricated renderer-ready tone map.
    let (draw, _) = primase_fromsoft::sekiro::read_area_draw_param(
        &archive,
        &decoder,
        "m11_01_00_00",
        &primase_fromsoft::sekiro::DrawParams::File("m11_01_0000".into()),
    )?;
    let draw = draw.expect("selected installed draw-param file");
    assert_eq!(draw.reinhard, Some(Ok(resolved)));
    let mut world = primase_core::World::new();
    world.insert_resource(draw);
    post::lut::read_into(&mut world, &archive, &decoder, "m11_01");
    use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new()).run(
        &mut world,
        &Pipeline {
            passes: vec![Box::new(
                primase_fromsoft::sekiro::passes::post::LowerSekiroReinhard,
            )],
        },
        &mut report,
    )?;
    let tone = world
        .resource::<primase_core::Atmosphere>()
        .unwrap()
        .tone_map
        .as_ref()
        .unwrap();
    assert_eq!(tone.curve.contrast, 1.45);
    assert_eq!(tone.curve.white_balance, 1.7);
    assert_eq!(
        tone.curve.gamma, 1.,
        "linear intermediate, not a native gamma assertion"
    );
    assert!(tone.colour_matrix.is_none() && tone.vignette.is_none());
    for stage in ["colour matrix", "vignette"] {
        assert_eq!(
            report.steps[0]
                .log
                .omitted
                .get(&format!("partial post: {stage} unresolved")),
            Some(&1)
        );
    }
    assert_eq!(tone.lut.as_ref().unwrap().input_power, 1. / 2.2);
    assert_eq!(tone.lut.as_ref().unwrap().values.len(), 4096);
    assert!(
        report.steps[0]
            .log
            .approximations
            .keys()
            .any(|reason| reason.contains("player brightness overwrite"))
    );
    assert!(world.verify_resources().is_empty());
    world.insert_resource(primase_core::TimeOfDay { hour: 2. });
    let mut other = RunReport::default();
    PassManager::new(PassRegistry::new()).run(
        &mut world,
        &Pipeline {
            passes: vec![Box::new(
                primase_fromsoft::sekiro::passes::post::LowerSekiroReinhard,
            )],
        },
        &mut other,
    )?;
    assert!(
        world
            .resource::<primase_core::Atmosphere>()
            .unwrap()
            .tone_map
            .as_ref()
            .unwrap()
            .lut
            .is_none()
    );
    assert!(
        other.steps[0]
            .log
            .omitted
            .keys()
            .any(|reason| reason.contains("SourceID 20"))
    );
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
