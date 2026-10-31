//! The source map's placement overview, without a viewer or region-selection policy.
use fromsoft_formats::{
    Archive, Result,
    dcx::Decoder,
    msb::{MapLayout, PartKind},
};

#[derive(Clone, Debug)]
pub struct AreaOverview {
    pub title: String,
    pub axes: &'static str,
    pub legend: Vec<String>,
    pub points: Vec<[f32; 2]>,
    pub classes: Vec<u8>,
    /// Authored records, available to an explicit region-selection operation.
    pub layout: MapLayout,
}

pub fn read(archive: &Archive, decoder: &Decoder<'_>, area: &str) -> Result<AreaOverview> {
    let area = super::open::full_area_name(area);
    let layout = MapLayout::read(&archive.read(&fromsoft_formats::paths::map_layout(&area), decoder)?)?;
    let legend = ["map pieces", "objects", "enemies", "collision", "other"]
        .map(str::to_owned)
        .to_vec();
    let mut points = Vec::with_capacity(layout.parts.len());
    let mut classes = Vec::with_capacity(layout.parts.len());
    for part in &layout.parts {
        let Ok(at) = part.matrix().map(|matrix| matrix[3]) else {
            continue;
        };
        points.push([at[0], at[2]]);
        classes.push(match part.kind {
            PartKind::MapPiece => 0,
            PartKind::Object | PartKind::Asset => 1,
            PartKind::Enemy => 2,
            PartKind::Collision | PartKind::ConnectCollision => 3,
            _ => 4,
        });
    }
    Ok(AreaOverview {
        title: format!("{area}: {} parts", points.len()),
        axes: "[x0, z0, x1, z1]: the read's region axes (x east, z ground: the core world frame's z negated), metres",
        legend,
        points,
        classes,
        layout,
    })
}
