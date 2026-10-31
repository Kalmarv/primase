//! Read the lightweight scope candidates; choices and heavy payload loads are separate operations.
use super::area::{Options, Parents, StateClasses};
use super::{groups::DisplayGroups, sky::Sky, texture_switch::TextureQuality};
use crate::sekiro::area_plan::{AreaCandidates, AreaSelection};
use crate::sekiro::source::StateReadScope;
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

/// The choices of an area read that its registered `select-fromsoft-area` selection does not state, with the options
/// the read takes. Every field is the program's.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaRead {
    /// How far past the selected region a placement's model may reach and still be kept.
    pub margin: f32,
    /// Raw state data families to retain for `resolve-fromsoft-state`.
    pub state_inputs: StateReadScope,
    /// Which world states to bring in.
    pub states: StateClasses,
    /// Whether the parts' draw groups become presence axes.
    pub display_groups: DisplayGroups,
    /// Whether the larger maps above the map also place their parts that belong to it.
    pub parents: Parents,
    /// The collision part whose scene config names the draw-param file the area is lit with, in place of the file the
    /// selection chose (an area selected with no environment).
    pub environment_collision: Option<String>,
    /// Whether the animation binder of each kept object model is read.
    pub object_animations: bool,
    /// Whether the layout's regions and events of every type are read as `fromsoft.map_records`.
    pub map_records: bool,
    /// Whether each map collision file's partner in the `f` collision binder is read with it.
    pub f_collision: bool,
    /// Whether the point and spot lights of effect files are read where they stand.
    pub effect_lights: bool,
    /// Whether the sky is read with the area.
    pub sky: Sky,
    /// Whether the frame the game's renderer draws is read as the world's `core.render_graph`.
    pub render_graph: bool,
    /// Whether authored environment metadata is read.
    pub irradiance: bool,
    /// Whether the effects themselves are read where they stand.
    pub effects: bool,
    /// Whether each read texture's low-resolution twin is read too.
    pub low_textures: bool,
    /// The texture quality Elden Ring's switch is read at, with `low_textures`.
    pub texture_quality: TextureQuality,
}

impl AreaRead {
    /// The area reader's options: the selection's region (with the margin) and draw-param environment (the collision
    /// part's, where one is stated), and the choices stated here.
    pub fn options(&self, selection: &AreaSelection) -> Options {
        Options {
            state_inputs: self.state_inputs,
            region: selection.read_region(self.margin),
            states: self.states,
            display_groups: self.display_groups,
            parents: self.parents,
            draw_param: match &self.environment_collision {
                Some(collision) => DrawParams::Collision(collision.clone()),
                None => selection.read_draw_params(),
            },
            object_animations: self.object_animations,
            map_records: self.map_records,
            f_collision: self.f_collision,
            effect_lights: self.effect_lights,
            sky: self.sky,
            render_graph: self.render_graph,
            irradiance: self.irradiance,
            effects: self.effects,
            low_textures: self.low_textures,
            texture_quality: self.texture_quality,
        }
    }
}
