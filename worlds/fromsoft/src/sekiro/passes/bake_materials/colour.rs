//! Why a finished Colour output is not an Albedo bake. This query diagnoses; it never changes output roles.
use iridia::Program;
use iridia::flow::{At, Node, Op, Values};
use primase_core::shader::{Attribute, ConstantRole, InputRole, OutputRole};
use primase_core::{shader::StageBinding, shader::TextureSource};

/// Stable categories aggregate across materials. Unknown resource names/reasons remain in their source binding;
/// the pass log does not create a new key for every material or program.
pub(super) fn refusal(program: &Program, stage: &StageBinding) -> &'static str {
    let colours: Vec<_> = stage
        .outputs
        .iter()
        .filter(|o| o.role == OutputRole::Colour)
        .collect();
    if colours.is_empty() {
        return "no albedo output role";
    }
    let Ok(values) = Values::of_program(program) else {
        return "no albedo output role: Colour dependency analysis unavailable";
    };
    let mut roots = Vec::new();
    for colour in colours {
        for lane in 0..4 {
            if colour.lanes & (1 << lane) == 0 {
                continue;
            }
            let Some(node) = values.output_labelled(&program.interface, &colour.label, lane) else {
                return "no albedo output role: Colour dependency analysis unavailable";
            };
            roots.push(node);
        }
    }
    roots.extend(values.discards().iter().flatten().copied());
    roots.extend(values.writes().iter().flatten().copied());
    let mut scene = false;
    let mut unknown = !values.writes().is_empty();
    for id in values.reached_from(roots) {
        match values.node(id) {
            Node::Global {
                resource: Some(resource),
                at: At::Component(word),
            } => {
                let offset = u64::from(*word) * 4;
                let fields: Vec<_> = stage
                    .constants
                    .iter()
                    .filter(|c| {
                        c.resource == *resource
                            && u64::from(c.offset) <= offset
                            && offset + 4 <= u64::from(c.offset) + u64::from(c.size)
                    })
                    .collect();
                unknown |= fields.is_empty();
                for field in fields {
                    match field.role {
                        // a value the material or the source's policy fixes
                        ConstantRole::Material(_)
                        | ConstantRole::AlphaReference { .. }
                        | ConstantRole::Default { .. } => {}
                        ConstantRole::View(_) | ConstantRole::Frame(_) | ConstantRole::Object(_) => {
                            scene = true
                        }
                        ConstantRole::Unknown { .. } => unknown = true,
                    }
                }
            }
            // An indexed resource cannot be assigned to one constant field by a guessed address.
            Node::Global { .. } => unknown = true,
            Node::Texel { resource, .. } | Node::Query { resource } => {
                match resource.and_then(|r| stage.textures.iter().find(|t| t.resource == r)) {
                    Some(t) => match t.source {
                        TextureSource::Texture { .. } | TextureSource::Default { .. } => {}
                        TextureSource::Frame(_) => scene = true,
                        TextureSource::Missing { .. } | TextureSource::Target { .. } => unknown = true,
                    },
                    None => unknown = true,
                }
            }
            Node::Input { input, .. } => {
                let input = &program.interface.inputs[*input];
                if input.system.is_some() {
                    scene = true;
                } else if !stage.inputs.iter().any(|i| {
                    i.label == input.label && matches!(i.role, InputRole::Stream(Attribute::TexCoord(_)))
                }) {
                    // A varying is not a portable coordinate just because its label looks like one.
                    unknown = true;
                }
            }
            Node::Op {
                op: Op::Derivative(..),
                ..
            } => scene = true,
            _ => {}
        }
    }
    if scene {
        "no albedo output role: scene-dependent Colour (frame, view, object or raster inputs); portable surface terms not established"
    } else if unknown {
        "no albedo output role: Colour has unresolved resource or varying dependencies; portable surface terms not established"
    } else {
        "no albedo output role: material-only Colour; portable Colour lowering not implemented"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iridia_interface::Stage;
    use primase_core::World;
    use primase_core::shader::{ConstantBinding, FrameParameter, OutputBinding};

    fn binding(program: &Program) -> StageBinding {
        StageBinding {
            constants: program
                .interface
                .resources
                .iter()
                .map(|r| ConstantBinding {
                    resource: r.id,
                    offset: 0,
                    size: 16,
                    name: Some(r.provenance.name.clone()),
                    lines: None,
                    role: if r.provenance.d3d.as_ref().unwrap().register == 1 {
                        ConstantRole::Frame(FrameParameter::LightColour)
                    } else {
                        ConstantRole::Material(vec![1f32.to_bits(); 4])
                    },
                })
                .collect(),
            outputs: vec![OutputBinding {
                label: program.interface.outputs[0].label.clone(),
                lanes: 15,
                role: OutputRole::Colour,
            }],
            ..StageBinding::new(Stage::Pixel, World::default().spawn())
        }
    }

    #[test]
    fn material_colour_is_not_silently_promoted_to_albedo() {
        let p = iridia_dxbc::lift(include_bytes!("../../../../tests/fixtures/colour_material.dxbc")).unwrap();
        let stage = binding(&p);
        assert_eq!(
            refusal(&p, &stage),
            "no albedo output role: material-only Colour; portable Colour lowering not implemented"
        );
        assert_eq!(stage.outputs[0].role, OutputRole::Colour);
    }

    #[test]
    fn colour_reading_the_frame_light_is_scene_dependent() {
        let p = iridia_dxbc::lift(include_bytes!("../../../../tests/fixtures/colour_scene.dxbc")).unwrap();
        assert_eq!(
            refusal(&p, &binding(&p)),
            "no albedo output role: scene-dependent Colour (frame, view, object or raster inputs); portable surface terms not established"
        );
    }
}
