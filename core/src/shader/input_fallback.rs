//! Source-stated replacement inputs when a pixel program has no executable previous stage. Standard surface
//! semantics are shared machinery; a source binding pass explicitly opts in and owns any further label policy.

use iridia::flow::{Node, Values};
use iridia_interface::{Interface, ScalarType, Stage};
use serde::Serialize;

use super::{Attribute, InputRole, StageBinding};
use crate::pbr::TextureSlot;

/// Coordinates resolved through a drawn material's PBR texture reference, distinct from a raw mesh UV stream.
/// Read the reference's UV set and apply its transform once per vertex, in the transform's stored order. With
/// no reference use `absent_texture_set` with no transform; with no such stream use `absent_stream` float words.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct FallbackCoordinates {
    pub slot: TextureSlot,
    pub absent_texture_set: u8,
    pub absent_stream: [u32; 2],
}

/// Lanes whose preceding vertex program only copies a coordinate component. `Some(0)` is u, `Some(1)` is v;
/// `None` keeps the base fallback's lane. The source binding states the coordinate interpretation explicitly.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct FallbackCoordinateCopies {
    pub coordinates: FallbackCoordinates,
    pub lanes: [Option<u8>; 4],
}

/// The base value of a replacement input, before any source-proved coordinate copies.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputFallbackValue {
    /// The drawn surface's normalized world normal, xyz, with w=0. Generic geometry normal generation is separate.
    Normal,
    /// COLOR_0 as surface colour, honoring `core.vertex_color`. Ignored or absent colour uses the stated float
    /// words; the PBR base-colour factor is not multiplied into this value. Raw `Stream(Color(0))` ignores that policy.
    MaterialVertexColour { absent: [u32; 4] },
    /// The material-resolved coordinate pair repeated as xyxy. Raw `Stream(TexCoord(n))` applies no PBR transform.
    MaterialCoordinates(FallbackCoordinates),
    /// Four finite IEEE-754 float words, stated by the source, not invented by the consumer.
    Literal([u32; 4]),
}

/// One source-stated input used only when a preceding stage cannot execute. The four base value forms cover
/// normals, material colour, material coordinates and literals; optional copies retain the vertex program's
/// proved lane wiring. A consumer with no description supplies zero and names the missing input in its report.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct InputFallback {
    pub value: InputFallbackValue,
    pub coordinate_copies: Option<FallbackCoordinateCopies>,
}

impl InputFallback {
    /// State a base value with no coordinate-copy lanes.
    pub fn new(value: InputFallbackValue) -> Self {
        Self {
            value,
            coordinate_copies: None,
        }
    }

    pub fn check(&self) -> Result<(), String> {
        let finite = |words: &[u32]| {
            if words.iter().all(|word| f32::from_bits(*word).is_finite()) {
                Ok(())
            } else {
                Err("replacement input contains a non-finite float word".to_owned())
            }
        };
        match &self.value {
            InputFallbackValue::Normal => {}
            InputFallbackValue::MaterialVertexColour { absent } | InputFallbackValue::Literal(absent) => {
                finite(absent)?
            }
            InputFallbackValue::MaterialCoordinates(coordinates) => finite(&coordinates.absent_stream)?,
        }
        if let Some(copies) = &self.coordinate_copies {
            finite(&copies.coordinates.absent_stream)?;
            if copies.lanes.iter().flatten().any(|lane| *lane > 1) {
                return Err("replacement coordinate-copy lane is neither u nor v".to_owned());
            }
        }
        Ok(())
    }
}

fn base_coordinates() -> FallbackCoordinates {
    FallbackCoordinates {
        slot: TextureSlot::BaseColor,
        absent_texture_set: 0,
        absent_stream: [0; 2],
    }
}

/// Opt a registered binding pass into standard surface-semantic replacement inputs. Only NORMAL0, COLOR0,
/// TEXCOORD0 and TANGENT0 float varyings whose role is Previous/Unknown are changed; explicit roles, system inputs
/// and every other label stay untouched. Returns the number stated so the calling pass can report it by name.
/// This is a compatibility approximation for a missing/unusable vertex stage, never a native shader inference.
pub fn bind_surface_input_fallbacks(stage: &mut StageBinding, interface: &Interface) -> usize {
    if stage.stage != Stage::Pixel || interface.stage != Stage::Pixel {
        return 0;
    }
    let mut count = 0;
    for input in &mut stage.inputs {
        if !matches!(input.role, InputRole::Previous | InputRole::Unknown { .. }) {
            continue;
        }
        let Some(varying) = interface.inputs.iter().find(|v| v.label == input.label) else {
            continue;
        };
        if varying.system.is_some() || varying.value.scalar != ScalarType::Float || input.label.index != 0 {
            continue;
        }
        let (value, why) = match input.label.name.to_ascii_uppercase().as_str() {
            "NORMAL" => (InputFallbackValue::Normal, "the mesh normal"),
            "COLOR" => (
                InputFallbackValue::MaterialVertexColour {
                    absent: [1.0f32.to_bits(); 4],
                },
                "the mesh vertex colour",
            ),
            "TEXCOORD" => (
                InputFallbackValue::MaterialCoordinates(base_coordinates()),
                "the mesh UV, in both pairs",
            ),
            "TANGENT" => (
                InputFallbackValue::Literal([1.0f32.to_bits(), 0, 0, 1.0f32.to_bits()]),
                "stand-in: the +X tangent (the scene has none)",
            ),
            _ => continue,
        };
        input.role = InputRole::PreviousOr {
            fallback: InputFallback::new(value),
            why: why.into(),
        };
        count += 1;
    }
    count
}

