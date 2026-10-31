//! Read every candidate of a requested state-data family. This module never evaluates it.
use crate::{
    invalid,
    sekiro::source::{CommonState, DrawParam, StateInputs, StateReadScope},
};
use fromsoft_formats::{
    Archive, Game, Install, Result, area::DrawParamId, dcx::Decoder, emevd::Script,
    gparam_params::WeatherParam, msb::PartGparam,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Inventory of the requested scope, including payload size rather than a selected state.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub weather_rows: usize,
    pub common_script_bytes: usize,
    pub draw_param_candidates: usize,
    pub draw_param_bytes: usize,
    pub draw_param_json_bytes: usize,
}
impl Report {
    pub fn of(inputs: &StateInputs) -> Self {
        Self {
            weather_rows: inputs.weather.as_ref().map_or(0, |p| p.rows.len()),
            common_script_bytes: inputs.common.as_ref().map_or(0, |p| p.script_bytes),
            draw_param_candidates: inputs.draw_params.len(),
            draw_param_bytes: inputs.draw_param_bytes,
            draw_param_json_bytes: inputs.draw_param_json_bytes,
        }
    }
}

pub(super) fn read(
    install: &Install,
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    scope: StateReadScope,
) -> Result<Option<StateInputs>> {
    if !scope.any() {
        return Ok(None);
    }
    let mut inputs = StateInputs::new(install.game, area.to_owned());
    if scope.weather {
        if install.game != Game::EldenRing {
            return Err(invalid("weather", 0, "weather snapshots require Elden Ring"));
        }
        let param =
            fromsoft_formats::game_params::read_param(install, archive, decoder, "WeatherParam.param")?;
        inputs.weather = Some(WeatherParam::from_param(&param)?);
    }
    if scope.common {
        if install.game != Game::Sekiro {
            return Err(invalid(
                "world-state",
                0,
                "common event 100 is understood only for Sekiro",
            ));
        }
        let bytes = archive.read("/event/common.emevd.dcx", decoder)?;
        inputs.common = Some(CommonState {
            script: Script::read(&bytes)?,
            script_sha256: Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            script_bytes: bytes.len(),
            map: [
                area[1..3].parse().expect("validated area"),
                area[4..6].parse().expect("validated block"),
            ],
        });
    }
    Ok(Some(inputs))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_params(
    inputs: &mut StateInputs,
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    area: &str,
    choice: &super::draw_param::DrawParams,
    configs: &[PartGparam],
    collisions: &BTreeMap<String, DrawParamId>,
) -> Result<()> {
    if inputs.common.is_none() {
        return Ok(());
    }
    inputs.draw_param_requested = *choice != super::draw_param::DrawParams::None;
    if !inputs.draw_param_requested {
        return Ok(());
    }
    for stem in fromsoft_formats::area::draw_params(archive, game, area) {
        let (source, report) = super::draw_param::read(
            archive,
            decoder,
            game,
            area,
            &super::draw_param::DrawParams::File(stem.clone()),
            configs,
            collisions,
        )?;
        inputs.draw_param_bytes += report.bytes;
        if let Some(source) = &source {
            inputs.draw_param_json_bytes += primase_core::json::to_vec(source, false)
                .map_err(|e| invalid("state inputs", 0, e.to_string()))?
                .len();
        }
        let candidate: std::result::Result<DrawParam, String> = source.ok_or_else(|| {
            report
                .unreadable
                .unwrap_or_else(|| "selected file is missing or unreadable".into())
        });
        inputs.draw_params.insert(stem, candidate);
    }
    Ok(())
}
