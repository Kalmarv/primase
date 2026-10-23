//! The module's globals for the declared resources, and the instructions that read them (samples, loads, sizes).

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use naga::{
    AddressSpace, ArraySize, BinaryOperator as Bo, Expression as E, GlobalVariable, Handle, ImageClass,
    ImageQuery, ResourceBinding, SampleLevel, StorageAccess, TypeInner,
};
use primase_shader::ResourceId;
use primase_shader::build::{Kind, Val, component};

use super::Lift;
use super::setup::{Declaration, Texture, View};
use crate::Result;
use crate::decode::{Instruction, Operand, Selection};

/// Where a buffer's elements are addressed in words.
#[derive(Clone, Copy)]
pub(super) enum Buffer {
    /// Four lanes per element.
    Typed,
    /// Words; a structured element takes `stride / 4` of them.
    Words { stride: u32 },
}

#[derive(Default)]
pub(super) struct Resources {
    /// Each resource of the interface and its global.
    pub bound: Vec<(ResourceId, Handle<GlobalVariable>)>,
    pub constant_buffers: BTreeMap<u32, (Handle<GlobalVariable>, u32)>,
    pub textures: BTreeMap<u32, (Handle<GlobalVariable>, Texture)>,
    pub samplers: BTreeMap<u32, Handle<GlobalVariable>>,
    pub buffers: BTreeMap<u32, (Handle<GlobalVariable>, Buffer)>,
    /// Typed unordered-access views `u<n>`.
    pub views: BTreeMap<u32, (Handle<GlobalVariable>, View)>,
    /// The immediate constant buffer and its rows.
    pub immediate: Option<(Handle<GlobalVariable>, u32)>,
}

pub(super) fn lanes_of(mask: u8) -> Vec<u8> {
    (0..4).filter(|i| mask >> i & 1 != 0).collect()
}

