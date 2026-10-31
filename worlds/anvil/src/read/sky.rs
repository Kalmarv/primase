//! The sky's lighting scale in core's terms, and a look's LayeredSky and controllers read from the forges.

use anvil_formats::archive::{Archive, At};
use anvil_formats::controller::{Controller, TIME_OF_DAY, WEATHER};
use primase_core::convert::{Notice, NoticeKind};
use primase_core::{Entity, World};

use crate::components::{AnvilSky, AnvilSkyControllers};

/// The first entry of the forges, in their order, named `name`.
pub fn named_resource(archive: &Archive, name: &str) -> Option<At> {
    let mut forge = 0;
    while let Some(file) = archive.forge(forge) {
        if let Some(entry) = file.listing().find(|entry| entry.name == name) {
            return Some(At {
                forge,
                entry: entry.entry,
            });
        }
        forge += 1;
    }
    None
}

/// The look `look`'s controllers (`TOD_<look>`, required, and `WEA_<look>`, when the forges hold one) attached to the
/// sky `entity` as `anvil.sky_controllers`; a look whose controllers can't be read keeps its LayeredSky's authored
/// defaults, with a notice saying why.
pub fn attach_look_controllers(
    archive: &mut Archive,
    world: &mut World,
    entity: Entity,
    look: &str,
    notices: &mut Vec<Notice>,
) {
    let read = |archive: &mut Archive, name: &str, class: u32| -> Result<Option<Controller>, String> {
        let Some(at) = named_resource(archive, name) else {
            return Ok(None);
        };
        let resource = archive.resource(at).map_err(|e| e.to_string())?;
        if resource.type_id != class {
            return Err(format!(
                "{name}: expected controller {class:#x}, found {:#x}",
                resource.type_id
            ));
        }
        Ok(Some(
            Controller::parse(&resource.body).map_err(|e| e.to_string())?,
        ))
    };
    let result = (|| -> Result<AnvilSkyControllers, String> {
        let time_of_day = read(archive, &format!("TOD_{look}"), TIME_OF_DAY)?
            .ok_or_else(|| format!("TOD_{look}: time-of-day controller absent"))?;
        let weather = read(archive, &format!("WEA_{look}"), WEATHER)?;
        Ok(AnvilSkyControllers {
            look: look.into(),
            time_of_day,
            weather,
        })
    })();
    match result {
        Ok(controllers) => {
            world.insert(entity, controllers);
        }
        Err(reason) => notices.push(Notice::new(
            "anvil atmosphere controllers",
            NoticeKind::Unsupported,
            format!("{look}: {reason}; authored LayeredSky defaults retained"),
            1,
        )),
    }
}

/// The look `look`'s LayeredSky (`LS_<look>`) read into `world` with its controllers ([`attach_look_controllers`]);
/// `None`, with a notice, when the forges hold none or its sky can't be parsed. A forge entry that can't be read is an
/// error.
pub fn read_look(
    archive: &mut Archive,
    world: &mut World,
    look: &str,
    notices: &mut Vec<Notice>,
) -> Result<Option<Entity>, anvil_formats::archive::Error> {
    let name = format!("LS_{look}");
    let Some(at) = named_resource(archive, &name) else {
        notices.push(Notice::new(
            "anvil default atmosphere",
            NoticeKind::Unsupported,
            format!("{name} is absent: source atmosphere unavailable; the presentation environment remains"),
            1,
        ));
        return Ok(None);
    };
    let resource = archive.resource(at)?;
    match anvil_formats::sky::StoredSky::parse(&resource.body) {
        Ok(stored) => {
            let entity = world.spawn();
            world.insert(entity, primase_core::Name(name));
            world.insert(
                entity,
                AnvilSky {
                    index_id: resource.class_id,
                    stored,
                },
            );
            attach_look_controllers(archive, world, entity, look, notices);
            Ok(Some(entity))
        }
        Err(reason) => {
            notices.push(Notice::new(
                "anvil default atmosphere",
                NoticeKind::Unsupported,
                reason.to_string(),
                1,
            ));
            Ok(None)
        }
    }
}

/// Convert the *evaluated lighting multiplier* cb1[39].x into core's log2 stops. PS_SunSkyModel (ShaderDataDX11
/// 1815) and DeferredAmbientOnly (448) multiply radiance by this value directly: it is a positive linear scale,
/// not EV and not the grading controller's property also named Exposure. This function requires an actual
/// resolved frame value; it must not be fed MinEV/MaxEV, t23, or the grading Exposure without their own mapping.
pub fn exposure_from_lighting_scale(scale: f32) -> Result<primase_core::Exposure, String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(format!(
            "cb1[39].x lighting exposure scale {scale} must be finite and positive"
        ));
    }
    Ok(primase_core::Exposure { stops: scale.log2() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_scale_is_linear_and_only_resolved_positive_values_become_stops() {
        for (scale, stops) in [(0.25, -2.0), (0.5, -1.0), (1.0, 0.0), (2.0, 1.0), (4.0, 2.0)] {
            assert_eq!(exposure_from_lighting_scale(scale).unwrap().stops, stops);
        }
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(exposure_from_lighting_scale(scale).is_err());
        }
    }
}
