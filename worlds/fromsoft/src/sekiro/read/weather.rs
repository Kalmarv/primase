//! Explicit Elden Ring weather state; lotteries are not defaults.
use fromsoft_formats::{Archive, Game, Install, Result, dcx::Decoder, gparam_params::WeatherParam};
use primase_core::{
    World,
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
            },
            sky: "weather-20-ashen".into(),
            weather_outdoor: String::new(),
        };
        assign(&mut world, &report).unwrap();
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
}
