//! Installed environment boxes and explicitly selected texture states. Clock selection is not inferred.
use crate::invalid;
use fromsoft_formats::{
    Archive, Binder, Result,
    dcx::Decoder,
    msb::{MapLayout, Region, RegionShape},
    tpf::{Package, TextureKind},
};
use primase_core::{
    ContentNote, Entity, EnvironmentProbeBlend, IrradianceVolume, Name, ProbeMipPolicy, ProbeState,
    ReflectionProbeRegion, Source, TextureLayers, Transform, World,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    High,
    Middle,
    Low,
}
impl Tier {
    pub fn word(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Middle => "middle",
            Self::Low => "low",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub state_a: u8,
    pub state_b: u8,
    pub factor: f32,
    pub tier: Tier,
}
impl Selection {
    pub fn verify(self) -> Result<()> {
        if self.state_a > 99
            || self.state_b > 99
            || !self.factor.is_finite()
            || !(0. ..=1.).contains(&self.factor)
        {
            return Err(invalid(
                "environment selection",
                0,
                "states must fit two decimal digits and factor must be finite in [0,1]",
            ));
        }
        Ok(())
    }
    pub fn notice(self) -> String {
        format!(
            "environment states {:02}->{:02} at {} and tier {} stated by the caller; the game's clock selector is not traced",
            self.state_a,
            self.state_b,
            self.factor,
            self.tier.word()
        )
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub selection: Option<Selection>,
    pub packages: usize,
    pub textures: usize,
    pub boxes: usize,
    pub groups: usize,
    pub unresolved_captured_boxes: usize,
}

/// Inferred from all 55 independently capture-labelled Stormveil boxes, with all129
/// installed boxes resolving to existing128 groups. No numeric record field matches
/// the default identity, so its stored name is parsed. The explicit signed override
/// at type-data+0x0a takes precedence. A missing payload is an error, never a skipped box.
pub fn group_for(region: &Region) -> Result<u32> {
    let bytes = region
        .type_data
        .get(0x0a..0x0c)
        .ok_or_else(|| invalid("environment box", region.index, "truncated group override"))?;
    let override_id = i16::from_le_bytes(bytes.try_into().expect("checked override"));
    if override_id >= 0 {
        return Ok(override_id as u32);
    }
    if override_id != -1 {
        return Err(invalid(
            "environment box",
            region.index,
            "negative group override other than -1",
        ));
    }
    region
        .name
        .strip_prefix("Env_Box")
        .and_then(|s| s.split('_').next())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            invalid(
                "environment box",
                region.index,
                "default probe identity is not an Env_Box numeric name",
            )
        })
}

#[derive(Clone, Copy, Debug, Default)]
struct Group {
    coefficients: [Option<Entity>; 4],
    cube: Option<Entity>,
}
fn texture_group(name: &str, area: &str, state: u8) -> Result<(u32, Option<usize>)> {
    let stem = name.strip_suffix(".dds").unwrap_or(name);
    let (rest, volume) = if let Some(rest) = stem.strip_prefix(&format!("{area}_GIIV")) {
        (rest, true)
    } else if let Some(rest) = stem.strip_prefix(&format!("{area}_GILM")) {
        (rest, false)
    } else {
        return Err(invalid(
            "environment texture",
            0,
            format!("unexpected texture {name}"),
        ));
    };
    let fields: Vec<_> = rest.split('_').collect();
    if fields.len() != 3 || fields[1] != format!("{state:02}") {
        return Err(invalid("environment texture", 0, "package/name state mismatch"));
    }
    let group = fields[0]
        .parse()
        .map_err(|_| invalid("environment texture", 0, "invalid group identity"))?;
    let channel = if volume {
        Some(match fields[2] {
            "X" => 0,
            "Y" => 1,
            "Z" => 2,
            "W" => 3,
            _ => return Err(invalid("environment texture", 0, "invalid coefficient channel")),
        })
    } else {
        if fields[2] != "rem" {
            return Err(invalid("environment texture", 0, "invalid cube suffix"));
        }
        None
    };
    Ok((group, channel))
}

