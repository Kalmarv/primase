//! Authored ER environment metadata and requested payload loading. Selection and lowering are registered passes.
use crate::{
    invalid,
    sekiro::environment_data::{
        EnvironmentCatalog, EnvironmentGroup, EnvironmentLoadRequest, EnvironmentPayload,
        EnvironmentPayloads, EnvironmentRegion, FarTileEnvironmentGroup, Placement,
    },
};
pub use fromsoft_formats::envmap::Tier;
use fromsoft_formats::{
    Archive, Binder, Result,
    dcx::Decoder,
    mapid::MapId,
    msb::{MapLayout, Region},
    tpf::{Package, TextureKind},
};
use primase_core::{Name, Resource, Source, TextureShape, World};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub packages: usize,
    pub textures: usize,
    pub boxes: usize,
    /// A far tile's groups that no box names, attached as [`FarTileEnvironmentGroup`]s for a placement pass.
    pub far_tile_groups: usize,
    pub groups: usize,
    /// Diagnostic timing for the offline measurement; excluded from deterministic conversion reports.
    #[serde(skip)]
    pub catalog_seconds: f64,
    pub catalog_largest_member_bytes: usize,
}

/// The source association inferred from55 independent Stormveil labels: explicit signed override first,
/// otherwise the authored name's identity. No state or quality selection is performed.
pub fn group_for(region: &Region, fields: &fromsoft_formats::envmap::BoxFields) -> Result<u32> {
    match fields.group_override {
        id if id >= 0 => Ok(id as u32),
        -1 => fromsoft_formats::envmap::region_identity(&region.name, "Env_Box"),
        _ => Err(invalid(
            "environment box",
            region.index,
            "negative group override other than -1",
        )),
    }
}

/// Whether `area` is an overworld far tile (`mAA_X_Z_02`, or its alternate `_12`).
fn is_far_tile(area: &str) -> bool {
    MapId::from_name(area).is_some_and(|id| matches!(id.area, 60 | 61) && id.variant & 15 == 2)
}

