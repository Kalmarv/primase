//! PEAK's reader: what its scripts hand every shader, on top of the Unity level read ([`read_globals`]).
pub mod scripts;

use std::collections::BTreeSet;
use std::sync::Arc;

use primase_core::World;
use primase_unity::components::{GlobalTexture, UnityShaderGlobals};
use primase_unity::level::texture_entity;
use unity_formats::player::{ObjRef, Player};
use unity_formats::scene::Scene;
use unity_formats::value::Value;

use crate::Result;

/// The global `Peak.FogShaderParams.Start` sets.
pub const FOG_TEXTURE: &str = "FogTexture";

/// What [`read_globals`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalsReport {
    /// `Peak.FogShaderParams` that run: enabled, on a GameObject active with every one above it.
    pub fog_shader_params: usize,
    /// Those stored off (disabled, or on an inactive GameObject): their `Start` runs only if the game turns them on.
    pub fog_shader_params_off: usize,
}

/// A stored reference as the value [`Player::resolve`] reads.
fn pptr_value(p: scripts::PPtr) -> Value {
    Value::Struct(vec![
        (Arc::from("m_FileID"), Value::Int(i64::from(p.file_id))),
        (Arc::from("m_PathID"), Value::Int(p.path_id)),
    ])
}

/// The globals PEAK's scripts hand every shader in scene `level` of `player`, as their stored fields state them, into
/// `w`'s `unity.shader_globals`, after [`primase_unity::level::read_level`] read the level into the same world (the
/// globals already there are kept). Script fields are decoded by [`scripts`]' named layouts; another layout is an
/// error.
///
/// - `FogTexture`: the `fogTexture` of the scene's `Peak.FogShaderParams` that runs (enabled, its GameObject and every
///   one above it active), which its `Start` sets: the Texture2D's entity, the one the level read made where a
///   material samples it too. It is named unset, with why, where the stored scene does not decide it: several run with
///   different textures (which `Start` runs last is not stated), the one that runs names no texture, or every one is
///   stored off.
pub fn read_globals(player: &Player, level: &str, w: &mut World) -> Result<GlobalsReport> {
    let scene = Scene::read(player, level)?;
    let version = player
        .files
        .get(&scene.file)
        .map_or("", |f| f.unity_version.as_str())
        .to_owned();
    let mut report = GlobalsReport::default();
    // the FogShaderParams that run, and the texture each names
    let mut running: Vec<(ObjRef, Option<ObjRef>)> = Vec::new();
    for (go, b) in scene.behaviours(player)? {
        if b.script != "Peak.FogShaderParams" {
            continue;
        }
        let field = scripts::fog_shader_params(&version, &b.data)?;
        let active = scene
            .ancestors(go)
            .iter()
            .all(|g| scene.game_objects.get(g).is_some_and(|g| g.active));
        if !(b.enabled && active) {
            report.fog_shader_params_off += 1;
            continue;
        }
        report.fog_shader_params += 1;
        let texture = player.resolve(&b.behaviour.file, &pptr_value(field))?;
        running.push((b.behaviour, texture));
    }
    let mut globals = w.resource::<UnityShaderGlobals>().cloned().unwrap_or_default();
    let textures: BTreeSet<&Option<ObjRef>> = running.iter().map(|(_, t)| t).collect();
    match (running.first(), textures.len()) {
        (None, _) if report.fog_shader_params_off > 0 => {
            globals.unset.insert(
                FOG_TEXTURE.into(),
                format!(
                    "the scene's {} Peak.FogShaderParams are stored off: their Start sets it only if the game turns one on",
                    report.fog_shader_params_off
                ),
            );
        }
        (None, _) => {}
        (Some((by, Some(texture))), 1) => {
            let entity = texture_entity(player, texture, w)?;
            globals.textures.insert(
                FOG_TEXTURE.into(),
                GlobalTexture {
                    texture: entity,
                    set_by: format!("Peak.FogShaderParams.fogTexture of {}/{}", by.file, by.path_id),
                },
            );
        }
        (Some((by, None)), 1) => {
            globals.unset.insert(
                FOG_TEXTURE.into(),
                format!(
                    "Peak.FogShaderParams of {}/{} sets it to no texture",
                    by.file, by.path_id
                ),
            );
        }
        (Some(_), n) => {
            globals.unset.insert(
                FOG_TEXTURE.into(),
                format!(
                    "{} Peak.FogShaderParams that run set {n} different textures: which Start runs last is not stated",
                    running.len()
                ),
            );
        }
    }
    w.insert_resource(globals);
    Ok(report)
}