fn read_state(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    state: u8,
    tier: Tier,
    world: &mut World,
    report: &mut Report,
) -> Result<BTreeMap<u32, Group>> {
    let family = &area[..3];
    let path = format!(
        "/map/{family}/{area}/{area}_envmap_{state:02}_{}_00.tpfbnd.dcx",
        tier.word()
    );
    let binder = Binder::read(&archive.read(&path, decoder)?, decoder)?;
    let mut groups: BTreeMap<u32, Group> = BTreeMap::new();
    for member in binder.files {
        for texture in Package::read(&member.bytes)?.textures {
            let (id, channel) = texture_group(&texture.name, area, state)?;
            let data = match (texture.kind, channel) {
                (TextureKind::Volume, Some(_)) => primase_dds::read_volume(&texture.dds),
                // These packages write six faces in DDS arraySize, confirmed by
                // the installed-payload census and full captured upload hashes.
                (TextureKind::Cube, None) => primase_dds::read_cube_faces(&texture.dds),
                _ => {
                    return Err(invalid(
                        "environment texture",
                        0,
                        "texture kind disagrees with member identity",
                    ));
                }
            }
            .map_err(|e| invalid("environment DDS", 0, e))?;
            let entity = world.spawn();
            world.insert(entity, Name(texture.name.clone()));
            world.insert(entity, Source(format!("{path}:{}", texture.name)));
            world.insert(entity, data);
            let group = groups.entry(id).or_default();
            let previous = match channel {
                Some(c) => group.coefficients[c].replace(entity),
                None => group.cube.replace(entity),
            };
            if previous.is_some() {
                return Err(invalid("environment texture", 0, "duplicate group channel/cube"));
            }
            report.textures += 1;
        }
    }
    for (&id, group) in &groups {
        if group.cube.is_none() || group.coefficients.iter().any(Option::is_none) {
            return Err(invalid(
                "environment group",
                id as usize,
                "group needs a cube and all four coefficients",
            ));
        }
    }
    report.packages += 1;
    Ok(groups)
}

