//! Read the lightweight scope candidates; choices and heavy payload loads are separate operations.
use crate::sekiro::area_plan::{AreaCandidates, AreaSelection};
use crate::sekiro::{DrawParams, Region};
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

impl AreaSelection {
    /// The region an area read keeps: the selected rectangle with the caller's `margin` (how far past it a placement's
    /// model may reach and still be kept); none where the selection reads the whole area.
    pub fn read_region(&self, margin: f32) -> Option<Region> {
        self.region.map(|rect| Region { rect, margin })
    }

    /// The draw-param environment an area read loads: the file the selection chose, or none.
    pub fn read_draw_params(&self) -> DrawParams {
        self.draw_param.clone().map_or(DrawParams::None, DrawParams::File)
    }
}
