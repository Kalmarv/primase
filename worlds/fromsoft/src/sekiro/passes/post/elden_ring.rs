//! The caller-selected Elden Ring file's Reinhard curve, preserving the separately lowered auto exposure.
use crate::sekiro::source::DrawParam;
use fromsoft_formats::{Game, gparam::Value};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Atmosphere, ReinhardCurve, Resource, ToneMap, World};

/// Lower mode 8's sampled Reinhard parameters on the native constructor's separate-gamma path.
/// This does not compose weather/override banks or infer the remaining display settings.
#[derive(Debug)]
pub struct LowerEldenRingReinhard;

/// An explicit partial display operation, independent of lighting and exposure lowering.
pub const LOWER_ELDEN_RING_REINHARD: PassSpec = PassSpec {
    name: "lower-elden-ring-reinhard",
    options: &[],
    make: |_| Ok(Box::new(LowerEldenRingReinhard)),
};

fn curve(draw: &DrawParam) -> Result<ReinhardCurve, String> {
    let group = draw
        .group("Tone Map[Yebis]")
        .ok_or("Tone Map[Yebis] not selected")?;
    if !matches!(
        group.value("GS Presents - ToneMap-ToneMapMode"),
        Some(Value::Int(8))
    ) {
        return Err("selected tone-map mode is not the established Reinhard mode 8".into());
    }
    let positive = |key: &str| match group.value(key) {
        Some(Value::Float(value)) if (*value as f32).is_finite() && (*value as f32) > 0. => Ok(*value as f32),
        _ => Err(format!("{key} not stored as a finite positive float")),
    };
    Ok(ReinhardCurve {
        contrast: positive("GS Presents - ToneMap-ReinhardContrast")?,
        white_balance: positive("GS Presents - ToneMap-ReinhardWhiteBalance")?,
        // Constructor 1b90000 sets separate-gamma flag+e9; CB writer 1b940ac/db sets table exponent 1.
        gamma: 1.,
    })
}

impl Pass for LowerEldenRingReinhard {
    fn name(&self) -> &'static str {
        "lower-elden-ring-reinhard"
    }
    fn requires(&self) -> &[&'static str] {
        &[DrawParam::NAME]
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[Atmosphere::NAME]
    }
    fn approximates(&self) -> &'static str {
        "the caller-selected Elden Ring file's mode-8 curve on the native separate-gamma default; layered overrides and player brightness overwrites are not resolved; matrix, vignette, final power and LUT omitted"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(mut draw) = world.resource::<DrawParam>().cloned() else {
            return Ok(());
        };
        if draw.game != Game::EldenRing {
            return Ok(());
        }
        let hour = world
            .resource::<primase_core::TimeOfDay>()
            .map_or(draw.default_hour.hour, |time| time.hour);
        super::super::draw_params::sample_at(&mut draw, hour, log)?;
        let mut atmosphere = world.resource::<Atmosphere>().cloned().unwrap_or_default();
        // A later unsupported selection must not leave the earlier selection's curve active.
        atmosphere.tone_map = match curve(&draw) {
            Ok(curve) => {
                log.count("partial Reinhard curves from the selected Elden Ring file", 1);
                for stage in ["colour matrix", "vignette", "final input power and colour lookup"] {
                    log.omit(
                        &format!("partial Elden Ring post: {stage} source binding unresolved"),
                        1,
                    );
                }
                log.approximate(
                    "selected-file tone values only: weather and override-bank composition is not applied",
                    1,
                );
                log.approximate("native separate-gamma constructor default: table exponent 1; player brightness overwrite is not traced", 1);
                Some(ToneMap {
                    curve,
                    colour_matrix: None,
                    vignette: None,
                    lut: None,
                })
            }
            Err(why) => {
                log.omit(&format!("Elden Ring Reinhard curve omitted: {why}"), 1);
                None
            }
        };
        world.insert_resource(atmosphere);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::source::{DefaultHour, DrawField, DrawGroup, FieldType, SetChosen};

    #[test]
    fn an_unsupported_selection_removes_the_old_curve_without_removing_exposure() {
        let fields = [
            ("GS Presents - ToneMap-ToneMapMode", Value::Int(8)),
            ("GS Presents - ToneMap-ReinhardContrast", Value::Float(1.6)),
            ("GS Presents - ToneMap-ReinhardWhiteBalance", Value::Float(1.0)),
        ]
        .into_iter()
        .map(|(key, value)| DrawField {
            key: key.into(),
            name: key.into(),
            stored: FieldType::fixture(&value),
            value,
            time: None,
        })
        .collect();
        let mut draw = DrawParam {
            reinhard: None,
            game: Game::EldenRing,
            file: "selected".into(),
            version: 5,
            time: None,
            groups: vec![DrawGroup {
                key: "Tone Map[Yebis]".into(),
                name: "Tone map".into(),
                id: 0,
                chosen: SetChosen::ZeroId,
                fields,
            }],
            default_hour: DefaultHour {
                hour: 13.,
                source: "explicit fixture".into(),
            },
        };
        let mut world = World::new();
        let exposure = primase_core::Exposure { stops: 2. };
        world.insert_resource(Atmosphere {
            exposure: Some(exposure.clone()),
            ..Default::default()
        });
        world.insert_resource(draw.clone());
        LowerEldenRingReinhard
            .run(&mut world, &mut PassLog::default())
            .unwrap();
        assert!(world.resource::<Atmosphere>().unwrap().tone_map.is_some());
        draw.groups[0].fields[0].value = Value::Int(7);
        world.insert_resource(draw);
        let mut log = PassLog::default();
        LowerEldenRingReinhard.run(&mut world, &mut log).unwrap();
        let atmosphere = world.resource::<Atmosphere>().unwrap();
        assert!(atmosphere.tone_map.is_none());
        assert_eq!(atmosphere.exposure, Some(exposure));
        assert!(log.omitted.keys().any(|key| key.contains("mode 8")));
    }
}
