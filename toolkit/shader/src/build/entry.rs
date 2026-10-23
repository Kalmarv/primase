//! The entry point's edges, from [`crate::entry_io`]: the arguments and the result structure every lifter declares the
//! same way.

use naga::{FunctionArgument, Handle, StructMember, Type, TypeInner};

use super::{Builder, Kind};
use crate::{EntryIo, IoType, ScalarType};

impl Kind {
    /// The kind a scalar type is carried as: a bool is a bool, integers and floats their own.
    pub fn of(scalar: ScalarType) -> Kind {
        match scalar {
            ScalarType::Bool => Kind::B,
            ScalarType::Sint => Kind::I,
            ScalarType::Uint => Kind::U,
            _ => Kind::F,
        }
    }
}

/// The entry point's declared arguments and result structure.
#[derive(Debug)]
pub struct EntryDecl {
    pub arguments: Vec<FunctionArgument>,
    /// The structure `Out`, none when the program has no outputs.
    pub result: Option<Handle<Type>>,
}

impl Builder {
    /// Declare the arguments (one per [`EntryIo::arguments`]) and the result structure `Out` (one member per
    /// [`EntryIo::results`]) with their bindings, member offsets by the alignment of their types. An argument of an
    /// array type is not carried by any stage and is an `Err`.
    pub fn declare_entry(&mut self, io: &EntryIo) -> Result<EntryDecl, String> {
        let mut arguments = Vec::new();
        for item in &io.arguments {
            let IoType::Value(v) = item.ty else {
                return Err(format!("{}: an array-typed input", item.name));
            };
            let ty = self.ty_of(Kind::of(v.scalar), v.width);
            arguments.push(FunctionArgument {
                name: Some(item.name.clone()),
                ty,
                binding: Some(item.binding.clone()),
            });
        }
        let mut result = None;
        if !io.results.is_empty() {
            let mut members = Vec::new();
            let mut offset = 0u32;
            for item in &io.results {
                let (ty, size, align) = match item.ty {
                    IoType::Value(v) => (
                        self.ty_of(Kind::of(v.scalar), v.width),
                        u32::from(v.width) * 4,
                        match v.width {
                            1 => 4,
                            2 => 8,
                            _ => 16,
                        },
                    ),
                    IoType::ClipDistances(n) => {
                        let f = self.ty_of(Kind::F, 1);
                        let len = std::num::NonZeroU32::new(n)
                            .ok_or_else(|| format!("{}: no clip distances", item.name))?;
                        (
                            self.ty(TypeInner::Array {
                                base: f,
                                size: naga::ArraySize::Constant(len),
                                stride: 4,
                            }),
                            n * 4,
                            4,
                        )
                    }
                };
                offset = offset.div_ceil(align) * align;
                members.push(StructMember {
                    name: Some(item.name.clone()),
                    ty,
                    binding: Some(item.binding.clone()),
                    offset,
                });
                offset += size;
            }
            let span = offset.div_ceil(16) * 16;
            result = Some(self.module.types.insert(
                Type {
                    name: Some("Out".into()),
                    inner: TypeInner::Struct { members, span },
                },
                naga::Span::UNDEFINED,
            ));
        }
        Ok(EntryDecl { arguments, result })
    }
}
