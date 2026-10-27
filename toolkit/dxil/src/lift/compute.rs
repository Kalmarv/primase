//! What a compute program does that the other stages do not: barriers, atomic operations (on thread-group variables
//! and on unordered-access buffers), and writes to unordered-access buffers.

use iridia::build::{Kind, Val};
use iridia::{BufferKind, ResourceKind, StorageTarget};
use naga::{AtomicFunction, BinaryOperator as Bo, Expression as E, Handle, Statement};

use super::memory::Root;
use super::resources::kind_of_atomic;
use super::setup::const_int;
use super::{Lift, Lv};
use crate::Result;
use crate::module::{Instruction, ValueId};

/// DXIL's `atomicBinOp` operation codes.
const ATOMIC_ADD: i64 = 0;
const ATOMIC_AND: i64 = 1;
const ATOMIC_OR: i64 = 2;
const ATOMIC_XOR: i64 = 3;
const ATOMIC_IMIN: i64 = 4;
const ATOMIC_IMAX: i64 = 5;
const ATOMIC_UMIN: i64 = 6;
const ATOMIC_UMAX: i64 = 7;
const ATOMIC_EXCHANGE: i64 = 8;

impl Lift<'_> {
    /// `Barrier(mode)`: the mode's flags are 1 synchronise the thread group, 2 fence buffer memory across groups, 4
    /// fence it within the group, 8 fence thread-group memory.
    pub(super) fn barrier(&mut self, args: &[ValueId]) -> Result<()> {
        let Some(mode) = const_int(self.m, self.f, args[1]) else {
            return self.refuse("a barrier of a computed mode");
        };
        let mut flags = naga::Barrier::empty();
        if mode & 0b0110 != 0 {
            flags |= naga::Barrier::STORAGE;
        }
        if mode & 0b1000 != 0 {
            flags |= naga::Barrier::WORK_GROUP;
        }
        if mode & 1 != 0 {
            // a synchronising barrier orders the memory its group shares too
            flags |= naga::Barrier::WORK_GROUP | naga::Barrier::STORAGE;
            self.b.push(Statement::ControlBarrier(flags));
        } else if !flags.is_empty() {
            self.b.push(Statement::MemoryBarrier(flags));
        }
        Ok(())
    }

    /// The atomic function of an LLVM `atomicrmw` operation.
    fn rmw_function(&self, op: u64) -> Result<AtomicFunction> {
        Ok(match op {
            0 => AtomicFunction::Exchange { compare: None },
            1 => AtomicFunction::Add,
            2 => AtomicFunction::Subtract,
            3 => AtomicFunction::And,
            5 => AtomicFunction::InclusiveOr,
            6 => AtomicFunction::ExclusiveOr,
            7 | 9 => AtomicFunction::Max,
            8 | 10 => AtomicFunction::Min,
            other => return self.refuse(format!("atomicrmw operation {other}")),
        })
    }

    /// The pointer and integer type of a thread-group variable an atomic instruction reaches.
    fn atomic_target(&mut self, pointer: ValueId) -> Result<(Handle<E>, naga::Scalar)> {
        let Lv::Pointer(p) = self.get(pointer)? else {
            return self.refuse("an atomic operation through something that is not a pointer");
        };
        let Root::Global(g) = p.root else {
            return self.refuse("an atomic operation on a local variable");
        };
        let Some(scalar) = self.memory_atomic_scalar(g) else {
            return self.refuse("an atomic operation on a variable that is not thread-group memory");
        };
        let expr = self.pointer_expr(&p)?;
        Ok((expr, scalar))
    }

    pub(super) fn atomic_rmw(
        &mut self,
        inst: &Instruction,
        pointer: ValueId,
        value: ValueId,
        op: u64,
    ) -> Result<()> {
        let fun = self.rmw_function(op)?;
        let (target, scalar) = self.atomic_target(pointer)?;
        let v = self.scalar(value)?;
        let old = self.b.atomic(target, fun, v, scalar);
        self.define_result(inst.result, Lv::Scalar(old))
    }

    pub(super) fn cmp_xchg(
        &mut self,
        inst: &Instruction,
        pointer: ValueId,
        compare: ValueId,
        value: ValueId,
    ) -> Result<()> {
        let (target, scalar) = self.atomic_target(pointer)?;
        let (c, v) = (self.scalar(compare)?, self.scalar(value)?);
        let (old, exchanged) = self.b.compare_exchange(target, c, v, scalar);
        self.define_result(inst.result, Lv::Agg(vec![Lv::Scalar(old), Lv::Scalar(exchanged)]))
    }

    /// The word of an unordered-access buffer an operation reads or writes, and the buffer's kind: the element for a
    /// typed buffer, the 32-bit word at a raw buffer's byte offset or a structured buffer's element and byte offset.
    fn uav_word(&mut self, resource: usize, index: ValueId, offset: ValueId) -> Result<(Val, BufferKind)> {
        let kind = match self.setup.kind(resource).clone() {
            ResourceKind::Storage {
                target: StorageTarget::Buffer(k),
                ..
            } => k,
            _ => {
                return self.refuse("a buffer operation on something that is not an unordered-access buffer");
            }
        };
        let index = self.uint(index)?;
        let word = match kind {
            BufferKind::Typed(_) => index,
            BufferKind::Raw => {
                let two = self.b.lit_u(2);
                self.b.binary(Bo::ShiftRight, index, two)
            }
            BufferKind::Structured { stride } => {
                let offset = self.uint(offset)?;
                let stride = self.b.lit_u(stride);
                let base = self.b.binary(Bo::Multiply, index, stride);
                let byte = self.b.binary(Bo::Add, base, offset);
                let two = self.b.lit_u(2);
                self.b.binary(Bo::ShiftRight, byte, two)
            }
        };
        Ok((word, kind))
    }

    /// `BufferStore` and `RawBufferStore(handle, index, offset, value x4, mask)`: the masked lanes written at the
    /// buffer's words; a write past the end of the buffer is dropped.
    pub(super) fn buffer_store(&mut self, args: &[ValueId]) -> Result<()> {
        let (resource, words) = self.buffer_words(args[1], 1)?;
        let Some(mask) = const_int(self.m, self.f, args[8]) else {
            return self.refuse("a buffer write with a computed mask");
        };
        let (word, kind) = self.uav_word(resource, args[2], args[3])?;
        let atomic = self.resources.atomic.get(&resource).copied();
        let mut lanes = Vec::new();
        for lane in 0..4usize {
            if mask >> lane & 1 != 0 {
                let v = self.scalar(args[4 + lane])?;
                lanes.push((lane as u32, self.b.bitcast_to(v, Kind::U)));
            }
        }
        for (lane, value) in lanes {
            let at = match kind {
                BufferKind::Typed(_) => word,
                _ => {
                    let k = self.b.lit_u(lane);
                    self.b.binary(Bo::Add, word, k)
                }
            };
            let length = Val {
                expr: self.b.app(E::ArrayLength(words)),
                kind: Kind::U,
                width: 1,
            };
            let inside = self.b.binary(Bo::Less, at, length);
            self.b.when(inside, |b| {
                let mut element = b.app(E::Access {
                    base: words,
                    index: at.expr,
                });
                if matches!(kind, BufferKind::Typed(_)) {
                    element = b.app(E::AccessIndex {
                        base: element,
                        index: lane,
                    });
                }
                let value = match atomic {
                    Some(scalar) => b.bitcast_to(value, kind_of_atomic(scalar)),
                    None => value,
                };
                b.store(element, value);
            });
        }
        Ok(())
    }

    /// Whether a value is read by any instruction of the function.
    fn value_is_read(&self, v: ValueId) -> bool {
        self.f.instructions.iter().any(|i| i.op.operands().contains(&v))
    }

    /// `AtomicBinOp` on an unordered-access texture: naga's image atomic, with the old value where the program reads it.
    fn image_atomic(&mut self, inst: &Instruction, args: &[ValueId], resource: usize) -> Result<()> {
        let wants_old = inst.result.is_some_and(|(v, _)| self.value_is_read(v));
        let fun = match const_int(self.m, self.f, args[2]) {
            Some(ATOMIC_ADD) => AtomicFunction::Add,
            Some(ATOMIC_AND) => AtomicFunction::And,
            Some(ATOMIC_OR) => AtomicFunction::InclusiveOr,
            Some(ATOMIC_XOR) => AtomicFunction::ExclusiveOr,
            Some(ATOMIC_IMIN | ATOMIC_UMIN) => AtomicFunction::Min,
            Some(ATOMIC_IMAX | ATOMIC_UMAX) => AtomicFunction::Max,
            Some(ATOMIC_EXCHANGE) => AtomicFunction::Exchange { compare: None },
            other => return self.refuse(format!("atomic operation {other:?} on a texture view")),
        };
        let (dim, arrayed, _, _, _) = self.texture_shape(resource)?;
        let (coordinate, array_index) = self.coordinates(args, 3, dim, arrayed, Kind::U)?;
        let coordinate = self.b.bitcast_to(coordinate, Kind::I);
        let array_index = array_index.map(|l| {
            let layer = Val {
                expr: l,
                kind: Kind::U,
                width: 1,
            };
            self.b.bitcast_to(layer, Kind::I).expr
        });
        let scalar = self
            .resources
            .atomic
            .get(&resource)
            .copied()
            .unwrap_or(naga::Scalar::U32);
        let value = self.scalar(args[6])?;
        let value = self.b.bitcast_to(value, kind_of_atomic(scalar));
        let (_, image) = self.handle_expr(args[1], 1)?;
        let old = wants_old.then(|| self.b.image_atomic_result(kind_of_atomic(scalar)));
        self.b.push(Statement::ImageAtomic {
            image,
            coordinate: coordinate.expr,
            array_index,
            fun,
            value: value.expr,
            result: old.map(|o| o.expr),
        });
        match old {
            Some(old) => self.define_result(inst.result, Lv::Scalar(old)),
            None => Ok(()),
        }
    }

    /// Whether word (or, for a typed buffer, element) `word` is inside the buffer `g`.
    fn buffer_word_inside(&mut self, words: Handle<E>, word: Val) -> Val {
        let length = Val {
            expr: self.b.app(E::ArrayLength(words)),
            kind: Kind::U,
            width: 1,
        };
        self.b.binary(Bo::Less, word, length)
    }

    /// `AtomicBinOp(handle, op, index, offset, unused, value)` on an unordered-access buffer: the old value, zero when
    /// the word is past the end of the buffer (the operation does nothing there, as in D3D).
    pub(super) fn buffer_atomic(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let resource = self.handle_resource(args[1], 1)?;
        if let ResourceKind::Storage {
            target: StorageTarget::Image { .. },
            ..
        } = self.setup.kind(resource)
        {
            return self.image_atomic(inst, args, resource);
        }
        let (_, words) = self.buffer_words(args[1], 1)?;
        let Some(op) = const_int(self.m, self.f, args[2]) else {
            return self.refuse("an atomic operation of a computed kind");
        };
        let fun = match op {
            ATOMIC_ADD => AtomicFunction::Add,
            ATOMIC_AND => AtomicFunction::And,
            ATOMIC_OR => AtomicFunction::InclusiveOr,
            ATOMIC_XOR => AtomicFunction::ExclusiveOr,
            ATOMIC_IMIN | ATOMIC_UMIN => AtomicFunction::Min,
            ATOMIC_IMAX | ATOMIC_UMAX => AtomicFunction::Max,
            ATOMIC_EXCHANGE => AtomicFunction::Exchange { compare: None },
            other => return self.refuse(format!("atomic operation {other}")),
        };
        let (word, kind) = self.uav_word(resource, args[3], args[4])?;
        let Some(scalar) = self.resources.atomic.get(&resource).copied() else {
            return self.refuse("an atomic operation on a buffer the lifter did not make atomic");
        };
        let value = self.scalar(args[6])?;
        let inside = self.buffer_word_inside(words, word);
        let typed = matches!(kind, BufferKind::Typed(_));
        let old = self.b.atomic_inside(inside, kind_of_atomic(scalar), |b| {
            let element = b.buffer_word(words, word, typed);
            b.atomic(element, fun, value, scalar)
        });
        self.define_result(inst.result, Lv::Scalar(old))
    }

    /// `AtomicCompareExchange(handle, index, offset, unused, compare, value)` on an unordered-access buffer.
    pub(super) fn buffer_compare_exchange(&mut self, inst: &Instruction, args: &[ValueId]) -> Result<()> {
        let (resource, words) = self.buffer_words(args[1], 1)?;
        let (word, kind) = self.uav_word(resource, args[2], args[3])?;
        let Some(scalar) = self.resources.atomic.get(&resource).copied() else {
            return self.refuse("an atomic operation on a buffer the lifter did not make atomic");
        };
        let (compare, value) = (self.scalar(args[5])?, self.scalar(args[6])?);
        let inside = self.buffer_word_inside(words, word);
        let typed = matches!(kind, BufferKind::Typed(_));
        let old = self.b.atomic_inside(inside, kind_of_atomic(scalar), |b| {
            let element = b.buffer_word(words, word, typed);
            b.compare_exchange(element, compare, value, scalar).0
        });
        self.define_result(inst.result, Lv::Scalar(old))
    }
}