/// Retain the bare UV-copy lanes of a preceding vertex program in replacement descriptions. A coordinate lane
/// is proved only when its value graph is a direct input lane and that input's role is raw TexCoord(0). The
/// replacement uses the explicitly stated coordinate interpretation `coordinates`, not a consumer's UV policy.
/// Arithmetic, non-float outputs and unresolved programs produce no guessed copies. Returns inputs augmented.
pub fn bind_fallback_uv_copies(
    pixel: &mut StageBinding,
    program: &iridia::Program,
    vertex: &StageBinding,
    coordinates: FallbackCoordinates,
) -> usize {
    if pixel.stage != Stage::Pixel || vertex.stage != Stage::Vertex || program.stage != Stage::Vertex {
        return 0;
    }
    let Ok(values) = Values::of_program(program) else {
        return 0;
    };
    let mut copies = std::collections::BTreeMap::new();
    for (&(output, lane), &id) in values.outputs() {
        let out = &program.interface.outputs[output];
        let Node::Input { input, lane: from } = values.node(id) else {
            continue;
        };
        let source = &program.interface.inputs[*input];
        let is_uv = vertex
            .inputs
            .iter()
            .any(|role| role.label == source.label && role.role == InputRole::Stream(Attribute::TexCoord(0)));
        if out.system.is_none()
            && out.value.scalar == ScalarType::Float
            && source.value.scalar == ScalarType::Float
            && *from < 2
            && is_uv
        {
            copies.insert(
                (out.label.name.to_ascii_uppercase(), out.label.index, lane),
                *from,
            );
        }
    }
    let mut count = 0;
    for input in &mut pixel.inputs {
        let lanes = std::array::from_fn(|lane| {
            copies
                .get(&(
                    input.label.name.to_ascii_uppercase(),
                    input.label.index,
                    lane as u8,
                ))
                .copied()
        });
        if lanes.iter().all(Option::is_none) {
            continue;
        }
        match &mut input.role {
            InputRole::PreviousOr { fallback, .. } => {
                fallback.coordinate_copies = Some(FallbackCoordinateCopies {
                    coordinates: coordinates.clone(),
                    lanes,
                });
            }
            InputRole::Previous | InputRole::Unknown { .. } => {
                input.role = InputRole::PreviousOr {
                    fallback: InputFallback {
                        value: InputFallbackValue::Literal([0; 4]),
                        coordinate_copies: Some(FallbackCoordinateCopies {
                            coordinates: coordinates.clone(),
                            lanes,
                        }),
                    },
                    why: "stand-in: zero (the scene has no such stream)".into(),
                };
            }
            _ => continue,
        }
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::InputBinding;
    use iridia_interface::{Label, Value, Varying, VaryingProvenance};

    fn varying(name: &str, index: u32, scalar: ScalarType, width: u8) -> Varying {
        Varying {
            label: Label {
                name: name.into(),
                index,
            },
            system: None,
            value: Value { scalar, width },
            interp: None,
            used: true,
            provenance: VaryingProvenance::default(),
        }
    }

    #[test]
    fn standard_surface_fallbacks_are_explicit_and_leave_other_labels_and_roles_alone() {
        let interface = Interface {
            stage: Stage::Pixel,
            inputs: vec![
                varying("NORMAL", 0, ScalarType::Float, 3),
                varying("COLOR", 0, ScalarType::Float, 4),
                varying("TEXCOORD", 0, ScalarType::Float, 4),
                varying("TANGENT", 0, ScalarType::Float, 4),
                varying("TEXCOORD", 7, ScalarType::Float, 4),
                varying("NORMAL", 1, ScalarType::Float, 3),
                varying("TANGENT", 1, ScalarType::Uint, 4),
            ],
            outputs: vec![],
            resources: vec![],
            facts: Default::default(),
        };
        let mut stage = StageBinding::new(Stage::Pixel, crate::World::new().spawn());
        stage.inputs = interface
            .inputs
            .iter()
            .map(|v| InputBinding {
                label: v.label.clone(),
                role: InputRole::Previous,
            })
            .collect();
        assert_eq!(bind_surface_input_fallbacks(&mut stage, &interface), 4);
        for input in &stage.inputs[..4] {
            let InputRole::PreviousOr { fallback, why } = &input.role else {
                panic!("explicit fallback")
            };
            assert!(!why.is_empty());
            assert!(fallback.check().is_ok());
        }
        assert!(
            stage.inputs[4..]
                .iter()
                .all(|input| input.role == InputRole::Previous)
        );
        assert_eq!(
            bind_surface_input_fallbacks(&mut stage, &interface),
            0,
            "idempotent"
        );
        stage.inputs[0].role = InputRole::Stream(Attribute::Normal);
        assert_eq!(bind_surface_input_fallbacks(&mut stage, &interface), 0);
        assert_eq!(stage.inputs[0].role, InputRole::Stream(Attribute::Normal));
    }

    #[test]
    fn fallback_words_and_copy_masks_refuse_nonfinite_or_undefined_components() {
        let mut fallback = InputFallback::new(InputFallbackValue::Literal([0; 4]));
        assert!(fallback.check().is_ok());
        fallback.value = InputFallbackValue::Literal([f32::NAN.to_bits(); 4]);
        assert!(fallback.check().is_err());
        fallback.value = InputFallbackValue::MaterialVertexColour {
            absent: [f32::INFINITY.to_bits(); 4],
        };
        assert!(fallback.check().is_err());
        fallback.value = InputFallbackValue::MaterialCoordinates(base_coordinates());
        fallback.coordinate_copies = Some(FallbackCoordinateCopies {
            coordinates: base_coordinates(),
            lanes: [Some(2), None, None, None],
        });
        assert!(fallback.check().is_err());
        fallback
            .coordinate_copies
            .as_mut()
            .expect("owned fallback fixture")
            .lanes[0] = Some(1);
        assert!(fallback.check().is_ok());
    }

    #[test]
    fn replacement_copy_masks_follow_only_bare_uv_lanes_in_the_vertex_program() {
        let mut interface = Interface {
            stage: Stage::Vertex,
            inputs: vec![varying("packed_uv", 0, ScalarType::Float, 2)],
            outputs: vec![varying("custom_varying", 2, ScalarType::Float, 4)],
            resources: vec![],
            facts: Default::default(),
        };
        let mut position = varying("position", 0, ScalarType::Float, 4);
        position.system = Some(iridia_interface::SystemValue::ClipPosition);
        interface.outputs.push(position);
        let program = iridia::Program {
            module: naga::front::wgsl::parse_str("struct Out { @location(0) @interpolate(perspective) uv: vec4<f32>, @builtin(position) position: vec4<f32> } @vertex fn main(@location(0) uv: vec2<f32>) -> Out { var out: Out; out.uv = vec4<f32>(uv.x, uv.y, uv.x + 1.0, 0.0); out.position = vec4<f32>(0.0, 0.0, 0.0, 1.0); return out; }").expect("owned fallback fixture"),
            stage: Stage::Vertex, entry: "main".into(), interface,
            bindings: vec![], conventions: iridia::Conventions::D3D,
            modes: Default::default(), notes: vec![], provenance: iridia::Provenance {
                format: iridia_interface::ShaderFormat::Glsl, target: "owned fixture".into(), lifter: "fixture".into(), lifter_version: 1, source_hash: None, instruction_count: None,
            },
        };
        let io = iridia::entry_io(&program.interface).expect("owned fixture interface");
        let result = program.module.entry_points[0]
            .function
            .result
            .as_ref()
            .expect("fixture output");
        let naga::TypeInner::Struct { members, .. } = &program.module.types[result.ty].inner else {
            panic!("fixture output struct")
        };
        assert_eq!(
            members[0].binding.as_ref(),
            Some(&io.results[0].binding),
            "fixture's WGSL interpolation must agree with the IR interface"
        );
        let entity = crate::World::new().spawn();
        let mut vertex = StageBinding::new(Stage::Vertex, entity);
        vertex.inputs.push(InputBinding {
            label: program.interface.inputs[0].label.clone(),
            role: InputRole::Stream(Attribute::TexCoord(0)),
        });
        let mut pixel = StageBinding::new(Stage::Pixel, entity);
        pixel.inputs.push(InputBinding {
            label: program.interface.outputs[0].label.clone(),
            role: InputRole::Previous,
        });
        assert_eq!(
            bind_fallback_uv_copies(&mut pixel, &program, &vertex, base_coordinates()),
            1
        );
        let InputRole::PreviousOr { fallback, .. } = &pixel.inputs[0].role else {
            panic!("copy description")
        };
        assert_eq!(
            fallback
                .coordinate_copies
                .as_ref()
                .expect("owned fallback fixture")
                .lanes,
            [Some(0), Some(1), None, None]
        );
        vertex.inputs[0].role = InputRole::Unknown {
            why: "not stated to be UVs".into(),
        };
        pixel.inputs[0].role = InputRole::Previous;
        assert_eq!(
            bind_fallback_uv_copies(&mut pixel, &program, &vertex, base_coordinates()),
            0
        );
        assert_eq!(pixel.inputs[0].role, InputRole::Previous);
    }
}
