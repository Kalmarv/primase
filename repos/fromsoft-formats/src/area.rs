//! The files a map keeps besides its layout, where the games keep them: its light bank (`<folder>/<map>_0000.btl.dcx`),
//! its hang-edge binder (`<folder>/<map>.edgebhd` and `.edgebdt`) and the draw-param files of its map
//! (`/param/drawparam/<mNN_NN>_<id>.gparam.dcx`). `folder` is the map's archive folder ([`crate::Game::map_folder`]).
use crate::btl::LightBank;
use crate::dcx::Decoder;
use crate::edge::Edges;
use crate::{Archive, Binder, Error, Game, Result};

/// The map's light bank zero, or `None` where the archives hold none.
pub fn light_bank(
    archive: &Archive,
    decoder: &Decoder<'_>,
    folder: &str,
    map: &str,
) -> Result<Option<LightBank>> {
    match archive.read(&format!("{folder}/{map}_0000.btl.dcx"), decoder) {
        Err(Error::Missing(_)) => Ok(None),
        Err(e) => Err(e),
        Ok(bytes) => LightBank::read(&bytes).map(Some),
    }
}

/// Every EDGE file of the map's edge binder, in binder order, by its file name (`m11_01_00_00_000.edge`), or `None`
/// where the archives hold no edge binder. Members that are not EDGE files are left out.
pub fn edge_files(
    archive: &Archive,
    decoder: &Decoder<'_>,
    folder: &str,
    map: &str,
) -> Result<Option<Vec<(String, Edges)>>> {
    let stem = format!("{folder}/{map}");
    let header = match archive.read(&format!("{stem}.edgebhd"), decoder) {
        Err(Error::Missing(_)) => return Ok(None),
        Err(e) => return Err(e),
        Ok(header) => header,
    };
    let binder = Binder::read_split(
        &header,
        &archive.read(&format!("{stem}.edgebdt"), decoder)?,
        decoder,
    )?;
    let mut files = Vec::new();
    for file in binder.files {
        let Some(name) = file.name.as_deref().filter(|s| {
            let lower = crate::names::fold(s);
            lower.strip_suffix(".dcx").unwrap_or(&lower).ends_with(".edge")
        }) else {
            continue;
        };
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name).to_owned();
        files.push((name, Edges::read(&decoder.decode_if_dcx(&file.bytes)?)?));
    }
    Ok(Some(files))
}

/// The stem the draw-param files of an area's map share: `m11_01` for `m11_01_00_00`; Elden Ring's open-world tiles
/// share `m60_00` and `m61_00` (no file carries a tile's coordinates). `None` for a name that is not an area's.
fn draw_param_map(game: Game, area: &str) -> Option<String> {
    let first = area.get(..3)?;
    let second = area.get(4..6)?;
    Some(match (game, first) {
        (Game::EldenRing, "m60" | "m61") => format!("{first}_00"),
        _ => format!("{first}_{second}"),
    })
}

/// The archive path of the draw-param file `stem` (`m11_01_0720`).
pub fn draw_param_path(stem: &str) -> String {
    format!("/param/drawparam/{stem}.gparam.dcx")
}

/// The draw-param files the archive holds for `area`'s map, by stem, in ID order: every `<map>_<0000..9999>`.
pub fn draw_params(archive: &Archive, game: Game, area: &str) -> Vec<String> {
    let Some(map) = draw_param_map(game, area) else {
        return Vec::new();
    };
    (0..10_000)
        .map(|id| format!("{map}_{id:04}"))
        .filter(|stem| archive.contains(&draw_param_path(stem)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_names_its_files_by_its_first_two_blocks_and_open_world_tiles_share_one_set() {
        assert_eq!(
            draw_param_map(Game::Sekiro, "m11_01_00_00").as_deref(),
            Some("m11_01")
        );
        assert_eq!(
            draw_param_map(Game::EldenRing, "m10_00_00_00").as_deref(),
            Some("m10_00")
        );
        assert_eq!(
            draw_param_map(Game::EldenRing, "m12_02_00_00").as_deref(),
            Some("m12_02")
        );
        assert_eq!(
            draw_param_map(Game::EldenRing, "m60_44_36_00").as_deref(),
            Some("m60_00")
        );
        assert_eq!(
            draw_param_map(Game::EldenRing, "m61_50_50_00").as_deref(),
            Some("m61_00")
        );
        assert_eq!(draw_param_map(Game::Sekiro, "m6").as_deref(), None);
        assert_eq!(
            draw_param_path("m11_01_0720"),
            "/param/drawparam/m11_01_0720.gparam.dcx"
        );
    }
}
