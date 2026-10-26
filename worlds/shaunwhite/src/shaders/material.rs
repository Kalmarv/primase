//! Material controls of the generic albedo slice, packed as the shipped shader header defines them.
//! No frame, light, fog, influence or vertex-output value is supplied here. Unknown words remain Unknown.
use super::albedo::DerivedAlbedo;
use crate::components::MaterialRef;
use primase_core::shader::{
    ConstantBinding, ConstantRole, ConstantType, ResourceId, ResourceKind, ScalarType, StageBinding,
};
use std::collections::BTreeMap;

fn part(c: &ConstantBinding, start: u32, end: u32) -> Result<ConstantBinding, String> {
    let mut out = c.clone();
    out.offset = start;
    out.size = end - start;
    if let ConstantRole::Material(words) = &c.role {
        let first = ((start - c.offset) / 4) as usize;
        let last = ((end - c.offset) / 4) as usize;
        out.role = ConstantRole::Material(
            words
                .get(first..last)
                .ok_or("material binding has fewer words than its declared span")?
                .to_vec(),
        );
    } else if !matches!(c.role, ConstantRole::Unknown { .. })
        && (start != c.offset || end != c.offset + c.size)
    {
        return Err("material controls overlap a non-material binding".into());
    }
    Ok(out)
}
fn overlay(bindings: &mut Vec<ConstantBinding>, value: ConstantBinding) -> Result<(), String> {
    let end = value.offset + value.size;
    let mut out = Vec::new();
    for c in bindings.iter() {
        let old_end = c.offset + c.size;
        if c.resource != value.resource || old_end <= value.offset || c.offset >= end {
            out.push(c.clone());
            continue;
        }
        if !matches!(c.role, ConstantRole::Material(_) | ConstantRole::Unknown { .. }) {
            return Err("material controls overlap a non-material binding".into());
        }
        if c.offset < value.offset {
            out.push(part(c, c.offset, value.offset)?);
        }
        if old_end > end {
            out.push(part(c, end, old_end)?);
        }
    }
    out.push(value);
    *bindings = out;
    Ok(())
}

