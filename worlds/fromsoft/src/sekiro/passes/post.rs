//! Partial Reinhard and installed colour-lookup processing. This does not stand for Sekiro's final display transform.
use crate::sekiro::source::DrawParam;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Atmosphere, ReinhardCurve, Resource, ToneMap, World};

/// Apply resolved steady Reinhard and the proven selected lookup, leaving unestablished stages absent.
#[derive(Debug)]
pub struct LowerSekiroReinhard;

/// The partial operation is an explicit pipeline choice, separate from environment lowering.
pub const LOWER_SEKIRO_REINHARD: PassSpec = PassSpec {
    name: "lower-sekiro-reinhard",
    options: &[],
    make: |_| Ok(Box::new(LowerSekiroReinhard)),
};

impl Pass for LowerSekiroReinhard {
    fn name(&self) -> &'static str {
        "lower-sekiro-reinhard"
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
        "Reinhard and proven SourceID0 LUT with native default gamma; matrix and vignette omitted, player brightness overwrite not traced"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(mut draw) = world.resource::<DrawParam>().cloned() else {
            return Ok(());
        };
        if draw.game != fromsoft_formats::Game::Sekiro {
            return Ok(());
        }
        let scalars = match &draw.reinhard {
            Some(Ok(scalars)) => *scalars,
            Some(Err(why)) => {
                log.omit(&format!("Reinhard scalars unresolved: {why}"), 1);
                return Ok(());
            }
            None => {
                log.omit("Reinhard scalars not composed from native banks", 1);
                return Ok(());
            }
        };
        let hour = world
            .resource::<primase_core::TimeOfDay>()
            .map_or(draw.default_hour.hour, |time| time.hour);
        super::draw_params::sample_at(&mut draw, hour, log)?;
        let lookup = match selected_lookup(&draw, world.resource::<crate::sekiro::source::PostLookup>()) {
            Ok(lookup) => {
                log.approximate("LUT input power uses the native constructor default 1/2.2; player brightness overwrite is not traced", 1);
                Some(lookup)
            }
            Err(why) => {
                log.omit(&format!("LUT stage omitted: {why}"), 1);
                None
            }
        };
        let mut atmosphere = world.resource::<Atmosphere>().cloned().unwrap_or_default();
        atmosphere.tone_map = Some(ToneMap {
            curve: ReinhardCurve {
                contrast: scalars.contrast,
                white_balance: scalars.white_balance,
                // Native separate-gamma default: curve gamma is one; the reciprocal power is applied before the LUT.
                gamma: 1.,
            },
            colour_matrix: None,
            vignette: None,
            lut: lookup,
        });
        world.insert_resource(atmosphere);
        log.count("partial linear Reinhard curves from steady native banks", 1);
        for stage in ["colour matrix", "vignette"] {
            log.omit(&format!("partial post: {stage} unresolved"), 1);
        }
        Ok(())
    }
}

/// Use only the proven installed binding at the sampled source's enabled SourceID.
fn selected_lookup(
    draw: &DrawParam,
    lookup: Option<&crate::sekiro::source::PostLookup>,
) -> Result<primase_core::ColourLut, String> {
    use fromsoft_formats::gparam::Value;
    let grading = draw
        .group("ColorGrading[Yebis]")
        .ok_or("colour-grading group unresolved")?;
    match grading.value("Yebis-ColorGrading-Enable") {
        Some(Value::Int(value)) if *value != 0 => {}
        Some(Value::Int(_)) => return Err("colour grading disabled by sampled source".into()),
        _ => return Err("colour-grading enable unresolved".into()),
    }
    let id = match grading.value("Yebis-LutSourceId") {
        Some(Value::Int(value)) => i32::try_from(*value).map_err(|_| "SourceID outside i32")?,
        _ => return Err("SourceID unresolved".into()),
    };
    let binding = lookup.ok_or("installed source lookup not read")?;
    if id != binding.source_id {
        return Err(format!("SourceID {id} installed binding unproven"));
    }
    let data = binding.value.as_ref().map_err(String::clone)?;
    Ok(primase_core::ColourLut {
        side: data.side,
        values: data.values.clone(),
        // 14024db30 initializes gamma=2.2 and flag+c9=true; 140250bb7 -> 140250bd3 computes/stores reciprocal.
        input_power: 1. / 2.2,
    })
}
