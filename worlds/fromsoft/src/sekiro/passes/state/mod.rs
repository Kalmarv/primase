//! Explicit weather and common-event selection over the reader's retained candidates.
mod weather;
mod world_state;

use crate::sekiro::{
    source::{DrawParam, ResolvedState, StateInputs},
    weather::{Snapshot, Wetness},
    world_state::FlagSet,
};
use fromsoft_formats::{Game, area::DrawParamId};
use primase_core::{
    Resource, World,
    pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec},
    state::StateAssignment,
};
use std::collections::BTreeMap;

/// Select one independently named state. The reader and its raw inputs stay unchanged.
#[derive(Clone, Debug)]
pub struct ResolveFromSoftState {
    weather: Option<Snapshot>,
    flags: Option<FlagSet>,
}
impl ResolveFromSoftState {
    pub fn new(weather: Option<Snapshot>, flags: Option<FlagSet>) -> Result<Self, String> {
        if weather.is_some() == flags.is_some() {
            return Err("state selection requires exactly one weather snapshot or flag set".into());
        }
        if let Some(input) = &weather {
            input.validate()?;
        }
        if let Some(input) = &flags
            && (input.name.trim().is_empty() || input.source.trim().is_empty())
        {
            return Err("world-state flags need a nonempty name and source".into());
        }
        Ok(Self { weather, flags })
    }
    fn from_given(g: &Given<'_>) -> Result<Self, String> {
        let text = |key: &str| -> Result<String, String> {
            decode(
                g.optional(key)
                    .ok_or_else(|| format!("{key} is required for this state"))?,
            )
        };
        let wetness = g
            .optional("wetness")
            .map(|value| -> Result<_, String> {
                Ok(Wetness {
                    name: text("wetness-name")?,
                    source: text("wetness-source")?,
                    value: value.parse().map_err(|_| "wetness is not a number")?,
                })
            })
            .transpose()?;
        let weather = g
            .optional("weather")
            .map(|id| -> Result<_, String> {
                Ok(Snapshot {
                    name: text("name")?,
                    source: text("source")?,
                    weather: id
                        .split_once(':')
                        .ok_or("weather needs row-id:on or row-id:off")?
                        .0
                        .parse()
                        .map_err(|_| "weather is not a row ID")?,
                    fire_ash: match id.split_once(':').map(|(_, ash)| ash) {
                        Some("off") => false,
                        Some("on") => true,
                        _ => return Err("weather fire-ash state must be on or off".into()),
                    },
                    wetness: wetness.clone(),
                })
            })
            .transpose()?;
        if weather.is_none() && wetness.is_some() {
            return Err("wetness requires an explicit weather row".into());
        }
        let flags = g
            .optional("flags")
            .map(|value| -> Result<_, String> {
                let mut flags = BTreeMap::new();
                if value != "none" {
                    for item in value.split(';') {
                        let (id, state) = item.split_once(':').ok_or("flags need id:on or id:off entries")?;
                        let id = id
                            .parse::<u32>()
                            .map_err(|_| "flag ID is not an unsigned integer")?;
                        let on = match state {
                            "on" => true,
                            "off" => false,
                            _ => return Err("flags need on or off".into()),
                        };
                        if flags.insert(id, on).is_some() {
                            return Err(format!("flag {id} is repeated"));
                        }
                    }
                }
                Ok(FlagSet {
                    name: text("name")?,
                    source: text("source")?,
                    flags,
                })
            })
            .transpose()?;
        Self::new(weather, flags)
    }
}

