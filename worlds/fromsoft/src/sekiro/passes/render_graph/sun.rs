//! Directional-light policy supplied by the registered graph preparation pass.
//!
//! The deferred sun program (`GXLightAcc_DDirLightShadowed1`) reads `cbLight`'s `FC_DirLight`, an array of light sets
//! (each pixel's G-buffer selects one) whose members the program's reflection names: the light directions `Vec_0..3`,
//! colours `Col_0..3` and specular colours `Spc_0..3`, then `bLocalLight` and `FC_SsaoWeight`; Sekiro's sets also hold
//! `AmbientSH` and `AmbientReflection` (288 bytes a set; Elden Ring's 224). Each member is bound by its name.
use iridia_interface::{ConstantType, ResourceKind};
use primase_core::shader::{ConstantBinding, ConstantRole, FrameParameter, Shader, StageBinding};

/// The members every light set is given the primary core sun or a finite stand-in through; the rest are zero.
pub(super) const NOTICE: &str = "directional lighting uses the primary core sun in every light set (Vec_0, Col_0, Spc_0) and its direction for the disabled secondary (Vec_1); directional sharpness is a finite stand-in of 1 (Vec_2, Vec_3) for both primary and disabled secondary lights until the GPARAM-to-shader mapping is established";

/// What one light set member is given.
enum Value {
    /// A core frame parameter in `xyz`, the stated `w`.
    Frame(FrameParameter, f32),
    /// A stated value.
    Words(Vec<u32>),
}

/// The value of the light set member `name`, `size` bytes; `None` for a member with no source (zero, named).
fn value(name: &str, size: u32) -> Result<Option<Value>, String> {
    let vector = |what: &str| {
        if size == 16 {
            Ok(())
        } else {
            Err(format!(
                "directional light set's {what} {name} is {size} bytes, expected 16"
            ))
        }
    };
    Ok(match name {
        // Give the disabled secondary the same finite direction as the sun; normalizing a zero half-vector can poison
        // a zero-radiance term.
        "Vec_0" | "Vec_1" => {
            vector("direction")?;
            Some(Value::Frame(FrameParameter::DirectionToLight, 1.))
        }
        "Col_0" => {
            vector("colour")?;
            Some(Value::Frame(FrameParameter::LightColour, 0.))
        }
        "Spc_0" => {
            vector("specular colour")?;
            Some(Value::Frame(FrameParameter::LightSpecularColour, 0.))
        }
        "Vec_2" | "Vec_3" => {
            vector("sharpness")?;
            Some(Value::Words([0.0f32, 0.0, 1.0, 1.0].map(f32::to_bits).to_vec()))
        }
        "Col_1" | "Col_2" | "Col_3" | "Spc_1" | "Spc_2" | "Spc_3" | "bLocalLight" | "FC_SsaoWeight"
        | "AmbientSH" | "AmbientReflection" => None,
        other => {
            return Err(format!(
                "directional light set member {other} has no stated value"
            ));
        }
    })
}

/// Bind the directional stage's light sets by their members' names. Returns the bindings added and the notice naming
/// the members left zero.
pub(super) fn bind(stage: &mut StageBinding, shader: &Shader) -> Result<(usize, String), String> {
    let resource = shader
        .interface
        .resources
        .iter()
        .find(|r| r.provenance.name == "cbLight")
        .ok_or("directional stage has no cbLight")?;
    let ResourceKind::ConstantBuffer { layout } = &resource.kind else {
        return Err("directional stage's cbLight is not a constant buffer".into());
    };
    let block = layout
        .constants
        .iter()
        .find(|c| c.name == "FC_DirLight")
        .ok_or("directional stage's cbLight names no FC_DirLight")?;
    let ConstantType::Array {
        element,
        count,
        stride,
    } = &block.ty
    else {
        return Err("directional stage's FC_DirLight is not an array of light sets".into());
    };
    if element.members().is_empty() {
        return Err("directional stage's light sets name no members".into());
    }
    let old = stage
        .constants
        .iter()
        .position(|c| c.resource == resource.id && c.name.as_deref() == Some("FC_DirLight"))
        .ok_or("directional stage has no light-set constant")?;
    let original = stage.constants.remove(old);
    if (original.offset, Some(original.size)) != (block.offset, block.ty.size()) {
        return Err(format!(
            "directional light-set binding is {} bytes at {}, its reflection {:?} at {}",
            original.size,
            original.offset,
            block.ty.size(),
            block.offset
        ));
    }
    let mut added = Vec::new();
    let mut zero = Vec::new();
    for set in 0..*count {
        for member in element.members() {
            let size = member
                .ty
                .size()
                .ok_or_else(|| format!("directional light set member {} has no size", member.name))?;
            let offset = block.offset + set * stride + member.offset;
            let name = format!("FC_DirLight[{set}].{}", member.name);
            match value(&member.name, size)? {
                Some(Value::Frame(parameter, w)) => {
                    added.push(ConstantBinding {
                        resource: resource.id,
                        offset,
                        size: 12,
                        name: Some(name.clone()),
                        role: ConstantRole::Frame(parameter),
                        lines: None,
                    });
                    added.push(ConstantBinding {
                        resource: resource.id,
                        offset: offset + 12,
                        size: 4,
                        name: Some(format!("{name} w")),
                        role: ConstantRole::Default {
                            words: vec![w.to_bits()],
                            why: NOTICE.into(),
                        },
                        lines: None,
                    });
                }
                Some(Value::Words(words)) => added.push(ConstantBinding {
                    resource: resource.id,
                    offset,
                    size,
                    name: Some(name),
                    role: ConstantRole::Default {
                        words,
                        why: NOTICE.into(),
                    },
                    lines: None,
                }),
                None => {
                    if set == 0 {
                        zero.push(member.name.as_str());
                    }
                    added.push(ConstantBinding {
                        resource: resource.id,
                        offset,
                        size,
                        name: Some(name),
                        role: ConstantRole::Default {
                            words: vec![0; size.div_ceil(4) as usize],
                            why: format!(
                                "directional light set member {} has no stated source: zero",
                                member.name
                            ),
                        },
                        lines: None,
                    });
                }
            }
        }
    }
    let count = added.len();
    stage.constants.extend(added);
    Ok((
        count,
        format!(
            "directional light set members with no stated source are zero: {}",
            zero.join(", ")
        ),
    ))
}
