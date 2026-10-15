//! Scene extras lowering. Light calibration, marker naming/facing and ledge
//! chaining are explicit policies, separate from the stored EDGE/BTL/MSB values.
use super::{
    ledges::{GameSurfaceIndex, chain},
    lights::SrgbLightPolicy,
};
use crate::{
    Archive, Binder, Error, Result,
    btl::LightBank,
    dcx::Decoder,
    edge::{Edges, Kind},
    invalid,
    msb::MapLayout,
};
use primase_core::{Collision, Name, Source, Surfaces, Transform, World};
use primase_skate_scene::{CollisionMode, MarkerKind, SkateCollision, SkateMarker, Snap, SnapMode};
#[derive(Debug, Default)]
pub struct Report {
    pub hang_segments: usize,
    pub curves: usize,
    pub lights: usize,
    pub travel_points: usize,
    pub missing_light_bank: bool,
    pub missing_edge_binder: bool,
}
fn marker_name(name: &str, i: usize) -> String {
    if name.contains("チェックポイント_") {
        let number = name
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();
        return format!("Checkpoint {number}");
    }
    if name.contains("本城ボス部屋") {
        return "Castle keep rooftop".into();
    }
    if name.contains("人魚竜") {
        return "Keep (dragon return)".into();
    }
    format!("Point {i}")
}
/// Call after map collision and before object soups are attached. Bank zero,
/// default light conversion, one-metre ledges, first travel point as spawn.
pub fn assemble(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    region: Option<[f32; 4]>,
    world: &mut World,
) -> Result<Report> {
    let mut report = Report::default();
    if layout.map_offsets.len() > 1 {
        return Err(invalid("extras", 0, "multiple map offsets"));
    }
    let map = layout
        .map_offsets
        .first()
        .map(|o| o.transform())
        .transpose()?
        .unwrap_or(Transform {
            cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
        });
    let inside =
        |p: [f32; 3]| region.is_none_or(|r| p[0] >= r[0] && p[0] < r[2] && -p[2] >= r[1] && -p[2] < r[3]);
    let reflect = |p: [f32; 3]| [p[0], p[1], -p[2]];
    let light_path = format!("/map/{area}/{area}_0000.btl.dcx");
    match archive.read(&light_path, decoder) {
        Err(Error::Missing(_)) => report.missing_light_bank = true,
        Err(e) => return Err(e),
        Ok(bytes) => {
            for light in LightBank::read(&bytes)?.lights {
                let converted = SrgbLightPolicy::default().convert(&light, &map)?;
                if !inside(converted.position) {
                    continue;
                }
                let e = world.spawn();
                world.insert(e, converted);
                world.insert(e, Source(format!("{area}_0000.btl/{}", light.name)));
                report.lights += 1;
            }
        }
    }
    let edge_path = format!("/map/{area}/{area}");
    let mut segments = vec![];
    match archive.read(&format!("{edge_path}.edgebhd"), decoder) {
        Err(Error::Missing(_)) => report.missing_edge_binder = true,
        Err(e) => return Err(e),
        Ok(header) => {
            let binder = Binder::read_split(
                &header,
                &archive.read(&format!("{edge_path}.edgebdt"), decoder)?,
                decoder,
            )?;
            for file in binder.files {
                if !file.name.as_deref().is_some_and(|s| {
                    let lower = s.to_ascii_lowercase();
                    lower.strip_suffix(".dcx").unwrap_or(&lower).ends_with(".edge")
                }) {
                    continue;
                }
                for edge in Edges::read(&decoder.decode_if_dcx(&file.bytes)?)?.edges {
                    if edge.kind == Kind::Hang {
                        segments.push([
                            super::point(&map, reflect(edge.points[0])),
                            super::point(&map, reflect(edge.points[1])),
                        ]);
                    }
                }
            }
        }
    }
    report.hang_segments = segments.len();
    let soups = world
        .iter::<SkateCollision>()
        .filter(|(_, c)| c.0.mode == Some(CollisionMode::TriangleMesh))
        .filter_map(|(e, _)| world.get::<Collision>(e))
        .collect::<Vec<_>>();
    let surface = GameSurfaceIndex::new(
        &soups,
        world.resource::<Surfaces>().unwrap_or(&Surfaces::default()),
    )?;
    for curve in chain(&segments, 1., inside, &surface)? {
        let e = world.spawn();
        world.insert(e, curve);
        report.curves += 1;
    }
    for point in layout.spawn_points()? {
        let position = reflect(point.position);
        if !inside(position) {
            continue;
        }
        let angle = f64::from(point.rotation_degrees[1]).to_radians();
        let yaw = angle.sin().atan2(-angle.cos()).to_degrees();
        let marker = SkateMarker {
            kind: MarkerKind::Travel,
            position: Some(([position[0], position[2]], Some(position[1]))),
            yaw: Some(yaw),
            m: None,
            blueprint: None,
            snap: Some(Snap::Mode(SnapMode::Ground)),
            lift: None,
            shelter: None,
            trainer: None,
            other: Default::default(),
        };
        if report.travel_points == 0 {
            let e = world.spawn();
            let mut spawn = marker.clone();
            spawn.kind = MarkerKind::Spawn;
            world.insert(e, spawn);
        }
        let e = world.spawn();
        world.insert(e, marker);
        world.insert(e, Name(marker_name(&point.name, report.travel_points)));
        world.insert(e, Source(format!("{area}:{}", point.name)));
        report.travel_points += 1;
    }
    Ok(report)
}
