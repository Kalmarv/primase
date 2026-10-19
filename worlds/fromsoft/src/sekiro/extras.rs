//! Area extras as the source stores them: bank-zero BTL lights where they stand (`fromsoft.light`), every hang-edge
//! segment and the spawn points inside the region (`fromsoft.area_extras`). Their interpretation (light units, marker
//! and curve meanings, ledge chaining) belongs to passes and programs.
use super::source::{AreaExtras, HangSegment, LightKind, LightSource, SpawnPoint, SpawnRotation};
use crate::{
    Archive, Binder, Error, Result,
    btl::{Kind, LightBank},
    dcx::Decoder,
    edge::{Edges, Kind as EdgeKind},
    invalid,
    msb::{MapLayout, euler_transform},
};
use primase_core::pass::{Pass, PassLog};
use primase_core::{Component, Curve, Marker, Name, Resource, Source, Transform, World};
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
}
/// `parent` then `child`.
fn compose(parent: &Transform, child: &Transform) -> Transform {
    let direction = |v: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|i| {
            parent.cols[0][i] * v[0] + parent.cols[1][i] * v[1] + parent.cols[2][i] * v[2]
        })
    };
    Transform {
        cols: [
            direction(child.cols[0]),
            direction(child.cols[1]),
            direction(child.cols[2]),
            super::point(parent, child.cols[3]),
        ],
    }
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
    let transform = compose(map, &euler_transform(light.position, light.rotation, [1.; 3], 0)?);
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
    let edge_path = format!("/map/{area}/{area}");
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
                    let lower = s.to_ascii_lowercase();
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
                                super::point(&map, reflect(edge.points[0])),
                                super::point(&map, reflect(edge.points[1])),
                            ],
                        });
                    }
                }
            }
        }
    }
    report.hang_segments = hang_segments.len();
    let mut spawn_points = Vec::new();
    for point in layout.spawn_points()? {
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

/// `lower-fromsoft-extras`: the area's extras (`fromsoft.area_extras`, taken out) as core records in source order:
/// each hang-edge segment a two-point `core.curve` of kind `ledge` (labelled `<edge file>:<n>` by its place in its
/// file), each spawn point a `core.marker` of kind `travel` at its position, named as stored, its stored rotation
/// kept as `fromsoft.spawn_point`. Nothing is joined, rounded, renamed or turned: chaining ledges, choosing a
/// spawn and naming markers for players are a program's.
#[derive(Clone, Copy, Debug)]
pub struct LowerFromSoftExtras;

impl Pass for LowerFromSoftExtras {
    fn name(&self) -> &'static str {
        "lower-fromsoft-extras"
    }
    fn requires(&self) -> &[&'static str] {
        &[AreaExtras::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Curve::NAME,
            Marker::NAME,
            Transform::NAME,
            Name::NAME,
            Source::NAME,
            SpawnRotation::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[AreaExtras::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let extras = world
            .remove_resource::<AreaExtras>()
            .ok_or("no fromsoft.area_extras")?;
        let mut in_file = std::collections::BTreeMap::<&str, usize>::new();
        for segment in &extras.hang_segments {
            let n = in_file.entry(&segment.file).or_default();
            let e = world.spawn();
            world.insert(
                e,
                Curve {
                    kind: "ledge".into(),
                    points: segment.points.to_vec(),
                    closed: false,
                    tangents: None,
                    up: None,
                },
            );
            world.insert(e, Source(format!("{}:{n}", segment.file)));
            *n += 1;
            log.place(e);
            log.count("ledge segments", 1);
        }
        for point in &extras.spawn_points {
            let e = world.spawn();
            world.insert(
                e,
                Marker {
                    kind: "travel".into(),
                    ground: None,
                    yaw: None,
                },
            );
            world.insert(
                e,
                Transform {
                    cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], point.position],
                },
            );
            world.insert(e, Name(point.name.clone()));
            world.insert(
                e,
                SpawnRotation {
                    rotation_degrees: point.rotation_degrees,
                },
            );
            log.place(e);
            log.count("travel markers", 1);
        }
        Ok(())
    }
}
