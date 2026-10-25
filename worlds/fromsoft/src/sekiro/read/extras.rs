//! Area extras as the source stores them: bank-zero BTL lights where they stand (`fromsoft.light`), every hang-edge
//! segment and the spawn points inside the region (`fromsoft.area_extras`). Their interpretation (light units, marker
//! and curve meanings, ledge chaining) belongs to passes and programs.
use crate::sekiro::source::{AreaExtras, HangSegment, LightKind, LightSource, SpawnPoint};
use crate::{
    Archive, Binder, Error, Result,
    btl::{Kind, LightBank},
    dcx::Decoder,
    edge::{Edges, Kind as EdgeKind},
    invalid,
    msb::{MapLayout, euler_transform},
};
use primase_core::{Source, Transform, World};

#[derive(Debug, Default)]
pub struct Report {
    pub hang_segments: usize,
    pub lights: usize,
    pub travel_points: usize,
    /// Bank-zero lights and spawn points outside the read region: not read.
    pub lights_outside_region: usize,
    pub spawn_points_outside_region: usize,
    /// EDGE records of kinds other than hang edges, by kind (`grapple`, `hug`): not read.
    pub other_edges: std::collections::BTreeMap<String, usize>,
    pub missing_light_bank: bool,
    pub missing_edge_binder: bool,
    /// The map layout's generation does not say which regions are spawn points: none read.
    pub spawn_points_not_read: bool,
    /// With map records asked for ([`records`]): the regions and events kept, by the word for their type (`type_<n>`
    /// for a code SoulsFormats does not name), and the regions outside the read region.
    pub region_types: std::collections::BTreeMap<String, usize>,
    pub event_types: std::collections::BTreeMap<String, usize>,
    pub regions_dropped_outside: usize,
}
fn unit(v: [f32; 3]) -> Result<[f32; 3]> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0. && n.is_finite() {
        Ok(v.map(|x| x / n))
    } else {
        Err(invalid("BTL light", 0, "degenerate light axis"))
    }
}
/// A BTL light where it stands: its stored position and rotation through the map offset `map` (world frame), and
/// its record.
pub fn light_record(light: &crate::btl::Light, map: &Transform) -> Result<(Transform, LightSource)> {
    let transform = map.compose(&euler_transform(light.position, light.rotation, [1.; 3], 0)?);
    let record = LightSource {
        name: light.name.clone(),
        kind: match light.kind {
            Kind::Point => LightKind::Point,
            Kind::Spot => LightKind::Spot,
            Kind::Directional => LightKind::Directional,
        },
        diffuse: light.diffuse,
        diffuse_power: light.diffuse_power,
        specular: light.specular,
        specular_power: light.specular_power,
        cast_shadows: light.cast_shadows,
        cone_angle: light.cone_angle,
        radius: light.radius,
        tangent: unit(transform.cols[0])?,
    };
    Ok((transform, record))
}
/// Bank zero's lights inside the region (by position), every hang segment in edge-file and edge order, and the spawn
/// points inside the region in MSB order; the lights and spawn points outside it, and the EDGE records of other kinds,
/// counted.
pub fn assemble(
    archive: &Archive,
    decoder: &Decoder<'_>,
    folder: &str,
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
    let light_path = format!("{folder}/{area}_0000.btl.dcx");
    match archive.read(&light_path, decoder) {
        Err(Error::Missing(_)) => report.missing_light_bank = true,
        Err(e) => return Err(e),
        Ok(bytes) => {
            for light in LightBank::read(&bytes)?.lights {
                let (transform, record) = light_record(&light, &map)?;
                if !inside(transform.cols[3]) {
                    report.lights_outside_region += 1;
                    continue;
                }
                let e = world.spawn();
                world.insert(e, record);
                world.insert(e, transform);
                world.insert(e, Source(format!("{area}_0000.btl/{}", light.name)));
                report.lights += 1;
            }
        }
    }
    let edge_path = format!("{folder}/{area}");
    let mut hang_segments = vec![];
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
                let Some(name) = file.name.as_deref().filter(|s| {
                    let lower = crate::names::fold(s);
                    lower.strip_suffix(".dcx").unwrap_or(&lower).ends_with(".edge")
                }) else {
                    continue;
                };
                let name = name.rsplit(['/', '\\']).next().unwrap_or(name).to_owned();
                for edge in Edges::read(&decoder.decode_if_dcx(&file.bytes)?)?.edges {
                    let other = match edge.kind {
                        EdgeKind::Hang => None,
                        EdgeKind::Grapple => Some("grapple"),
                        EdgeKind::Hug => Some("hug"),
                    };
                    if let Some(kind) = other {
                        *report.other_edges.entry(kind.to_owned()).or_default() += 1;
                    } else {
                        hang_segments.push(HangSegment {
                            file: name.clone(),
                            points: [
                                map.apply(reflect(edge.points[0])),
                                map.apply(reflect(edge.points[1])),
                            ],
                        });
                    }
                }
            }
        }
    }
    report.hang_segments = hang_segments.len();
    let mut spawn_points = Vec::new();
    let points = if layout.generation.spawn_region.is_some() {
        layout.spawn_points()?
    } else {
        report.spawn_points_not_read = true;
        Vec::new()
    };
    for point in points {
        let position = reflect(point.position);
        if !inside(position) {
            report.spawn_points_outside_region += 1;
            continue;
        }
        spawn_points.push(SpawnPoint {
            name: point.name,
            position,
            rotation_degrees: point.rotation_degrees,
        });
        report.travel_points += 1;
    }
    world.insert_resource(AreaExtras {
        hang_segments,
        spawn_points,
    });
    Ok(report)
}

/// Every region (inside the read region by its position, as spawn points are) and every event of the layout as the
/// `fromsoft.map_records` resource, counted by type into `report`. The spawn regions are among them: the extras make
/// markers of those, and the records pass leaves them to it.
pub fn records(layout: &MapLayout, region: Option<[f32; 4]>, report: &mut Report, world: &mut World) -> Result<()> {
    let inside = |p: [f32; 3]| region.is_none_or(|r| p[0] >= r[0] && p[0] < r[2] && -p[2] >= r[1] && -p[2] < r[3]);
    let word = |name: Option<&str>, code: u32| name.map_or_else(|| format!("type_{code}"), str::to_owned);
    let mut regions = Vec::new();
    for r in layout.map_regions()? {
        if !inside(r.position) {
            report.regions_dropped_outside += 1;
            continue;
        }
        *report.region_types.entry(word(r.type_name, r.type_code)).or_default() += 1;
        regions.push(r);
    }
    let events = layout.map_events()?;
    for e in &events {
        *report.event_types.entry(word(e.type_name, e.type_code)).or_default() += 1;
    }
    world.insert_resource(crate::sekiro::source::MapRecords {
        regions,
        events,
        spawn_region: layout.generation.spawn_region,
    });
    Ok(())
}
