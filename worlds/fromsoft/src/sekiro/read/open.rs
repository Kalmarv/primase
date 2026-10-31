//! Verify source-area requests without a viewer's option vocabulary or file access.
use fromsoft_formats::Game;

/// The program's explicit read and selection inputs, checked before opening an area.
#[derive(Clone, Copy, Debug)]
pub struct AreaInputs {
    pub game: Game,
    pub hour: Option<f32>,
    pub read_environment: bool,
    pub explicit_environment: bool,
    pub environment_collision: bool,
    pub weather: bool,
    pub sky: bool,
    pub world_state: bool,
    pub read_states: bool,
    pub irradiance_package: Option<u8>,
}

impl AreaInputs {
    /// The same incompatibilities as the area reader and its state passes; no input is selected or changed.
    pub fn verify_hour(self) -> Result<(), String> {
        if self
            .hour
            .is_some_and(|hour| !hour.is_finite() || !(0.0..=24.0).contains(&hour) || !self.read_environment)
        {
            return Err(
                "fromsoft-area: hour requires read_environment=true and a finite value in 0..=24".into(),
            );
        }
        Ok(())
    }
    /// Validate the explicit weather request's source prerequisites.
    pub fn verify_weather(self) -> Result<(), String> {
        if self.weather && (self.game != Game::EldenRing || !self.sky) {
            return Err("weather requires Elden Ring and sky=true".into());
        }
        Ok(())
    }
    /// Validate a collision-linked environment load request.
    pub fn verify_collision(self) -> Result<(), String> {
        if self.environment_collision && (!self.read_environment || self.explicit_environment) {
            return Err(
                "environment_collision requires read_environment=true and no explicit environment file"
                    .into(),
            );
        }
        Ok(())
    }
    /// Validate the explicit observed world-state request.
    pub fn verify_world_state(self) -> Result<(), String> {
        if self.world_state
            && (self.game != Game::Sekiro
                || self.irradiance_package.is_some()
                || !self.read_states
                || !self.sky)
        {
            return Err(
                "world_state requires Sekiro, read_states=true, sky=true, and no explicit irradiance_package"
                    .into(),
            );
        }
        Ok(())
    }

    /// Validate all opening prerequisites without choosing any value.
    pub fn verify_open(self) -> Result<(), String> {
        self.verify_hour()?;
        self.verify_weather()?;
        self.verify_collision()?;
        self.verify_world_state()
    }
    /// Validate the package request at the read boundary; this does not choose a package.
    pub fn verify_read(self) -> Result<(), String> {
        if self
            .irradiance_package
            .is_some_and(|package| package > 5 || self.game != Game::Sekiro)
        {
            return Err("irradiance_package requires Sekiro and a source package index0..5".into());
        }
        Ok(())
    }
}

/// The authored full identity corresponding to the supported short map spelling.
pub fn full_area_name(area: &str) -> String {
    if area.len() == 6 {
        format!("{area}_00_00")
    } else {
        area.to_owned()
    }
}

/// A collision part whose stored graphics config names an installed draw-param file.
#[derive(Clone, Debug)]
pub struct CollisionEnvironment {
    pub part: String,
    pub file: String,
}

/// Read collision-to-draw-param links without asserting where the player stands.
pub fn collision_environments(
    archive: &fromsoft_formats::Archive,
    decoder: &fromsoft_formats::dcx::Decoder<'_>,
    game: Game,
    area: &str,
) -> fromsoft_formats::Result<Vec<CollisionEnvironment>> {
    use fromsoft_formats::{
        area::DrawParamId,
        msb::{MapLayout, PartKind},
    };
    let area = full_area_name(area);
    let layout = MapLayout::read(&archive.read(&fromsoft_formats::paths::map_layout(&area), decoder)?)?;
    let configs = layout.gparam_configs()?;
    let files = fromsoft_formats::area::draw_params(archive, game, &area);
    let mut entries = Vec::new();
    for part in layout
        .parts
        .iter()
        .filter(|part| part.kind == PartKind::Collision)
    {
        let Some(id) = configs
            .iter()
            .find(|config| config.part == part.index)
            .and_then(|config| config.draw_param_id())
        else {
            continue;
        };
        let Some(file) = files
            .iter()
            .find(|file| DrawParamId::from_stem(file).is_some_and(|(_, value)| value == id))
        else {
            continue;
        };
        entries.push(CollisionEnvironment {
            part: part.name.clone(),
            file: file.clone(),
        });
    }
    Ok(entries)
}
