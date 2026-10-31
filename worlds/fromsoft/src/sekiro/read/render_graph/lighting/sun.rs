//! The first directional-light stage, with the source's explicit finite stand-ins.
use primase_core::shader::{ConstantBinding, ConstantRole, FrameParameter, Shader, StageBinding};

pub(super) const NOTICE: &str = "directional lighting uses the primary core sun in both light sets; secondary and local fill lights are off; directional sharpness is a finite stand-in of 1 for both primary and disabled secondary lights until the GPARAM-to-shader mapping is established";

pub(super) fn bind(stage: &mut StageBinding, shader: &Shader) -> Result<usize, String> {
    let resource = shader
        .interface
        .resources
        .iter()
        .find(|r| r.provenance.name == "cbLight")
        .ok_or("directional stage has no cbLight")?;
    let old = stage
        .constants
        .iter()
        .position(|c| c.resource == resource.id && c.name.as_deref() == Some("FC_DirLight"))
        .ok_or("directional stage has no light-set constant")?;
    let original = stage.constants.remove(old);
    if original.size != 448 {
        return Err(format!(
            "directional light-set layout is {} bytes, expected 448",
            original.size
        ));
    }
    let mut added = Vec::new();
    for set in 0..2u32 {
        for row in 0..14u32 {
            let offset = original.offset + (set * 14 + row) * 16;
            let name = format!("FC_DirLight[{set}] row {row}");
            let role = match row {
                // Give the disabled secondary the same finite direction as the sun;
                // normalizing a zero half-vector can poison a zero-radiance term.
                0 | 1 => Some(FrameParameter::DirectionToLight),
                4 => Some(FrameParameter::LightColour),
                8 => Some(FrameParameter::LightSpecularColour),
                _ => None,
            };
            if let Some(parameter) = role {
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
                        words: vec![if row <= 1 { 1.0f32.to_bits() } else { 0 }],
                        why: NOTICE.into(),
                    },
                    lines: None,
                });
            } else {
                added.push(ConstantBinding {
                    resource: resource.id,
                    offset,
                    size: 16,
                    name: Some(name),
                    role: ConstantRole::Default {
                        words: if row <= 3 {
                            [0.0f32, 0.0, 1.0, 1.0].map(f32::to_bits).to_vec()
                        } else {
                            vec![0; 4]
                        },
                        why: NOTICE.into(),
                    },
                    lines: None,
                });
            }
        }
    }
    let count = added.len();
    stage.constants.extend(added);
    Ok(count)
}
