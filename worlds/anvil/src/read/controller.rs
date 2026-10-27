//! A look's authored controllers on the sky entity.

use anvil_formats::archive::Archive;
use anvil_formats::controller::{LookControllers, look_controllers};
use primase_core::{Entity, World};

/// Attach the look's authored controllers to the sky entity `entity` as `anvil.sky_controllers`: the time-of-day one
/// (`TOD_<look>`, which the atmosphere has) and the weather one (`WEA_<look>`, when a forge holds it). Reading the
/// look's controllers is reading ([`look_controllers`]). The error is the reason (the caller states it as a notice
/// beside the sky's own defaults); an absent time-of-day controller is one, an absent weather controller is not.
pub fn attach_sky_controllers(
    archive: &mut Archive,
    world: &mut World,
    entity: Entity,
    look: &str,
) -> Result<(), String> {
    let LookControllers { time_of_day, weather } = look_controllers(archive, look)?;
    world.insert(
        entity,
        crate::components::AnvilSkyControllers {
            look: look.to_owned(),
            time_of_day,
            weather,
        },
    );
    Ok(())
}
