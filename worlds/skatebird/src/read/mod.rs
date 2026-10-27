//! SkateBIRD's reader: its scripts' facts on top of the Unity level read ([`read_markers`]).
pub mod convert;
pub mod scripted;
pub mod scripts;

pub use convert::SkatebirdReader;
pub use scripted::{ScriptedReport, read_scripted};

use primase_core::{Name, Source, World};
use primase_unity::components::{UNITY, UnityObject};
use primase_unity::level::{OffAxes, transform_of};
use unity_formats::player::Player;
use unity_formats::scene::Scene;
use unity_formats::scene::UnityBehaviour;

use crate::Result;
use crate::components::{SkatebirdMissionGiver, SkatebirdResetPosition};
use primase_unity::components::UnityRenderSettings;
use primase_unity::program::{ProgramsReport, read_programs};

/// The compiled programs of the materials of `w` drawn with `Bumped Specular Voronoi`, which
/// `lower-skatebird-voronoi` bakes ([`crate::passes::voronoi`]): [`read_programs`] for that shader under the player's
/// render pipeline (`unity.render_settings`, which the level read stores), after
/// [`primase_unity::level::read_level`].
pub fn read_voronoi_programs(player: &Player, w: &mut World) -> Result<ProgramsReport> {
    let pipeline = w
        .resource::<UnityRenderSettings>()
        .and_then(|s| s.pipeline.clone())
        .ok_or_else(|| crate::Error::Layout("no render pipeline read: read the level first".into()))?;
    Ok(read_programs(
        player,
        w,
        &[crate::passes::voronoi::VORONOI],
        &pipeline,
    )?)
}

/// What [`read_markers`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkersReport {
    pub reset_positions: usize,
    /// Of those, the ones stored as the level's start (`startingPosition`).
    pub starting_positions: usize,
    pub mission_givers: usize,
    /// Of those, the ones whose title is stored (not empty).
    pub titled_mission_givers: usize,
    /// Scripts on a GameObject without a Transform: no position, left out.
    pub without_transform: usize,
}

/// The script's class: its name without the namespace.
fn class(b: &UnityBehaviour) -> &str {
    b.script.rsplit('.').next().unwrap_or(&b.script)
}

/// Every `ResetPosition` and `MissionGiver` of scene `level` of `player` into `w`, after
/// [`primase_unity::level::read_level`] read the level into the same world: each as an entity with its GameObject's
/// world matrix as `core.transform` (the core frame), its GameObject's `core.name`, `core.source`
/// (`<scene file>/<path id>` of the script), a `core.condition` on the level read's `unity.active` /
/// `unity.enabled` axes when its GameObject or an ancestor is stored inactive or the script disabled, and its
/// component. Script fields are decoded by [`scripts`]' named layouts; another layout is an error.
pub fn read_markers(player: &Player, level: &str, w: &mut World) -> Result<MarkersReport> {
    let scene = Scene::read(player, level)?;
    let version = player
        .files
        .get(&scene.file)
        .map_or("", |f| f.unity_version.as_str())
        .to_owned();
    let mut axes = OffAxes::of(w);
    let mut report = MarkersReport::default();
    for (go, b) in scene.behaviours(player)? {
        let starting = match class(&b) {
            "ResetPosition" => Some(scripts::reset_position(&version, &b.data)?),
            "MissionGiver" => None,
            _ => continue,
        };
        let Some(tp) = scene.transform_of(go) else {
            report.without_transform += 1;
            continue;
        };
        let path = scene.path(tp);
        let off = scene.off_flags(tp, Some((b.behaviour.path_id, b.enabled)));
        let e = w.spawn();
        w.insert(e, UNITY.transform(&transform_of(&scene.world_of(tp)?)));
        let name = scene.game_objects.get(&go).map_or("", |g| g.name.as_str());
        w.insert(e, Name(name.to_owned()));
        w.insert(e, Source(format!("{}/{}", b.behaviour.file, b.behaviour.path_id)));
        if let Some(c) = axes.condition(w, &scene, &off, &path) {
            w.insert(e, c);
        }
        match starting {
            Some(starting_position) => {
                report.reset_positions += 1;
                report.starting_positions += usize::from(starting_position);
                w.insert(
                    e,
                    SkatebirdResetPosition {
                        starting_position,
                        behaviour: UnityObject::from(&b.behaviour),
                        path,
                    },
                );
            }
            None => {
                let title = scripts::mission_giver_title(&version, &b.data)?;
                report.mission_givers += 1;
                report.titled_mission_givers += usize::from(title.is_some());
                w.insert(
                    e,
                    SkatebirdMissionGiver {
                        title,
                        behaviour: UnityObject::from(&b.behaviour),
                        path,
                    },
                );
            }
        }
    }
    Ok(report)
}