/// Flat options preserve arbitrary provenance using percent-escaped UTF-8 text.
pub const RESOLVE_FROMSOFT_STATE: PassSpec = PassSpec {
    name: "resolve-fromsoft-state",
    options: &[
        OptionSpec::required("name", "observation", "input name, percent-escaped UTF-8 text"),
        OptionSpec::required(
            "source",
            "capture",
            "independent provenance, percent-escaped UTF-8 text",
        ),
        OptionSpec::one_of(
            "state",
            "weather",
            "0:off",
            "observed WeatherParam row ID and fire-ash state, row-id:on or row-id:off",
        ),
        OptionSpec::one_of(
            "state",
            "flags",
            "8302:off;8305:off",
            "observed id:on/id:off pairs separated by semicolons; none for an empty set",
        ),
        OptionSpec::optional(
            "wetness",
            "0",
            "independently observed effective wetting weight",
            "wetness remains unknown",
        ),
        OptionSpec::optional(
            "wetness-name",
            "observation",
            "wetness input name, percent-escaped text",
            "required with wetness",
        ),
        OptionSpec::optional(
            "wetness-source",
            "capture",
            "wetness provenance, percent-escaped text",
            "required with wetness",
        ),
    ],
    make: |g| Ok(Box::new(ResolveFromSoftState::from_given(g)?)),
};

