//! Read the lightweight scope candidates; choices and heavy payload loads are separate operations.
use crate::sekiro::area_plan::AreaCandidates;
use fromsoft_formats::{Archive, Game, Result, dcx::Decoder, msb::MapLayout};
use primase_core::World;

/// Every authored drawn-part point and candidate draw-param identity of the named area.
/// No region, GPARAM file or preview policy is selected here.
pub fn read_area_candidates(
    archive: &Archive,
    decoder: &Decoder<'_>,
    game: Game,
    area: &str,
) -> Result<World> {
    let layout = MapLayout::read(&archive.read(&fromsoft_formats::paths::map_layout(area), decoder)?)?;
    let mut world = World::new();
    world.insert_resource(AreaCandidates {
        game,
        area: area.into(),
        drawn_points: fromsoft_formats::msb::drawn_part_points(&layout)?,
        draw_params: fromsoft_formats::area::draw_params(archive, game, area),
    });
    Ok(world)
}