/// Fill only established material controls of an explicitly derived albedo program. The original derivation,
/// source shader and forward binding are untouched. The result retains every unresolved frame/varying/texture
/// binding; missing slots remain Unknown with the slot's name.
///
/// Packing comes from the shipped generic shader header: g_VectorArray row8 is the second tint, rows2..16
/// the seven two-row UV matrices, row17 normal height, row19.x self illumination, row20 the UV-set switches.
/// The native UV setter converts authored degrees to radians in double precision, rounds the angle to
/// float, then writes (scaleU*cos,-scaleU*sin,offsetU,unused) and
/// (scaleV*sin,scaleV*cos,offsetV,unused). Applied to (u,v,1,0), this rotates about the UV origin,
/// then scales and offsets; it does not rotate about the texture centre. The unused fourth words stay Unknown.
/// This packing is cross-checked against execution of the original native setter and a full PS on REF.
/// The native alpha setter gates layer1's switch
/// on blended/alpha-tested material state; the shader uses y/z directly for the later layer weights.
/// Optimized CTAB allocations may end partway through this table; no word outside their span is added.
/// Non-finite supplied values, invalid coordinate sets and overlapping/misaligned bindings are errors.
pub fn bind_material(derived: &DerivedAlbedo, material: &MaterialRef) -> Result<StageBinding, String> {
    let state = material.state.ok_or("albedo material fields not read")?;
    if state.water {
        return Err("water material is not generic albedo".into());
    }
    let mut binding = derived.binding.clone();
    let mut unresolved = BTreeMap::<(ResourceId, u32), String>::new();
    let mut supplied = BTreeMap::<(ResourceId, u32), (u32, String)>::new();
    for resource in &derived.program.interface.resources {
        let ResourceKind::ConstantBuffer { layout } = &resource.kind else {
            continue;
        };
        if !layout.size.is_multiple_of(4) {
            return Err("unaligned constant bank size".into());
        }
        let mut spans = binding
            .constants
            .iter()
            .filter(|c| c.resource == resource.id)
            .map(|c| (c.offset, c.size))
            .collect::<Vec<_>>();
        spans.sort_unstable();
        let mut end = 0;
        for (start, size) in spans {
            let next = start.checked_add(size).ok_or("material constant span overflow")?;
            if !start.is_multiple_of(4)
                || !size.is_multiple_of(4)
                || size == 0
                || start < end
                || next > layout.size
            {
                return Err("material constant spans overlap, are unaligned or exceed the buffer".into());
            }
            if start > end {
                binding.constants.push(ConstantBinding {
                    resource: resource.id,
                    offset: end,
                    size: start - end,
                    name: None,
                    role: ConstantRole::Unknown {
                        why: "constant-bank words not resolved".into(),
                    },
                    lines: None,
                });
            }
            end = next;
        }
        if end < layout.size {
            binding.constants.push(ConstantBinding {
                resource: resource.id,
                offset: end,
                size: layout.size - end,
                name: None,
                role: ConstantRole::Unknown {
                    why: "constant-bank words not resolved".into(),
                },
                lines: None,
            });
        }
        for c in &layout.constants {
            let size = c.ty.size().ok_or("material constant has no bounded size")?;
            if !c.offset.is_multiple_of(4) || c.offset.checked_add(size).is_none_or(|end| end > layout.size) {
                return Err(format!("{} allocation exceeds its constant bank", c.name));
            }
            let mut rows = BTreeMap::<u32, ([Option<f32>; 4], String)>::new();
            let alpha = [
                state.layering.use_alpha[0] && (state.blended || state.alpha_test),
                state.layering.use_alpha[1],
                state.layering.use_alpha[2],
                false,
            ]
            .map(|v| Some(if v { 1. } else { 0. }));
            match c.name.as_str() {
                "g_Layer1_ModulationColor" => {
                    rows.insert(0, (state.color.map(Some), c.name.clone()));
                }
                "g_Layer2_ModulationColor" => {
                    rows.insert(0, (state.layering.colors[0].map(Some), c.name.clone()));
                }
                "g_Layer3_ModulationColor" => {
                    rows.insert(0, (state.layering.colors[1].map(Some), c.name.clone()));
                }
                "g_FlagsUseTexAlpha" => {
                    rows.insert(0, (alpha, c.name.clone()));
                }
                "g_VectorArray" => {
                    if !matches!(&c.ty,ConstantType::Array {element,stride:16,..} if matches!(element.as_ref(),ConstantType::Vector(v) if v.width==4 && v.scalar==ScalarType::Float))
                        || size > 24 * 16
                        || !size.is_multiple_of(16)
                    {
                        return Err("g_VectorArray is not a bounded float4 allocation".into());
                    }
                    rows.insert(
                        8,
                        (
                            state.layering.colors[0].map(Some),
                            "Layer2_ModulationColor".into(),
                        ),
                    );
                    rows.insert(
                        17,
                        (
                            [Some(state.normal_scale), Some(1.), Some(1.), Some(1.)],
                            "NormalMap_HeightScales".into(),
                        ),
                    );
                    rows.insert(
                        19,
                        (
                            [Some(state.illumination), None, None, None],
                            "SelfIllumStrength".into(),
                        ),
                    );
                    let mut uv = [Some(0.), None, None, None];
                    for (lane, slot) in [(1, "diffuse_b"), (2, "diffuse_c"), (3, "mask")] {
                        if let Some(s) = material.slots.get(slot) {
                            if s.texcoord > 1 {
                                return Err(format!("{slot} UV set {} is not 0 or 1", s.texcoord));
                            }
                            uv[lane] = Some(f32::from(s.texcoord));
                        } else if 20 * 16 + lane as u32 * 4 < size {
                            unresolved.insert(
                                (resource.id, c.offset + 20 * 16 + lane as u32 * 4),
                                format!("{slot} UV channel not read"),
                            );
                        }
                    }
                    rows.insert(20, (uv, "FlagsUseUV2".into()));
                    for (row, slot) in [
                        (2, "diffuse"),
                        (4, "normal"),
                        (6, "specular"),
                        (9, "diffuse_b"),
                        (11, "specular_b"),
                        (13, "diffuse_c"),
                        (15, "mask"),
                    ] {
                        if row * 16 >= size {
                            continue;
                        }
                        let missing = match material.slots.get(slot) {
                            None => Some(format!("{slot} UV fields not read")),
                            Some(s) if !s.rotation.is_finite() => {
                                return Err(format!("{slot} UV rotation is non-finite"));
                            }
                            _ => None,
                        };
                        if let Some(why) = missing {
                            for relative in (row * 16..(row + 2) * 16).step_by(4).filter(|at| *at < size) {
                                unresolved.insert((resource.id, c.offset + relative), why.clone());
                            }
                        } else if let Some(s) = material.slots.get(slot) {
                            let radians = (f64::from(s.rotation) * (std::f64::consts::PI / 180.)) as f32;
                            let (sin, cos) = radians.sin_cos();
                            rows.insert(
                                row,
                                (
                                    [
                                        Some(s.scale[0] * cos),
                                        Some(-s.scale[0] * sin),
                                        Some(s.offset[0]),
                                        None,
                                    ],
                                    format!("{slot} UV row0"),
                                ),
                            );
                            rows.insert(
                                row + 1,
                                (
                                    [
                                        Some(s.scale[1] * sin),
                                        Some(s.scale[1] * cos),
                                        Some(s.offset[1]),
                                        None,
                                    ],
                                    format!("{slot} UV row1"),
                                ),
                            );
                        }
                    }
                }
                _ => continue,
            }
            if c.name != "g_VectorArray" && size != 16 {
                return Err(format!("{} is not one float4", c.name));
            }
            for (row, (values, name)) in rows {
                for (lane, value) in values.into_iter().enumerate() {
                    let relative = row * 16 + lane as u32 * 4;
                    if relative >= size {
                        continue;
                    }
                    let Some(value) = value else { continue };
                    if !value.is_finite() {
                        return Err(format!("{name} is non-finite"));
                    }
                    supplied.insert(
                        (resource.id, c.offset + relative),
                        (value.to_bits(), name.clone()),
                    );
                }
            }
        }
    }
    for ((resource, offset), (word, name)) in supplied {
        overlay(
            &mut binding.constants,
            ConstantBinding {
                resource,
                offset,
                size: 4,
                name: Some(name),
                role: ConstantRole::Material(vec![word]),
                lines: None,
            },
        )?;
    }
    binding.constants.sort_by_key(|c| (c.resource, c.offset));
    for ((resource, offset), why) in unresolved {
        overlay(
            &mut binding.constants,
            ConstantBinding {
                resource,
                offset,
                size: 4,
                name: Some("g_VectorArray UV matrix".into()),
                role: ConstantRole::Unknown { why },
                lines: None,
            },
        )?;
    }
    binding.constants.sort_by_key(|c| (c.resource, c.offset));
    let mut compact: Vec<ConstantBinding> = Vec::new();
    for c in binding.constants {
        if let Some(last) = compact.last_mut()
            && last.resource == c.resource
            && last.offset + last.size == c.offset
            && last.name == c.name
            && last.lines == c.lines
        {
            match (&mut last.role, &c.role) {
                (ConstantRole::Material(a), ConstantRole::Material(b)) => {
                    a.extend(b);
                    last.size += c.size;
                    continue;
                }
                (ConstantRole::Unknown { why: a }, ConstantRole::Unknown { why: b }) if a == b => {
                    last.size += c.size;
                    continue;
                }
                _ => {}
            }
        }
        compact.push(c);
    }
    binding.constants = compact;
    Ok(binding)
}
