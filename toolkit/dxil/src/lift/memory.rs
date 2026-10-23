//! Local arrays and constant tables: `alloca`, `getelementptr`, `load` and `store`. A pointer is symbolic (a root and
//! the steps from it) and becomes an access chain where it is loaded or stored.

use std::collections::BTreeMap;

use naga::{ArraySize, Expression as E, GlobalVariable, Handle, LocalVariable, TypeInner};
use primase_shader::build::{Kind, Val};

use super::{Lift, Lv};
use crate::Result;
use crate::module::{Constant, Instruction, Op, Type, TypeId, Value, ValueId};

#[derive(Clone, Debug)]
pub(crate) enum Root {
    Local(Handle<LocalVariable>),
    Global(Handle<GlobalVariable>),
    /// dxc's no-op marker globals (`dx.nothing`): loads of them are nothing.
    Nothing,
}

/// One dynamic index: an expression of the block that made it, or a local when another block reads the pointer.
#[derive(Clone, Debug)]
pub(crate) enum Index {
    Expr(Val),
    Local(Handle<LocalVariable>),
}

#[derive(Clone, Debug)]
pub(crate) enum Step {
    Const(u32),
    Dynamic(Index),
}

#[derive(Clone, Debug)]
pub(crate) struct Ptr {
    pub root: Root,
    pub steps: Vec<Step>,
}

#[derive(Default)]
pub(crate) struct Memory {
    globals: BTreeMap<ValueId, Root>,
    /// The thread-group variables an atomic operation reaches, and the integer type their atomics have: a variable
    /// that is also read and written plainly is an atomic one, read and written by atomic loads and stores.
    atomic: BTreeMap<ValueId, naga::Scalar>,
    /// The same, by the global the lifter made.
    atomic_globals: BTreeMap<Handle<GlobalVariable>, naga::Scalar>,
    /// The one kind of scalar a global or local array holds, where it holds one: a pointer to it may be cast to a
    /// pointer to another type of the same width, and the memory is still of the kind it was made as.
    leaf_globals: BTreeMap<Handle<GlobalVariable>, Kind>,
    leaf_locals: BTreeMap<Handle<LocalVariable>, Kind>,
}