impl Pass for ResolveFromSoftState {
    fn name(&self) -> &'static str {
        "resolve-fromsoft-state"
    }
    fn requires(&self) -> &[&'static str] {
        &[StateInputs::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[StateAssignment::NAME, Wetness::NAME, ResolvedState::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            StateAssignment::NAME,
            Wetness::NAME,
            ResolvedState::NAME,
            DrawParam::NAME,
        ]
    }
    fn options(&self) -> Options {
        let mut out = Options::new();
        if let Some(w) = &self.weather {
            out.extend([
                (
                    "weather".into(),
                    format!("{}:{}", w.weather, if w.fire_ash { "on" } else { "off" }),
                ),
                ("name".into(), encode(&w.name)),
                ("source".into(), encode(&w.source)),
            ]);
            if let Some(wet) = &w.wetness {
                out.extend([
                    ("wetness".into(), wet.value.to_string()),
                    ("wetness-name".into(), encode(&wet.name)),
                    ("wetness-source".into(), encode(&wet.source)),
                ]);
            }
        }
        if let Some(f) = &self.flags {
            let flags = if f.flags.is_empty() {
                "none".into()
            } else {
                f.flags
                    .iter()
                    .map(|(id, on)| format!("{id}:{}", if *on { "on" } else { "off" }))
                    .collect::<Vec<_>>()
                    .join(";")
            };
            out.extend([
                ("flags".into(), flags),
                ("name".into(), encode(&f.name)),
                ("source".into(), encode(&f.source)),
            ]);
        }
        out
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let inputs = world
            .resource::<StateInputs>()
            .ok_or("raw state inputs are absent")?;
        let mut resolved = ResolvedState::default();
        let mut draw_param = None;
        if let Some(input) = &self.weather {
            if inputs.game != Game::EldenRing {
                return Err("weather snapshots require Elden Ring".into());
            }
            let report = weather::select(
                inputs
                    .weather
                    .as_ref()
                    .ok_or("weather data was not in the read scope")?,
                input,
            )?;
            resolved.sky_selection = Some(format!(
                "{}: {} ({})",
                report.input.name, report.sky, report.input.source
            ));
            resolved.weather = Some(report);
        }
        if let Some(input) = &self.flags {
            if inputs.game != Game::Sekiro {
                return Err("common event 100 is understood only for Sekiro".into());
            }
            let common = inputs
                .common
                .as_ref()
                .ok_or("common event data was not in the read scope")?;
            let selection = world_state::evaluate(&common.script, input, common.map)?;
            let mut report = crate::sekiro::world_state::Report {
                input: input.clone(),
                common_script_sha256: common.script_sha256.clone(),
                selection,
                sky: None,
                assignments: BTreeMap::new(),
            };
            if !inputs.sky_values.is_empty() {
                let masks = inputs
                    .domes
                    .iter()
                    .filter(|d| {
                        d.groups
                            .iter()
                            .any(|id| report.selection.groups.get(id) == Some(&true))
                    })
                    .map(|d| d.mask)
                    .collect::<Vec<_>>();
                report.sky = Some(world_state::selected_sky(&inputs.sky_values, &masks)?);
            }
            if let Some(sub) = report.selection.gparam_sub_ids.get(&inputs.area[..6])
                && let Some(base) = world.resource::<DrawParam>()
            {
                let (map, id) = DrawParamId::from_stem(&base.file)
                    .ok_or("Set Area Gparam Sub ID: draw-param slot is unresolved")?;
                let id = u8::try_from(*sub)
                    .ok()
                    .and_then(|sub| DrawParamId::new(id.slot(), sub))
                    .ok_or("Set Area Gparam Sub ID: invalid sub id")?;
                let stem = id.stem(map);
                draw_param = Some(
                    inputs
                        .draw_params
                        .get(&stem)
                        .ok_or_else(|| {
                            format!("Set Area Gparam Sub ID: selected file {stem} is missing or unreadable")
                        })?
                        .clone()?,
                );
            }
            resolved.world_state = Some(report);
        }
        let area = inputs.area.clone();
        if let Some(report) = &resolved.weather {
            weather::assign(world, report)?;
            log.omit(&format!("weather GPARAM layer merging (WeatherOutdoor, WeatherBase wetting weight, CommonEvent) is not traced; the collision-selected area's file is read without these layers: {}", report.weather_outdoor), 1);
            if let Some(wetness) = &report.input.wetness {
                log.omit(&format!("named wetness retained as fromsoft.wetness; WeatherBase environment composition and surface wetting are not yet applied: {}", wetness.name), 1);
            }
            log.count("named weather snapshots resolved", 1);
        }
        if let Some(report) = &mut resolved.world_state {
            world_state::assign(world, &area, report)?;
            log.count("named common-event flag snapshots resolved", 1);
        }
        if let Some(draw_param) = draw_param {
            world.insert_resource(draw_param);
        }
        world.insert_resource(resolved);
        Ok(())
    }
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b" -_./:".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn decode(text: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut chars = text.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let digits = [
                chars.next().ok_or("incomplete percent escape")?,
                chars.next().ok_or("incomplete percent escape")?,
            ];
            let digits = std::str::from_utf8(&digits).map_err(|_| "invalid percent escape")?;
            bytes.push(u8::from_str_radix(digits, 16).map_err(|_| "invalid percent escape")?);
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).map_err(|_| "state provenance is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassRegistry, Pipeline};

    #[test]
    fn named_snapshot_options_round_trip_arbitrary_provenance() {
        let pass = ResolveFromSoftState::new(
            Some(Snapshot {
                name: "snapshot {row=0}, café".into(),
                source: "independent 100% observation".into(),
                weather: 0,
                fire_ash: false,
                wetness: Some(Wetness {
                    name: "wetness=0".into(),
                    source: "captured {native}".into(),
                    value: 0.,
                }),
            }),
            None,
        )
        .unwrap();
        let options = pass.options();
        let mut registry = PassRegistry::new();
        registry.add(&RESOLVE_FROMSOFT_STATE);
        let text = Pipeline {
            passes: vec![Box::new(pass)],
        }
        .to_string();
        let parsed = registry.parse(&text).unwrap();
        assert_eq!(parsed.passes[0].options(), options);
        assert_eq!(decode(&encode("{a=b},50% é")).unwrap(), "{a=b},50% é");
        assert!(decode("incomplete%0").is_err());
    }

    #[test]
    fn a_state_choice_requires_provenance_and_an_explicit_data_scope() {
        let mut registry = PassRegistry::new();
        registry.add(&RESOLVE_FROMSOFT_STATE);
        assert!(registry.parse("resolve-fromsoft-state{weather=0}").is_err());
        assert!(
            registry
                .parse("resolve-fromsoft-state{wetness=0,wetness-name=x,wetness-source=y}")
                .is_err()
        );
        let pipeline = registry
            .parse("resolve-fromsoft-state{weather=0:off,name=x,source=y}")
            .unwrap();
        let mut world = World::new();
        world.insert_resource(StateInputs::new(Game::EldenRing, "m10_00_00_00".into()));
        let error = primase_core::pass::PassManager::new(registry)
            .run(&mut world, &pipeline, &mut Default::default())
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("weather data was not in the read scope")
        );
        assert!(world.resource::<ResolvedState>().is_none());
    }
}
