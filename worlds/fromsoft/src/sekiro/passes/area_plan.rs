//! A program-stated area scope selected before the heavy reader loads payloads.
use crate::sekiro::area_plan::{AreaCandidates, AreaSelection, AutoRegion};
use fromsoft_formats::Game;
use primase_core::{
    Resource, World,
    pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec},
    read_scope::ReadScopeSpec,
};

/// Explicit load-region choice. Preview is a named application policy, not an authored game default.
#[derive(Clone, Debug)]
pub enum RegionChoice {
    Whole,
    Rectangle([f32; 4]),
    Preview,
}
/// Explicit choice among the inventoried draw-param files.
#[derive(Clone, Debug)]
pub enum EnvironmentChoice {
    None,
    File(String),
    First,
}
#[derive(Clone, Debug)]
pub struct SelectFromSoftArea {
    pub region: RegionChoice,
    pub environment: EnvironmentChoice,
}

const OPTIONS: &[OptionSpec] = &[
    OptionSpec::required(
        "region",
        "preview",
        "whole, rect:x0:z0:x1:z1, or preview (Ashina courtyard; otherwise whole through1500 parts, then densest150m square on50m cells)",
    ),
    OptionSpec::required(
        "environment",
        "first",
        "none, file:<stem>, or first (lowest inventoried ID; a reported preview choice, never the game's selection)",
    ),
];
/// The same caller-stated choices are retained as portable read-scope metadata after selection.
pub const AREA_READ_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "fromsoft-area",
    options: OPTIONS,
};
pub const SELECT_FROMSOFT_AREA: PassSpec = PassSpec {
    name: "select-fromsoft-area",
    options: OPTIONS,
    make: |g| Ok(Box::new(SelectFromSoftArea::from_given(g)?)),
};

impl SelectFromSoftArea {
    fn from_given(g: &Given<'_>) -> Result<Self, String> {
        let region = match g.required("region") {
            "whole" => RegionChoice::Whole,
            "preview" => RegionChoice::Preview,
            text => {
                let values = text
                    .strip_prefix("rect:")
                    .ok_or("region needs whole, preview or rect:x0:z0:x1:z1")?
                    .split(':')
                    .map(|n| {
                        n.parse::<f32>()
                            .map_err(|_| "invalid rectangle coordinate".to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let rect: [f32; 4] = values
                    .try_into()
                    .map_err(|_| "region rectangle needs four coordinates")?;
                if !rect.iter().all(|v| v.is_finite()) {
                    return Err("region rectangle must be finite".into());
                }
                RegionChoice::Rectangle(rect)
            }
        };
        let environment = match g.required("environment") {
            "none" => EnvironmentChoice::None,
            "first" => EnvironmentChoice::First,
            text => EnvironmentChoice::File(
                text.strip_prefix("file:")
                    .filter(|s| !s.is_empty())
                    .ok_or("environment needs none, first or file:<stem>")?
                    .into(),
            ),
        };
        Ok(Self { region, environment })
    }
}

/// The numerical preview policy used by the pass and by metadata-only overview requests.
pub fn densest_square(points: &[[f32; 2]]) -> Option<AutoRegion> {
    let chosen = primase_meshops::select::densest_square(points, 150.0, 50.0).ok()??;
    Some(AutoRegion {
        rect: chosen.rect,
        kept: chosen.kept,
        parts: chosen.points,
    })
}

impl Pass for SelectFromSoftArea {
    fn name(&self) -> &'static str {
        "select-fromsoft-area"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "region".into(),
                match self.region {
                    RegionChoice::Whole => "whole".into(),
                    RegionChoice::Preview => "preview".into(),
                    RegionChoice::Rectangle(r) => format!("rect:{}:{}:{}:{}", r[0], r[1], r[2], r[3]),
                },
            ),
            (
                "environment".into(),
                match &self.environment {
                    EnvironmentChoice::None => "none".into(),
                    EnvironmentChoice::First => "first".into(),
                    EnvironmentChoice::File(f) => format!("file:{f}"),
                },
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AreaCandidates::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[AreaSelection::NAME, primase_core::read_scope::ReadScopes::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let input = world
            .resource::<AreaCandidates>()
            .ok_or("no fromsoft.area_candidates")?;
        let mut automatic_region = None;
        let region = match self.region {
            RegionChoice::Whole => None,
            RegionChoice::Rectangle(r) => Some(r),
            RegionChoice::Preview if input.game == Game::Sekiro && input.area == "m11_01_00_00" => {
                Some([-150.0, 200.0, -50.0, 300.0])
            }
            RegionChoice::Preview => {
                automatic_region = densest_square(&input.drawn_points).filter(|c| c.parts > 1500);
                automatic_region.as_ref().map(|c| c.rect)
            }
        };
        let mut draw_param_choice = None;
        let draw_param = match &self.environment {
            EnvironmentChoice::None => None,
            EnvironmentChoice::File(file) => Some(file.clone()),
            EnvironmentChoice::First => input.draw_params.first().cloned().inspect(|file| {
                draw_param_choice = Some((file.clone(), format!("no draw-param file was asked for, and how the game picks one for a place is not established: the lowest-numbered \
                 of the {} the archive holds for the area's map is read ({}); environment states another", input.draw_params.len(), input.draw_params.join(", "))));
            }),
        };
        let no_environment = !matches!(self.environment, EnvironmentChoice::None) && draw_param.is_none();
        let mut scope = AREA_READ_SCOPE.resolve(self.options())?;
        for file in &input.draw_params {
            if Some(file) != draw_param.as_ref() {
                AREA_READ_SCOPE.record_unloaded(
                    &mut scope,
                    "environment",
                    &format!("file:{file}"),
                    fromsoft_formats::paths::draw_param(file),
                )?;
            }
        }
        if let Some(choice) = &automatic_region {
            log.approximate(&choice.why(), 1);
        }
        if let Some((_, why)) = &draw_param_choice {
            log.approximate(why, 1);
        }
        log.count("candidate draw-param files", input.draw_params.len() as u64);
        scope.record_in(world);
        world.insert_resource(AreaSelection {
            region,
            automatic_region,
            draw_param,
            draw_param_choice,
            no_environment,
            scope,
        });
        Ok(())
    }
}
