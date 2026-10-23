//! A mesh program's outputs as naga states them: one workgroup variable holding the counts, the vertices and the
//! primitives (a structure whose members carry the `vertex_count`, `primitive_count`, `vertices` and `primitives`
//! built-ins), which the program writes and the entry point names in its mesh information.

use naga::{
    AddressSpace, ArraySize, Binding, BuiltIn, GlobalVariable, Handle, MeshOutputTopology, MeshStageInfo,
    StructMember, Type, TypeInner,
};

use super::{Builder, Kind};
use crate::{IoItem, IoType, MeshFacts, MeshTopology};

/// The output variable and where its parts are.
#[derive(Clone, Debug)]
pub struct MeshOutput {
    pub global: Handle<GlobalVariable>,
    pub vertex_type: Handle<Type>,
    pub primitive_type: Handle<Type>,
    /// The output variable's member indices.
    pub vertex_count: u32,
    pub primitive_count: u32,
    pub vertices: u32,
    pub primitives: u32,
    /// The primitive type's one member: the vertex indices.
    pub indices_member: u32,
}

impl Builder {
    /// The size and alignment of a type in bytes.
    fn type_layout(&self, ty: Handle<Type>) -> (u32, u32) {
        let inner = &self.module.types[ty].inner;
        let size = inner.size(self.module.to_ctx());
        let align = match inner {
            TypeInner::Struct { members, .. } => members
                .iter()
                .map(|m| self.type_layout(m.ty).1)
                .max()
                .unwrap_or(4),
            TypeInner::Vector { size, .. } => match size {
                naga::VectorSize::Bi => 8,
                _ => 16,
            },
            _ => 4,
        };
        (size, align)
    }

    /// Declare the output variable of a mesh program: `vertex` are the members of its vertex type (see
    /// [`crate::mesh_vertex_io`]), the primitive type holds the indices of a primitive.
    pub fn declare_mesh_output(
        &mut self,
        facts: &MeshFacts,
        vertex: &[IoItem],
    ) -> Result<MeshOutput, String> {
        let mut members = Vec::new();
        let mut offset = 0u32;
        for item in vertex {
            let IoType::Value(v) = item.ty else {
                return Err(format!("{}: a vertex output that is not a value", item.name));
            };
            let ty = self.ty_of(Kind::of(v.scalar), v.width);
            let align = match v.width {
                1 => 4,
                2 => 8,
                _ => 16,
            };
            offset = offset.div_ceil(align) * align;
            members.push(StructMember {
                name: Some(item.name.clone()),
                ty,
                binding: Some(item.binding.clone()),
                offset,
            });
            offset += u32::from(v.width) * 4;
        }
        let span = offset.div_ceil(16) * 16;
        let vertex_type = self.module.types.insert(
            Type {
                name: Some("MeshVertex".into()),
                inner: TypeInner::Struct { members, span },
            },
            naga::Span::UNDEFINED,
        );
        let (indices, width) = match facts.topology {
            MeshTopology::Triangle => (BuiltIn::TriangleIndices, 3),
            MeshTopology::Line => (BuiltIn::LineIndices, 2),
        };
        let index_type = self.ty_of(Kind::U, width);
        let primitive_type = self.module.types.insert(
            Type {
                name: Some("MeshPrimitive".into()),
                inner: TypeInner::Struct {
                    members: vec![StructMember {
                        name: Some("indices".into()),
                        ty: index_type,
                        binding: Some(Binding::BuiltIn(indices)),
                        offset: 0,
                    }],
                    span: 16,
                },
            },
            naga::Span::UNDEFINED,
        );
        let array = |b: &mut Self, element: Handle<Type>, count: u32| -> Result<Handle<Type>, String> {
            let count = std::num::NonZeroU32::new(count).ok_or("a mesh program that writes nothing")?;
            let (size, align) = b.type_layout(element);
            Ok(b.ty(TypeInner::Array {
                base: element,
                size: ArraySize::Constant(count),
                stride: size.div_ceil(align) * align,
            }))
        };
        let vertices = array(self, vertex_type, facts.max_vertices)?;
        let primitives = array(self, primitive_type, facts.max_primitives)?;
        let u = self.ty_of(Kind::U, 1);
        // the counts, then the two arrays, each at the next multiple of 16
        let vertices_at = 16;
        let primitives_at = vertices_at + self.type_layout(vertices).0.div_ceil(16) * 16;
        let span = (primitives_at + self.type_layout(primitives).0).div_ceil(16) * 16;
        let member = |name: &str, ty, built_in, offset| StructMember {
            name: Some(name.into()),
            ty,
            binding: Some(Binding::BuiltIn(built_in)),
            offset,
        };
        let output_type = self.module.types.insert(
            Type {
                name: Some("MeshOutput".into()),
                inner: TypeInner::Struct {
                    members: vec![
                        member("vertex_count", u, BuiltIn::VertexCount, 0),
                        member("primitive_count", u, BuiltIn::PrimitiveCount, 4),
                        member("vertices", vertices, BuiltIn::Vertices, vertices_at),
                        member("primitives", primitives, BuiltIn::Primitives, primitives_at),
                    ],
                    span,
                },
            },
            naga::Span::UNDEFINED,
        );
        let global = self.global(GlobalVariable {
            name: Some("mesh_output".into()),
            space: AddressSpace::WorkGroup,
            binding: None,
            ty: output_type,
            init: None,
            memory_decorations: Default::default(),
        });
        Ok(MeshOutput {
            global,
            vertex_type,
            primitive_type,
            vertex_count: 0,
            primitive_count: 1,
            vertices: 2,
            primitives: 3,
            indices_member: 0,
        })
    }

    /// Finish the module with `main` as the mesh entry point.
    pub fn finish_mesh(
        mut self,
        arguments: Vec<naga::FunctionArgument>,
        facts: &MeshFacts,
        output: &MeshOutput,
    ) -> naga::Module {
        assert_eq!(self.blocks.len(), 1, "unclosed control flow (a lifter bug)");
        let mut function = self.function;
        function.name = Some("main".to_owned());
        function.arguments = arguments;
        function.result = None;
        function.body = self.blocks.pop().expect("one block");
        self.module.entry_points.push(naga::EntryPoint {
            name: "main".to_owned(),
            stage: naga::ShaderStage::Mesh,
            early_depth_test: None,
            workgroup_size: self.workgroup_size,
            workgroup_size_overrides: None,
            function,
            mesh_info: Some(MeshStageInfo {
                topology: match facts.topology {
                    MeshTopology::Triangle => MeshOutputTopology::Triangles,
                    MeshTopology::Line => MeshOutputTopology::Lines,
                },
                max_vertices: facts.max_vertices,
                max_vertices_override: None,
                max_primitives: facts.max_primitives,
                max_primitives_override: None,
                vertex_output_type: output.vertex_type,
                primitive_output_type: output.primitive_type,
                output_variable: output.global,
            }),
            task_payload: None,
            incoming_ray_payload: None,
        });
        self.module
    }
}
