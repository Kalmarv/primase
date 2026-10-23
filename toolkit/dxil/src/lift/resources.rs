//! The module's globals for the resources the program creates handles for, and the operations on them: samples,
//! loads, stores, queries, constant buffer reads.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use naga::{
    AddressSpace, ArraySize, BinaryOperator as Bo, Expression as E, GlobalVariable, Handle, ImageClass,
    ImageQuery, ResourceBinding, SampleLevel, StorageAccess, TypeInner,
};
use primase_shader::build::{Kind, Val, component};
use primase_shader::{
    Access, BufferKind, ResourceId, ResourceKind, SampleType, ScalarType, StorageTarget, TextureDim,
};

use super::setup::{binding_of, const_int, dx_opcode};
use super::{Lift, Lv};
use crate::Result;
use crate::module::{Instruction, Op, ValueId};

#[derive(Default)]
pub(super) struct Resources {
    pub bound: Vec<(ResourceId, Handle<GlobalVariable>)>,
    globals: Vec<Handle<GlobalVariable>>,
    /// Resources compared against a reference: depth images.
    depth: BTreeSet<usize>,
    /// The access the program makes of each unordered-access view: (loads, stores).
    views: BTreeMap<usize, (bool, bool)>,
    /// The table of D3D's standard sample positions, made when a program asks for one.
    sample_positions: Option<Handle<GlobalVariable>>,
}

/// D3D's standard sample positions in sixteenths of a pixel from its centre (x right, y down), for 1, 2, 4, 8 and 16
/// samples one after the other: the positions of a count start at `count - 1`.
const STANDARD_POSITIONS: [(i8, i8); 31] = [
    (0, 0),
    (4, 4),
    (-4, -4),
    (-2, -6),
    (6, -2),
    (-6, 2),
    (2, 6),
    (1, -3),
    (-1, 3),
    (5, 1),
    (-3, -5),
    (-5, 5),
    (-7, -1),
    (3, 7),
    (7, -7),
    (1, 1),
    (-1, -3),
    (-3, 2),
    (4, -1),
    (-5, -2),
    (2, 5),
    (5, 3),
    (3, -5),
    (-2, 6),
    (0, -7),
    (-4, -6),
    (-6, 4),
    (-8, 0),
    (7, -4),
    (6, 7),
    (-7, -8),
];

fn dim_of(d: TextureDim) -> naga::ImageDimension {
    match d {
        TextureDim::D1 => naga::ImageDimension::D1,
        TextureDim::D2 => naga::ImageDimension::D2,
        TextureDim::D3 => naga::ImageDimension::D3,
        TextureDim::Cube => naga::ImageDimension::Cube,
    }
}

fn kind_of_sample(s: SampleType) -> Kind {
    match s {
        SampleType::Sint => Kind::I,
        SampleType::Uint => Kind::U,
        _ => Kind::F,
    }
}