impl Lift<'_> {
    /// The kind of scalar every element of a type is, if there is only one.
    fn uniform_kind(&self, ty: TypeId) -> Option<Kind> {
        match self.m.types.get(ty)? {
            Type::Array(_, e) | Type::Vector(_, e) => self.uniform_kind(*e),
            Type::Struct { elements, .. } => {
                let mut kinds = elements.iter().map(|e| self.uniform_kind(*e));
                let first = kinds.next()??;
                kinds.all(|k| k == Some(first)).then_some(first)
            }
            _ => self.kind_of_type(ty).ok(),
        }
    }

    /// The kind of scalar the memory a pointer reaches holds, when that is known.
    fn memory_kind(&self, root: &Root) -> Option<Kind> {
        match root {
            Root::Global(g) => match self.memory.atomic_globals.get(g) {
                Some(scalar) => Some(super::resources::kind_of_atomic(*scalar)),
                None => self.memory.leaf_globals.get(g).copied(),
            },
            Root::Local(l) => self.memory.leaf_locals.get(l).copied(),
            Root::Nothing => None,
        }
    }

    /// The integer type of the atomic variable `g`, if it is one.
    pub(super) fn memory_atomic_scalar(&self, g: Handle<GlobalVariable>) -> Option<naga::Scalar> {
        self.memory.atomic_globals.get(&g).copied()
    }

    /// The module global a pointer value is made from, through getelementptrs and casts.
    fn root_global(&self, v: ValueId) -> Option<ValueId> {
        let mut v = v;
        loop {
            // a module value, or a constant of the function's own constants block
            let value = if v < self.f.first {
                self.m.values.get(v)
            } else {
                self.f.locals.get(v - self.f.first)
            };
            match value {
                Some(Value::Global { .. }) if v < self.f.first => return Some(v),
                Some(Value::Constant(Constant::Gep { base, .. })) => {
                    v = *base;
                    continue;
                }
                Some(Value::Constant(Constant::Cast { value, .. })) => {
                    v = *value;
                    continue;
                }
                Some(Value::Constant(_)) => return None,
                _ if v < self.f.first => return None,
                _ => {}
            }
            let inst = self
                .f
                .instructions
                .iter()
                .find(|i| i.result.is_some_and(|(r, _)| r == v))?;
            v = match &inst.op {
                Op::Gep { base, .. } => *base,
                Op::Cast { value, .. } => *value,
                _ => return None,
            };
        }
    }

    /// Which thread-group variables the program reaches with atomic operations, and as which integer type: signed
    /// where a signed min or max asks, unsigned otherwise.
    pub(super) fn declare_memory(&mut self) -> Result<()> {
        let mut chosen: BTreeMap<ValueId, Option<naga::Scalar>> = BTreeMap::new();
        for inst in &self.f.instructions {
            let (pointer, want) = match &inst.op {
                Op::AtomicRmw { pointer, op, .. } => (
                    *pointer,
                    match op {
                        7 | 8 => Some(naga::Scalar::I32),
                        9 | 10 => Some(naga::Scalar::U32),
                        _ => None,
                    },
                ),
                Op::CmpXchg { pointer, .. } => (*pointer, None),
                _ => continue,
            };
            let Some(root) = self.root_global(pointer) else {
                continue;
            };
            let entry = chosen.entry(root).or_insert(None);
            match (*entry, want) {
                (Some(have), Some(want)) if have != want => {
                    self.at = inst.at;
                    return self
                        .refuse("a thread-group variable with both signed and unsigned atomic min or max");
                }
                (_, Some(want)) => *entry = Some(want),
                _ => {}
            }
        }
        self.memory.atomic = chosen
            .into_iter()
            .map(|(root, scalar)| (root, scalar.unwrap_or(naga::Scalar::U32)))
            .collect();
        Ok(())
    }

    /// The naga type of an LLVM type of scalars, vectors, arrays and structures, and its size in bytes.
    fn naga_type(&mut self, ty: TypeId) -> Result<(Handle<naga::Type>, u32)> {
        let (t, size, _) = self.naga_layout(ty, None)?;
        Ok((t, size))
    }

    /// The type, size and alignment of an LLVM type in memory; `atomic` makes its 32-bit integers atomic ones.
    fn naga_layout(
        &mut self,
        ty: TypeId,
        atomic: Option<naga::Scalar>,
    ) -> Result<(Handle<naga::Type>, u32, u32)> {
        match self.m.types[ty].clone() {
            Type::Integer(32) if atomic.is_some() => {
                let scalar = atomic.expect("checked");
                Ok((self.b.ty(TypeInner::Atomic(scalar)), 4, 4))
            }
            Type::Float | Type::Integer(1) | Type::Integer(32) => {
                let kind = self.kind_of_type(ty)?;
                Ok((self.b.ty_of(kind, 1), 4, 4))
            }
            Type::Vector(n, e) => {
                let kind = self.kind_of_type(e)?;
                let n = u8::try_from(n)
                    .ok()
                    .filter(|n| (2..=4).contains(n))
                    .map_or_else(|| self.refuse("a vector of this width"), Ok)?;
                Ok((
                    self.b.ty_of(kind, n),
                    u32::from(n) * 4,
                    if n == 2 { 8 } else { 16 },
                ))
            }
            Type::Array(n, e) => {
                let (base, size, align) = self.naga_layout(e, atomic)?;
                let stride = size.next_multiple_of(align.max(if size > 8 { 16 } else { size.max(4) }));
                let n32 = u32::try_from(n).map_err(|_| crate::error(self.at, "array length"))?;
                let len = std::num::NonZeroU32::new(n32).map_or_else(|| self.refuse("an empty array"), Ok)?;
                let t = self.b.ty(TypeInner::Array {
                    base,
                    size: ArraySize::Constant(len),
                    stride,
                });
                Ok((t, stride * n32, align))
            }
            Type::Struct { elements, .. } => {
                let mut members = Vec::new();
                let mut offset = 0u32;
                let mut struct_align = 4;
                for (k, e) in elements.iter().enumerate() {
                    let (t, size, align) = self.naga_layout(*e, atomic)?;
                    offset = offset.next_multiple_of(align);
                    members.push(naga::StructMember {
                        name: Some(format!("m{k}")),
                        ty: t,
                        binding: None,
                        offset,
                    });
                    offset += size;
                    struct_align = struct_align.max(align);
                }
                if members.is_empty() {
                    return self.refuse("an empty structure");
                }
                let span = offset.next_multiple_of(struct_align);
                let t = self.b.ty(TypeInner::Struct { members, span });
                Ok((t, span, struct_align))
            }
            other => self.refuse(format!("memory of type {other:?}")),
        }
    }

    /// A constant as an initialiser expression of the module.
    fn init_expr(&mut self, c: &Constant, ty: TypeId) -> Result<Handle<E>> {
        let (naga_ty, _) = self.naga_type(ty)?;
        Ok(match (c, self.m.types[ty].clone()) {
            (Constant::Integer(i), Type::Integer(1)) => {
                self.b.global_expr(E::Literal(naga::Literal::Bool(i & 1 != 0)))
            }
            (Constant::Integer(i), Type::Integer(32)) => {
                self.b.global_expr(E::Literal(naga::Literal::U32(*i as u32)))
            }
            (Constant::Float(bits), Type::Float) => self
                .b
                .global_expr(E::Literal(naga::Literal::F32(f32::from_bits(*bits as u32)))),
            (Constant::Null | Constant::Undef, _) => self.b.global_expr(E::ZeroValue(naga_ty)),
            (Constant::Aggregate(items), Type::Array(_, element)) => {
                let components = items
                    .iter()
                    .map(|&id| match &self.m.values[id] {
                        Value::Constant(c) => {
                            let c = c.clone();
                            self.init_expr(&c, element)
                        }
                        _ => self.refuse("an initialiser that is not a constant"),
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.b.global_expr(E::Compose {
                    ty: naga_ty,
                    components,
                })
            }
            (Constant::Data(words), Type::Array(_, element)) => {
                let components = words
                    .iter()
                    .map(|&w| {
                        let c = match self.m.types[element] {
                            Type::Float => Constant::Float(w),
                            _ => Constant::Integer(w as i64),
                        };
                        self.init_expr(&c, element)
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.b.global_expr(E::Compose {
                    ty: naga_ty,
                    components,
                })
            }
            (other, t) => return self.refuse(format!("an initialiser {other:?} of type {t:?}")),
        })
    }

    /// A module global read as a pointer: a constant table becomes a private global with its initial value.
    pub(crate) fn global_pointer(&mut self, v: ValueId) -> Result<Lv> {
        if let Some(root) = self.memory.globals.get(&v) {
            return Ok(Lv::Pointer(Ptr {
                root: root.clone(),
                steps: Vec::new(),
            }));
        }
        let Value::Global {
            value_ty,
            init,
            constant,
            address_space,
            ..
        } = self.m.values[v].clone()
        else {
            return self.refuse("a global that is not a global");
        };
        let name = self.m.name(v).unwrap_or("").to_owned();
        let root = if name.starts_with("dx.nothing") {
            Root::Nothing
        } else if constant && address_space != 3 && init.is_some() {
            let init_value = init.expect("checked");
            let Value::Constant(c) = self.m.values[init_value].clone() else {
                return self.refuse("a global whose initial value is not a constant");
            };
            let (ty, _) = self.naga_type(value_ty)?;
            let init = self.init_expr(&c, value_ty)?;
            let g = self.b.global(GlobalVariable {
                name: Some(if name.is_empty() { "table".into() } else { name }),
                space: naga::AddressSpace::Private,
                binding: None,
                ty,
                init: Some(init),
                memory_decorations: Default::default(),
            });
            if let Some(kind) = self.uniform_kind(value_ty) {
                self.memory.leaf_globals.insert(g, kind);
            }
            Root::Global(g)
        } else if address_space == 3 {
            // thread-group memory: zero at the start of the group in naga, undefined in D3D
            let atomic = self.memory.atomic.get(&v).copied();
            let (ty, _, _) = self.naga_layout(value_ty, atomic)?;
            let g = self.b.global(GlobalVariable {
                name: Some(if name.is_empty() { "shared".into() } else { name }),
                space: naga::AddressSpace::WorkGroup,
                binding: None,
                ty,
                init: None,
                memory_decorations: Default::default(),
            });
            if let Some(scalar) = atomic {
                self.memory.atomic_globals.insert(g, scalar);
            } else if let Some(kind) = self.uniform_kind(value_ty) {
                self.memory.leaf_globals.insert(g, kind);
            }
            Root::Global(g)
        } else {
            return self.refuse(format!("the global {name} (address space {address_space})"));
        };
        self.memory.globals.insert(v, root.clone());
        Ok(Lv::Pointer(Ptr {
            root,
            steps: Vec::new(),
        }))
    }

    /// A constant `getelementptr` of a module constant: a global and constant indices.
    pub(crate) fn constant_gep(&mut self, base: ValueId, indices: &[ValueId]) -> Result<Lv> {
        let Lv::Pointer(mut p) = self.global_pointer(base)? else {
            return self.refuse("a constant getelementptr of something that is not a global");
        };
        for (n, &i) in indices.iter().enumerate() {
            let Some(k) = super::setup::const_int(self.m, self.f, i) else {
                return self.refuse("a constant getelementptr with an index that is not constant");
            };
            if n == 0 {
                if k != 0 {
                    return self.refuse("pointer arithmetic on a global");
                }
            } else {
                p.steps.push(Step::Const(
                    u32::try_from(k).map_err(|_| crate::error(self.at, "index"))?,
                ));
            }
        }
        Ok(Lv::Pointer(p))
    }

    /// Keep a pointer's dynamic indices in locals, so another block can read the pointer.
    pub(crate) fn spill_pointer(&mut self, p: &Ptr) -> Result<Ptr> {
        let mut steps = Vec::new();
        for s in &p.steps {
            steps.push(match s {
                Step::Dynamic(Index::Expr(v)) => {
                    let t = self.b.ty_of(v.kind, 1);
                    let local = self.b.local("index", t);
                    let ptr = self.b.local_ptr(local);
                    self.b.store(ptr, *v);
                    Step::Dynamic(Index::Local(local))
                }
                other => other.clone(),
            });
        }
        Ok(Ptr {
            root: p.root.clone(),
            steps,
        })
    }

    /// The access chain a pointer stands for.
    pub(super) fn pointer_expr(&mut self, p: &Ptr) -> Result<Handle<E>> {
        let mut e = match p.root {
            Root::Local(l) => self.b.local_ptr(l),
            Root::Global(g) => self.b.global_ptr(g),
            Root::Nothing => return self.refuse("a pointer to nothing"),
        };
        for s in &p.steps {
            e = match s {
                Step::Const(k) => self.b.app(E::AccessIndex { base: e, index: *k }),
                Step::Dynamic(Index::Expr(v)) => {
                    let index = self.b.bitcast_to(*v, Kind::U);
                    self.b.app(E::Access {
                        base: e,
                        index: index.expr,
                    })
                }
                Step::Dynamic(Index::Local(l)) => {
                    let lp = self.b.local_ptr(*l);
                    let index = self.b.load(lp, Kind::U, 1);
                    self.b.app(E::Access {
                        base: e,
                        index: index.expr,
                    })
                }
            };
        }
        Ok(e)
    }

    pub(super) fn memory_instruction(&mut self, inst: &Instruction) -> Result<()> {
        let result = inst.result;
        match &inst.op {
            Op::Alloca { ty } => {
                let (t, _) = self.naga_type(*ty)?;
                let local = self.b.local("array", t);
                if let Some(kind) = self.uniform_kind(*ty) {
                    self.memory.leaf_locals.insert(local, kind);
                }
                self.define_result(
                    result,
                    Lv::Pointer(Ptr {
                        root: Root::Local(local),
                        steps: Vec::new(),
                    }),
                )
            }
            Op::Gep { base, indices } => {
                let Lv::Pointer(mut p) = self.get(*base)? else {
                    return self.refuse("a getelementptr of something that is not a pointer");
                };
                for (n, &i) in indices.iter().enumerate() {
                    if n == 0 {
                        if super::setup::const_int(self.m, self.f, i) != Some(0) {
                            return self.refuse("pointer arithmetic");
                        }
                        continue;
                    }
                    p.steps.push(match super::setup::const_int(self.m, self.f, i) {
                        Some(k) => Step::Const(u32::try_from(k).map_err(|_| crate::error(self.at, "index"))?),
                        None => Step::Dynamic(Index::Expr(self.scalar(i)?)),
                    });
                }
                self.define_result(result, Lv::Pointer(p))
            }
            Op::Load { pointer } => {
                let Lv::Pointer(p) = self.get(*pointer)? else {
                    return self.refuse("a load of something that is not a pointer");
                };
                let ty = result.map(|(_, t)| t).expect("a load has a result");
                let kind = self.kind_of_type(ty)?;
                if matches!(p.root, Root::Nothing) {
                    let zero = self.b.lit_bits(kind, 0);
                    return self.define_result(result, Lv::Scalar(zero));
                }
                let e = self.pointer_expr(&p)?;
                // the memory is read as the kind it holds (an atomic variable by an atomic load), then as the type
                // the program reads it as
                let memory = self.memory_kind(&p.root).unwrap_or(kind);
                let loaded = self.b.load(e, memory, 1);
                let v = self.b.bitcast_to(loaded, kind);
                self.define_result(result, Lv::Scalar(v))
            }
            Op::Store { pointer, value } => {
                let Lv::Pointer(p) = self.get(*pointer)? else {
                    return self.refuse("a store through something that is not a pointer");
                };
                if matches!(p.root, Root::Nothing) {
                    return Ok(());
                }
                let v = self.scalar(*value)?;
                let e = self.pointer_expr(&p)?;
                // stored as the kind the memory holds
                let v = match self.memory_kind(&p.root) {
                    Some(memory) => self.b.bitcast_to(v, memory),
                    None => {
                        let pointee = if *pointer < self.f.first {
                            self.m.value_types.get(*pointer).copied()
                        } else {
                            self.f.local_types.get(*pointer - self.f.first).copied()
                        };
                        match pointee.map(|t| self.m.types[t].clone()) {
                            Some(Type::Pointer(t)) => {
                                let kind = self.kind_of_type(t)?;
                                self.b.bitcast_to(v, kind)
                            }
                            _ => v,
                        }
                    }
                };
                self.b.store(e, v);
                Ok(())
            }
            _ => unreachable!("a memory instruction"),
        }
    }
}