/// `WaveActiveOp` codes.
const WAVE_SUM: i64 = 0;
const WAVE_PRODUCT: i64 = 1;
const WAVE_MIN: i64 = 2;
const WAVE_MAX: i64 = 3;

impl Lift<'_> {
    /// A subgroup operation's result of `kind`, made before the statement that fills it.
    fn subgroup_result(&mut self, kind: Kind) -> Handle<E> {
        let ty = self.b.ty_of(kind, 1);
        self.b.result_expr(E::SubgroupOperationResult { ty })
    }

    pub(super) fn collective(
        &mut self,
        op: naga::SubgroupOperation,
        scan: naga::CollectiveOperation,
        argument: Val,
    ) -> Val {
        let result = self.subgroup_result(argument.kind);
        self.b.push(Statement::SubgroupCollectiveOperation {
            op,
            collective_op: scan,
            argument: argument.expr,
            result,
        });
        Val {
            expr: result,
            kind: argument.kind,
            width: 1,
        }
    }

    fn gather(&mut self, mode: naga::GatherMode, argument: Val) -> Val {
        let result = self.subgroup_result(argument.kind);
        self.b.push(Statement::SubgroupGather {
            mode,
            argument: argument.expr,
            result,
        });
        Val {
            expr: result,
            kind: argument.kind,
            width: 1,
        }
    }

    /// The four-word ballot of a condition: bit `k` of the whole is lane `k`'s.
    fn ballot(&mut self, predicate: Val) -> Val {
        let result = self.b.result_expr(E::SubgroupBallotResult);
        self.b.push(Statement::SubgroupBallot {
            result,
            predicate: Some(predicate.expr),
        });
        Val {
            expr: result,
            kind: Kind::U,
            width: 4,
        }
    }

    /// The operand of a wave operation as the type its signedness says: a float stays one.
    fn wave_operand(&mut self, value: ValueId, unsigned: Option<i64>) -> Result<Val> {
        let v = self.scalar(value)?;
        Ok(match (v.kind, unsigned) {
            (Kind::F, _) => v,
            (_, Some(0)) => self.b.bitcast_to(v, Kind::I),
            _ => self.b.bitcast_to(v, Kind::U),
        })
    }

    /// The wave operations: the subgroup of the device is the wave.
    pub(super) fn wave(&mut self, inst: &Instruction, code: u64, args: &[ValueId]) -> Result<()> {
        use naga::{CollectiveOperation as Scan, SubgroupOperation as Sop};
        self.notes.insert(
            "wave operations act on the device's subgroup, as D3D's act on the hardware's wave: the lane count is \
             the device's, not a fixed 32 or 64"
                .into(),
        );
        let konst = |lift: &Self, n: usize| const_int(lift.m, lift.f, args[n]);
        let result: Val = match code {
            // IsFirstLane, LaneIndex, LaneCount
            110 => {
                let lane = self.system_input(iridia::SystemValue::LaneIndex)?;
                let first = self.gather(naga::GatherMode::BroadcastFirst, lane);
                self.b.binary(Bo::Equal, lane, first)
            }
            111 => self.system_input(iridia::SystemValue::LaneIndex)?,
            112 => self.system_input(iridia::SystemValue::LaneCount)?,
            // AnyTrue, AllTrue
            113 | 114 => {
                let c = self.scalar(args[1])?;
                let c = self.b.bitcast_to(c, Kind::B);
                let op = if code == 113 { Sop::Any } else { Sop::All };
                self.collective(op, Scan::Reduce, c)
            }
            // ActiveAllEqual: every lane's bits are the first lane's
            115 => {
                let v = self.scalar(args[1])?;
                let bits = self.b.bitcast_to(v, Kind::U);
                let first = self.gather(naga::GatherMode::BroadcastFirst, bits);
                let same = self.b.binary(Bo::Equal, bits, first);
                self.collective(Sop::All, Scan::Reduce, same)
            }
            116 => {
                let c = self.scalar(args[1])?;
                let c = self.b.bitcast_to(c, Kind::B);
                let ballot = self.ballot(c);
                let lanes: Vec<Lv> = (0..4).map(|k| Lv::Scalar(self.b.lane(ballot, k))).collect();
                return self.define_result(inst.result, Lv::Agg(lanes));
            }
            117 => {
                let v = self.scalar(args[1])?;
                let lane = self.uint(args[2])?;
                self.gather(naga::GatherMode::Shuffle(lane.expr), v)
            }
            118 => {
                let v = self.scalar(args[1])?;
                self.gather(naga::GatherMode::BroadcastFirst, v)
            }
            // ActiveOp, PrefixOp: (value, operation, signedness)
            119 | 121 => {
                let sign = konst(self, 3);
                let v = self.wave_operand(args[1], sign)?;
                let op = match konst(self, 2) {
                    Some(WAVE_SUM) => Sop::Add,
                    Some(WAVE_PRODUCT) => Sop::Mul,
                    Some(WAVE_MIN) if code == 119 => Sop::Min,
                    Some(WAVE_MAX) if code == 119 => Sop::Max,
                    _ => return self.refuse("a wave operation of a kind that is not lifted"),
                };
                let scan = if code == 119 {
                    Scan::Reduce
                } else {
                    Scan::ExclusiveScan
                };
                self.collective(op, scan, v)
            }
            // ActiveBit: and, or, xor
            120 => {
                let v = self.scalar(args[1])?;
                let v = self.b.bitcast_to(v, Kind::U);
                let op = match konst(self, 2) {
                    Some(0) => Sop::And,
                    Some(1) => Sop::Or,
                    Some(2) => Sop::Xor,
                    _ => return self.refuse("a wave bit operation of a kind that is not lifted"),
                };
                self.collective(op, Scan::Reduce, v)
            }
            // QuadReadLaneAt, QuadOp (across x, y, the diagonal)
            122 => {
                let v = self.scalar(args[1])?;
                let lane = self.uint(args[2])?;
                self.gather(naga::GatherMode::QuadBroadcast(lane.expr), v)
            }
            123 => {
                let v = self.scalar(args[1])?;
                let direction = match konst(self, 2) {
                    Some(0) => naga::Direction::X,
                    Some(1) => naga::Direction::Y,
                    Some(2) => naga::Direction::Diagonal,
                    _ => return self.refuse("a quad operation of a kind that is not lifted"),
                };
                self.gather(naga::GatherMode::QuadSwap(direction), v)
            }
            // AllBitCount, PrefixBitCount: the lanes whose condition holds, all of them or those before this one
            135 | 136 => {
                let c = self.scalar(args[1])?;
                let c = self.b.bitcast_to(c, Kind::B);
                let ballot = self.ballot(c);
                let lane = if code == 136 {
                    Some(self.system_input(iridia::SystemValue::LaneIndex)?)
                } else {
                    None
                };
                let mut total = self.b.lit_u(0);
                for k in 0..4u32 {
                    let word = self.b.lane(ballot, k as u8);
                    let word = match lane {
                        // the bits of this word below the lane: all, none, or the low bits up to it
                        Some(lane) => {
                            let base = self.b.lit_u(32 * k);
                            let relative = self.b.binary(Bo::Subtract, lane, base);
                            let thirty_two = self.b.lit_u(32);
                            let in_word = self.b.binary(Bo::Less, relative, thirty_two);
                            let above = self.b.binary(Bo::GreaterEqual, lane, base);
                            let one = self.b.lit_u(1);
                            let shifted = self.b.binary(Bo::ShiftLeft, one, relative);
                            let low = self.b.binary(Bo::Subtract, shifted, one);
                            let all = self.b.lit_u(u32::MAX);
                            let none = self.b.lit_u(0);
                            // lane in this word: the low bits; past it: every bit; before it: none
                            let before = self.b.select(in_word, low, all);
                            let mask = self.b.select(above, before, none);
                            self.b.binary(Bo::And, word, mask)
                        }
                        None => word,
                    };
                    let ones = self.b.math(naga::MathFunction::CountOneBits, &[word]);
                    total = self.b.binary(Bo::Add, total, ones);
                }
                total
            }
            other => return self.refuse(format!("wave operation {other}")),
        };
        // the result is the type the program says, whatever the operand's bits were carried as
        let result = match inst.result.map(|(_, t)| self.kind_of_type(t)) {
            Some(Ok(kind)) => self.b.bitcast_to(result, kind),
            _ => result,
        };
        self.define_result(inst.result, Lv::Scalar(result))
    }
}
