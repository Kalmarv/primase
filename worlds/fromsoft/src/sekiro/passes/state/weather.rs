//! Resolve an observed weather against the rows the reader retained.
use crate::sekiro::weather::{Report, Snapshot};
use fromsoft_formats::gparam_params::WeatherParam;
use primase_core::{
    World,
    state::{Domain, StateAssignment, StateAxis},
};

pub(super) fn select(rows: &WeatherParam, input: &Snapshot) -> Result<Report, String> {
    input.validate()?;
    let row = rows
        .row(input.weather)
        .ok_or_else(|| format!("WeatherParam has no row {}", input.weather))?;
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
        .find_map(|(_, a)| (a.key == crate::sekiro::sky::AXIS).then_some(a))
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
    assignment.0.insert(crate::sekiro::sky::AXIS.into(), index as u32);
    world.insert_resource(assignment);
    if let Some(wetness) = &report.input.wetness {
        world.insert_resource(wetness.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::weather::Wetness;

    #[test]
    fn a_snapshot_assigns_the_installed_axis_value_and_refuses_an_absent_one() {
        let mut world = World::new();
        crate::sekiro::sky::spawn_axis(
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
                .get(crate::sekiro::sky::AXIS),
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
