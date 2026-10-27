//! `lower-shred-fog{metres-per-unit=...}`: the level script's `change_fog` as core distance fog.
//!
//! The game's fog is a fullscreen pass (`shaders\pixel\fsfx\fog.psp`) whose microcode was decoded whole. With
//! `change_fog(a1 .. a9)`, `dA` the view-space z of the surface and `dB` its world height, and `S(t) = t^2 (3 - 2t)`:
//!
//! ```text
//! tD  = sat((dA - a1) / (a2 - a1))
//! tH1 = sat((dB - a3) / ((a4 - a3) / 2))
//! tH2 = sat((a4 - dB) / ((a4 - a3) / 2))
//! f   = S(tD) * S(tH1) * S(tH2)
//! out = scene + f * a8 * (fogColor.rgb - scene),   fogColor.rgb = (a5, a6, a7)
//! ```
//!
//! Core's fog is a fraction by camera distance only, so this pass keeps `Linear { a1, a2 }` with the stored colour and
//! names what it drops. `a9` is not read by the shader; the script's blend is not modelled.
use crate::components::LevelParams;
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Atmosphere, Fog, FogFalloff, Resource, World};
use rme_formats::level_params::{ScriptCall, ScriptValue};

pub const LOWER_SHRED_FOG: PassSpec = PassSpec {
    name: "lower-shred-fog",
    options: &[OptionSpec::required(
        "metres-per-unit",
        "1",
        "metres in one source unit: the unit of the script's fog distances, as the reader's source frame states it",
    )],
    make: |g| {
        let text = g.required("metres-per-unit");
        let metres_per_unit: f64 = text
            .parse()
            .map_err(|_| format!("metres-per-unit {text:?} is not a number"))?;
        if !(metres_per_unit.is_finite() && metres_per_unit > 0.0) {
            return Err(format!("metres-per-unit {text} is not a finite positive number"));
        }
        Ok(Box::new(LowerShredFog { metres_per_unit }))
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowerShredFog {
    pub metres_per_unit: f64,
}

/// The nine numbers of a `change_fog` call.
fn fog_arguments(call: &ScriptCall) -> Result<[f64; 9], String> {
    let numbers: Vec<f64> = call
        .args
        .iter()
        .map(|a| match a {
            ScriptValue::Number(n) => Ok(*n),
            other => Err(format!("change_fog argument {other:?} is not a number")),
        })
        .collect::<Result<_, _>>()?;
    <[f64; 9]>::try_from(numbers.as_slice())
        .map_err(|_| format!("change_fog takes 9 arguments, the script gives {}", numbers.len()))
}

impl Pass for LowerShredFog {
    fn name(&self) -> &'static str {
        "lower-shred-fog"
    }
    fn options(&self) -> Options {
        Options::from([("metres-per-unit".into(), self.metres_per_unit.to_string())])
    }
    fn requires(&self) -> &[&'static str] {
        &[LevelParams::NAME]
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[Atmosphere::NAME]
    }
    fn approximates(&self) -> &'static str {
        "change_fog as camera-distance linear fog from a1 to a2 with the stored colour a5..a7; dropped: the opacity \
         a8 (the game's fog fraction is f*a8, core's reaches 1), the smoothstep S(t)=t^2(3-2t) of the distance ramp \
         tD=sat((dA-a1)/(a2-a1)), the height window S(tH1)*S(tH2) with tH1=sat((dB-a3)/((a4-a3)/2)), \
         tH2=sat((a4-dB)/((a4-a3)/2)) on the surface's world height dB, and the game's view-space z dA against core's \
         radial distance; the colour is taken as stored, not converted between encodings; the script's blend a9 and \
         enable_fog ordering are not modelled"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let Some(params) = world.resource::<LevelParams>() else {
            return Err("shred.level_params is required".into());
        };
        let fogs: Vec<&ScriptCall> = params.calls_to("change_fog").collect();
        let enabled = params.calls_to("enable_fog").count();
        let call = match fogs.as_slice() {
            [] => {
                log.omit("fog: the level script never calls change_fog", 1);
                return Ok(());
            }
            [one] => *one,
            many => {
                return Err(format!(
                    "{} calls to change_fog in {}: which one applies is not decided",
                    many.len(),
                    params.script
                ));
            }
        };
        let [a1, a2, a3, a4, a5, a6, a7, a8, _a9] = fog_arguments(call)?;
        if enabled == 0 {
            log.omit("fog: the level script never calls enable_fog", 1);
            return Ok(());
        }
        if !(0.0..=1.0).contains(&a8) {
            return Err(format!("fog opacity a8 {a8} is not in 0..=1"));
        }
        if a8 == 0.0 {
            log.omit("fog: opacity a8 is 0, the game's fog adds nothing", 1);
            return Ok(());
        }
        if !(a1 >= 0.0 && a1 < a2 && a2.is_finite()) {
            return Err(format!(
                "fog distances a1 {a1} and a2 {a2}: a linear ramp needs 0 <= a1 < a2"
            ));
        }
        if world.resource::<Atmosphere>().is_some() {
            log.count("existing atmospheres kept", 1);
            return Ok(());
        }
        let m = self.metres_per_unit;
        let fog = Fog {
            colour: [a5 as f32, a6 as f32, a7 as f32],
            falloff: FogFalloff::Linear {
                start: (a1 * m) as f32,
                end: (a2 * m) as f32,
            },
        };
        world.insert_resource(Atmosphere {
            fog: Some(fog),
            exposure: None,
            volumetric: None,
        });
        log.count("fog set from change_fog", 1);
        if a8 < 1.0 {
            log.approximate(
                &format!("fog opacity a8 {a8} dropped: the game's fraction is f*{a8}, core's reaches 1"),
                1,
            );
        }
        log.approximate(
            &format!("distance ramp {a1}..{a2} is linear, the game's is S(tD)=tD^2(3-2tD)"),
            1,
        );
        log.approximate(
            &format!(
                "height window dropped: the game multiplies by S(tH1)*S(tH2), tH1=sat((dB-({a3}))/{h}), tH2=sat(({a4}-dB)/{h}), h=({a4}-({a3}))/2, dB the surface's world height",
                h = (a4 - a3) / 2.0
            ),
            1,
        );
        log.approximate(
            "distance is radial; the game's is view-space z (dA = P.z of the unprojected depth)",
            1,
        );
        log.approximate(
            "fog colour a5..a7 taken as stored; its encoding is not established",
            1,
        );
        Ok(())
    }
}
