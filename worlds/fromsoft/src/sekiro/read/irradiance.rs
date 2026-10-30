//! Sekiro environment packages: retain the authored boxes and every GIIV coefficient group.
//! Package suffixes are source states, not hours. A recorded 18:00 frame selects 03;
//! the general time-to-package producer has not yet been established.
use crate::{invalid, sekiro::IrradianceRegion};
use fromsoft_formats::{
    Archive, Result,
    dcx::Decoder,
    msb::{MapLayout, RegionShape},
    tpf::{Package, TextureKind},
};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Entity, Name, Source, World};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub packages: usize,
    pub textures: usize,
    pub boxes: usize,
    pub unplaced_groups: usize,
}
/// Read all six stored states. No spatial crop: a box centred outside a requested view
/// can still contain it. Extra coefficient groups remain texture entities with provenance.
pub fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    world: &mut World,
) -> Result<Report> {
    let records: Vec<_> = layout
        .map_regions()?
        .into_iter()
        .filter(|r| r.type_name == Some("environment_map_effect_box"))
        .collect();
    let mut packages = Vec::new();
    for state in 0_u8..6 {
        let path = format!("/map/{area}/{area}_envmap_{state:02}.tpf.dcx");
        if archive.contains(&path) {
            packages.push((state, path));
        }
    }
    if packages.is_empty() {
        return Ok(Report::default());
    }
    let axis = world.spawn();
    world.insert(axis, Name(format!("fromsoft.irradiance.{area}")));
    world.insert(
        axis,
        StateAxis {
            key: format!("fromsoft.irradiance.{area}"),
            domain: Domain::Enum(packages.iter().map(|(n, _)| format!("package-{n:02}")).collect()),
            default: None,
            scope: Scope::Global,
            class: "irradiance".into(),
            source: format!("{area} environment packages; time selector unresolved"),
        },
    );
    let mut report = Report::default();
    for (index, (state, path)) in packages.into_iter().enumerate() {
        let mut groups: BTreeMap<u32, [Option<Entity>; 4]> = BTreeMap::new();
        let package = Package::read(&archive.read(&path, decoder)?)?;
        for texture in package
            .textures
            .into_iter()
            .filter(|t| t.kind == TextureKind::Volume)
        {
            let stem = texture.name.strip_suffix(".dds").unwrap_or(&texture.name);
            let prefix = format!("{area}_GIIV");
            let rest = stem
                .strip_prefix(&prefix)
                .ok_or_else(|| invalid("GIIV", 0, format!("unexpected volume name {stem}")))?;
            let fields: Vec<_> = rest.split('_').collect();
            if fields.len() != 3 || fields[1] != format!("{state:02}") {
                return Err(invalid("GIIV", 0, "package/name state mismatch"));
            }
            let id = fields[0]
                .parse::<u32>()
                .map_err(|_| invalid("GIIV", 0, "invalid coefficient group id"))?;
            let channel = match fields[2] {
                "X" => 0,
                "Y" => 1,
                "Z" => 2,
                "W" => 3,
                _ => return Err(invalid("GIIV", 0, "unknown coefficient channel")),
            };
            let data = primase_dds::read_volume(&texture.dds).map_err(|e| invalid("GIIV DDS", 0, e))?;
            let entity = world.spawn();
            world.insert(entity, Name(texture.name.clone()));
            world.insert(entity, Source(format!("{path}:{}", texture.name)));
            world.insert(entity, data);
            if groups.entry(id).or_default()[channel].replace(entity).is_some() {
                return Err(invalid("GIIV", 0, "duplicate coefficient channel"));
            }
            report.textures += 1;
        }
        for group in groups.values() {
            if group.iter().any(Option::is_none) {
                return Err(invalid("GIIV", 0, "incomplete coefficient group"));
            }
        }
        let mut used = std::collections::BTreeSet::new();
        for region in &records {
            let id = region
                .name
                .strip_prefix("Env_Box")
                .and_then(|s| s.split('_').next())
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or_else(|| invalid("GIIV box", 0, "invalid box identity"))?;
            let group = groups
                .get(&id)
                .ok_or_else(|| invalid("GIIV box", 0, format!("box {id} has no coefficients")))?;
            let RegionShape::Box { width, height, depth } = region.shape else {
                return Err(invalid("GIIV box", 0, "environment box is not a box"));
            };
            let data = &region.type_data;
            if data.len() < 0x34 {
                return Err(invalid("GIIV box", 0, "truncated environment box data"));
            }
            let float =
                |at: usize| f32::from_le_bytes(data[at..at + 4].try_into().expect("checked type data"));
            let entity = world.spawn();
            let mut position = region.position;
            position[1] += height / 2.;
            let mut transform = crate::pose::placed(fromsoft_formats::msb::euler_matrix(
                position,
                region.rotation_degrees,
                [width / 2., height / 2., depth / 2.],
                0,
            )?);
            // Texture coordinates remain in the stored source box frame: reflect the
            // world frame only, undoing placed()'s conversion of the local Z axis.
            transform.cols[2] = transform.cols[2].map(|v| -v);
            world.insert(entity, transform);
            world.insert(entity, Name(format!("{area}:{}:{state:02}", region.name)));
            world.insert(entity, Source(format!("{area}:region:{}", region.index)));
            world.insert(
                entity,
                Condition(vec![Clause(vec![Literal {
                    axis,
                    values: 1_u64 << index,
                }])]),
            );
            world.insert(
                entity,
                IrradianceRegion {
                    area: area.to_owned(),
                    package: state,
                    region: region.clone(),
                    coefficients: group.map(|e| e.expect("complete group")),
                    display_distance: float(0),
                    fade_distance: float(4),
                    priority: data[9],
                    diffuse_intensity: float(0x28),
                    specular_intensity: float(0x2c),
                    terms: i32::from_le_bytes(data[0x24..0x28].try_into().expect("checked type data")),
                },
            );
            used.insert(id);
            report.boxes += 1;
        }
        report.unplaced_groups += groups.len() - used.len();
        report.packages += 1;
    }
    Ok(report)
}