impl Lift<'_> {
    /// One global per declared resource, and the immediate constant buffer.
    pub(super) fn declare_resources(&mut self) {
        let u4 = self.u4();
        let u1 = self.b.ty_of(Kind::U, 1);
        let declarations = self.setup.declarations.clone();
        for (i, d) in declarations.iter().enumerate() {
            let id = ResourceId(i as u32);
            let (name, space, group, slot, ty) = match d {
                Declaration::ConstantBuffer { slot, rows } => {
                    let rows = NonZeroU32::new((*rows).max(1)).expect("at least one row");
                    let ty = self.b.ty(TypeInner::Array {
                        base: u4,
                        size: ArraySize::Constant(rows),
                        stride: 16,
                    });
                    (format!("cb{slot}"), AddressSpace::Uniform, 0, *slot, ty)
                }
                Declaration::Texture(t) => {
                    let class = if t.depth {
                        ImageClass::Depth { multi: t.multi }
                    } else {
                        ImageClass::Sampled {
                            kind: t.kind.kind(),
                            multi: t.multi,
                        }
                    };
                    let ty = self.b.ty(TypeInner::Image {
                        dim: t.dim,
                        arrayed: t.arrayed,
                        class,
                    });
                    (format!("t{}", t.slot), AddressSpace::Handle, 1, t.slot, ty)
                }
                Declaration::Sampler { slot, comparison } => {
                    let ty = self.b.ty(TypeInner::Sampler {
                        comparison: *comparison,
                    });
                    (format!("s{slot}"), AddressSpace::Handle, 2, *slot, ty)
                }
                Declaration::View(v) => {
                    // the view's own format is the binding's: the module carries four 32-bit lanes of the kind the
                    // program loads and stores, the one format that holds any of them
                    let format = match v.kind {
                        Kind::I => naga::StorageFormat::Rgba32Sint,
                        Kind::U => naga::StorageFormat::Rgba32Uint,
                        _ => naga::StorageFormat::Rgba32Float,
                    };
                    let mut access = StorageAccess::empty();
                    access.set(StorageAccess::LOAD, v.load || !v.store);
                    access.set(StorageAccess::STORE, v.store);
                    let ty = self.b.ty(TypeInner::Image {
                        dim: v.dim,
                        arrayed: v.arrayed,
                        class: ImageClass::Storage { format, access },
                    });
                    self.notes.insert(
                        "a typed unordered-access view is carried as a storage image of four 32-bit lanes: its \
                         format is the binding's"
                            .into(),
                    );
                    (format!("u{}", v.slot), AddressSpace::Handle, 3, v.slot, ty)
                }
                Declaration::TypedBuffer { slot } => {
                    let ty = self.b.ty(TypeInner::Array {
                        base: u4,
                        size: ArraySize::Dynamic,
                        stride: 16,
                    });
                    (
                        format!("tb{slot}"),
                        AddressSpace::Storage {
                            access: StorageAccess::LOAD,
                        },
                        1,
                        *slot,
                        ty,
                    )
                }
                Declaration::RawBuffer { slot } | Declaration::StructuredBuffer { slot, .. } => {
                    let ty = self.b.ty(TypeInner::Array {
                        base: u1,
                        size: ArraySize::Dynamic,
                        stride: 4,
                    });
                    (
                        format!("sb{slot}"),
                        AddressSpace::Storage {
                            access: StorageAccess::LOAD,
                        },
                        1,
                        *slot,
                        ty,
                    )
                }
            };
            let global = self.b.global(GlobalVariable {
                name: Some(name),
                space,
                binding: Some(ResourceBinding { group, binding: slot }),
                ty,
                init: None,
                memory_decorations: Default::default(),
            });
            self.resources.bound.push((id, global));
            match d {
                Declaration::ConstantBuffer { slot, rows } => {
                    self.resources
                        .constant_buffers
                        .insert(*slot, (global, (*rows).max(1)));
                }
                Declaration::Texture(t) => {
                    self.resources.textures.insert(t.slot, (global, *t));
                }
                Declaration::Sampler { slot, .. } => {
                    self.resources.samplers.insert(*slot, global);
                }
                Declaration::View(v) => {
                    self.resources.views.insert(v.slot, (global, *v));
                }
                Declaration::TypedBuffer { slot } => {
                    self.resources.buffers.insert(*slot, (global, Buffer::Typed));
                }
                Declaration::RawBuffer { slot } => {
                    self.resources
                        .buffers
                        .insert(*slot, (global, Buffer::Words { stride: 4 }));
                }
                Declaration::StructuredBuffer { slot, stride } => {
                    self.resources.buffers.insert(
                        *slot,
                        (
                            global,
                            Buffer::Words {
                                stride: (*stride).max(4),
                            },
                        ),
                    );
                }
            }
        }
        if !self.setup.immediates.is_empty() {
            let rows = self.setup.immediates.len() as u32;
            let size = NonZeroU32::new(rows).expect("rows");
            let ty = self.b.ty(TypeInner::Array {
                base: u4,
                size: ArraySize::Constant(size),
                stride: 16,
            });
            let immediates = self.setup.immediates.clone();
            let mut components = Vec::new();
            for row in &immediates {
                let lanes: Vec<_> = row
                    .iter()
                    .map(|w| self.b.global_expr(E::Literal(naga::Literal::U32(*w))))
                    .collect();
                components.push(self.b.global_expr(E::Compose {
                    ty: u4,
                    components: lanes,
                }));
            }
            let init = self.b.global_expr(E::Compose { ty, components });
            let global = self.b.global(GlobalVariable {
                name: Some("icb".into()),
                space: AddressSpace::Private,
                binding: None,
                ty,
                init: Some(init),
                memory_decorations: Default::default(),
            });
            self.resources.immediate = Some((global, rows));
        }
    }

    /// A read of element `index` of the array global `g` (`len` elements; `None`: sized at run time), zero when out of
    /// range: D3D's answer for constant buffers, the immediate constant buffer and buffer views, which WGSL leaves open.
    pub(super) fn guarded_load(
        &mut self,
        g: Handle<GlobalVariable>,
        index: Val,
        len: Option<u32>,
        kind: Kind,
        width: u8,
    ) -> Val {
        let pointer = self.b.global_ptr(g);
        let length = match len {
            Some(n) => self.b.lit_u(n),
            None => Val {
                expr: self.b.app(E::ArrayLength(pointer)),
                kind: Kind::U,
                width: 1,
            },
        };
        let inside = self.b.binary(Bo::Less, index, length);
        let zero = self.b.lit_u(0);
        let safe = self.b.select(inside, index, zero);
        let pointer = self.b.global_ptr(g);
        let element = self.b.app(E::Access {
            base: pointer,
            index: safe.expr,
        });
        let value = self.b.load(element, kind, width);
        let none = if width == 1 {
            self.b.lit_u(0)
        } else {
            let z = self.b.lit_u(0);
            self.b.splat(z, width)
        };
        self.b.select(inside, value, none)
    }

    fn texture(&self, slot: u32) -> Result<(Handle<GlobalVariable>, Texture)> {
        match self.resources.textures.get(&slot) {
            Some(t) => Ok(*t),
            None if self.resources.buffers.contains_key(&slot) => {
                self.refuse(format!("t{slot} is a buffer, used as a texture"))
            }
            None => self.refuse(format!("t{slot} is used but not declared")),
        }
    }
    fn sampler(&self, slot: u32) -> Result<Handle<GlobalVariable>> {
        self.resources
            .samplers
            .get(&slot)
            .copied()
            .map_or_else(|| self.refuse(format!("s{slot} is used but not declared")), Ok)
    }

    /// The result lanes `mask` of a resource operand's swizzle applied to a four-lane result.
    pub(super) fn pick(&mut self, result: Val, op: &Operand, mask: u8) -> Val {
        let lanes: Vec<u8> = lanes_of(mask)
            .iter()
            .map(|l| match op.selection {
                Selection::Swizzle(s) => s[*l as usize],
                Selection::Scalar(c) => c,
                _ => 0,
            })
            .collect();
        self.b.swizzle(result, &lanes)
    }

    /// Coordinate lanes of a texture: 1 (1D), 2 (2D), 3 (3D, cube), and whether an array layer follows.
    fn coordinates(t: &Texture) -> (u8, bool) {
        let n = match t.dim {
            naga::ImageDimension::D1 => 1,
            naga::ImageDimension::D2 => 2,
            naga::ImageDimension::D3 | naga::ImageDimension::Cube => 3,
        };
        (n, t.arrayed)
    }

    pub(super) fn sample(&mut self, ins: &Instruction) -> Result<()> {
        let name = ins.name();
        let (image, t) = self.texture(ins.operands[2].indices[0].base)?;
        let sampler = self.sampler(ins.operands[3].indices[0].base)?;
        if t.multi {
            return self.refuse("a sample of a multisampled texture");
        }
        let (n, arrayed) = Self::coordinates(&t);
        let mask = self.dest_mask(&ins.operands[0]);
        let lanes: Vec<u8> = (0..n + u8::from(arrayed)).collect();
        let all = self.read_lanes(&ins.operands[1], &lanes, Kind::F)?;
        let coordinate = if n == all.width {
            all
        } else {
            let c: Vec<u8> = (0..n).collect();
            self.b.swizzle(all, &c)
        };
        let array_index = if arrayed {
            let layer = self.b.lane(all, n);
            let rounded = self.b.math(naga::MathFunction::Round, &[layer]);
            let zero = self.b.lit_f(0.0);
            let layer = self.b.d3d_min_max(naga::MathFunction::Max, rounded, zero);
            Some(self.b.convert(layer, Kind::U).expr)
        } else {
            None
        };
        let offset = match ins.sample_offset() {
            Some(o) if o != [0, 0, 0] => {
                let parts: Vec<_> = (0..n as usize)
                    .map(|c| self.b.app(E::Literal(naga::Literal::I32(o[c]))))
                    .collect();
                Some(if n == 1 {
                    parts[0]
                } else {
                    let ty = self.b.ty_of(Kind::I, n);
                    self.b.app(E::Compose {
                        ty,
                        components: parts,
                    })
                })
            }
            _ => None,
        };
        let component_of = |op: &Operand| match op.selection {
            Selection::Scalar(c) => c,
            Selection::Swizzle(s) => s[0],
            _ => 0,
        };
        let (gather, depth_ref, level) = match name {
            "sample" => (None, None, SampleLevel::Auto),
            "sample_l" => (
                None,
                None,
                SampleLevel::Exact(self.read(&ins.operands[4], 1, Kind::F)?.expr),
            ),
            "sample_b" => (
                None,
                None,
                SampleLevel::Bias(self.read(&ins.operands[4], 1, Kind::F)?.expr),
            ),
            "sample_d" => {
                let g: Vec<u8> = (0..n).collect();
                let x = self.read_lanes(&ins.operands[4], &g, Kind::F)?;
                let y = self.read_lanes(&ins.operands[5], &g, Kind::F)?;
                (None, None, SampleLevel::Gradient { x: x.expr, y: y.expr })
            }
            "sample_c" => (
                None,
                Some(self.read(&ins.operands[4], 1, Kind::F)?.expr),
                SampleLevel::Auto,
            ),
            "sample_c_lz" => (
                None,
                Some(self.read(&ins.operands[4], 1, Kind::F)?.expr),
                SampleLevel::Zero,
            ),
            "gather4" => (
                Some(component(component_of(&ins.operands[3]))),
                None,
                SampleLevel::Zero,
            ),
            "gather4_c" => (
                Some(component(component_of(&ins.operands[3]))),
                Some(self.read(&ins.operands[4], 1, Kind::F)?.expr),
                SampleLevel::Zero,
            ),
            other => return self.refuse(format!("{other} is not lifted")),
        };
        let image = self.b.global_ptr(image);
        let sampler = self.b.global_ptr(sampler);
        let expr = self.b.app(E::ImageSample {
            image,
            sampler,
            gather,
            coordinate: coordinate.expr,
            array_index,
            offset,
            level,
            depth_ref,
            clamp_to_edge: false,
        });
        let mut result = Val {
            expr,
            kind: Kind::F,
            width: if t.depth && gather.is_none() { 1 } else { 4 },
        };
        if result.width == 1 {
            // a depth sample is one float; D3D's float view returns it in x and (0, 0, 1) in the others, a comparison
            // in all four
            let zero = self.b.lit_f(0.0);
            let one = self.b.lit_f(1.0);
            result = if depth_ref.is_some() {
                self.b.splat(result, 4)
            } else {
                self.b.compose(&[result, zero, zero, one])
            };
        }
        let picked = self.pick(result, &ins.operands[2], mask);
        self.write(&ins.operands[0], picked, ins.saturate)
    }

    pub(super) fn load(&mut self, ins: &Instruction) -> Result<()> {
        let slot = ins.operands[2].indices[0].base;
        let mask = self.dest_mask(&ins.operands[0]);
        if let Some((g, buffer)) = self.resources.buffers.get(&slot).copied() {
            let Buffer::Typed = buffer else {
                return self.refuse("ld on a raw or structured buffer");
            };
            self.notes_typed_buffer();
            let address = self.read(&ins.operands[1], 1, Kind::U)?;
            let row = self.guarded_load(g, address, None, Kind::U, 4);
            let picked = self.pick(row, &ins.operands[2], mask);
            return self.write(&ins.operands[0], picked, false);
        }
        let (image, t) = self.texture(slot)?;
        let (n, arrayed) = Self::coordinates(&t);
        let all = self.read_lanes(&ins.operands[1], &[0, 1, 2, 3], Kind::I)?;
        let cl: Vec<u8> = (0..n).collect();
        let coordinate = self.b.swizzle(all, &cl);
        let array_index = arrayed.then(|| self.b.lane(all, n).expr);
        let level = (!t.multi).then(|| self.b.lane(all, 3).expr);
        let sample = if ins.name() == "ld_ms" {
            Some(self.read(&ins.operands[3], 1, Kind::I)?.expr)
        } else {
            None
        };
        let image = self.b.global_ptr(image);
        let expr = self.b.app(E::ImageLoad {
            image,
            coordinate: coordinate.expr,
            array_index,
            sample,
            level,
        });
        let mut result = Val {
            expr,
            kind: t.kind,
            width: 4,
        };
        if t.depth {
            result.width = 1;
            let zero = self.b.lit_f(0.0);
            let one = self.b.lit_f(1.0);
            result.kind = Kind::F;
            result = self.b.compose(&[result, zero, zero, one]);
        }
        let bits = self.b.bitcast(result, Kind::U);
        let picked = self.pick(bits, &ins.operands[2], mask);
        self.write(&ins.operands[0], picked, false)
    }

    pub(super) fn resource_info(&mut self, ins: &Instruction) -> Result<()> {
        let slot = ins.operands[2].indices[0].base;
        let (image, t) = self.texture(slot)?;
        let mask = self.dest_mask(&ins.operands[0]);
        let mip = self.read(&ins.operands[1], 1, Kind::U)?;
        let g = self.b.global_ptr(image);
        let dims = match t.dim {
            naga::ImageDimension::D1 => 1,
            naga::ImageDimension::D2 => 2,
            _ => 3,
        };
        let size = Val {
            expr: self.b.app(E::ImageQuery {
                image: g,
                query: ImageQuery::Size {
                    level: (!t.multi).then_some(mip.expr),
                },
            }),
            kind: Kind::U,
            width: dims,
        };
        let levels = if t.multi {
            self.b.lit_u(1)
        } else {
            let g = self.b.global_ptr(image);
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
        let mut parts: Vec<Val> = (0..size.width).map(|i| self.b.lane(size, i)).collect();
        if t.arrayed {
            let g = self.b.global_ptr(image);
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
        let all = self.b.compose(&parts);
        // the return type: float (0), reciprocal float (1) or uint (2)
        let result = match ins.control & 3 {
            2 => all,
            0 => self.b.convert(all, Kind::F),
            _ => {
                let f = self.b.convert(all, Kind::F);
                let one = self.b.lit_f(1.0);
                self.b.binary(Bo::Divide, one, f)
            }
        };
        let picked = self.pick(result, &ins.operands[2], mask);
        self.write(&ins.operands[0], picked, false)
    }

    /// The lanes of a raw or structured load: word `word` plus each result lane's source component.
    fn load_words(&mut self, g: Handle<GlobalVariable>, word: Val, resource: &Operand, mask: u8) -> Val {
        let components: Vec<u8> = lanes_of(mask)
            .iter()
            .map(|l| match resource.selection {
                Selection::Swizzle(s) => s[*l as usize],
                Selection::Scalar(c) => c,
                _ => 0,
            })
            .collect();
        let mut lanes = Vec::new();
        for c in components {
            let offset = self.b.lit_u(u32::from(c));
            let index = self.b.binary(Bo::Add, word, offset);
            lanes.push(self.guarded_load(g, index, None, Kind::U, 1));
        }
        self.b.compose(&lanes)
    }

    pub(super) fn load_raw(&mut self, ins: &Instruction) -> Result<()> {
        let slot = ins.operands[2].indices[0].base;
        let Some((g, Buffer::Words { .. })) = self.resources.buffers.get(&slot).copied() else {
            return self.refuse(format!("t{slot} is not a raw buffer"));
        };
        let mask = self.dest_mask(&ins.operands[0]);
        let offset = self.read(&ins.operands[1], 1, Kind::U)?;
        let two = self.b.lit_u(2);
        let word = self.b.binary(Bo::ShiftRight, offset, two);
        let result = self.load_words(g, word, &ins.operands[2], mask);
        self.write(&ins.operands[0], result, false)
    }

    pub(super) fn load_structured(&mut self, ins: &Instruction) -> Result<()> {
        let slot = ins.operands[3].indices[0].base;
        let Some((g, Buffer::Words { stride })) = self.resources.buffers.get(&slot).copied() else {
            return self.refuse(format!("t{slot} is not a structured buffer"));
        };
        let mask = self.dest_mask(&ins.operands[0]);
        let element = self.read(&ins.operands[1], 1, Kind::U)?;
        let offset = self.read(&ins.operands[2], 1, Kind::U)?;
        let stride = self.b.lit_u(stride);
        let base = self.b.binary(Bo::Multiply, element, stride);
        let byte = self.b.binary(Bo::Add, base, offset);
        let two = self.b.lit_u(2);
        let word = self.b.binary(Bo::ShiftRight, byte, two);
        let result = self.load_words(g, word, &ins.operands[3], mask);
        self.write(&ins.operands[0], result, false)
    }

    fn view(&self, slot: u32) -> Result<(Handle<GlobalVariable>, View)> {
        self.resources
            .views
            .get(&slot)
            .copied()
            .map_or_else(|| self.refuse(format!("u{slot} is not a typed texture view")), Ok)
    }

    /// The address lanes of a typed view's texel: its dimensions' coordinates, and an array layer after them.
    fn view_address(&mut self, op: &Operand, v: &View) -> Result<(Val, Option<naga::Handle<E>>)> {
        let n = match v.dim {
            naga::ImageDimension::D1 => 1,
            naga::ImageDimension::D2 => 2,
            _ => 3,
        };
        let lanes: Vec<u8> = (0..n + u8::from(v.arrayed)).collect();
        let all = self.read_lanes(op, &lanes, Kind::U)?;
        let coordinate = if all.width == n {
            all
        } else {
            let c: Vec<u8> = (0..n).collect();
            self.b.swizzle(all, &c)
        };
        let layer = v.arrayed.then(|| self.b.lane(all, n).expr);
        Ok((coordinate, layer))
    }

    /// `store_uav_typed u, address, value`: the texel's four lanes. D3D writes the lanes the view's mask names; a
    /// partial mask would keep the others, which a store of four lanes cannot, so it is refused.
    pub(super) fn store_view(&mut self, ins: &Instruction) -> Result<()> {
        let slot = ins.operands[0].indices[0].base;
        let (global, v) = self.view(slot)?;
        if self.dest_mask(&ins.operands[0]) != 15 {
            return self.refuse("a store to a view with a partial write mask");
        }
        let (coordinate, array_index) = self.view_address(&ins.operands[1], &v)?;
        let value = self.read_lanes(&ins.operands[2], &[0, 1, 2, 3], v.kind)?;
        let image = self.b.global_ptr(global);
        self.b.push(naga::Statement::ImageStore {
            image,
            coordinate: coordinate.expr,
            array_index,
            value: value.expr,
        });
        Ok(())
    }

    /// `ld_uav_typed dest, address, u`.
    pub(super) fn load_view(&mut self, ins: &Instruction) -> Result<()> {
        let slot = ins.operands[2].indices[0].base;
        let (global, v) = self.view(slot)?;
        let mask = self.dest_mask(&ins.operands[0]);
        let (coordinate, array_index) = self.view_address(&ins.operands[1], &v)?;
        let image = self.b.global_ptr(global);
        let expr = self.b.app(E::ImageLoad {
            image,
            coordinate: coordinate.expr,
            array_index,
            sample: None,
            level: None,
        });
        let result = Val {
            expr,
            kind: v.kind,
            width: 4,
        };
        let bits = self.b.bitcast(result, Kind::U);
        let picked = self.pick(bits, &ins.operands[2], mask);
        self.write(&ins.operands[0], picked, false)
    }

    fn notes_typed_buffer(&mut self) {
        self.notes.insert(
            "a typed buffer view is read as four 32-bit lanes per element: converting from the view's format is the \
             binding's"
                .into(),
        );
    }
}