impl Lift<'_> {
    /// The interface resource a handle value names: a `CreateHandle` call with constant arguments.
    fn handle_of(&self, v: ValueId) -> Option<usize> {
        let inst = self
            .f
            .instructions
            .iter()
            .find(|i| i.result.is_some_and(|(r, _)| r == v))?;
        let Op::Call { function, args } = &inst.op else {
            return None;
        };
        match dx_opcode(self.m, self.f, *function, args)? {
            57 => {
                let class = const_int(self.m, self.f, args[1])?;
                let id = const_int(self.m, self.f, args[2])?;
                self.setup.resource(class as u8, id as u32)
            }
            217 => {
                let (class, lower, space) = binding_of(self.m, self.f, args[1])?;
                self.setup.resource_by_binding(class, lower, space)
            }
            // a handle annotated with its properties is the same handle
            216 => self.handle_of(args[1]),
            _ => None,
        }
    }

    /// Scan the program for how it uses each resource, before the globals are made.
    fn scan_resources(&mut self) {
        let mut depth = BTreeSet::new();
        let mut views: BTreeMap<usize, (bool, bool)> = BTreeMap::new();
        for inst in &self.f.instructions {
            let Op::Call { function, args } = &inst.op else {
                continue;
            };
            let Some(code) = dx_opcode(self.m, self.f, *function, args) else {
                continue;
            };
            let handle = args.get(1).and_then(|&a| self.handle_of(a));
            let Some(r) = handle else { continue };
            match code {
                64 | 65 | 74 => {
                    depth.insert(r);
                }
                66 | 68 => {
                    if matches!(self.setup.kind(r), ResourceKind::Storage { .. }) {
                        views.entry(r).or_default().0 = true;
                    }
                }
                67 | 69 => views.entry(r).or_default().1 = true,
                _ => {}
            }
        }
        self.resources.depth = depth;
        self.resources.views = views;
    }

    pub(super) fn declare_resources(&mut self) -> Result<()> {
        self.scan_resources();
        let u4 = self.b.ty_of(Kind::U, 4);
        let u1 = self.b.ty_of(Kind::U, 1);
        for i in 0..self.setup.interface.resources.len() {
            let range = self.setup.ranges[i];
            let resource = self.setup.interface.resources[i].clone();
            let depth = self.resources.depth.contains(&i);
            let (name, space, group, ty) = match &resource.kind {
                ResourceKind::ConstantBuffer { layout } => {
                    let rows = layout.size.div_ceil(16).max(1);
                    let ty = self.b.ty(TypeInner::Array {
                        base: u4,
                        size: ArraySize::Constant(NonZeroU32::new(rows).expect("at least one row")),
                        stride: 16,
                    });
                    (format!("cb{}", range.lower), AddressSpace::Uniform, 0, ty)
                }
                ResourceKind::Texture {
                    dim,
                    arrayed,
                    multisampled,
                    sample,
                } => {
                    let class = if depth {
                        ImageClass::Depth { multi: *multisampled }
                    } else {
                        ImageClass::Sampled {
                            kind: kind_of_sample(*sample).kind(),
                            multi: *multisampled,
                        }
                    };
                    let ty = self.b.ty(TypeInner::Image {
                        dim: dim_of(*dim),
                        arrayed: *arrayed,
                        class,
                    });
                    if depth {
                        self.setup.interface.resources[i].kind = ResourceKind::Texture {
                            dim: *dim,
                            arrayed: *arrayed,
                            multisampled: *multisampled,
                            sample: SampleType::Depth,
                        };
                    }
                    (format!("t{}", range.lower), AddressSpace::Handle, 1, ty)
                }
                ResourceKind::Sampler { comparison } => {
                    let ty = self.b.ty(TypeInner::Sampler {
                        comparison: *comparison,
                    });
                    (format!("s{}", range.lower), AddressSpace::Handle, 2, ty)
                }
                ResourceKind::Buffer(kind) => {
                    let (base, stride) = match kind {
                        BufferKind::Typed(_) => (u4, 16),
                        _ => (u1, 4),
                    };
                    let ty = self.b.ty(TypeInner::Array {
                        base,
                        size: ArraySize::Dynamic,
                        stride,
                    });
                    // a typed buffer is carried as four 32-bit lanes per element, whatever the view's own width
                    if let BufferKind::Typed(v) = kind {
                        self.setup.interface.resources[i].kind =
                            ResourceKind::Buffer(BufferKind::Typed(primase_shader::Value { width: 4, ..*v }));
                    }
                    (
                        format!("sb{}", range.lower),
                        AddressSpace::Storage {
                            access: StorageAccess::LOAD,
                        },
                        1,
                        ty,
                    )
                }
                ResourceKind::Storage {
                    target: StorageTarget::Image { dim, arrayed, sample },
                    ..
                } => {
                    let (load, store) = self.resources.views.get(&i).copied().unwrap_or((true, false));
                    let mut access = StorageAccess::empty();
                    access.set(StorageAccess::LOAD, load || !store);
                    access.set(StorageAccess::STORE, store);
                    let format = match kind_of_sample(*sample) {
                        Kind::I => naga::StorageFormat::Rgba32Sint,
                        Kind::U => naga::StorageFormat::Rgba32Uint,
                        _ => naga::StorageFormat::Rgba32Float,
                    };
                    let ty = self.b.ty(TypeInner::Image {
                        dim: dim_of(*dim),
                        arrayed: *arrayed,
                        class: ImageClass::Storage { format, access },
                    });
                    self.setup.interface.resources[i].kind = ResourceKind::Storage {
                        target: StorageTarget::Image {
                            dim: *dim,
                            arrayed: *arrayed,
                            sample: *sample,
                        },
                        access: match (load, store) {
                            (true, true) => Access::ReadWrite,
                            (false, true) => Access::Write,
                            _ => Access::Read,
                        },
                    };
                    self.notes.insert(
                        "a typed unordered-access view is carried as a storage image of four 32-bit lanes: its format \
                         is the binding's"
                            .into(),
                    );
                    (format!("u{}", range.lower), AddressSpace::Handle, 3, ty)
                }
                ResourceKind::Storage { .. } => {
                    return self.refuse(format!(
                        "{}: unordered-access buffer views are not lifted",
                        resource.provenance.name
                    ));
                }
            };
            // a resource array is a binding array of the resource's own type, dynamically sized when unbounded
            let ty = if resource.is_array() {
                self.notes.insert(
                    "a resource array is a binding array: it needs TEXTURE_AND_SAMPLER_BINDING_ARRAY (with non-uniform \
                     indexing), and an unbounded one is sized by the bound data"
                        .into(),
                );
                let size = match NonZeroU32::new(resource.count) {
                    Some(n) if !resource.is_unbounded() => ArraySize::Constant(n),
                    _ => ArraySize::Dynamic,
                };
                self.b.ty(TypeInner::BindingArray { base: ty, size })
            } else {
                ty
            };
            let binding = if range.space == 0 {
                range.lower
            } else {
                (range.space << 16) | range.lower
            };
            let g = self.b.global(GlobalVariable {
                name: Some(name),
                space,
                binding: Some(ResourceBinding { group, binding }),
                ty,
                init: None,
                memory_decorations: Default::default(),
            });
            self.resources.bound.push((resource.id, g));
            self.resources.globals.push(g);
        }
        Ok(())
    }

    fn texture_shape(&self, resource: usize) -> Result<(TextureDim, bool, bool, Kind, bool)> {
        match self.setup.kind(resource) {
            ResourceKind::Texture {
                dim,
                arrayed,
                multisampled,
                sample,
            } => Ok((
                *dim,
                *arrayed,
                *multisampled,
                kind_of_sample(*sample),
                *sample == SampleType::Depth,
            )),
            ResourceKind::Storage {
                target: StorageTarget::Image { dim, arrayed, sample },
                ..
            } => Ok((*dim, *arrayed, false, kind_of_sample(*sample), false)),
            _ => self.refuse("a handle that is not a texture"),
        }
    }

    /// The interface resource a handle of `class` names, where the resource is one binding.
    fn handle(&mut self, v: ValueId, class: u8) -> Result<usize> {
        match self.get(v)? {
            Lv::Handle {
                class: c,
                resource,
                element: None,
            } if c == class => Ok(resource),
            Lv::Handle { element: Some(_), .. } => {
                self.refuse("an element of a resource array where only a texture or a sampler is read")
            }
            _ => self.refuse(format!("a handle of class {class} was expected")),
        }
    }

    /// The interface resource a handle of `class` names and the expression of the image or sampler it reads: the
    /// global, or the element of its binding array the program's index picks.
    fn handle_expr(&mut self, v: ValueId, class: u8) -> Result<(usize, Handle<E>)> {
        let Lv::Handle {
            class: c,
            resource,
            element,
        } = self.get(v)?
        else {
            return self.refuse(format!("a handle of class {class} was expected"));
        };
        if c != class {
            return self.refuse(format!("a handle of class {class} was expected"));
        }
        let global = self.b.global_ptr(self.global_of(resource));
        let expr = match element {
            None => global,
            Some(index) => self.b.app(E::Access {
                base: global,
                index: index.expr,
            }),
        };
        Ok((resource, expr))
    }

    /// The element of an array a handle's index picks: the index less the range's first register.
    fn element_of(&mut self, resource: usize, index: ValueId, constant: Option<i64>) -> Result<Option<Val>> {
        let count = self.setup.interface.resources[resource].count;
        let lower = self.setup.ranges[resource].lower;
        if count == 1 {
            return match constant {
                Some(i) if i == i64::from(lower) => Ok(None),
                Some(_) => self.refuse("a handle at an index other than its range's register"),
                None => self.refuse("a handle of a computed index into a range of one"),
            };
        }
        let element = match constant {
            Some(i) => match u32::try_from(i).ok().and_then(|i| i.checked_sub(lower)) {
                Some(e) => self.b.lit_u(e),
                None => return self.refuse("a handle at an index below its range"),
            },
            None => {
                let at = self.uint(index)?;
                let lower = self.b.lit_u(lower);
                self.b.binary(Bo::Subtract, at, lower)
            }
        };
        Ok(Some(element))
    }

    /// `CreateHandle(class, range id, index, non-uniform)`: the interface resource of a constant range, at an element
    /// when the range is an array.
    pub(super) fn create_handle(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let konst = |lift: &Self, n: usize| const_int(lift.m, lift.f, args[n]);
        let (Some(class), Some(id)) = (konst(self, 1), konst(self, 2)) else {
            return self.refuse("a handle of a computed range");
        };
        let Some(resource) = self.setup.resource(class as u8, id as u32) else {
            return self.refuse("a handle of an undeclared range");
        };
        let constant = konst(self, 3);
        let element = self.element_of(resource, args[3], constant)?;
        self.define_result(
            inst.result,
            Lv::Handle {
                class: class as u8,
                resource,
                element,
            },
        )
    }

    /// `CreateHandleFromBinding(binding, index, non-uniform)` (Shader Model 6.6): the resource of a constant binding,
    /// at an element when the binding is an array.
    pub(super) fn create_handle_from_binding(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let Some((class, lower, space)) = binding_of(self.m, self.f, args[1]) else {
            return self.refuse("a handle of a binding that is not constant");
        };
        let Some(resource) = self.setup.resource_by_binding(class, lower, space) else {
            return self.refuse("a handle of an undeclared binding");
        };
        let constant = const_int(self.m, self.f, args[2]);
        let element = self.element_of(resource, args[2], constant)?;
        self.define_result(
            inst.result,
            Lv::Handle {
                class,
                resource,
                element,
            },
        )
    }

    /// `AnnotateHandle(handle, properties)`: the handle itself; the properties repeat what the resource declares.
    pub(super) fn annotate_handle(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let handle = self.get(args[1])?;
        if !matches!(handle, Lv::Handle { .. }) {
            return self.refuse("an annotation of something that is not a handle");
        }
        self.define_result(inst.result, handle)
    }

    fn global_of(&self, resource: usize) -> Handle<GlobalVariable> {
        self.resources.globals[resource]
    }

    /// `CBufferLoadLegacy(handle, row)`: a row of four 32-bit lanes as the call's overload type.
    pub(super) fn cbuffer_load(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let resource = self.handle(args[1], 2)?;
        let g = self.global_of(resource);
        let rows = match self.setup.kind(resource) {
            ResourceKind::ConstantBuffer { layout } => layout.size.div_ceil(16).max(1),
            _ => return self.refuse("a constant load from a non-buffer"),
        };
        // a row known to be inside the buffer needs no guard
        let lanes = match const_int(self.m, self.f, args[2]) {
            Some(r) if (0..i64::from(rows)).contains(&r) => {
                let pointer = self.b.global_ptr(g);
                let element = self.b.app(E::AccessIndex {
                    base: pointer,
                    index: r as u32,
                });
                self.b.load(element, Kind::U, 4)
            }
            _ => {
                let row = self.uint(args[2])?;
                self.guarded_row(g, row, rows)
            }
        };
        let ty = inst.result.map(|(_, t)| t).expect("a call has a result");
        let crate::module::Type::Struct { elements, .. } = self.m.types[ty].clone() else {
            return self.refuse("a constant load whose result is not a struct");
        };
        if elements.len() != 4 {
            return self.refuse(
                "a constant load of other than four 32-bit lanes (16-bit and 64-bit rows are not lifted)",
            );
        }
        let mut out = Vec::new();
        for (lane, &e) in elements.iter().enumerate() {
            let kind = self.kind_of_type(e)?;
            let x = self.b.lane(lanes, lane as u8);
            out.push(Lv::Scalar(self.b.bitcast_to(x, kind)));
        }
        self.define_result(inst.result, Lv::Agg(out))
    }

    /// Row `row` of a `vec4<u32>` array global, zero out of range as D3D reads.
    fn guarded_row(&mut self, g: Handle<GlobalVariable>, row: Val, rows: u32) -> Val {
        let pointer = self.b.global_ptr(g);
        let length = self.b.lit_u(rows);
        let inside = self.b.binary(Bo::Less, row, length);
        let zero = self.b.lit_u(0);
        let safe = self.b.select(inside, row, zero);
        let element = self.b.app(E::Access {
            base: pointer,
            index: safe.expr,
        });
        let value = self.b.load(element, Kind::U, 4);
        let none = {
            let z = self.b.lit_u(0);
            self.b.splat(z, 4)
        };
        let _ = pointer;
        self.b.select(inside, value, none)
    }

    fn coordinates(
        &mut self,
        args: &[ValueId],
        first: usize,
        dim: TextureDim,
        arrayed: bool,
        kind: Kind,
    ) -> Result<(Val, Option<Handle<E>>)> {
        let n = match dim {
            TextureDim::D1 => 1,
            TextureDim::D2 => 2,
            _ => 3,
        };
        let mut parts = Vec::new();
        for k in 0..n {
            let v = self.scalar(args[first + k])?;
            parts.push(self.b.bitcast_to(v, kind));
        }
        let coordinate = self.b.compose(&parts);
        let layer = if arrayed {
            let v = self.scalar(args[first + n])?;
            Some(self.b.bitcast_to(v, kind))
        } else {
            None
        };
        let array_index = match layer {
            Some(l) if kind == Kind::F => {
                let rounded = self.b.math(naga::MathFunction::Round, &[l]);
                let zero = self.b.lit_f(0.0);
                let clamped = self.b.d3d_min_max(naga::MathFunction::Max, rounded, zero);
                Some(self.b.convert(clamped, Kind::U).expr)
            }
            Some(l) => Some(l.expr),
            None => None,
        };
        Ok((coordinate, array_index))
    }

    fn sample_offset(
        &mut self,
        args: &[ValueId],
        first: usize,
        dim: TextureDim,
    ) -> Result<Option<Handle<E>>> {
        let n = match dim {
            TextureDim::D1 => 1,
            TextureDim::D2 => 2,
            TextureDim::D3 => 3,
            TextureDim::Cube => return Ok(None),
        };
        let mut values = Vec::new();
        for k in 0..n {
            match const_int(self.m, self.f, args[first + k]) {
                Some(v) => values.push(v as i32),
                None if self.is_undef(args[first + k]) => values.push(0),
                None => return self.refuse("a sample offset that is not constant"),
            }
        }
        if values.iter().all(|&v| v == 0) {
            return Ok(None);
        }
        let parts: Vec<_> = values
            .iter()
            .map(|&v| self.b.app(E::Literal(naga::Literal::I32(v))))
            .collect();
        Ok(Some(if n == 1 {
            parts[0]
        } else {
            let ty = self.b.ty_of(Kind::I, n as u8);
            self.b.app(E::Compose {
                ty,
                components: parts,
            })
        }))
    }

    fn is_undef(&self, v: ValueId) -> bool {
        let value = if v < self.f.first {
            self.m.values.get(v)
        } else {
            self.f.locals.get(v - self.f.first)
        };
        matches!(
            value,
            Some(crate::module::Value::Constant(crate::module::Constant::Undef))
        )
    }

    /// The sample operations: `Sample`, `SampleBias`, `SampleLevel`, `SampleGrad`, `SampleCmp`,
    /// `SampleCmpLevelZero`, `TextureGather`, `TextureGatherCmp`.
    pub(super) fn sample(&mut self, inst: &Instruction, code: u64, args: &[ValueId]) -> Result<()> {
        let (resource, image) = self.handle_expr(args[1], 0)?;
        let (_, smp) = self.handle_expr(args[2], 3)?;
        let (dim, arrayed, multi, _, depth) = self.texture_shape(resource)?;
        if multi {
            return self.refuse("a sample of a multisampled texture");
        }
        let (coordinate, array_index) = self.coordinates(args, 3, dim, arrayed, Kind::F)?;
        let gather_offsets = matches!(code, 73 | 74);
        let offset = self.sample_offset(args, 7, dim)?;
        if gather_offsets && offset.is_some() && matches!(dim, TextureDim::D3) {
            return self.refuse("a gather of a 3D texture");
        }
        let expect_undef = |lift: &Self, v: ValueId| {
            if lift.is_undef(v) || const_int(lift.m, lift.f, v) == Some(0) {
                Ok(())
            } else {
                lift.refuse("a LOD clamp")
            }
        };
        let (gather, depth_ref, level) = match code {
            60 => {
                expect_undef(self, args[10])?;
                (None, None, SampleLevel::Auto)
            }
            61 => {
                expect_undef(self, args[11])?;
                let bias = self.float(args[10])?;
                (None, None, SampleLevel::Bias(bias.expr))
            }
            62 => {
                let lod = self.float(args[10])?;
                (None, None, SampleLevel::Exact(lod.expr))
            }
            63 => {
                expect_undef(self, args[16])?;
                let n = match dim {
                    TextureDim::D1 => 1,
                    TextureDim::D2 => 2,
                    _ => 3,
                };
                let mut x = Vec::new();
                let mut y = Vec::new();
                for k in 0..n {
                    x.push(self.float(args[10 + k])?);
                    y.push(self.float(args[13 + k])?);
                }
                let (x, y) = (self.b.compose(&x), self.b.compose(&y));
                (None, None, SampleLevel::Gradient { x: x.expr, y: y.expr })
            }
            64 => {
                expect_undef(self, args[11])?;
                let reference = self.float(args[10])?;
                (None, Some(reference.expr), SampleLevel::Auto)
            }
            65 => {
                let reference = self.float(args[10])?;
                (None, Some(reference.expr), SampleLevel::Zero)
            }
            73 | 74 => {
                let channel = const_int(self.m, self.f, args[9])
                    .map_or_else(|| self.refuse("a gather channel that is not constant"), Ok)?;
                let reference = if code == 74 {
                    Some(self.float(args[10])?.expr)
                } else {
                    None
                };
                (Some(component(channel as u8)), reference, SampleLevel::Zero)
            }
            other => return self.refuse(format!("sample operation {other}")),
        };
        let expr = self.b.app(E::ImageSample {
            image,
            sampler: smp,
            gather,
            coordinate: coordinate.expr,
            array_index,
            offset,
            level,
            depth_ref,
            clamp_to_edge: false,
        });
        let mut r = Val {
            expr,
            kind: Kind::F,
            width: if depth && gather.is_none() { 1 } else { 4 },
        };
        if r.width == 1 {
            // a depth sample is one float; D3D's float view returns it in x and (0, 0, 1) in the others, a comparison
            // in all four
            let (zero, one) = (self.b.lit_f(0.0), self.b.lit_f(1.0));
            r = if depth_ref.is_some() {
                self.b.splat(r, 4)
            } else {
                self.b.compose(&[r, zero, zero, one])
            };
        }
        let lv = self.resret(r);
        self.define_result(inst.result, lv)
    }

    /// A four-lane result as the `ResRet` aggregate: four lanes and the status word (0, every access mapped).
    fn resret(&mut self, v: Val) -> Lv {
        let mut items = Vec::new();
        for lane in 0..4 {
            items.push(Lv::Scalar(self.b.lane(v, lane)));
        }
        items.push(Lv::Scalar(self.b.lit_u(0)));
        Lv::Agg(items)
    }

    /// `TextureLoad(handle, mip or sample, x, y, z, offsets)`.
    pub(super) fn texture_load(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let class = match self.get(args[1])? {
            Lv::Handle { class, .. } => class,
            _ => return self.refuse("a load through something that is not a handle"),
        };
        let (resource, image) = self.handle_expr(args[1], class)?;
        let (dim, arrayed, multi, kind, depth) = self.texture_shape(resource)?;
        let (coordinate, array_index) = self.coordinates(args, 3, dim, arrayed, Kind::I)?;
        let mut coordinate = coordinate;
        // a load's constant texel offset moves the texel it reads
        if let Some(offset) = self.sample_offset(args, 6, dim)? {
            let offset = Val {
                expr: offset,
                kind: Kind::I,
                width: coordinate.width,
            };
            coordinate = self.b.binary(Bo::Add, coordinate, offset);
        }
        let storage = class == 1;
        let (level, sample) = if storage {
            (None, None)
        } else if multi {
            (None, Some(self.uint(args[2])?.expr))
        } else {
            let mip = self.uint(args[2])?;
            let mip = self.b.bitcast(mip, Kind::I);
            (Some(mip.expr), None)
        };
        let sample = sample.map(|s| {
            let v = Val {
                expr: s,
                kind: Kind::U,
                width: 1,
            };
            self.b.bitcast(v, Kind::I).expr
        });
        if storage {
            // a view takes unsigned addresses
            coordinate = self.b.bitcast(coordinate, Kind::U);
        }
        let array_index = array_index.map(|a| {
            if storage {
                let v = Val {
                    expr: a,
                    kind: Kind::I,
                    width: 1,
                };
                self.b.bitcast(v, Kind::U).expr
            } else {
                a
            }
        });
        let expr = self.b.app(E::ImageLoad {
            image,
            coordinate: coordinate.expr,
            array_index,
            sample,
            level,
        });
        let mut r = Val {
            expr,
            kind: if depth { Kind::F } else { kind },
            width: if depth { 1 } else { 4 },
        };
        if depth {
            let (zero, one) = (self.b.lit_f(0.0), self.b.lit_f(1.0));
            r = self.b.compose(&[r, zero, zero, one]);
        }
        let ty = inst.result.map(|(_, t)| t).expect("a call has a result");
        let _ = ty;
        let lv = self.resret(r);
        self.define_result(inst.result, lv)
    }

    /// `TextureStore(uav, x, y, z, v0..v3, mask)`.
    pub(super) fn texture_store(&mut self, args: &[ValueId]) -> Result<()> {
        let resource = self.handle(args[1], 1)?;
        let (dim, arrayed, _, kind, _) = self.texture_shape(resource)?;
        if const_int(self.m, self.f, args[9]) != Some(15) {
            return self.refuse("a store through a view with a partial write mask");
        }
        let (coordinate, array_index) = self.coordinates(args, 2, dim, arrayed, Kind::U)?;
        let mut lanes = Vec::new();
        for k in 0..4 {
            let v = self.scalar(args[5 + k])?;
            lanes.push(self.b.bitcast_to(v, kind));
        }
        let value = self.b.compose(&lanes);
        let image = self.b.global_ptr(self.global_of(resource));
        self.b.push(naga::Statement::ImageStore {
            image,
            coordinate: coordinate.expr,
            array_index,
            value: value.expr,
        });
        Ok(())
    }

    /// `GetDimensions(handle, mip)`: width, height, depth or layers, and the number of levels.
    pub(super) fn get_dimensions(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let class = match self.get(args[1])? {
            Lv::Handle { class, .. } => class,
            _ => return self.refuse("dimensions of something that is not a handle"),
        };
        let (resource, g) = self.handle_expr(args[1], class)?;
        let (dim, arrayed, multi, _, _) = self.texture_shape(resource)?;
        let mip = self.uint(args[2])?;
        let dims = match dim {
            TextureDim::D1 => 1,
            TextureDim::D2 | TextureDim::Cube => 2,
            TextureDim::D3 => 3,
        };
        let size = Val {
            expr: self.b.app(E::ImageQuery {
                image: g,
                query: ImageQuery::Size {
                    level: (!multi && class == 0).then_some(mip.expr),
                },
            }),
            kind: Kind::U,
            width: dims,
        };
        let levels = if multi || class != 0 {
            self.b.lit_u(1)
        } else {
            Val {
                expr: self.b.app(E::ImageQuery {
                    image: g,
                    query: ImageQuery::NumLevels,
                }),
                kind: Kind::U,
                width: 1,
            }
        };
        let zero = self.b.lit_u(0);
        let mut parts: Vec<Val> = (0..dims).map(|i| self.b.lane(size, i)).collect();
        if arrayed {
            parts.push(Val {
                expr: self.b.app(E::ImageQuery {
                    image: g,
                    query: ImageQuery::NumLayers,
                }),
                kind: Kind::U,
                width: 1,
            });
        }
        while parts.len() < 3 {
            parts.push(zero);
        }
        parts.truncate(3);
        parts.push(levels);
        self.define_result(inst.result, Lv::Agg(parts.into_iter().map(Lv::Scalar).collect()))
    }

    /// `Texture2DMSGetSamplePosition(handle, sample)`: the sample's offset from the pixel's centre in pixels, as D3D's
    /// standard pattern for the texture's sample count gives it.
    pub(super) fn sample_position(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let (resource, image) = self.handle_expr(args[1], 0)?;
        let (_, _, multi, _, _) = self.texture_shape(resource)?;
        if !multi {
            return self.refuse("a sample position of a texture that is not multisampled");
        }
        self.notes.insert(
            "a sample position is D3D's standard pattern for the texture's sample count (1, 2, 4, 8 or 16)"
                .into(),
        );
        let table = match self.resources.sample_positions {
            Some(g) => g,
            None => {
                let v2 = self.b.ty_of(Kind::F, 2);
                let ty = self.b.ty(TypeInner::Array {
                    base: v2,
                    size: ArraySize::Constant(NonZeroU32::new(31).expect("31 is not 0")),
                    stride: 8,
                });
                let mut rows = Vec::new();
                for (x, y) in STANDARD_POSITIONS {
                    let (x, y) = (f32::from(x) / 16.0, f32::from(y) / 16.0);
                    let x = self.b.global_expr(E::Literal(naga::Literal::F32(x)));
                    let y = self.b.global_expr(E::Literal(naga::Literal::F32(y)));
                    rows.push(self.b.global_expr(E::Compose {
                        ty: v2,
                        components: vec![x, y],
                    }));
                }
                let init = self.b.global_expr(E::Compose { ty, components: rows });
                let g = self.b.global(GlobalVariable {
                    name: Some("sample_positions".into()),
                    space: AddressSpace::Private,
                    binding: None,
                    ty,
                    init: Some(init),
                    memory_decorations: Default::default(),
                });
                self.resources.sample_positions = Some(g);
                g
            }
        };
        // the table row of sample `s` of `n`: n - 1 + (s & (n - 1)), kept inside the table
        let count = Val {
            expr: self.b.app(E::ImageQuery {
                image,
                query: ImageQuery::NumSamples,
            }),
            kind: Kind::U,
            width: 1,
        };
        let one = self.b.lit_u(1);
        let below = self.b.binary(Bo::Subtract, count, one);
        let sample = self.uint(args[2])?;
        let within = self.b.binary(Bo::And, sample, below);
        let row = self.b.binary(Bo::Add, below, within);
        let last = self.b.lit_u(30);
        let row = self.b.math(naga::MathFunction::Min, &[row, last]);
        let base = self.b.global_ptr(table);
        let element = self.b.app(E::Access {
            base,
            index: row.expr,
        });
        let position = self.b.load(element, Kind::F, 2);
        let (x, y) = (self.b.lane(position, 0), self.b.lane(position, 1));
        self.define_result(inst.result, Lv::Agg(vec![Lv::Scalar(x), Lv::Scalar(y)]))
    }

    /// `BufferLoad(handle, index, offset)`: four lanes of a typed, raw or structured buffer.
    pub(super) fn buffer_load(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let resource = self.handle(args[1], 0)?;
        let g = self.global_of(resource);
        let kind = match self.setup.kind(resource).clone() {
            ResourceKind::Buffer(k) => k,
            _ => return self.refuse("a buffer load from something that is not a buffer"),
        };
        let index = self.uint(args[2])?;
        let ty = inst.result.map(|(_, t)| t).expect("a call has a result");
        let crate::module::Type::Struct { elements, .. } = self.m.types[ty].clone() else {
            return self.refuse("a buffer load whose result is not a struct");
        };
        let lane_kind = self.kind_of_type(elements[0])?;
        let row = match kind {
            BufferKind::Typed(_) => {
                let pointer = self.b.global_ptr(g);
                let length = Val {
                    expr: self.b.app(E::ArrayLength(pointer)),
                    kind: Kind::U,
                    width: 1,
                };
                let inside = self.b.binary(Bo::Less, index, length);
                let zero = self.b.lit_u(0);
                let safe = self.b.select(inside, index, zero);
                let pointer = self.b.global_ptr(g);
                let element = self.b.app(E::Access {
                    base: pointer,
                    index: safe.expr,
                });
                let value = self.b.load(element, Kind::U, 4);
                let none = {
                    let z = self.b.lit_u(0);
                    self.b.splat(z, 4)
                };
                self.b.select(inside, value, none)
            }
            BufferKind::Raw | BufferKind::Structured { .. } => {
                // the word of the first lane: a raw buffer's index is a byte offset, a structured one's an element and
                // a byte offset within it
                let two = self.b.lit_u(2);
                let word = match kind {
                    BufferKind::Structured { stride } => {
                        let offset = self.uint(args[3])?;
                        let st = self.b.lit_u(stride);
                        let base = self.b.binary(Bo::Multiply, index, st);
                        let byte = self.b.binary(Bo::Add, base, offset);
                        self.b.binary(Bo::ShiftRight, byte, two)
                    }
                    _ => self.b.binary(Bo::ShiftRight, index, two),
                };
                let mut lanes = Vec::new();
                for lane in 0..4u32 {
                    let k = self.b.lit_u(lane);
                    let at = self.b.binary(Bo::Add, word, k);
                    let pointer = self.b.global_ptr(g);
                    let length = Val {
                        expr: self.b.app(E::ArrayLength(pointer)),
                        kind: Kind::U,
                        width: 1,
                    };
                    let inside = self.b.binary(Bo::Less, at, length);
                    let zero = self.b.lit_u(0);
                    let safe = self.b.select(inside, at, zero);
                    let pointer = self.b.global_ptr(g);
                    let element = self.b.app(E::Access {
                        base: pointer,
                        index: safe.expr,
                    });
                    let value = self.b.load(element, Kind::U, 1);
                    lanes.push(self.b.select(inside, value, zero));
                }
                self.b.compose(&lanes)
            }
        };
        let mut items = Vec::new();
        for lane in 0..4 {
            let x = self.b.lane(row, lane);
            items.push(Lv::Scalar(self.b.bitcast_to(x, lane_kind)));
        }
        items.push(Lv::Scalar(self.b.lit_u(0)));
        let _ = ScalarType::Float;
        self.define_result(inst.result, Lv::Agg(items))
    }
}