/// Describe every authored box and available package. No environment texture is decoded or attached here; one binder at a time supplies names/headers only.
pub fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    world: &mut World,
) -> Result<Report> {
    let started = std::time::Instant::now();
    let paths = fromsoft_formats::envmap::packages(archive, area)?;
    let packages = fromsoft_formats::envmap::inventory(archive, decoder, &paths)?;
    let mut report = Report {
        packages: packages.len(),
        catalog_seconds: started.elapsed().as_secs_f64(),
        catalog_largest_member_bytes: packages.iter().map(|p| p.member_bytes).max().unwrap_or(0),
        ..Default::default()
    };
    let regions = layout.map_regions()?;
    let points: BTreeMap<u32, [f32; 3]> = regions
        .iter()
        .filter(|r| r.type_name == Some("environment_map_point"))
        .map(|r| {
            Ok((
                fromsoft_formats::envmap::region_identity(&r.name, "Env_Point")?,
                crate::pose::FRAME.point(r.position),
            ))
        })
        .collect::<Result<_>>()?;
    let mut groups = BTreeSet::new();
    for region in regions
        .into_iter()
        .filter(|r| r.type_name == Some("environment_map_effect_box"))
    {
        let fields = fromsoft_formats::envmap::box_fields(layout, &region)?
            .ok_or_else(|| invalid("environment box", region.index, "unsupported MSB generation"))?;
        let group = group_for(&region, &fields)?;
        let reflection_origin = *points.get(&group).ok_or_else(|| {
            invalid(
                "environment box",
                region.index,
                format!("computed group {group:04} has no authored reflection point"),
            )
        })?;
        let available_packages: Vec<_> = packages
            .iter()
            .filter(|p| p.members.iter().any(|m| m.identity.group == group))
            .map(|p| p.package.path.clone())
            .collect();
        if available_packages.is_empty() {
            return Err(invalid(
                "environment box",
                region.index,
                format!("computed group {group:04} has no installed payload"),
            ));
        }
        let entity = world.spawn();
        world.insert(entity, Name(format!("{area}:{}", region.name)));
        world.insert(
            entity,
            Source(format!("{area}:region:{}:probe:{group:04}", region.index)),
        );
        world.insert(
            entity,
            EnvironmentRegion {
                area: area.into(),
                placement: Placement::Authored(region),
                fields,
                group,
                available_packages,
                reflection_origin,
            },
        );
        report.boxes += 1;
        groups.insert(group);
    }
    // a far tile's own groups, which no box names: the source places them nowhere
    if is_far_tile(area) {
        let unnamed: BTreeSet<u32> = packages
            .iter()
            .flat_map(|p| p.members.iter().map(|m| m.identity.group))
            .filter(|g| !groups.contains(g))
            .collect();
        for group in unnamed {
            let available_packages: Vec<_> = packages
                .iter()
                .filter(|p| p.members.iter().any(|m| m.identity.group == group))
                .map(|p| p.package.path.clone())
                .collect();
            let entity = world.spawn();
            world.insert(entity, Name(format!("{area}:far tile group {group:04}")));
            world.insert(entity, Source(format!("{area}:far-tile:probe:{group:04}")));
            world.insert(
                entity,
                FarTileEnvironmentGroup {
                    area: area.into(),
                    group,
                    available_packages,
                },
            );
            report.far_tile_groups += 1;
            groups.insert(group);
        }
    }
    report.groups = groups.len();
    world.insert_resource(EnvironmentCatalog {
        area: area.into(),
        packages,
    });
    report.catalog_seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// Load exactly the package request already attached by a selection pass, without applying its blend factor.
pub fn load_requested(archive: &Archive, decoder: &Decoder<'_>, world: &mut World) -> Result<Report> {
    let Some(request) = world.resource::<EnvironmentLoadRequest>().cloned() else {
        return Ok(Report::default());
    };
    request
        .verify(world)
        .map_err(|e| invalid("environment load", 0, e))?;
    let mut loaded = world
        .resource::<EnvironmentPayloads>()
        .cloned()
        .unwrap_or_default();
    let mut report = Report::default();
    for package in request.0 {
        if loaded.0.iter().any(|p| p.package == package) {
            continue;
        }
        let mut groups: BTreeMap<u32, EnvironmentGroup> = BTreeMap::new();
        let binder = Binder::read(&archive.read(&package.path, decoder)?, decoder)?;
        for member in binder.files {
            for texture in Package::read(&member.bytes)?.textures {
                let identity = fromsoft_formats::envmap::texture_identity(&texture.name, &package.area)?;
                if identity.state != package.state {
                    return Err(invalid("environment texture", 0, "package/name state mismatch"));
                }
                if !matches!(
                    (texture.kind, identity.channel),
                    (TextureKind::Volume, Some(_)) | (TextureKind::Cube, None)
                ) {
                    return Err(invalid(
                        "environment texture",
                        0,
                        "texture kind disagrees with member identity",
                    ));
                }
                // The cubes' DX10 arraySize counts faces (6), not cubes; the DDS reader reads that as one cube.
                let data =
                    primase_dds::read_layers(&texture.dds).map_err(|e| invalid("environment DDS", 0, e))?;
                if !matches!(
                    (texture.kind, data.shape),
                    (TextureKind::Volume, TextureShape::Volume { .. })
                        | (TextureKind::Cube, TextureShape::Cube { cubes: 1 })
                ) {
                    return Err(invalid(
                        "environment DDS",
                        0,
                        format!("{:?} for a {:?} texture", data.shape, texture.kind),
                    ));
                }
                let entity = world.spawn();
                world.insert(entity, Name(texture.name.clone()));
                world.insert(entity, Source(format!("{}:{}", package.path, texture.name)));
                world.insert(entity, data);
                let group = groups.entry(identity.group).or_default();
                let previous = match identity.channel {
                    Some(c) => group.coefficients[c].replace(entity),
                    None => group.cube.replace(entity),
                };
                if previous.is_some() {
                    return Err(invalid("environment texture", 0, "duplicate group channel/cube"));
                }
                report.textures += 1;
            }
        }
        report.groups += groups.len();
        loaded.0.push(EnvironmentPayload { package, groups });
        report.packages += 1;
    }
    world.insert_resource(loaded);
    Ok(report)
}
