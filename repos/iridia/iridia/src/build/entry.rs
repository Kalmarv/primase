//! The entry point's edges, from [`crate::entry_io`]: the arguments and the result structure every lifter declares the
//! same way.

use naga::{FunctionArgument, Handle, StructMember, Type, TypeInner};

use super::{Builder, Kind};
use crate::{EntryIo, FunctionSig, IoType, ScalarType};

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

/// A stage function's declared arguments and result structure.
#[derive(Debug)]
pub struct FunctionDecl {
    pub arguments: Vec<FunctionArgument>,
    pub result: Option<Handle<Type>>,
}

impl Builder {
    /// The naga type of an interface type, its size in bytes and its alignment.
    fn io_type(&mut self, ty: IoType, name: &str) -> Result<(Handle<Type>, u32, u32), String> {
        let align_of = |width: u8| match width {
            1 => 4,
            2 => 8,
            _ => 16,
        };
        Ok(match ty {
            IoType::Value(v) => (
                self.ty_of(Kind::of(v.scalar), v.width),
                u32::from(v.width) * 4,
                align_of(v.width),
            ),
            IoType::ClipDistances(n) => {
                let f = self.ty_of(Kind::F, 1);
                let len = std::num::NonZeroU32::new(n).ok_or_else(|| format!("{name}: no clip distances"))?;
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
            IoType::Array(v, n) => {
                let len =
                    std::num::NonZeroU32::new(n).ok_or_else(|| format!("{name}: an array of no elements"))?;
                let base = self.ty_of(Kind::of(v.scalar), v.width);
                // a 3-lane element takes 16 bytes in an array, as in uniform memory
                let stride = (u32::from(v.width) * 4).next_multiple_of(align_of(v.width));
                (
                    self.ty(TypeInner::Array {
                        base,
                        size: naga::ArraySize::Constant(len),
                        stride,
                    }),
                    stride * n,
                    align_of(v.width),
                )
            }
            IoType::Memory => return Err(format!("{name}: a pointer to the shader's own structure")),
        })
    }

    /// The type of an array of `n` values of `v`, as an interface array has it (see [`IoType::Array`]).
    pub fn io_array(&mut self, v: crate::Value, n: u32) -> Result<Handle<Type>, String> {
        Ok(self.io_type(IoType::Array(v, n), "an array")?.0)
    }

    /// The structure `Out` of these members, offsets by the alignment of their types.
    fn result_struct(
        &mut self,
        members: &[(String, IoType, Option<naga::Binding>)],
    ) -> Result<Handle<Type>, String> {
        let mut out = Vec::new();
        let mut offset = 0u32;
        for (name, ty, binding) in members {
            let (t, size, align) = self.io_type(*ty, name)?;
            offset = offset.div_ceil(align) * align;
            out.push(StructMember {
                name: Some(name.clone()),
                ty: t,
                binding: binding.clone(),
                offset,
            });
            offset += size;
        }
        let span = offset.div_ceil(16) * 16;
        Ok(self.module.types.insert(
            Type {
                name: Some("Out".into()),
                inner: TypeInner::Struct { members: out, span },
            },
            naga::Span::UNDEFINED,
        ))
    }

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
            let members: Vec<_> = io
                .results
                .iter()
                .map(|i| (i.name.clone(), i.ty, Some(i.binding.clone())))
                .collect();
            result = Some(self.result_struct(&members)?);
        }
        Ok(EntryDecl { arguments, result })
    }

    /// Declare a stage function's arguments and result structure (no bindings anywhere).
    pub fn declare_function(&mut self, sig: &FunctionSig) -> Result<FunctionDecl, String> {
        self.declare_function_with(sig, &[])
    }

    /// [`Self::declare_function`] for a function with pointer arguments ([`IoType::Memory`]): each is a pointer in
    /// function memory to the next of `structures`.
    pub fn declare_function_with(
        &mut self,
        sig: &FunctionSig,
        structures: &[Handle<Type>],
    ) -> Result<FunctionDecl, String> {
        let mut arguments = Vec::new();
        let mut structures = structures.iter();
        for item in &sig.arguments {
            let ty = if item.ty == IoType::Memory {
                let base = *structures
                    .next()
                    .ok_or_else(|| format!("{}: no structure to point to", item.name))?;
                self.ty(TypeInner::Pointer {
                    base,
                    space: naga::AddressSpace::Function,
                })
            } else {
                self.io_type(item.ty, &item.name)?.0
            };
            arguments.push(FunctionArgument {
                name: Some(item.name.clone()),
                ty,
                binding: None,
            });
        }
        let result = if sig.result.is_empty() {
            None
        } else {
            let members: Vec<_> = sig.result.iter().map(|i| (i.name.clone(), i.ty, None)).collect();
            Some(self.result_struct(&members)?)
        };
        Ok(FunctionDecl { arguments, result })
    }
}
