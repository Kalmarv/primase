//! `core.shader`: one shader program as stored, with its interface.

use serde::Serialize;
use serde_json::Value;

use crate::json::JsonError;
use crate::world::{Component, Entity};
use iridia_interface::{Interface, ShaderFormat};

/// `core.shader`: one shader program, on its own entity. The bytecode (or source text) is kept exactly as the game
/// stored it, in its [`ShaderFormat`]; what the program exposes, its stage, varyings and resources, is in primase's
/// vocabulary ([`Interface`]). Its operations may be supplied separately as [`LiftedShader`] when lifting
/// needs container metadata; otherwise consumers lift these bytes on demand. A material's
/// `core.shader_binding` refers to the entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shader {
    pub format: ShaderFormat,
    pub bytecode: Vec<u8>,
    pub interface: Interface,
}

/// An exact lift of the accompanying [`Shader`], supplied by its reader or resolver when lifting
/// needs container metadata absent from the shader bytes. Any format may use this transport.
/// This is neither a replacement shader nor a material specialization: no constants or branches
/// may be frozen here. The original bytes and interface remain on `Shader`; derivations belong
/// to [`super::ShaderSpecialisations`] or a consumer's explicit derivation mechanism.
/// Consumers may use this instead of re-lifting.
#[derive(Clone, Debug)]
pub struct LiftedShader(pub std::sync::Arc<iridia::Program>);

impl Component for LiftedShader {
    const NAME: &'static str = "core.lifted_shader";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn verify(&self, e: Entity, w: &crate::World) -> Result<(), String> {
        let shader = w.get::<Shader>(e).ok_or("lifted shader has no original shader")?;
        if self.0.interface != shader.interface || self.0.stage != shader.interface.stage {
            return Err("lifted shader differs from the original shader interface".into());
        }
        self.0.validate().map(|_| ()).map_err(|e| format!("{e:?}"))
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}

/// What [`Shader::to_json`] shows: the bytecode by its size (it is bulk data), the rest in full.
#[derive(Serialize)]
struct View<'a> {
    format: ShaderFormat,
    bytecode_bytes: usize,
    interface: &'a Interface,
}

