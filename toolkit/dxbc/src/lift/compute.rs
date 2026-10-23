//! What a compute program does that the other stages do not: reads, writes and atomic operations on unordered-access
//! buffers and thread-group shared memory, and barriers.

use naga::{AtomicFunction, BinaryOperator as Bo, Expression as E, GlobalVariable, Handle, Statement};
use primase_shader::build::{Kind, Val};

use super::Lift;
use super::resources::lanes_of;
use super::setup::UavKind;
use crate::Result;
use crate::decode::{Instruction, Operand, Register};

/// The memory a word-addressed instruction reaches.
#[derive(Clone, Copy)]
pub(super) struct Mem {
    pub global: Handle<GlobalVariable>,
    /// Its length in words; `None` for a buffer sized at run time.
    pub len: Option<u32>,
    /// The element type: the atomic integer type when atomic operations reach it, else unsigned.
    pub atomic: Option<naga::Scalar>,
    /// A structured one's element stride in bytes.
    pub stride: Option<u32>,
    /// A typed buffer: four lanes an element, addressed by element.
    pub typed: bool,
}

impl Mem {
    fn kind(&self) -> Kind {
        match self.atomic {
            Some(naga::Scalar::I32) => Kind::I,
            _ => Kind::U,
        }
    }
}

impl Lift<'_> {
    /// The memory a resource operand names: an unordered-access buffer `u<n>` or thread-group memory `g<n>`.
    pub(super) fn memory(&self, op: &Operand) -> Result<Mem> {
        let index = op.indices.first().map_or(0, |i| i.base);
        match op.register {
            Register::Uav => match self.resources.uav_buffers.get(&index) {
                Some((global, b)) => Ok(Mem {
                    global: *global,
                    len: None,
                    atomic: b.atomic,
                    stride: match b.kind {
                        UavKind::Structured { stride } => Some(stride),
                        _ => None,
                    },
                    typed: matches!(b.kind, UavKind::Typed(_)),
                }),
                None if self.resources.views.contains_key(&index) => self.refuse(format!(
                    "u{index} is a texture view: buffer operations and atomics on it are not lifted"
                )),
                None => self.refuse(format!("u{index} is not an unordered-access buffer")),
            },
            Register::GroupSharedMemory => match self.resources.shared.get(&index) {
                Some((global, g)) => Ok(Mem {
                    global: *global,
                    len: Some(g.words),
                    atomic: g.atomic,
                    stride: g.stride,
                    typed: false,
                }),
                None => self.refuse(format!("g{index} is not declared")),
            },
            other => self.refuse(format!("a memory operand of register type {other:?}")),
        }
    }

    /// An atomic operation on a typed texture view: naga's image atomic, which returns no old value; where the program
    /// reads the old value, a call of the module's image atomic function that has it (see `primase_shader::ops`).
    fn view_atomic(&mut self, ins: &Instruction, slot: u32) -> Result<()> {
        let name = ins.name();
        let immediate = name.starts_with("imm_");
        let ops = &ins.operands;
        let at = usize::from(immediate);
        let wants_old = immediate && ops[0].register != Register::Null;
        let fun = match name.trim_start_matches("imm_").trim_start_matches("atomic_") {
            "iadd" => AtomicFunction::Add,
            "and" => AtomicFunction::And,
            "or" => AtomicFunction::InclusiveOr,
            "xor" => AtomicFunction::ExclusiveOr,
            "imax" | "umax" => AtomicFunction::Max,
            "imin" | "umin" => AtomicFunction::Min,
            "exch" => AtomicFunction::Exchange { compare: None },
            other => return self.refuse(format!("atomic operation {other} on a texture view")),
        };
        let (global, v) = self
            .resources
            .views
            .get(&slot)
            .copied()
            .expect("the caller found the view");
        let (coordinate, layer) = self.view_address(&ops[at + 1], &v)?;
        let kind = v.kind;
        let value = self.read(&ops[at + 2], 1, kind)?;
        let coordinate = self.b.bitcast_to(coordinate, Kind::I);
        let array_index = layer.map(|l| {
            let layer = Val {
                expr: l,
                kind: Kind::U,
                width: 1,
            };
            self.b.bitcast_to(layer, Kind::I).expr
        });
        // naga's image atomic statement has no exchange, and none has an old value: both are the function's
        if wants_old || matches!(fun, AtomicFunction::Exchange { .. }) {
            let op = match fun {
                AtomicFunction::Add => "add",
                AtomicFunction::And => "and",
                AtomicFunction::InclusiveOr => "or",
                AtomicFunction::ExclusiveOr => "xor",
                AtomicFunction::Max => "max",
                AtomicFunction::Min => "min",
                AtomicFunction::Exchange { compare: None } => "exchange",
                _ => return self.refuse("an image atomic operation the module's functions do not name"),
            };
            let old = self
                .b
                .image_atomic_fetch(global, op, coordinate, array_index, value);
            if !wants_old {
                return Ok(());
            }
            let bits = self.b.bitcast_to(old, Kind::U);
            return self.write(&ops[0], bits, false);
        }
        let image = self.b.global_ptr(global);
        self.b.push(Statement::ImageAtomic {
            image,
            coordinate: coordinate.expr,
            array_index,
            fun,
            value: value.expr,
        });
        Ok(())
    }

    /// The word of a raw address (a byte offset), or of a structured one (an element and a byte offset in it): one
    /// operand, the structured pair in its x and y (the atomics), or two (the loads and stores).
    fn word_address(&mut self, mem: &Mem, address: &[&Operand]) -> Result<Val> {
        let two = self.b.lit_u(2);
        Ok(match mem.stride {
            None if mem.typed => self.read(address[0], 1, Kind::U)?,
            None => {
                let byte = self.read(address[0], 1, Kind::U)?;
                self.b.binary(Bo::ShiftRight, byte, two)
            }
            Some(stride) => {
                let (element, offset) = match address {
                    [one] => {
                        let lanes = self.read_lanes(one, &[0, 1], Kind::U)?;
                        (self.b.lane(lanes, 0), self.b.lane(lanes, 1))
                    }
                    [element, offset] => (self.read(element, 1, Kind::U)?, self.read(offset, 1, Kind::U)?),
                    _ => return self.refuse("a structured address of other than one or two operands"),
                };
                let stride = self.b.lit_u(stride);
                let base = self.b.binary(Bo::Multiply, element, stride);
                let byte = self.b.binary(Bo::Add, base, offset);
                self.b.binary(Bo::ShiftRight, byte, two)
            }
        })
    }

    /// The word `index` of `mem` as unsigned bits, zero past its end (D3D's answer for a buffer).
    fn read_word(&mut self, mem: &Mem, index: Val) -> Val {
        let word = self.guarded_load(mem.global, index, mem.len, mem.kind(), 1);
        self.b.bitcast_to(word, Kind::U)
    }

    /// `ld_raw` and `ld_structured` of an unordered-access buffer or shared memory: the lanes `mask` names, from the
    /// resource operand's swizzle.
    pub(super) fn load_memory(
        &mut self,
        ins: &Instruction,
        address: &[&Operand],
        resource: &Operand,
    ) -> Result<()> {
        let mem = self.memory(resource)?;
        let mask = self.dest_mask(&ins.operands[0]);
        let word = self.word_address(&mem, address)?;
        let components: Vec<u8> = lanes_of(mask)
            .iter()
            .map(|l| match resource.selection {
                crate::decode::Selection::Swizzle(s) => s[*l as usize],
                crate::decode::Selection::Scalar(c) => c,
                _ => 0,
            })
            .collect();
        let mut lanes = Vec::new();
        for c in components {
            let offset = self.b.lit_u(u32::from(c));
            let index = self.b.binary(Bo::Add, word, offset);
            lanes.push(self.read_word(&mem, index));
        }
        let result = self.b.compose(&lanes);
        self.write(&ins.operands[0], result, false)
    }

    /// `ld_uav_typed` of a typed buffer: the element's four lanes.
    pub(super) fn load_typed_buffer(&mut self, ins: &Instruction) -> Result<()> {
        let resource = &ins.operands[2];
        let mem = self.memory(resource)?;
        let mask = self.dest_mask(&ins.operands[0]);
        let element = self.read(&ins.operands[1], 1, Kind::U)?;
        let row = match mem.atomic {
            None => self.guarded_load(mem.global, element, None, Kind::U, 4),
            // the element is four atomic words: read each, zero past the end
            Some(_) => {
                let mut lanes = Vec::new();
                for k in 0..4u32 {
                    let word = self.guarded_word_of_quad(&mem, element, k);
                    lanes.push(word);
                }
                self.b.compose(&lanes)
            }
        };
        let picked = self.pick(row, resource, mask);
        self.write(&ins.operands[0], picked, false)
    }

    /// `store_uav_typed` to a typed buffer: the element's four lanes.
    pub(super) fn store_typed_buffer(&mut self, ins: &Instruction) -> Result<()> {
        let mem = self.memory(&ins.operands[0])?;
        if self.dest_mask(&ins.operands[0]) != 15 {
            return self.refuse("a store to a view with a partial write mask");
        }
        let element = self.read(&ins.operands[1], 1, Kind::U)?;
        let value = self.read_lanes(&ins.operands[2], &[0, 1, 2, 3], Kind::U)?;
        let pointer = self.b.global_ptr(mem.global);
        let length = Val {
            expr: self.b.app(E::ArrayLength(pointer)),
            kind: Kind::U,
            width: 1,
        };
        let inside = self.b.binary(Bo::Less, element, length);
        self.b.when(inside, |b| {
            let pointer = b.global_ptr(mem.global);
            let target = b.app(E::Access {
                base: pointer,
                index: element.expr,
            });
            match mem.atomic {
                None => b.store(target, value),
                Some(_) => {
                    for k in 0..4u32 {
                        let word = b.app(E::AccessIndex {
                            base: target,
                            index: k,
                        });
                        let lane = b.lane(value, k as u8);
                        let lane = b.bitcast_to(lane, mem.kind());
                        b.store(word, lane);
                    }
                }
            }
        });
        Ok(())
    }

    /// Word `k` of element `element` of a typed buffer made of atomic words, as unsigned bits; zero past the end.
    fn guarded_word_of_quad(&mut self, mem: &Mem, element: Val, k: u32) -> Val {
        let pointer = self.b.global_ptr(mem.global);
        let length = Val {
            expr: self.b.app(E::ArrayLength(pointer)),
            kind: Kind::U,
            width: 1,
        };
        let inside = self.b.binary(Bo::Less, element, length);
        let zero = self.b.lit_u(0);
        let safe = self.b.select(inside, element, zero);
        let pointer = self.b.global_ptr(mem.global);
        let row = self.b.app(E::Access {
            base: pointer,
            index: safe.expr,
        });
        let word = self.b.app(E::AccessIndex { base: row, index: k });
        let value = self.b.load(word, mem.kind(), 1);
        let bits = self.b.bitcast_to(value, Kind::U);
        self.b.select(inside, bits, zero)
    }

    /// `store_raw` and `store_structured`: the destination's masked lanes, consecutive words from the address; a
    /// write past the end of the memory is dropped.
    pub(super) fn store_memory(
        &mut self,
        ins: &Instruction,
        address: &[&Operand],
        value: &Operand,
    ) -> Result<()> {
        let mem = self.memory(&ins.operands[0])?;
        let mask = self.dest_mask(&ins.operands[0]);
        let count = lanes_of(mask).len() as u8;
        let word = self.word_address(&mem, address)?;
        let lanes: Vec<u8> = (0..count).collect();
        let data = self.read_lanes(value, &lanes, Kind::U)?;
        for k in 0..count {
            let offset = self.b.lit_u(u32::from(k));
            let at = self.b.binary(Bo::Add, word, offset);
            let datum = self.b.lane(data, k);
            self.write_word(&mem, at, datum);
        }
        Ok(())
    }

    /// Word `at` of `mem` set to the unsigned bits `value`, unless it is past the end.
    fn write_word(&mut self, mem: &Mem, at: Val, value: Val) {
        let pointer = self.b.global_ptr(mem.global);
        let length = match mem.len {
            Some(n) => self.b.lit_u(n),
            None => Val {
                expr: self.b.app(E::ArrayLength(pointer)),
                kind: Kind::U,
                width: 1,
            },
        };
        let inside = self.b.binary(Bo::Less, at, length);
        let mem = *mem;
        self.b.when(inside, |b| {
            let pointer = b.global_ptr(mem.global);
            let target = b.app(E::Access {
                base: pointer,
                index: at.expr,
            });
            let value = b.bitcast_to(value, mem.kind());
            b.store(target, value);
        });
    }

    /// The atomic function of an `atomic_*` or `imm_atomic_*` instruction.
    fn atomic_function(&self, name: &str) -> Result<AtomicFunction> {
        let op = name.trim_start_matches("imm_").trim_start_matches("atomic_");
        Ok(match op {
            "iadd" => AtomicFunction::Add,
            "and" => AtomicFunction::And,
            "or" => AtomicFunction::InclusiveOr,
            "xor" => AtomicFunction::ExclusiveOr,
            "imax" | "umax" => AtomicFunction::Max,
            "imin" | "umin" => AtomicFunction::Min,
            "exch" => AtomicFunction::Exchange { compare: None },
            other => return self.refuse(format!("atomic operation {other}")),
        })
    }

    /// An atomic operation on word `word` of `mem`: the old value as unsigned bits.
    pub(super) fn atomic_instruction(&mut self, ins: &Instruction) -> Result<()> {
        let name = ins.name();
        let immediate = name.starts_with("imm_");
        let ops = &ins.operands;
        // [old,] resource, address, source [, second source]
        let at = usize::from(immediate);
        let slot = ops[at].indices.first().map_or(0, |i| i.base);
        if ops[at].register == Register::Uav && self.resources.views.contains_key(&slot) {
            return self.view_atomic(ins, slot);
        }
        let mem = self.memory(&ops[at])?;
        let Some(scalar) = mem.atomic else {
            return self.refuse("an atomic operation on memory the lifter did not make atomic");
        };
        let word = self.word_address(&mem, &[&ops[at + 1]])?;
        let kind = mem.kind();
        let compare = name.ends_with("cmp_exch") || name.ends_with("cmp_store");
        let (first, second) = {
            let v = self.read(&ops[at + 2], 1, Kind::U)?;
            if compare {
                (v, Some(self.read(&ops[at + 3], 1, Kind::U)?))
            } else {
                (v, None)
            }
        };
        let fun = if compare {
            None
        } else {
            Some(self.atomic_function(name)?)
        };
        // past the end of the memory the operation does nothing and its old value is zero, as in D3D
        let pointer = self.b.global_ptr(mem.global);
        let length = match mem.len {
            Some(n) => self.b.lit_u(n),
            None => Val {
                expr: self.b.app(E::ArrayLength(pointer)),
                kind: Kind::U,
                width: 1,
            },
        };
        let inside = self.b.binary(Bo::Less, word, length);
        let old = self.b.atomic_inside(inside, kind, |b| {
            let element = b.buffer_word(pointer, word, mem.typed);
            match (fun, second) {
                (Some(fun), _) => b.atomic(element, fun, first, scalar),
                (None, Some(new)) => b.compare_exchange(element, first, new, scalar).0,
                (None, None) => unreachable!("a compare-and-swap names its new value"),
            }
        });
        if immediate {
            let bits = self.b.bitcast_to(old, Kind::U);
            self.write(&ops[0], bits, false)?;
        }
        Ok(())
    }

    /// `sync` with its flags: 1 synchronise the group, 2 fence shared memory, 4 fence buffers within the group, 8
    /// fence buffers across groups.
    pub(super) fn sync(&mut self, ins: &Instruction) {
        let mut flags = naga::Barrier::empty();
        if ins.control & 0b1100 != 0 {
            flags |= naga::Barrier::STORAGE;
        }
        if ins.control & 0b0010 != 0 {
            flags |= naga::Barrier::WORK_GROUP;
        }
        if ins.control & 1 != 0 {
            flags |= naga::Barrier::WORK_GROUP | naga::Barrier::STORAGE;
            self.b.push(Statement::ControlBarrier(flags));
        } else if !flags.is_empty() {
            self.b.push(Statement::MemoryBarrier(flags));
        }
    }
}