/// Read every authored box: an off-screen or enclosing box can still affect the view.
/// Geometry and coefficients come from installed data; capture values remain test oracles.
pub fn read(
    archive: &Archive,
    decoder: &Decoder<'_>,
    area: &str,
    layout: &MapLayout,
    selection: Selection,
    world: &mut World,
) -> Result<Report> {
    selection.verify()?;
    if !area.is_ascii() || area.len() != 12 || !area.starts_with('m') {
        return Err(invalid("environment area", 0, "expected full map identity"));
    }
    let mut report = Report {
        selection: Some(selection),
        ..Report::default()
    };
    let a = read_state(
        archive,
        decoder,
        area,
        selection.state_a,
        selection.tier,
        world,
        &mut report,
    )?;
    let b = if selection.state_a == selection.state_b {
        a.clone()
    } else {
        read_state(
            archive,
            decoder,
            area,
            selection.state_b,
            selection.tier,
            world,
            &mut report,
        )?
    };
    if a.keys().ne(b.keys()) {
        return Err(invalid("environment states", 0, "state group identities differ"));
    }
    report.groups = a.len();
    let regions = layout.map_regions()?;
    let points: BTreeMap<u32, [f32; 3]> = regions
        .iter()
        .filter(|r| r.type_name == Some("environment_map_point"))
        .map(|r| {
            let id = r
                .name
                .strip_prefix("Env_Point")
                .and_then(|s| s.split('_').next())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| invalid("environment point", r.index, "invalid point identity"))?;
            Ok((id, crate::pose::FRAME.point(r.position)))
        })
        .collect::<Result<_>>()?;
    for region in regions
        .into_iter()
        .filter(|r| r.type_name == Some("environment_map_effect_box"))
    {
        let id = group_for(&region)?;
        let ga = require_group(&a, id, region.index)?;
        let gb = require_group(&b, id, region.index)?;
        let reflection_origin = *points.get(&id).ok_or_else(|| {
            invalid(
                "environment box",
                region.index,
                format!("computed group {id:04} has no authored reflection point"),
            )
        })?;
        let RegionShape::Box { width, height, depth } = region.shape else {
            return Err(invalid(
                "environment box",
                region.index,
                "non-box environment effect",
            ));
        };
        let raw = &region.type_data;
        let float = |offset: usize| -> Result<f32> {
            let b = raw
                .get(offset..offset + 4)
                .ok_or_else(|| invalid("environment box", region.index, "truncated box parameters"))?;
            Ok(f32::from_le_bytes(b.try_into().expect("checked float")))
        };
        let half_extents = [width / 2., height / 2., depth / 2.];
        let fade = float(4)?;
        let specular = float(0x24)?;
        let diffuse = float(0x28)?;
        let priority = *raw
            .get(9)
            .ok_or_else(|| invalid("environment box", region.index, "truncated priority"))?;
        let mut cols =
            fromsoft_formats::msb::euler_matrix(region.position, region.rotation_degrees, half_extents, 0)?;
        let centre_offset = cols[1];
        for (position, offset) in cols[3].iter_mut().zip(centre_offset) {
            *position += offset;
        }
        let mut transform = crate::pose::placed(cols);
        // Source texture axes stay source-oriented; reflect only the world frame.
        transform.cols[2] = transform.cols[2].map(|v| -v);
        let mut make_state = |group: &Group, state: u8| -> Result<ProbeState> {
            let coefficients = group.coefficients.map(|v| v.expect("validated complete group"));
            let diffuse_entity = world.spawn();
            world.insert(
                diffuse_entity,
                Name(format!("{area}:{}:diffuse:{state:02}", region.name)),
            );
            world.insert(diffuse_entity, transform);
            world.insert(
                diffuse_entity,
                IrradianceVolume {
                    coefficients,
                    basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., -1.]],
                    sampling_from_region: Transform::IDENTITY,
                    normal_offset: true,
                    half_extents,
                    fade_distance: fade,
                    diffuse_scale: diffuse,
                    priority,
                },
            );
            let cube = group.cube.expect("validated complete group");
            let mips = world.get::<TextureLayers>(cube).expect("read cube").levels.len();
            Ok(ProbeState {
                cube,
                diffuse_volume: Some(diffuse_entity),
                intensity: specular,
                mip_policy: ProbeMipPolicy::LinearRoughness {
                    minimum: 0.,
                    maximum: mips.saturating_sub(1) as f32,
                },
            })
        };
        let state_a = make_state(ga, selection.state_a)?;
        let state_b = make_state(gb, selection.state_b)?;
        let entity = world.spawn();
        world.insert(entity, Name(format!("{area}:{}", region.name)));
        world.insert(
            entity,
            Source(format!("{area}:region:{}:probe:{id:04}", region.index)),
        );
        world.insert(entity, transform);
        world.insert(
            entity,
            ReflectionProbeRegion {
                blend: EnvironmentProbeBlend {
                    state_a,
                    state_b,
                    factor: selection.factor,
                },
                half_extents,
                fade_distance: fade,
                priority,
                reflection_origin,
                parallax_from_region: None,
            },
        );
        report.boxes += 1;
    }
    if area == "m10_00_00_00" {
        report.unresolved_captured_boxes = 1;
    }
    for (key,reason,unsupported) in [
        ("environment-selection",selection.notice(),false),
        ("environment-group-rule","environment box-to-group association inferred from 55 capture-verified Elden Ring 2.7.2.0 joins: type-data +0x0a signed override, default numeric Env_Box name; the executable producer is not traced".into(),false),
    ] {
        let entity=world.spawn();world.insert(entity,ContentNote{key:key.into(),reason,unsupported,at:None});
    }
    if report.unresolved_captured_boxes > 0 {
        let entity = world.spawn();
        world.insert(entity,ContentNote{key:"environment-unresolved-capture".into(),
            reason:"1 captured environment box (frame 6993 sequence 1897) has no installed association and is not reconstructed".into(),unsupported:true,at:None});
    }
    Ok(report)
}

fn require_group(groups: &BTreeMap<u32, Group>, id: u32, region: usize) -> Result<&Group> {
    groups.get(&id).ok_or_else(|| {
        invalid(
            "environment box",
            region,
            format!("computed group {id:04} has no installed payload"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_group_is_an_error_instead_of_a_skipped_box() {
        let groups = BTreeMap::new();
        let error = require_group(&groups, 9999, 3).expect_err("missing payload");
        assert!(
            error
                .to_string()
                .contains("computed group 9999 has no installed payload")
        );
    }

    #[test]
    fn explicit_environment_selection_refuses_invalid_factor_and_keeps_notice() {
        let mut selection = Selection {
            state_a: 5,
            state_b: 6,
            factor: 0.5,
            tier: Tier::High,
        };
        assert!(selection.verify().is_ok());
        assert_eq!(
            selection.notice(),
            "environment states 05->06 at 0.5 and tier high stated by the caller; the game's clock selector is not traced"
        );
        selection.factor = f32::NAN;
        assert!(selection.verify().is_err());
    }
}
