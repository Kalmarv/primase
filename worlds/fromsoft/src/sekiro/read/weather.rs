//! Explicit Elden Ring weather state; lotteries are not defaults.
use fromsoft_formats::{Archive, Game, Install, Result, dcx::Decoder, gparam_params::WeatherParam};
use primase_core::{
    Resource, World,
    state::{Domain, StateAssignment, StateAxis},
};
use serde::{Deserialize, Serialize};

/// An independently observed current weather and fire-ash state. No hour selects these.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub name: String,
    pub source: String,
    pub weather: i32,
    pub fire_ash: bool,
    /// Independently observed WeatherBase weight. Absent means unknown, never dry.
    #[serde(default)]
    pub wetness: Option<Wetness>,
}

/// Named effective wetting state, not a value computed from a weather row or an hour.
/// The native WeatherBase getter returns the accumulated wetting weight (or its explicit override).
/// Reading this state does not yet apply WeatherBase to the environment or surface materials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wetness {
    pub name: String,
    pub source: String,
    pub value: f32,
}

impl Wetness {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() || self.source.trim().is_empty() {
            return Err("wetness requires a nonempty name and independent source".into());
        }
        if !self.value.is_finite() || !(0.0..=1.0).contains(&self.value) {
            return Err("wetness requires a finite observed WeatherBase weight from 0 to 1".into());
        }
        Ok(())
    }
}

impl Resource for Wetness {
    const NAME: &'static str = "fromsoft.wetness";

    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, serde_json::Error> {
        serde_json::to_value(self).map(Some)
    }
}

/// The installed row and sky axis selected by a snapshot. Layer composition remains unresolved.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub input: Snapshot,
    pub sky: String,
    pub weather_outdoor: String,
}

impl Snapshot {
    /// Validate provenance before opening game data.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() || self.source.trim().is_empty() || self.weather < 0 {
            return Err("weather requires a nonempty name and independent source and a nonnegative WeatherParam row id".into());
        }
        if let Some(wetness) = &self.wetness {
            wetness.validate()?;
        }
        Ok(())
    }
}

pub(super) fn read(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    input: &Snapshot,
) -> Result<Report> {
    input.validate().map_err(|e| crate::invalid("weather", 0, e))?;
    if install.game != Game::EldenRing {
        return Err(crate::invalid(
            "weather",
            0,
            "weather snapshots require Elden Ring",
        ));
    }
    let param = fromsoft_formats::game_params::read_param(install, archive, decoder, "WeatherParam.param")?;
    let param = WeatherParam::from_param(&param)?;
    let row = param
        .row(input.weather)
        .ok_or_else(|| crate::invalid("weather", 0, format!("WeatherParam has no row {}", input.weather)))?;
    Ok(Report {
        input: input.clone(),
        sky: format!(
            "weather-{}{}",
            input.weather,
            if input.fire_ash { "-ashen" } else { "" }
        ),
        weather_outdoor: row.gparam_path(),
    })
}

pub(super) fn assign(world: &mut World, report: &Report) -> std::result::Result<(), String> {
    report.input.validate()?;
    let axis = world
        .iter::<StateAxis>()
        .find_map(|(_, a)| (a.key == super::sky::AXIS).then_some(a))
        .ok_or_else(|| "weather: the area has no sky axis".to_owned())?;
    let Domain::Enum(values) = &axis.domain else {
        return Err("weather: the sky axis is not an enum".into());
    };
    let index = values.iter().position(|v| *v == report.sky).ok_or_else(|| {
        format!(
            "weather: the area's installed sky rows have no state {}",
            report.sky
        )
    })?;
    let mut assignment = world.remove_resource::<StateAssignment>().unwrap_or_default();
    assignment.0.insert(super::sky::AXIS.into(), index as u32);
    world.insert_resource(assignment);
    if let Some(wetness) = &report.input.wetness {
        world.insert_resource(wetness.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_assigns_the_installed_axis_value_and_refuses_an_absent_one() {
        let mut world = World::new();
        super::super::sky::spawn_axis(
            &mut world,
            &["weather-20-ashen".into(), "weather-0".into()],
            "synthetic membership".into(),
        );
        let mut report = Report {
            input: Snapshot {
                name: "independent fixture".into(),
                source: "synthetic state contract".into(),
                weather: 20,
                fire_ash: true,
                wetness: Some(Wetness {
                    name: "wetting observation".into(),
                    source: "independent fixture".into(),
                    value: 0.75,
                }),
            },
            sky: "weather-20-ashen".into(),
            weather_outdoor: String::new(),
        };
        assign(&mut world, &report).unwrap();
        assert_eq!(world.resource::<Wetness>(), report.input.wetness.as_ref());
        assert_eq!(
            world
                .resource::<StateAssignment>()
                .unwrap()
                .0
                .get(super::super::sky::AXIS),
            Some(&0)
        );
        report.sky = "weather-21-ashen".into();
        assert!(
            assign(&mut world, &report)
                .unwrap_err()
                .contains("no state weather-21-ashen")
        );
        report.input.source.clear();
        assert!(report.input.validate().is_err());
    }

    #[test]
    fn wetness_is_optional_state_and_refuses_missing_provenance_or_invalid_weights() {
        let snapshot: Snapshot =
            serde_json::from_str(r#"{"name":"observation","source":"fixture","weather":0,"fire_ash":false}"#)
                .unwrap();
        assert!(snapshot.wetness.is_none());
        let mut wetness = Wetness {
            name: "observation".into(),
            source: "fixture".into(),
            value: 0.0,
        };
        for value in [0.0, 0.5, 1.0] {
            wetness.value = value;
            wetness.validate().unwrap();
        }
        for value in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            wetness.value = value;
            assert!(wetness.validate().unwrap_err().contains("finite observed"));
        }
        wetness.value = 0.0;
        wetness.source.clear();
        assert!(wetness.validate().unwrap_err().contains("independent source"));
    }
}