impl Component for Shader {
    const NAME: &'static str = "core.shader";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(&View {
            format: self.format,
            bytecode_bytes: self.bytecode.len(),
            interface: &self.interface,
        })
        .map(Some)
    }
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        if self.bytecode.is_empty() {
            return Err("a shader without bytecode".into());
        }
        self.interface.verify()
    }
    fn payload_bytes(&self) -> u64 {
        self.bytecode.len() as u64
    }
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;
    use iridia_interface::{
        BufferKind, BufferLayout, Constant, ConstantType, Label, Resource, ResourceId, ResourceKind,
        ResourceProvenance, ScalarType, Stage, StageFacts, Value as Lanes, Varying, VaryingProvenance,
    };

    fn varying(name: &str, index: u32, width: u8) -> Varying {
        Varying {
            label: Label {
                name: name.into(),
                index,
            },
            system: None,
            value: Lanes {
                scalar: ScalarType::Float,
                width,
            },
            interp: None,
            used: true,
            provenance: VaryingProvenance::default(),
        }
    }
    fn buffer(id: u32, size: u32, offset: u32) -> Resource {
        Resource {
            id: ResourceId(id),
            kind: ResourceKind::ConstantBuffer {
                layout: BufferLayout {
                    size,
                    constants: vec![Constant {
                        name: "tint".into(),
                        offset,
                        ty: ConstantType::Vector(Lanes {
                            scalar: ScalarType::Float,
                            width: 4,
                        }),
                        default: None,
                    }],
                },
            },
            count: 1,
            provenance: ResourceProvenance::default(),
        }
    }
    fn shader(interface: Interface) -> Shader {
        Shader {
            format: ShaderFormat::Dxbc,
            bytecode: vec![1, 2, 3, 4],
            interface,
        }
    }
    fn interface() -> Interface {
        Interface {
            stage: Stage::Pixel,
            inputs: vec![varying("TEXCOORD", 0, 2)],
            outputs: vec![varying("SV_Target", 0, 4)],
            resources: vec![buffer(0, 16, 0)],
            facts: iridia_interface::StageFacts::None,
        }
    }

    #[test]
    fn a_shader_shows_its_bytecode_by_size_and_its_interface_in_full() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, shader(interface()));
        let json = w.get::<Shader>(e).unwrap().to_json().unwrap().unwrap();
        assert_eq!(json["format"], "dxbc");
        assert_eq!(json["bytecode_bytes"], 4);
        assert_eq!(json["interface"]["inputs"][0]["label"]["name"], "TEXCOORD");
        assert_eq!(w.get::<Shader>(e).unwrap().payload_bytes(), 4);
    }

    #[test]
    fn a_consistent_interface_verifies_and_each_broken_rule_is_named() {
        let mut w = World::new();
        let e = w.spawn();
        assert_eq!(shader(interface()).verify(e, &w), Ok(()));
        type Break = Box<dyn Fn(&mut Interface)>;
        let broken: Vec<(Break, &str)> = vec![
            (Box::new(|i| i.inputs.push(varying("TEXCOORD", 0, 4))), "twice"),
            (Box::new(|i| i.inputs[0].value.width = 5), "width"),
            (Box::new(|i| i.resources.push(buffer(0, 16, 0))), "id 0"),
            (Box::new(|i| i.resources[0].count = 0), "no bindings"),
            // facts of another stage, or a hull program without its patch
            (
                Box::new(|i| i.facts = StageFacts::VertexWithoutPosition),
                "Pixel program with",
            ),
            (Box::new(|i| i.stage = Stage::Hull), "without its patch facts"),
            (Box::new(|i| i.resources = vec![buffer(0, 16, 4)]), "past the end"),
            (
                Box::new(|i| {
                    i.resources[0].kind = ResourceKind::Buffer(BufferKind::Structured { stride: 0 });
                }),
                "stride",
            ),
        ];
        for (break_it, expect) in broken {
            let mut i = interface();
            break_it(&mut i);
            let message = shader(i).verify(e, &w).unwrap_err();
            assert!(message.contains(expect), "{expect}: {message}");
        }
        let mut empty = shader(interface());
        empty.bytecode.clear();
        assert!(empty.verify(e, &w).is_err());
    }
}

#[cfg(test)]
mod lifted_tests {
    use super::*;
    #[test]
    fn an_exact_lift_requires_its_original_interface() {
        let program = iridia::Program {
            module: naga::front::wgsl::parse_str("@fragment fn main() {}").unwrap(),
            stage: iridia::Stage::Pixel,
            entry: "main".into(),
            interface: Interface {
                stage: iridia::Stage::Pixel,
                inputs: vec![],
                outputs: vec![],
                resources: vec![],
                facts: iridia_interface::StageFacts::None,
            },
            bindings: vec![],
            conventions: iridia::Conventions::D3D,
            modes: Default::default(),
            notes: vec![],
            provenance: iridia::Provenance {
                format: ShaderFormat::Glsl,
                target: "fixture".into(),
                lifter: "fixture".into(),
                lifter_version: 1,
                source_hash: None,
                instruction_count: None,
            },
        };
        let mut world = crate::World::new();
        let e = world.spawn();
        let lifted = LiftedShader(std::sync::Arc::new(program));
        assert!(lifted.verify(e, &world).is_err());
        world.insert(
            e,
            Shader {
                format: ShaderFormat::Glsl,
                bytecode: b"void main() {}".to_vec(),
                interface: lifted.0.interface.clone(),
            },
        );
        assert!(lifted.verify(e, &world).is_ok());
        world.get_mut::<Shader>(e).unwrap().interface.stage = iridia::Stage::Vertex;
        assert!(lifted.verify(e, &world).is_err());
    }
}
