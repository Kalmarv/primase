//! Explicit partial linear Reinhard processing. This does not stand for Sekiro's final display transform.
use crate::sekiro::source::DrawParam;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Atmosphere, ReinhardCurve, Resource, ToneMap, World};

/// Apply only the resolved steady Reinhard scalars, leaving each unestablished stage absent.
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
        "partial linear Reinhard only; native gamma, colour matrix, vignette and LUT input encoding are unresolved"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(draw) = world.resource::<DrawParam>() else {
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
        let mut atmosphere = world.resource::<Atmosphere>().cloned().unwrap_or_default();
        atmosphere.tone_map = Some(ToneMap {
            curve: ReinhardCurve {
                contrast: scalars.contrast,
                white_balance: scalars.white_balance,
                // Linear intermediate: this is not an inferred native gamma setting.
                gamma: 1.,
            },
            colour_matrix: None,
            vignette: None,
            lut: None,
        });
        world.insert_resource(atmosphere);
        log.count("partial linear Reinhard curves from steady native banks", 1);
        for stage in ["native gamma", "colour matrix", "vignette", "LUT input encoding"] {
            log.omit(&format!("partial post: {stage} unresolved"), 1);
        }
        Ok(())
    }
}
