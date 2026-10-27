use std::collections::BTreeMap;

use super::flat::{
    AtomOp, BinOp, CvtOp, Executable, GlobalKind, LevelOp, Loc, MathOp, Member, Op, Pick, Ptr, Reg, Res, St,
    UnOp, WaveGather, WaveKind, WaveOp, WaveScan,
};
use super::mask::{Mask, Wide};
use super::rayquery::{Built, Query, Reported, Scenes};
use super::request::{
    Backend, Element, ImageAtomic, ImageLoad, ImageLod, ImageQuery, ImageSample, ImageStore,
};
use iridia::ResourceId;

/// The most operations and loop turns one invocation may execute: a program that does not end is refused.
pub const STEP_LIMIT: u64 = 1 << 24;

/// Where a global's words are during a run.
#[derive(Debug)]
enum Words<'a> {
    Owned(Vec<u32>),
    /// A constant buffer as the caller holds it: rows of four lanes.
    Rows(&'a [[u32; 4]]),
    Slice(&'a [u32]),
    /// The elements of an array of buffers; one the caller gave none of is `None`.
    Elements(Vec<Option<&'a [u32]>>),
    /// A buffer the backend answers reads of.
    Backend,
}

/// The buffers a program reads, ready for runs: built once per material and reused for every pixel.
#[derive(Debug)]
pub struct Bound<'a> {
    data: Vec<Words<'a>>,
    scenes: Scenes,
}

impl Bound<'_> {
    /// Give the acceleration structure of resource `id`: what the program's ray queries search.
    pub fn with_scene(mut self, id: ResourceId, scene: std::sync::Arc<Built>) -> Self {
        self.scenes.insert(id, scene);
        self
    }
}

/// What a run reads besides its inputs: constant buffers (rows of four lanes) and buffers (32-bit words), by resource.
#[derive(Clone, Debug, Default)]
pub struct Resources {
    pub constants: BTreeMap<ResourceId, Vec<[u32; 4]>>,
    pub buffers: BTreeMap<ResourceId, Vec<u32>>,
    /// The elements of arrays of buffers, by resource and element.
    pub elements: BTreeMap<(ResourceId, u32), Vec<u32>>,
    /// The acceleration structures, by resource.
    pub scenes: BTreeMap<ResourceId, super::rayquery::Scene>,
}

impl Executable {
    /// Bind the buffers the program reads, as `constants` and `buffers` find them by resource. A constant buffer the
    /// program reads and the caller does not give is an error; a buffer not given is one the backend answers reads of
    /// ([`Backend::read_buffer`]). Nothing is copied: the caller's buffers are read in place.
    pub fn bind<'a>(
        &self,
        constants: impl Fn(ResourceId) -> Option<&'a [[u32; 4]]>,
        buffers: impl Fn(ResourceId) -> Option<&'a [u32]>,
    ) -> Result<Bound<'a>, String> {
        self.bind_elements(constants, buffers, |_, _| None)
    }

    /// [`Executable::bind`] for a program that also reads arrays of buffers: `elements` finds the words of element
    /// `i` of the array resource `id`. An element not given errors when a run reads it.
    pub fn bind_elements<'a>(
        &self,
        constants: impl Fn(ResourceId) -> Option<&'a [[u32; 4]]>,
        buffers: impl Fn(ResourceId) -> Option<&'a [u32]>,
        elements: impl Fn(ResourceId, u32) -> Option<&'a [u32]>,
    ) -> Result<Bound<'a>, String> {
        let mut data = Vec::with_capacity(self.globals.len());
        for g in &self.globals {
            data.push(match &g.kind {
                GlobalKind::Constants(id) => Words::Rows(
                    constants(*id).ok_or_else(|| format!("no constant buffer for resource {}", id.0))?,
                ),
                GlobalKind::Private(words) => Words::Owned(words.clone()),
                GlobalKind::Storage(id) => buffers(*id).map_or(Words::Backend, Words::Slice),
                GlobalKind::StorageArray(id, count) => {
                    Words::Elements((0..*count).map(|k| elements(*id, k)).collect())
                }
                // thread-group memory is the run's own
                GlobalKind::Workgroup(_) => Words::Owned(Vec::new()),
            });
        }
        Ok(Bound {
            data,
            scenes: BTreeMap::new(),
        })
    }

    /// [`Executable::bind`] over a [`Resources`].
    pub fn bind_resources<'a>(&self, resources: &'a Resources) -> Result<Bound<'a>, String> {
        let mut bound = self.bind_elements(
            |id| resources.constants.get(&id).map(Vec::as_slice),
            |id| resources.buffers.get(&id).map(Vec::as_slice),
            |id, k| resources.elements.get(&(id, k)).map(Vec::as_slice),
        )?;
        for (id, scene) in &resources.scenes {
            bound.scenes.insert(*id, std::sync::Arc::new(Built::new(scene)?));
        }
        Ok(bound)
    }
}

/// What a ray tracing program decided about the hit it was offered, besides its payload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decision {
    /// Nothing: the hit stands.
    #[default]
    None,
    /// `IgnoreHit`: an any-hit program rejects the candidate.
    Ignore,
    /// `AcceptHitAndEndSearch`: an any-hit program accepts it and ends the search.
    AcceptAndEnd,
}

/// What one invocation produced.
#[derive(Clone, Debug)]
pub struct Output {
    /// The words of each pointer argument (a ray tracing program's payload, attributes or parameter) as the program
    /// left them, in argument order.
    pub pointees: Vec<Vec<u32>>,
    pub decision: Decision,
    /// One entry per member of the result, lanes beyond the member's width 0.
    pub values: Vec<[u32; 4]>,
    pub discarded: bool,
    /// Operations and loop turns executed.
    pub steps: u64,
    /// Sample requests this lane made.
    pub samples: u64,
}

/// The registers and local memory of a [`Runner`], kept from one run of a program to the next so that the literals are
/// written once and nothing is allocated per pixel.
#[derive(Debug)]
pub struct Scratch<const N: usize> {
    regs: Vec<Reg>,
    mem: Vec<u32>,
    /// Each lane's result members, their room kept.
    outs: [Vec<Reg>; N],
}
impl<const N: usize> Scratch<N> {
    pub fn new(exe: &Executable) -> Self {
        let mut regs = vec![[0u32; 4]; exe.slots * N];
        for (s, v) in &exe.inits {
            for l in 0..N {
                regs[*s as usize * N + l] = *v;
            }
        }
        Self {
            regs,
            mem: vec![0; exe.local_words * N],
            outs: std::array::from_fn(|_| Vec::new()),
        }
    }
}

/// Runs an [`Executable`] for a 2 x 2 quad in lockstep (`N` = 4: lanes 0 and 1 the top row, 2 and 3 the bottom), as a
/// GPU does: lanes that diverge are masked, a screen derivative is a difference inside the quad, a lane that is
/// discarded goes on as a helper. Any other `N` runs that many invocations of their own (`N` = 1, one), in lockstep
/// and masked where they diverge, each as it would run alone: its own steps and budget, a discard ending it, the
/// derivatives of its inputs the ones the caller gives for every lane. A runner is reused for any number of runs.
pub struct Runner<'a, const N: usize, M: Mask = u8> {
    c: &'a Executable,
    b: &'a Bound<'a>,
    regs: Vec<Reg>,
    mem: Vec<u32>,
    sampler: &'a mut dyn Backend,
    ftz: bool,
    grouping: Grouping,
    /// The steps of a quad or a thread group, counted once for all its lanes.
    steps: u64,
    /// The steps of each lane of independent invocations.
    lane_steps: [u64; N],
    samples: [u64; N],
    outs: [Vec<Reg>; N],
    /// What an any-hit program decided, per lane.
    decisions: [Decision; N],
    killed: M,
    /// The memory a run changes, by global: thread-group memory and the buffers a compute run writes (empty for a
    /// pixel run, which hands its writes to the backend).
    store: Vec<Option<Vec<u32>>>,
    /// The ray queries of each invocation, by query and lane.
    queries: Vec<Vec<Option<Query>>>,
    /// The same for the elements of arrays of buffers, by global and element.
    elem_store: Vec<Vec<Option<Vec<u32>>>>,
    /// The lanes of a thread group, and those of them that have left the program.
    group: M,
    retired: M,
    /// The lanes in use and the width of a wave of them (0 outside a compute run).
    lanes: usize,
    wave: usize,
    /// The derivatives of the arguments (scalar mode): per argument, d/dx then d/dy lanes.
    derivs: Option<&'a [[Reg; 2]]>,
}

impl<const N: usize, M: Mask> std::fmt::Debug for Runner<'_, N, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // independent invocations: the most steps a lane has taken
        let steps = match self.grouping {
            Grouping::Independent => self.lane_steps.iter().copied().max().unwrap_or(0),
            Grouping::Quad | Grouping::Group => self.steps,
        };
        f.debug_struct("Runner")
            .field("lanes", &N)
            .field("grouping", &self.grouping)
            .field("steps", &steps)
            .finish_non_exhaustive()
    }
}

/// What the lanes of a run are to each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grouping {
    /// Invocations of their own.
    Independent,
    /// A 2 x 2 quad.
    Quad,
    /// A compute thread group.
    Group,
}

#[derive(Clone, Copy)]
struct Exits<M> {
    brk: M,
    cont: M,
    ret: M,
}
impl<M: Mask> Default for Exits<M> {
    fn default() -> Self {
        Self {
            brk: M::none(),
            cont: M::none(),
            ret: M::none(),
        }
    }
}

/// `$body` for each lane `$l` of `$active` (a runner's lanes): without a test per lane when every lane is active.
macro_rules! each {
    ($active:expr, $l:ident, $body:block) => {
        if $active == M::first(N) {
            for $l in 0..N $body
        } else {
            for $l in 0..N {
                if $active.has($l) $body
            }
        }
    };
}

#[inline(always)]
fn fl(x: f32, on: bool) -> f32 {
    if on && x.is_subnormal() {
        f32::from_bits(x.to_bits() & 0x8000_0000)
    } else {
        x
    }
}
#[inline(always)]
fn f(x: u32) -> f32 {
    f32::from_bits(x)
}

impl<'a, const N: usize, M: Mask> Runner<'a, N, M> {
    /// A runner of `exe` over `bound`, answering requests through `backend`. Invocations of their own that read
    /// derivatives of their inputs take them in `derivatives`, the same for every lane; a quad takes its own.
    pub fn new(
        exe: &'a Executable,
        bound: &'a Bound<'a>,
        backend: &'a mut dyn Backend,
        derivatives: Option<&'a [[[u32; 4]; 2]]>,
    ) -> Self {
        Self::with_scratch(exe, bound, backend, derivatives, Scratch::new(exe))
    }

    /// [`Runner::new`] on registers kept from an earlier runner of the same program ([`Runner::into_scratch`]): the
    /// literals are already in them.
    pub fn with_scratch(
        exe: &'a Executable,
        bound: &'a Bound<'a>,
        backend: &'a mut dyn Backend,
        derivatives: Option<&'a [[[u32; 4]; 2]]>,
        scratch: Scratch<N>,
    ) -> Self {
        let c = exe;
        let (b, sampler, ftz, derivs) = (bound, backend, exe.flush_denormals, derivatives);
        Self {
            c,
            b,
            regs: scratch.regs,
            mem: scratch.mem,
            sampler,
            ftz,
            grouping: if N == 4 {
                Grouping::Quad
            } else {
                Grouping::Independent
            },
            steps: 0,
            lane_steps: [0; N],
            samples: [0; N],
            outs: scratch.outs,
            decisions: [Decision::None; N],
            killed: M::none(),
            store: Vec::new(),
            elem_store: Vec::new(),
            queries: Vec::new(),
            group: M::none(),
            retired: M::none(),
            lanes: 0,
            wave: 0,
            derivs,
        }
    }

    /// Give the registers back, to start the next runner of this program with.
    pub fn into_scratch(self) -> Scratch<N> {
        Scratch {
            regs: self.regs,
            mem: self.mem,
            outs: self.outs,
        }
    }

    /// Count `n` steps the lanes of `active` take, refusing a run past its budget: each lane's own for independent
    /// invocations, the quad's or the group's once.
    #[inline(always)]
    fn count(&mut self, active: M, n: u64) -> Result<(), String> {
        let over = if self.grouping == Grouping::Independent {
            let mut over = false;
            for l in 0..N {
                if active.has(l) {
                    self.lane_steps[l] += n;
                    over |= self.lane_steps[l] > STEP_LIMIT;
                }
            }
            over
        } else {
            self.steps += n;
            self.steps > STEP_LIMIT
        };
        if over {
            return Err("execution budget exceeded".into());
        }
        Ok(())
    }

    /// Where local word `word` of lane `lane` is: the words outermost and the lanes innermost, as in the registers.
    #[inline(always)]
    fn local(word: usize, lane: usize) -> usize {
        word * N + lane
    }
    /// The first local word of `n` from `word`, refusing words past the program's locals.
    #[inline(always)]
    fn locals(&self, word: usize, n: usize) -> Result<usize, String> {
        if word + n > self.c.local_words {
            return Err("local index out of range".into());
        }
        Ok(word)
    }
    /// Lane `lane`'s local words `at..at + n`.
    fn read_locals(&self, lane: usize, at: usize, n: usize) -> Vec<u32> {
        (at..at + n).map(|w| self.mem[Self::local(w, lane)]).collect()
    }
    /// Lane `lane`'s local words from `at` set to `words`.
    fn write_locals(&mut self, lane: usize, at: usize, words: &[u32]) {
        for (k, w) in words.iter().enumerate() {
            self.mem[Self::local(at + k, lane)] = *w;
        }
    }

    fn mem_get(&mut self, loc: Loc, lane: usize, extra: u32, n: u8) -> Result<Reg, String> {
        let mut r = [0u32; 4];
        match loc {
            Loc::Local(o) => {
                let base = self.locals((o + extra) as usize, n as usize)?;
                for (i, w) in r.iter_mut().take(n as usize).enumerate() {
                    *w = self.mem[Self::local(base + i, lane)];
                }
            }
            Loc::Elem(g, o, pick) => {
                let base = (o + extra) as usize;
                let e = self.element_index(g, pick, lane)?;
                let d = self.element_words(g, e)?;
                let s = d
                    .get(base..base + n as usize)
                    .ok_or("buffer index out of range")?;
                r[..n as usize].copy_from_slice(s);
            }
            Loc::Global(g, o) => {
                let base = (o + extra) as usize;
                if let Some(Some(d)) = self.store.get(g as usize) {
                    let s = d
                        .get(base..base + n as usize)
                        .ok_or("memory index out of range")?;
                    r[..n as usize].copy_from_slice(s);
                    return Ok(r);
                }
                let bound = self.b;
                match &bound.data[g as usize] {
                    Words::Elements(_) => return Err("an array of buffers read without an element".into()),
                    Words::Owned(d) => {
                        let s = d
                            .get(base..base + n as usize)
                            .ok_or("buffer index out of range")?;
                        r[..n as usize].copy_from_slice(s);
                    }
                    Words::Slice(d) => {
                        let s = d
                            .get(base..base + n as usize)
                            .ok_or("buffer index out of range")?;
                        r[..n as usize].copy_from_slice(s);
                    }
                    Words::Rows(rows) => {
                        if n == 4 && base.is_multiple_of(4) {
                            r = *rows.get(base / 4).ok_or("buffer index out of range")?;
                        } else {
                            for (i, w) in r.iter_mut().take(n as usize).enumerate() {
                                let word = base + i;
                                *w = rows.get(word / 4).ok_or("buffer index out of range")?[word % 4];
                            }
                        }
                    }
                    // a buffer the caller answers for word by word
                    Words::Backend => {
                        let GlobalKind::Storage(view) = self.c.globals[g as usize].kind else {
                            return Err("a global that is neither bound nor a buffer".into());
                        };
                        r = self.sampler.read_buffer(view, base as u32, n)?;
                        for w in r.iter_mut().skip(n as usize) {
                            *w = 0;
                        }
                    }
                }
            }
        }
        Ok(r)
    }
    fn mem_put(&mut self, loc: Loc, lane: usize, extra: u32, n: u8, v: Reg) -> Result<(), String> {
        match loc {
            Loc::Local(o) => {
                let base = self.locals((o + extra) as usize, n as usize)?;
                for (i, w) in v.iter().take(n as usize).enumerate() {
                    self.mem[Self::local(base + i, lane)] = *w;
                }
                Ok(())
            }
            Loc::Elem(g, o, pick) => {
                let e = self.element_index(g, pick, lane)?;
                let at = (o + extra) as usize;
                let Some(Some(d)) = self
                    .elem_store
                    .get_mut(g as usize)
                    .and_then(|v| v.get_mut(e as usize))
                else {
                    return Err("a store to an element of an array of buffers the run does not write".into());
                };
                let s = d
                    .get_mut(at..at + n as usize)
                    .ok_or("memory index out of range")?;
                s.copy_from_slice(&v[..n as usize]);
                Ok(())
            }
            Loc::Global(g, o) => {
                if let Some(Some(d)) = self.store.get_mut(g as usize) {
                    let at = (o + extra) as usize;
                    let s = d
                        .get_mut(at..at + n as usize)
                        .ok_or("memory index out of range")?;
                    s.copy_from_slice(&v[..n as usize]);
                    return Ok(());
                }
                // a write to a storage buffer is the caller's effect (it reaches none of this evaluation's reads)
                let GlobalKind::Storage(view) = self.c.globals[g as usize].kind else {
                    return Err("store to a read-only global".into());
                };
                self.sampler
                    .store_buffer(view, o + extra, v, ((1u16 << n) - 1) as u8)
            }
        }
    }
    /// The element of an array of buffers a location names in `lane`.
    fn element_index(&self, g: u32, pick: Pick, lane: usize) -> Result<u32, String> {
        let index = match pick {
            Pick::One => 0,
            Pick::Fixed(i) => i,
            Pick::Slot(s) => self.regs[s as usize * N + lane][0],
        };
        match &self.c.globals[g as usize].kind {
            GlobalKind::StorageArray(id, count) if index >= *count => {
                Err(format!("buffer {}: element {index} of an array of {count}", id.0))
            }
            _ => Ok(index),
        }
    }
    /// The words of element `e` of the array of buffers `g`: those the run writes, else those it was bound with.
    fn element_words(&self, g: u32, e: u32) -> Result<&[u32], String> {
        if let Some(Some(d)) = self.elem_store.get(g as usize).and_then(|v| v.get(e as usize)) {
            return Ok(d);
        }
        match &self.b.data[g as usize] {
            Words::Elements(v) => v
                .get(e as usize)
                .copied()
                .flatten()
                .ok_or_else(|| format!("no words given for element {e} of an array of buffers")),
            _ => Err("a global that is not an array of buffers".into()),
        }
    }
    fn dyn_extra(&self, p: &Ptr, lane: usize) -> Result<u32, String> {
        match p.dynamic {
            None => Ok(0),
            Some((s, stride, len)) => {
                let i = self.regs[s as usize * N + lane][0];
                if i >= len {
                    return Err(format!("dynamic index {i} past the end ({len})"));
                }
                Ok(i * stride)
            }
        }
    }

    fn ops(&mut self, lo: u32, hi: u32, active: M) -> Result<(), String> {
        self.count(active, (hi - lo) as u64)?;
        let c = self.c;
        let ftz = self.ftz;
        for k in lo..hi {
            let (dst, op) = &c.ops[k as usize];
            let dst = *dst as usize * N;
            match op {
                // a local's words, at offsets the compiler checked: read straight from every lane's memory
                Op::Load {
                    loc: Loc::Local(o),
                    n,
                } => {
                    let (base, n) = (self.locals(*o as usize, *n as usize)?, *n as usize);
                    each!(active, l, {
                        let mut r = [0u32; 4];
                        for (i, w) in r.iter_mut().take(n).enumerate() {
                            *w = self.mem[Self::local(base + i, l)];
                        }
                        self.regs[dst + l] = r;
                    })
                }
                Op::LoadLanes {
                    loc: Loc::Local(o),
                    offs,
                    n,
                } => {
                    let (o, n) = (*o as usize, *n as usize);
                    for &k in &offs[..n] {
                        self.locals(o + k as usize, 1)?;
                    }
                    each!(active, l, {
                        let mut r = [0u32; 4];
                        for (w, &k) in r.iter_mut().zip(&offs[..n]) {
                            *w = self.mem[Self::local(o + k as usize, l)];
                        }
                        self.regs[dst + l] = r;
                    })
                }
                Op::Load { loc, n } => each!(active, l, {
                    self.regs[dst + l] = self.mem_get(*loc, l, 0, *n)?;
                }),
                Op::LoadLanes { loc, offs, n } => each!(active, l, {
                    let mut r = [0u32; 4];
                    for i in 0..*n as usize {
                        r[i] = self.mem_get(*loc, l, offs[i] as u32, 1)?[0];
                    }
                    self.regs[dst + l] = r;
                }),
                Op::LoadDyn { ptr, n } => each!(active, l, {
                    let x = self.dyn_extra(ptr, l)?;
                    self.regs[dst + l] = self.mem_get(ptr.loc, l, x, *n)?;
                }),
                Op::Swizzle { src, pat, n } => {
                    let src = *src as usize * N;
                    each!(active, l, {
                        let s = self.regs[src + l];
                        let mut r = [0u32; 4];
                        for i in 0..*n as usize {
                            r[i] = s[pat[i] as usize];
                        }
                        self.regs[dst + l] = r;
                    })
                }
                Op::Lane { src, i } => {
                    let (src, i) = (*src as usize * N, *i as usize);
                    each!(active, l, {
                        self.regs[dst + l] = [self.regs[src + l][i]; 4];
                    })
                }
                Op::Splat { src, n } => {
                    let _ = n;
                    let src = *src as usize * N;
                    each!(active, l, {
                        self.regs[dst + l] = [self.regs[src + l][0]; 4];
                    })
                }
                Op::Compose { parts, np } => each!(active, l, {
                    let mut r = [0u32; 4];
                    let mut at = 0;
                    for p in &parts[..*np as usize] {
                        let s = self.regs[p.0 as usize * N + l];
                        for w in &s[..p.1 as usize] {
                            r[at] = *w;
                            at += 1;
                        }
                    }
                    self.regs[dst + l] = r;
                }),
                Op::Un { op, src, n } => {
                    let (src, n) = (*src as usize * N, *n as usize);
                    // each operation's own loop over the lanes
                    macro_rules! un {
                        ($w:ident => $e:expr) => {
                            each!(active, l, {
                                let s = self.regs[src + l];
                                let mut r = [0u32; 4];
                                for i in 0..n {
                                    let $w = s[i];
                                    r[i] = $e;
                                }
                                self.regs[dst + l] = r;
                            })
                        };
                    }
                    match op {
                        UnOp::NegF => un!(w => w ^ 0x8000_0000),
                        UnOp::NegI => un!(w => (w as i32).wrapping_neg() as u32),
                        UnOp::Not => un!(w => (w == 0) as u32),
                        UnOp::BitNot => un!(w => !w),
                    }
                }
                Op::Bin { op, a, b, n, a1, b1 } => {
                    let (a, b, n) = (*a as usize * N, *b as usize * N, *n as usize);
                    // a scalar operand's lane 0 for every component
                    let (sa, sb) = (usize::from(!*a1), usize::from(!*b1));
                    // each operation's own loop over the lanes, `bin` of a constant operation inlined into it
                    macro_rules! bin {
                        ($($o:ident),*) => {
                            match op {
                                $(BinOp::$o => each!(active, l, {
                                    let x = self.regs[a + l];
                                    let y = self.regs[b + l];
                                    let mut r = [0u32; 4];
                                    for i in 0..n {
                                        r[i] = bin(BinOp::$o, x[i * sa], y[i * sb], ftz)?;
                                    }
                                    self.regs[dst + l] = r;
                                }),)*
                            }
                        };
                    }
                    bin!(
                        AddF, SubF, MulF, DivF, ModF, AddU, SubU, MulU, DivU, ModU, And, Or, Xor, Shl, ShrU,
                        ShrI, EqW, NeW, EqF, NeF, LtF, LeF, GtF, GeF, LtU, LeU, GtU, GeU, LtI, LeI, GtI, GeI,
                        LAnd, LOr
                    )
                }
                Op::Select { c: cs, a, b, n, c1 } => {
                    let (cs, a, b, n) = (*cs as usize * N, *a as usize * N, *b as usize * N, *n as usize);
                    let sc = usize::from(!*c1);
                    each!(active, l, {
                        let cc = self.regs[cs + l];
                        let x = self.regs[a + l];
                        let y = self.regs[b + l];
                        let mut r = [0u32; 4];
                        for i in 0..n {
                            r[i] = if cc[i * sc] != 0 { x[i] } else { y[i] };
                        }
                        self.regs[dst + l] = r;
                    })
                }
                Op::Cvt { op, src, n } => {
                    let (src, n) = (*src as usize * N, *n as usize);
                    macro_rules! cvt {
                        ($w:ident => $e:expr) => {
                            each!(active, l, {
                                let s = self.regs[src + l];
                                let mut r = [0u32; 4];
                                for i in 0..n {
                                    let $w = s[i];
                                    r[i] = $e;
                                }
                                self.regs[dst + l] = r;
                            })
                        };
                    }
                    match op {
                        CvtOp::F2I => cvt!(w => (f(w) as i32) as u32),
                        CvtOp::F2U => cvt!(w => f(w) as u32),
                        CvtOp::I2F => cvt!(w => (w as i32 as f32).to_bits()),
                        CvtOp::U2F => cvt!(w => (w as f32).to_bits()),
                        CvtOp::B2F => cvt!(w => (if w != 0 { 1.0f32 } else { 0.0 }).to_bits()),
                        CvtOp::B2U => cvt!(w => u32::from(w != 0)),
                    }
                }
                Op::Math {
                    f: mf,
                    a,
                    b,
                    c: cc,
                    n,
                    c1,
                } => {
                    let (a, b, cc) = (*a as usize * N, *b as usize * N, *cc as usize * N);
                    each!(active, l, {
                        let x = self.regs[a + l];
                        let y = self.regs[b + l];
                        let mut z = self.regs[cc + l];
                        if *c1 {
                            z = [z[0]; 4];
                        }
                        self.regs[dst + l] = math(*mf, x, y, z, *n, ftz);
                    })
                }
                Op::CopyWords { src, dst, words } => {
                    for l in 0..N {
                        if active.has(l) {
                            for k in 0..*words {
                                let w = self.mem_get(*src, l, k, 1)?[0];
                                let at = self.locals((*dst + k) as usize, 1)?;
                                self.mem[Self::local(at, l)] = w;
                            }
                        }
                    }
                }
                Op::RqHit { q, committed, first } => {
                    for l in 0..N {
                        if !active.has(l) {
                            continue;
                        }
                        let b = self.b;
                        let query = self.query_slots(*q)[l]
                            .as_ref()
                            .ok_or("a ray query read before it was initialized")?;
                        let scene = b
                            .scenes
                            .get(&query.scene())
                            .ok_or("a ray query without its scene")?;
                        let hit = if *committed {
                            &query.committed
                        } else {
                            &query.candidate
                        };
                        let r = hit
                            .as_ref()
                            .map_or_else(Reported::none, |h| Reported::of(h, scene));
                        let first = *first as usize;
                        let mut put = |k: usize, v: [u32; 4]| self.regs[(first + k) * N + l] = v;
                        put(0, [r.kind; 4]);
                        put(1, [r.t.to_bits(); 4]);
                        put(2, [r.custom; 4]);
                        put(3, [r.instance; 4]);
                        put(4, [r.sbt; 4]);
                        put(5, [r.geometry; 4]);
                        put(6, [r.primitive; 4]);
                        put(
                            7,
                            [r.barycentrics[0].to_bits(), r.barycentrics[1].to_bits(), 0, 0],
                        );
                        put(8, [u32::from(r.front); 4]);
                        for c in 0..4 {
                            let o = r.object_to_world[c].map(f32::to_bits);
                            put(9 + c, [o[0], o[1], o[2], 0]);
                            let w = r.world_to_object[c].map(f32::to_bits);
                            put(13 + c, [w[0], w[1], w[2], 0]);
                        }
                    }
                }
                Op::ArrLen {
                    gid,
                    elem_words,
                    pick: Some(pick),
                } => {
                    for l in 0..N {
                        if active.has(l) {
                            let e = self.element_index(*gid, *pick, l)?;
                            let words = self.element_words(*gid, e)?.len() as u32;
                            self.regs[dst + l] = [words / elem_words; 4];
                        }
                    }
                }
                Op::ArrLen {
                    gid,
                    elem_words,
                    pick: None,
                } => {
                    let bound = self.b;
                    let words = match &bound.data[*gid as usize] {
                        _ if matches!(self.store.get(*gid as usize), Some(Some(_))) => {
                            self.store[*gid as usize].as_ref().map_or(0, Vec::len) as u32
                        }
                        Words::Elements(_) => {
                            return Err("arrayLength of an array of buffers without an element".into());
                        }
                        Words::Owned(d) => d.len() as u32,
                        Words::Slice(d) => d.len() as u32,
                        Words::Rows(d) => d.len() as u32 * 4,
                        Words::Backend => match self.c.globals[*gid as usize].kind {
                            GlobalKind::Storage(view) => self.sampler.buffer_words(view)?,
                            _ => return Err("arrayLength of a global that is not a buffer".into()),
                        },
                    };
                    let len = words / elem_words;
                    for l in 0..N {
                        self.regs[dst + l] = [len; 4];
                    }
                }
                Op::Deriv {
                    src,
                    axis,
                    fine,
                    n,
                    prov,
                } => {
                    if self.grouping == Grouping::Quad {
                        if active != M::first(4) {
                            return Err("a quad's lanes diverge at a derivative".into());
                        }
                        let v: [Reg; 4] = std::array::from_fn(|l| self.regs[*src as usize * N + l]);
                        for l in 0..4 {
                            let (from, to) = match (*axis == 0, *fine) {
                                (true, false) => (0, 1),
                                (false, false) => (0, 2),
                                (true, true) => {
                                    if l < 2 {
                                        (0, 1)
                                    } else {
                                        (2, 3)
                                    }
                                }
                                (false, true) => {
                                    if l % 2 == 0 {
                                        (0, 2)
                                    } else {
                                        (1, 3)
                                    }
                                }
                            };
                            let mut r = [0u32; 4];
                            for i in 0..*n as usize {
                                r[i] = fl(fl(f(v[to][i]), ftz) - fl(f(v[from][i]), ftz), ftz).to_bits();
                            }
                            self.regs[dst + l] = r;
                        }
                    } else {
                        let p = prov.as_ref().ok_or("a derivative of a computed value (a scalar evaluation has those of its inputs only)")?;
                        let d = self.derivs.ok_or("no derivatives given")?;
                        let mut r = [0u32; 4];
                        for i in 0..*n as usize {
                            let (arg, lane) = p[i];
                            r[i] = d
                                .get(arg as usize)
                                .ok_or("no derivatives given for the argument")?
                                [*axis as usize][lane as usize];
                        }
                        for l in 0..N {
                            if active.has(l) {
                                self.regs[dst + l] = r;
                            }
                        }
                    }
                }
                Op::Sample {
                    tex,
                    smp,
                    coord,
                    cn,
                    layer,
                    level,
                    dim,
                    offset,
                    gather,
                    depth_ref,
                    depth,
                } => {
                    let cn = *cn as usize;
                    let c3 = |me: &Self, l: usize, s: u32| -> [f32; 3] {
                        let r = me.regs[s as usize * N + l];
                        std::array::from_fn(|i| if i < cn { fl(f(r[i]), ftz) } else { 0.0 })
                    };
                    // a quad's implicit level of detail comes from the coordinates' differences
                    let footprint = if self.grouping == Grouping::Quad
                        && matches!(level, LevelOp::Auto | LevelOp::Bias(_))
                    {
                        if active != M::first(4) {
                            return Err("a quad's lanes diverge at an implicit-LOD sample".into());
                        }
                        let uv: [[f32; 3]; 4] = std::array::from_fn(|l| c3(self, l, *coord));
                        let d = |a: usize| -> [f32; 3] { std::array::from_fn(|i| uv[a][i] - uv[0][i]) };
                        Some([d(1), d(2)])
                    } else {
                        None
                    };
                    for l in 0..N {
                        if !active.has(l) {
                            continue;
                        }
                        let rd = |s: u32, me: &Self| me.regs[s as usize * N + l];
                        let lod = match (level, footprint) {
                            (LevelOp::Auto, None) => ImageLod::Implicit,
                            (LevelOp::Auto, Some(g)) => ImageLod::Gradients(g),
                            (LevelOp::Zero, _) => ImageLod::Level(0.0),
                            (LevelOp::Exact(s), _) => ImageLod::Level(fl(f(rd(*s, self)[0]), ftz)),
                            (LevelOp::Bias(s), None) => ImageLod::Bias(fl(f(rd(*s, self)[0]), ftz)),
                            (LevelOp::Bias(s), Some(g)) => {
                                ImageLod::GradientsBias(g, fl(f(rd(*s, self)[0]), ftz))
                            }
                            (LevelOp::Gradient(x, y), _) => {
                                ImageLod::Gradients([c3(self, l, *x), c3(self, l, *y)])
                            }
                        };
                        let req = ImageSample {
                            texture: self.element(tex, l)?,
                            sampler: self.element(smp, l)?,
                            dim: *dim,
                            coord: c3(self, l, *coord),
                            layer: layer.map(|s| rd(s, self)[0] as f32),
                            lod,
                            offset: *offset,
                            gather: *gather,
                            depth_ref: depth_ref.map(|s| fl(f(rd(s, self)[0]), ftz)),
                            depth: *depth,
                        };
                        let r = self.sampler.sample(req)?;
                        self.samples[l] += 1;
                        self.regs[dst + l] = [r[0].to_bits(), r[1].to_bits(), r[2].to_bits(), r[3].to_bits()];
                    }
                }
                Op::Texel {
                    tex,
                    coord,
                    cn,
                    dim,
                    storage,
                    layer,
                    sample,
                    mip,
                } => {
                    for l in 0..N {
                        if !active.has(l) {
                            continue;
                        }
                        let cd = self.regs[*coord as usize * N + l];
                        let one = |s: &Option<u32>| s.map(|s| self.regs[s as usize * N + l][0] as i32);
                        let req = ImageLoad {
                            texture: self.element(tex, l)?,
                            dim: *dim,
                            storage: *storage,
                            texel: std::array::from_fn(|i| if i < *cn as usize { cd[i] as i32 } else { 0 }),
                            layer: one(layer),
                            sample: one(sample),
                            mip: one(mip),
                        };
                        self.regs[dst + l] = self.sampler.load(req)?;
                    }
                }
                Op::Query {
                    tex,
                    dim,
                    kind,
                    level,
                } => {
                    for l in 0..N {
                        if !active.has(l) {
                            continue;
                        }
                        let level = level.map(|s| self.regs[s as usize * N + l][0]);
                        let texture = self.element(tex, l)?;
                        self.regs[dst + l] = self.sampler.query(ImageQuery {
                            texture,
                            dim: *dim,
                            kind: *kind,
                            level,
                        })?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The lanes' states of ray query `q`.
    fn query_slots(&mut self, q: u32) -> &mut Vec<Option<Query>> {
        if self.queries.len() <= q as usize {
            self.queries.resize_with(q as usize + 1, Vec::new);
        }
        let slots = &mut self.queries[q as usize];
        if slots.len() < N {
            slots.resize(N, None);
        }
        slots
    }

    /// The wave width and the lane count of a compute run; wave operations outside one are errors.
    fn wave_shape(&self) -> Result<(usize, usize), String> {
        if self.wave == 0 || self.wave > 128 {
            return Err("a wave operation outside a compute run (or of a wave wider than 128 lanes)".into());
        }
        Ok((self.wave, self.lanes))
    }

    /// The resource an operation names for lane `l`: for an array, the element its index picks in that lane.
    fn element(&self, res: &Res, l: usize) -> Result<Element, String> {
        let index = match res.pick {
            Pick::One => 0,
            Pick::Fixed(i) => i,
            Pick::Slot(s) => self.regs[s as usize * N + l][0],
        };
        if !matches!(res.pick, Pick::One) && res.count != iridia::Resource::UNBOUNDED && index >= res.count {
            return Err(format!(
                "resource {}: element {index} of an array of {} (D3D12 leaves it undefined)",
                res.id.0, res.count
            ));
        }
        Ok(Element { id: res.id, index })
    }

    fn block(&mut self, b: &'a [St], active: M) -> Result<Exits<M>, String> {
        let mut ex = Exits::<M>::default();
        let mut active = active;
        for st in b {
            if active.is_empty() {
                break;
            }
            match st {
                St::Run(lo, hi) => self.ops(*lo, *hi, active)?,
                St::Store {
                    loc: Loc::Local(o),
                    src,
                    n,
                } => {
                    let (base, n, src) = (
                        self.locals(*o as usize, *n as usize)?,
                        *n as usize,
                        *src as usize * N,
                    );
                    for l in 0..N {
                        if active.has(l) {
                            let v = self.regs[src + l];
                            for (i, w) in v.iter().take(n).enumerate() {
                                self.mem[Self::local(base + i, l)] = *w;
                            }
                        }
                    }
                }
                St::Store { loc, src, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            let v = self.regs[*src as usize * N + l];
                            self.mem_put(*loc, l, 0, *n, v)?;
                        }
                    }
                }
                St::StoreDyn { ptr, src, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            let x = self.dyn_extra(ptr, l)?;
                            let v = self.regs[*src as usize * N + l];
                            self.mem_put(ptr.loc, l, x, *n, v)?;
                        }
                    }
                }
                St::ImageStore {
                    tex,
                    dim,
                    coord,
                    cn,
                    layer,
                    value,
                    vn,
                } => {
                    for l in 0..N {
                        if active.has(l) {
                            let cd = self.regs[*coord as usize * N + l];
                            let element = self.element(tex, l)?;
                            self.sampler.store_image(ImageStore {
                                view: element.id,
                                index: element.index,
                                dim: *dim,
                                texel: std::array::from_fn(|i| if i < *cn as usize { cd[i] } else { 0 }),
                                layer: layer.map(|s| self.regs[s as usize * N + l][0]),
                                value: self.regs[*value as usize * N + l],
                                lanes: *vn,
                            })?;
                        }
                    }
                }
                St::ImageAtomic {
                    tex,
                    dim,
                    coord,
                    cn,
                    layer,
                    op,
                    signed,
                    value,
                    result,
                } => {
                    for l in 0..N {
                        if active.has(l) {
                            let cd = self.regs[*coord as usize * N + l];
                            let element = self.element(tex, l)?;
                            let old = self.sampler.atomic_image(ImageAtomic {
                                view: element.id,
                                index: element.index,
                                dim: *dim,
                                texel: std::array::from_fn(|i| if i < *cn as usize { cd[i] } else { 0 }),
                                layer: layer.map(|s| self.regs[s as usize * N + l][0]),
                                op: *op,
                                signed: *signed,
                                value: self.regs[*value as usize * N + l][0],
                            })?;
                            if let Some(r) = result {
                                self.regs[*r as usize * N + l] = [old, 0, 0, 0];
                            }
                        }
                    }
                }
                St::Atomic {
                    ptr,
                    fun,
                    signed,
                    value,
                    compare,
                    result,
                    exchanged,
                } => {
                    for l in 0..N {
                        if active.has(l) {
                            let (g, o, elem) = match ptr.loc {
                                Loc::Global(g, o) => (g, o, None),
                                Loc::Elem(g, o, pick) => (g, o, Some(self.element_index(g, pick, l)?)),
                                Loc::Local(_) => return Err("atomic on a local".into()),
                            };
                            let word = (o + self.dyn_extra(ptr, l)?) as usize;
                            let v = self.regs[*value as usize * N + l][0];
                            let expected = compare.map_or(0, |c| self.regs[c as usize * N + l][0]);
                            let memory = match elem {
                                Some(e) => self
                                    .elem_store
                                    .get_mut(g as usize)
                                    .and_then(|v| v.get_mut(e as usize)),
                                None => self.store.get_mut(g as usize),
                            };
                            let (old, swapped) = if let Some(Some(d)) = memory {
                                let cell = d.get_mut(word).ok_or("atomic past the end of the memory")?;
                                let old = *cell;
                                let (new, swapped) = atomic(*fun, *signed, old, v, expected);
                                *cell = new;
                                (old, swapped)
                            } else {
                                let view = match self.c.globals[g as usize].kind {
                                    GlobalKind::Storage(view) => view,
                                    GlobalKind::StorageArray(..) => {
                                        return Err("atomic on an element of an array of buffers this run does not hold (only a compute run writes them; the backend is not asked for element atomics)".into());
                                    }
                                    _ => return Err("atomic on a read-only global".into()),
                                };
                                if *fun != AtomOp::Add {
                                    return Err(format!(
                                        "atomic {fun:?} on a buffer the backend holds (only add is passed on)"
                                    ));
                                }
                                (self.sampler.atomic_add(view, word as u32, v)?, true)
                            };
                            if let Some(r) = result {
                                self.regs[*r as usize * N + l] = [old; 4];
                            }
                            if let Some(x) = exchanged {
                                self.regs[*x as usize * N + l] = [u32::from(swapped); 4];
                            }
                        }
                    }
                }
                St::WaveBallot { predicate, result } => {
                    let (w, lanes) = self.wave_shape()?;
                    for base in (0..lanes).step_by(w) {
                        let end = (base + w).min(lanes);
                        let mut bits = [0u32; 4];
                        for l in base..end {
                            let on = predicate.is_none_or(|p| self.regs[p as usize * N + l][0] != 0);
                            if active.has(l) && on {
                                bits[(l - base) / 32] |= 1 << ((l - base) % 32);
                            }
                        }
                        for l in base..end {
                            if active.has(l) {
                                self.regs[*result as usize * N + l] = bits;
                            }
                        }
                    }
                }
                St::WaveGather {
                    mode,
                    value,
                    n,
                    result,
                } => {
                    let (w, lanes) = self.wave_shape()?;
                    for base in (0..lanes).step_by(w) {
                        let end = (base + w).min(lanes);
                        let first = (base..end).find(|&l| active.has(l));
                        let mut taken: Vec<(usize, Reg)> = Vec::new();
                        for l in base..end {
                            if !active.has(l) {
                                continue;
                            }
                            let at = l - base;
                            let arg = |s: u32| self.regs[s as usize * N + l][0] as usize;
                            let from = match mode {
                                WaveGather::First => first.unwrap_or(l),
                                WaveGather::Lane(s) => base + arg(*s),
                                WaveGather::Shuffle(s) => base + arg(*s),
                                WaveGather::Down(s) => base + at + arg(*s),
                                WaveGather::Up(s) if arg(*s) <= at => base + at - arg(*s),
                                WaveGather::Up(_) => base + at,
                                WaveGather::Xor(s) => base + (at ^ arg(*s)),
                                WaveGather::QuadLane(s) => base + (at & !3) + (arg(*s) & 3),
                                WaveGather::QuadSwap(x) => base + (at ^ *x as usize),
                            };
                            // a lane outside the wave is undefined in D3D: the lane's own value
                            let from = if from < end { from } else { l };
                            let mut r = self.regs[*value as usize * N + from];
                            for w in r.iter_mut().skip(*n as usize) {
                                *w = 0;
                            }
                            taken.push((l, r));
                        }
                        for (l, r) in taken {
                            self.regs[*result as usize * N + l] = r;
                        }
                    }
                }
                St::WaveOp {
                    op,
                    scan,
                    value,
                    n,
                    kind,
                    result,
                } => {
                    let (w, lanes) = self.wave_shape()?;
                    let ftz = self.ftz;
                    for base in (0..lanes).step_by(w) {
                        let end = (base + w).min(lanes);
                        let members: Vec<usize> = (base..end).filter(|&l| active.has(l)).collect();
                        for i in 0..*n as usize {
                            let values: Vec<u32> = members
                                .iter()
                                .map(|&l| self.regs[*value as usize * N + l][i])
                                .collect();
                            let total = values
                                .iter()
                                .copied()
                                .reduce(|a, b| wave_combine(*op, *kind, a, b, ftz))
                                .unwrap_or(0);
                            let mut running = wave_identity(*op, *kind);
                            for (k, &l) in members.iter().enumerate() {
                                let out = match scan {
                                    WaveScan::Reduce => total,
                                    WaveScan::Inclusive => {
                                        running = if k == 0 {
                                            values[0]
                                        } else {
                                            wave_combine(*op, *kind, running, values[k], ftz)
                                        };
                                        running
                                    }
                                    WaveScan::Exclusive => {
                                        let before = running;
                                        running = if k == 0 {
                                            values[0]
                                        } else {
                                            wave_combine(*op, *kind, running, values[k], ftz)
                                        };
                                        before
                                    }
                                };
                                self.regs[*result as usize * N + l][i] = out;
                            }
                        }
                    }
                }
                St::StoreAgg { loc, members } => {
                    for l in 0..N {
                        if active.has(l) {
                            let mut at = 0;
                            for &(s, n) in members {
                                let v = self.regs[s as usize * N + l];
                                self.mem_put(*loc, l, at, n, v)?;
                                at += u32::from(n);
                            }
                        }
                    }
                }
                St::RqInit {
                    q,
                    scene,
                    flags,
                    mask,
                    tmin,
                    tmax,
                    origin,
                    direction,
                } => {
                    if !self.b.scenes.contains_key(scene) {
                        return Err(format!(
                            "no acceleration structure given for resource {}",
                            scene.0
                        ));
                    }
                    for l in 0..N {
                        if active.has(l) {
                            let reg = |s: u32| self.regs[s as usize * N + l];
                            let float = |s: u32| fl(f(reg(s)[0]), self.ftz);
                            let vec3 = |s: u32| -> [f32; 3] {
                                let r = reg(s);
                                [
                                    fl(f(r[0]), self.ftz),
                                    fl(f(r[1]), self.ftz),
                                    fl(f(r[2]), self.ftz),
                                ]
                            };
                            let query = Query::new(
                                *scene,
                                reg(*flags)[0],
                                reg(*mask)[0],
                                float(*tmin),
                                float(*tmax),
                                vec3(*origin),
                                vec3(*direction),
                            );
                            self.query_slots(*q)[l] = Some(query);
                        }
                    }
                }
                St::RqProceed { q, result } => {
                    for l in 0..N {
                        if active.has(l) {
                            let b = self.b;
                            let query = self.query_slots(*q)[l]
                                .as_mut()
                                .ok_or("a ray query proceeded before it was initialized")?;
                            let scene = b
                                .scenes
                                .get(&query.scene())
                                .ok_or("a ray query without its scene")?;
                            let more = query.proceed(scene);
                            self.regs[*result as usize * N + l] = [u32::from(more); 4];
                        }
                    }
                }
                St::RqConfirm { q } => {
                    for l in 0..N {
                        if active.has(l) {
                            self.query_slots(*q)[l]
                                .as_mut()
                                .ok_or("a ray query confirmed before it was initialized")?
                                .confirm()?;
                        }
                    }
                }
                St::RtDecide(decision) => {
                    for l in 0..N {
                        if active.has(l) {
                            self.decisions[l] = *decision;
                        }
                    }
                }
                St::RtTrace {
                    scene,
                    operands,
                    payload,
                } => {
                    for l in 0..N {
                        if !active.has(l) {
                            continue;
                        }
                        let word = |k: usize| self.regs[operands[k] as usize * N + l][0];
                        let vec3 = |k: usize| -> [f32; 3] {
                            let r = self.regs[operands[k] as usize * N + l];
                            [f(r[0]), f(r[1]), f(r[2])]
                        };
                        let (flags, mask, offset, stride, miss) =
                            (word(0), word(1), word(2), word(3), word(4));
                        let (origin, direction) = (vec3(5), vec3(7));
                        let (t_min, t_max) = (f(word(6)), f(word(8)));
                        let at = payload.0 as usize;
                        let mut words = self.read_locals(l, at, payload.1 as usize);
                        self.sampler.trace_ray(super::request::TraceRay {
                            scene: *scene,
                            flags,
                            mask,
                            hit_group_offset: offset,
                            hit_group_stride: stride,
                            miss_index: miss,
                            origin,
                            t_min,
                            direction,
                            t_max,
                            payload: &mut words,
                        })?;
                        self.write_locals(l, at, &words);
                    }
                }
                St::RtReport {
                    t,
                    kind,
                    attributes,
                    result,
                } => {
                    for l in 0..N {
                        if !active.has(l) {
                            continue;
                        }
                        let at = attributes.0 as usize;
                        let mut words = self.read_locals(l, at, attributes.1 as usize);
                        let accepted = self.sampler.report_hit(super::request::ReportHit {
                            t: f(self.regs[*t as usize * N + l][0]),
                            hit_kind: self.regs[*kind as usize * N + l][0],
                            attributes: &mut words,
                        })?;
                        self.write_locals(l, at, &words);
                        if let Some(r) = result {
                            self.regs[*r as usize * N + l] = [u32::from(accepted); 4];
                        }
                    }
                }
                St::RtCall { index, parameter } => {
                    for l in 0..N {
                        if !active.has(l) {
                            continue;
                        }
                        let at = parameter.0 as usize;
                        let mut words = self.read_locals(l, at, parameter.1 as usize);
                        self.sampler.call_shader(super::request::CallShader {
                            index: self.regs[*index as usize * N + l][0],
                            parameter: &mut words,
                        })?;
                        self.write_locals(l, at, &words);
                    }
                }
                St::RqGenerate { q, t } => {
                    for l in 0..N {
                        if active.has(l) {
                            let t = fl(f(self.regs[*t as usize * N + l][0]), self.ftz);
                            self.query_slots(*q)[l]
                                .as_mut()
                                .ok_or("an intersection generated before the query was initialized")?
                                .generate(t)?;
                        }
                    }
                }
                St::RqTerminate { q } => {
                    for l in 0..N {
                        if active.has(l) {
                            self.query_slots(*q)[l]
                                .as_mut()
                                .ok_or("a ray query ended before it was initialized")?
                                .terminate();
                        }
                    }
                }
                St::Barrier => {
                    // lockstep makes every write before the barrier visible after it; what a barrier needs is that
                    // every invocation still in the program is at it (D3D leaves a divergent barrier undefined)
                    if self.store.is_empty() {
                        return Err("a barrier outside a compute run".into());
                    }
                    if active != self.group.minus(self.retired) {
                        return Err("a barrier that not every invocation of the group reaches".into());
                    }
                }
                St::If { c, a, r } => {
                    let mut take = M::none();
                    for l in 0..N {
                        if active.has(l) && self.regs[*c as usize * N + l][0] != 0 {
                            take.insert(l);
                        }
                    }
                    let e1 = self.block(a, take)?;
                    let e2 = self.block(r, active.minus(take))?;
                    let (b, cn, rt) = (e1.brk.or(e2.brk), e1.cont.or(e2.cont), e1.ret.or(e2.ret));
                    ex.brk = ex.brk.or(b);
                    ex.cont = ex.cont.or(cn);
                    ex.ret = ex.ret.or(rt);
                    active = active.minus(b.or(cn).or(rt));
                }
                St::Loop { body, cont, break_if } => {
                    let mut lm = active;
                    loop {
                        self.count(lm, 1)?;
                        let e = self.block(body, lm)?;
                        ex.ret = ex.ret.or(e.ret);
                        lm = lm.minus(e.ret.or(e.brk));
                        if lm.is_empty() {
                            break;
                        }
                        let e2 = self.block(cont, lm)?;
                        ex.ret = ex.ret.or(e2.ret);
                        lm = lm.minus(e2.ret.or(e2.brk));
                        if let Some(bi) = break_if {
                            for l in 0..N {
                                if lm.has(l) && self.regs[*bi as usize * N + l][0] != 0 {
                                    lm.remove(l);
                                }
                            }
                        }
                        if lm.is_empty() {
                            break;
                        }
                    }
                    active = active.minus(ex.ret);
                }
                St::Switch { sel, cases } => {
                    let mut chosen = [usize::MAX; N];
                    for (l, slot) in chosen.iter_mut().enumerate() {
                        if active.has(l) {
                            let v = self.regs[*sel as usize * N + l][0];
                            *slot = cases
                                .iter()
                                .position(|c| c.0 == Some(v))
                                .or_else(|| cases.iter().position(|c| c.0.is_none()))
                                .unwrap_or(usize::MAX);
                        }
                    }
                    let mut carry = M::none();
                    for (i, (_, fall, body)) in cases.iter().enumerate() {
                        let mut enter = carry;
                        for (l, slot) in chosen.iter().enumerate() {
                            if *slot == i {
                                enter.insert(l);
                            }
                        }
                        carry = M::none();
                        if enter.is_empty() {
                            continue;
                        }
                        let e = self.block(body, enter)?;
                        ex.cont = ex.cont.or(e.cont);
                        ex.ret = ex.ret.or(e.ret);
                        let done = e.brk.or(e.cont).or(e.ret);
                        if *fall {
                            carry = enter.minus(done);
                        }
                    }
                    active = active.minus(ex.cont.or(ex.ret));
                }
                St::Break => {
                    ex.brk = ex.brk.or(active);
                    active = M::none();
                }
                St::Continue => {
                    ex.cont = ex.cont.or(active);
                    active = M::none();
                }
                St::Return(result) => {
                    self.retired = self.retired.or(active);
                    for l in 0..N {
                        if active.has(l) {
                            // into the lane's own room, kept from run to run
                            let mut members = std::mem::take(&mut self.outs[l]);
                            members.clear();
                            for member in result {
                                match *member {
                                    Member::Reg(s, n) => {
                                        let mut r = [0u32; 4];
                                        r[..n as usize]
                                            .copy_from_slice(&self.regs[s as usize * N + l][..n as usize]);
                                        members.push(r);
                                    }
                                    Member::Mem {
                                        word, count, width, ..
                                    } => {
                                        for k in 0..count as usize {
                                            let at = word as usize + k * width as usize;
                                            let mut r = [0u32; 4];
                                            for (i, w) in r.iter_mut().take(width as usize).enumerate() {
                                                *w = self.mem[Self::local(at + i, l)];
                                            }
                                            members.push(r);
                                        }
                                    }
                                }
                            }
                            self.outs[l] = members;
                        }
                    }
                    ex.ret = ex.ret.or(active);
                    active = M::none();
                }
                St::Kill => {
                    self.killed = self.killed.or(active);
                    self.retired = self.retired.or(active);
                    if self.grouping == Grouping::Independent {
                        ex.ret = ex.ret.or(active);
                        active = M::none();
                    }
                }
            }
        }
        Ok(ex)
    }

    /// Run once, each lane with its arguments (one register per entry argument, the lanes of its value first).
    pub fn run(&mut self, args: [&[[u32; 4]]; N]) -> Result<[Output; N], String> {
        self.run_first(args, N)
    }

    /// [`Runner::run`] with lanes `0..count` only; the outputs of the others are empty (independent invocations).
    pub fn run_first(&mut self, args: [&[[u32; 4]]; N], count: usize) -> Result<[Output; N], String> {
        self.run_in_place(args, count)?;
        let c: &'a Executable = self.c;
        Ok(std::array::from_fn(|l| {
            let (values, discarded, steps, samples) = self.lane(l);
            Output {
                pointees: c
                    .pointer_args
                    .iter()
                    .map(|&(_, off, words)| self.read_locals(l, off as usize, words as usize))
                    .collect(),
                decision: self.decisions[l],
                values: values.to_vec(),
                discarded,
                steps,
                samples,
            }
        }))
    }

    /// [`Runner::run_first`] with the results left in the runner, for [`Runner::lane`] to read: a run that
    /// allocates nothing once the runner's room is grown.
    pub(super) fn run_in_place(&mut self, args: [&[[u32; 4]]; N], count: usize) -> Result<(), String> {
        if count > N || (count < N && self.grouping != Grouping::Independent) {
            return Err(format!(
                "{count} of {N} lanes run: only independent invocations run fewer than all"
            ));
        }
        self.steps = 0;
        self.lane_steps = [0; N];
        self.mem.fill(0);
        self.samples = [0; N];
        self.killed = M::none();
        self.queries.clear();
        self.decisions = [Decision::None; N];
        for o in self.outs.iter_mut() {
            o.clear();
        }
        for l in 0..N {
            if args[l].len() < self.c.arg_len {
                return Err(format!(
                    "{} argument registers given, the function takes {}",
                    args[l].len(),
                    self.c.arg_len
                ));
            }
            for (i, (s, n)) in self.c.args.iter().enumerate() {
                if *n != 0 {
                    self.regs[*s as usize * N + l] = args[l][self.c.arg_at[i]];
                }
            }
            for &(at, off, count, width) in &self.c.array_args {
                for k in 0..count as usize {
                    let base = off as usize + k * width as usize;
                    for (i, w) in args[l][at + k][..width as usize].iter().enumerate() {
                        self.mem[Self::local(base + i, l)] = *w;
                    }
                }
            }
            for &(at, off, words) in &self.c.pointer_args {
                for w in 0..words as usize {
                    self.mem[Self::local(off as usize + w, l)] = args[l][at + w / 4][w % 4];
                }
            }
        }
        let c: &'a Executable = self.c;
        self.block(&c.body, M::first(count))?;
        Ok(())
    }

    /// Lane `l` of the last run: its result members, whether it was discarded, its steps and its samples.
    pub(super) fn lane(&self, l: usize) -> (&[Reg], bool, u64, u64) {
        let steps = match self.grouping {
            Grouping::Independent => self.lane_steps[l],
            Grouping::Quad | Grouping::Group => self.steps,
        };
        (&self.outs[l], self.killed.has(l), steps, self.samples[l])
    }
}

/// Two words combined by a wave operation (booleans are 0 and 1).
fn wave_combine(op: WaveOp, kind: WaveKind, a: u32, b: u32, ftz: bool) -> u32 {
    let g = |v: u32| fl(f(v), ftz);
    match (op, kind) {
        (WaveOp::All, _) => u32::from(a != 0 && b != 0),
        (WaveOp::Any, _) => u32::from(a != 0 || b != 0),
        (WaveOp::And, _) => a & b,
        (WaveOp::Or, _) => a | b,
        (WaveOp::Xor, _) => a ^ b,
        (WaveOp::Add, WaveKind::F) => fl(g(a) + g(b), ftz).to_bits(),
        (WaveOp::Mul, WaveKind::F) => fl(g(a) * g(b), ftz).to_bits(),
        (WaveOp::Min, WaveKind::F) => g(a).min(g(b)).to_bits(),
        (WaveOp::Max, WaveKind::F) => g(a).max(g(b)).to_bits(),
        (WaveOp::Add, _) => a.wrapping_add(b),
        (WaveOp::Mul, _) => a.wrapping_mul(b),
        (WaveOp::Min, WaveKind::I) => (a as i32).min(b as i32) as u32,
        (WaveOp::Max, WaveKind::I) => (a as i32).max(b as i32) as u32,
        (WaveOp::Min, WaveKind::U) => a.min(b),
        (WaveOp::Max, WaveKind::U) => a.max(b),
    }
}
/// What a wave operation over no lanes gives, and an exclusive scan's first lane.
fn wave_identity(op: WaveOp, kind: WaveKind) -> u32 {
    match (op, kind) {
        (WaveOp::All, _) => 1,
        (WaveOp::Any | WaveOp::Or | WaveOp::Xor, _) => 0,
        (WaveOp::And, _) => u32::MAX,
        (WaveOp::Add, WaveKind::F) => 0,
        (WaveOp::Add, _) => 0,
        (WaveOp::Mul, WaveKind::F) => 1.0f32.to_bits(),
        (WaveOp::Mul, _) => 1,
        (WaveOp::Min, WaveKind::F) => f32::INFINITY.to_bits(),
        (WaveOp::Max, WaveKind::F) => f32::NEG_INFINITY.to_bits(),
        (WaveOp::Min, WaveKind::I) => i32::MAX as u32,
        (WaveOp::Max, WaveKind::I) => i32::MIN as u32,
        (WaveOp::Min, WaveKind::U) => u32::MAX,
        (WaveOp::Max, WaveKind::U) => 0,
    }
}

/// The new word and whether a compare-exchange swapped, of an atomic operation on `old`.
fn atomic(fun: AtomOp, signed: bool, old: u32, v: u32, expected: u32) -> (u32, bool) {
    let new = match fun {
        AtomOp::Add => old.wrapping_add(v),
        AtomOp::Sub => old.wrapping_sub(v),
        AtomOp::And => old & v,
        AtomOp::Or => old | v,
        AtomOp::Xor => old ^ v,
        AtomOp::Min if signed => (old as i32).min(v as i32) as u32,
        AtomOp::Max if signed => (old as i32).max(v as i32) as u32,
        AtomOp::Min => old.min(v),
        AtomOp::Max => old.max(v),
        AtomOp::Exchange => v,
        AtomOp::CompareExchange => {
            return if old == expected { (v, true) } else { (old, false) };
        }
    };
    (new, true)
}

#[inline(always)]
fn bin(op: BinOp, x: u32, y: u32, ftz: bool) -> Result<u32, String> {
    use BinOp::*;
    let fa = || fl(f(x), ftz);
    let fb = || fl(f(y), ftz);
    Ok(match op {
        AddF => fl(fa() + fb(), ftz).to_bits(),
        SubF => fl(fa() - fb(), ftz).to_bits(),
        MulF => fl(fa() * fb(), ftz).to_bits(),
        DivF => fl(fa() / fb(), ftz).to_bits(),
        // C fmod: the remainder of a truncating division, the sign of the dividend
        ModF => fl(fa() % fb(), ftz).to_bits(),
        AddU => x.wrapping_add(y),
        SubU => x.wrapping_sub(y),
        MulU => x.wrapping_mul(y),
        DivU => x.checked_div(y).ok_or("integer divide by zero in the program")?,
        ModU => x.checked_rem(y).ok_or("integer modulo by zero in the program")?,
        And => x & y,
        Or => x | y,
        Xor => x ^ y,
        Shl => x.wrapping_shl(y),
        ShrU => x.wrapping_shr(y),
        ShrI => (x as i32).wrapping_shr(y) as u32,
        EqW => (x == y) as u32,
        NeW => (x != y) as u32,
        EqF => (fa() == fb()) as u32,
        NeF => (fa() != fb()) as u32,
        LtF => (fa() < fb()) as u32,
        LeF => (fa() <= fb()) as u32,
        GtF => (fa() > fb()) as u32,
        GeF => (fa() >= fb()) as u32,
        LtU => (x < y) as u32,
        LeU => (x <= y) as u32,
        GtU => (x > y) as u32,
        GeU => (x >= y) as u32,
        LtI => ((x as i32) < (y as i32)) as u32,
        LeI => ((x as i32) <= (y as i32)) as u32,
        GtI => ((x as i32) > (y as i32)) as u32,
        GeI => ((x as i32) >= (y as i32)) as u32,
        LAnd => (x != 0 && y != 0) as u32,
        LOr => (x != 0 || y != 0) as u32,
    })
}

fn math(mf: MathOp, a: Reg, b: Reg, c: Reg, n: u8, z: bool) -> Reg {
    let mut r = [0u32; 4];
    if mf == MathOp::Dot {
        let mut acc = 0.0f32;
        for i in 0..n as usize {
            let prod = fl(fl(f(a[i]), z) * fl(f(b[i]), z), z);
            acc = if i == 0 { prod } else { fl(acc + prod, z) };
        }
        r[0] = acc.to_bits();
        return r;
    }
    match mf {
        MathOp::All => {
            r[0] = u32::from(a[..n as usize].iter().all(|&w| w != 0));
            return r;
        }
        MathOp::Any => {
            r[0] = u32::from(a[..n as usize].iter().any(|&w| w != 0));
            return r;
        }
        MathOp::Normalize => {
            let lanes = &a[..n as usize];
            let mut sum = 0.0f32;
            for (i, &lane) in lanes.iter().enumerate() {
                let sq = fl(fl(f(lane), z) * fl(f(lane), z), z);
                sum = if i == 0 { sq } else { fl(sum + sq, z) };
            }
            let length = fl((sum as f64).sqrt() as f32, z);
            for (out, &lane) in r.iter_mut().zip(lanes) {
                *out = fl(fl(f(lane), z) / length, z).to_bits();
            }
            return r;
        }
        _ => {}
    }
    if mf == MathOp::PackHalf {
        r[0] = half::f16::from_f32(f(a[0])).to_bits() as u32
            | ((half::f16::from_f32(f(a[1])).to_bits() as u32) << 16);
        return r;
    }
    if mf == MathOp::UnpackHalf {
        r[0] = half::f16::from_bits(a[0] as u16).to_f32().to_bits();
        r[1] = half::f16::from_bits((a[0] >> 16) as u16).to_f32().to_bits();
        return r;
    }
    for i in 0..n as usize {
        let (x, y, w) = (a[i], b[i], c[i]);
        let g = |v: f32| fl(v, z);
        let one = |h: &dyn Fn(f32) -> f32| g(h(g(f(x)))).to_bits();
        r[i] = match mf {
            MathOp::AbsF => (f(x).abs()).to_bits(),
            MathOp::AbsI => (x as i32).wrapping_abs() as u32,
            MathOp::MinF => g(g(f(x)).min(g(f(y)))).to_bits(),
            MathOp::MaxF => g(g(f(x)).max(g(f(y)))).to_bits(),
            MathOp::MinU => x.min(y),
            MathOp::MaxU => x.max(y),
            MathOp::MinI => (x as i32).min(y as i32) as u32,
            MathOp::MaxI => (x as i32).max(y as i32) as u32,
            MathOp::ClampF => g(g(f(x)).max(g(f(y))).min(g(f(w)))).to_bits(),
            MathOp::SatF => one(&|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }),
            MathOp::Floor => one(&|v| v.floor()),
            MathOp::Ceil => one(&|v| v.ceil()),
            MathOp::Trunc => one(&|v| v.trunc()),
            MathOp::Round => one(&|v| v.round_ties_even()),
            MathOp::Fract => one(&|v| v - v.floor()),
            MathOp::Exp2 => one(&|v| (v as f64).exp2() as f32),
            MathOp::Log2 => one(&|v| (v as f64).log2() as f32),
            MathOp::Sin => one(&|v| (v as f64).sin() as f32),
            MathOp::Cos => one(&|v| (v as f64).cos() as f32),
            // to the nearest half, ties to even, and back
            MathOp::QuantizeF16 => one(&|v| half::f16::from_f32(v).to_f32()),
            MathOp::Tan => one(&|v| (v as f64).tan() as f32),
            MathOp::Asin => one(&|v| (v as f64).asin() as f32),
            MathOp::Acos => one(&|v| (v as f64).acos() as f32),
            MathOp::Atan => one(&|v| (v as f64).atan() as f32),
            MathOp::Sinh => one(&|v| (v as f64).sinh() as f32),
            MathOp::Cosh => one(&|v| (v as f64).cosh() as f32),
            MathOp::Tanh => one(&|v| (v as f64).tanh() as f32),
            // one rounding, as a fused multiply-add
            MathOp::Fma => g(g(f(x)).mul_add(g(f(y)), g(f(w)))).to_bits(),
            MathOp::Sqrt => one(&|v| (v as f64).sqrt() as f32),
            MathOp::Rsqrt => one(&|v| (1.0 / (v as f64).sqrt()) as f32),
            MathOp::Popcnt => x.count_ones(),
            MathOp::RevBits => x.reverse_bits(),
            MathOp::Clz => x.leading_zeros(),
            MathOp::Ctz => {
                if x == 0 {
                    u32::MAX
                } else {
                    x.trailing_zeros()
                }
            }
            // x * (1 - a) + y * a, each product and the sum rounded
            MathOp::Mix => g(g(g(f(x)) * g(1.0 - g(f(w)))) + g(g(f(y)) * g(f(w)))).to_bits(),
            MathOp::SignF => {
                let v = f(x);
                (if v > 0.0 {
                    1.0f32
                } else if v < 0.0 {
                    -1.0
                } else {
                    0.0
                })
                .to_bits()
            }
            MathOp::Pow => g((g(f(x)) as f64).powf(g(f(y)) as f64) as f32).to_bits(),
            MathOp::SignI => (x as i32).signum() as u32,
            MathOp::IsNan => u32::from(f(x).is_nan()),
            MathOp::IsInf => u32::from(f(x).is_infinite()),
            MathOp::Dot
            | MathOp::PackHalf
            | MathOp::UnpackHalf
            | MathOp::All
            | MathOp::Any
            | MathOp::Normalize => unreachable!(),
        };
    }
    r
}

// ---------------------------------------------------------------------------------------------------- compute

/// What a run of a mesh program does with the memory of a thread group's output variable when the group has finished.
pub(super) type GroupHook<'a> = dyn FnMut([u32; 3], &[u32]) -> Result<(), String> + 'a;

/// What a compute run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dispatched {
    /// Thread groups run.
    pub groups: u64,
    /// Operations and loop turns executed, summed over the groups (a group's lanes run each together).
    pub steps: u64,
}

impl Executable {
    /// The thread group of a compute program (`numthreads`), or `None` for any other.
    pub fn workgroup_size(&self) -> Option<[u32; 3]> {
        self.workgroup
    }

    /// Run `groups` thread groups of a compute program. Each group's invocations run in lockstep (a barrier is the
    /// point where every invocation still in the program has arrived), thread-group memory starts at zero in every
    /// group, and the groups run one after another in x, y, z order. `buffers` holds the words of every read-write
    /// buffer by resource: writes and atomics change them and later reads see the changes, as they do between
    /// groups. A buffer not given is one the backend answers reads of and takes writes to ([`Backend::read_buffer`],
    /// [`Backend::store_buffer`]). `wave` is the wave width the wave operations and lane built-ins report; it must
    /// divide the thread group's invocations into whole waves only for programs that care.
    pub fn dispatch(
        &self,
        bound: &Bound<'_>,
        backend: &mut dyn Backend,
        buffers: &mut BTreeMap<ResourceId, Vec<u32>>,
        groups: [u32; 3],
        wave: u32,
    ) -> Result<Dispatched, String> {
        self.dispatch_elements(bound, backend, buffers, &mut BTreeMap::new(), groups, wave)
    }

    /// Run `groups` thread groups of a mesh program, handing the memory of the output variable (its words, as
    /// [`Executable::mesh_output`] says) to `after` when each group has finished.
    pub(super) fn dispatch_mesh(
        &self,
        bound: &Bound<'_>,
        backend: &mut dyn Backend,
        groups: [u32; 3],
        wave: u32,
        after: &mut GroupHook<'_>,
    ) -> Result<Dispatched, String> {
        let (global, _) = self.mesh_output.ok_or("not a mesh program")?;
        let size = self.workgroup.ok_or("not a mesh program")?;
        if wave == 0 {
            return Err("a wave of no lanes".into());
        }
        let ids = (0..groups[2])
            .flat_map(move |z| (0..groups[1]).flat_map(move |y| (0..groups[0]).map(move |x| [x, y, z])));
        let mut buffers = BTreeMap::new();
        let mut elements = BTreeMap::new();
        match size[0] * size[1] * size[2] {
            0 => Err("a thread group of no invocations".into()),
            1..=64 => self.groups::<64>(
                bound,
                backend,
                &mut buffers,
                &mut elements,
                ids,
                size,
                wave,
                Some((global, after)),
            ),
            65..=256 => self.groups::<256>(
                bound,
                backend,
                &mut buffers,
                &mut elements,
                ids,
                size,
                wave,
                Some((global, after)),
            ),
            257..=1024 => self.groups::<1024>(
                bound,
                backend,
                &mut buffers,
                &mut elements,
                ids,
                size,
                wave,
                Some((global, after)),
            ),
            _ => Err("a thread group of more than 1024 invocations".into()),
        }
    }

    /// [`Executable::dispatch`] for a program that also writes elements of arrays of buffers, `elements` holding
    /// their words by resource and element.
    pub fn dispatch_elements(
        &self,
        bound: &Bound<'_>,
        backend: &mut dyn Backend,
        buffers: &mut BTreeMap<ResourceId, Vec<u32>>,
        elements: &mut BTreeMap<(ResourceId, u32), Vec<u32>>,
        groups: [u32; 3],
        wave: u32,
    ) -> Result<Dispatched, String> {
        let size = self.workgroup.ok_or("not a compute program")?;
        if wave == 0 {
            return Err("a wave of no lanes".into());
        }
        let ids = (0..groups[2])
            .flat_map(move |z| (0..groups[1]).flat_map(move |y| (0..groups[0]).map(move |x| [x, y, z])));
        match size[0] * size[1] * size[2] {
            0 => Err("a thread group of no invocations".into()),
            1..=64 => self.groups::<64>(bound, backend, buffers, elements, ids, size, wave, None),
            65..=256 => self.groups::<256>(bound, backend, buffers, elements, ids, size, wave, None),
            257..=1024 => self.groups::<1024>(bound, backend, buffers, elements, ids, size, wave, None),
            _ => Err("a thread group of more than 1024 invocations".into()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn groups<const N: usize>(
        &self,
        bound: &Bound<'_>,
        backend: &mut dyn Backend,
        buffers: &mut BTreeMap<ResourceId, Vec<u32>>,
        elements: &mut BTreeMap<(ResourceId, u32), Vec<u32>>,
        ids: impl Iterator<Item = [u32; 3]>,
        size: [u32; 3],
        wave: u32,
        mut after: Option<(usize, &mut GroupHook<'_>)>,
    ) -> Result<Dispatched, String> {
        let lanes = (size[0] * size[1] * size[2]) as usize;
        // the buffers move into the run, and back whatever happens
        let store: Vec<Option<Vec<u32>>> = self
            .globals
            .iter()
            .map(|g| match g.kind {
                GlobalKind::Storage(id) => buffers.remove(&id),
                _ => None,
            })
            .collect();
        let elem_store: Vec<Vec<Option<Vec<u32>>>> = self
            .globals
            .iter()
            .map(|g| match g.kind {
                GlobalKind::StorageArray(id, count) => {
                    (0..count).map(|k| elements.remove(&(id, k))).collect()
                }
                _ => Vec::new(),
            })
            .collect();
        let scratch = Scratch::<N>::new(self);
        let mut runner = Runner::<N, Wide> {
            c: self,
            b: bound,
            regs: scratch.regs,
            mem: scratch.mem,
            sampler: backend,
            ftz: self.flush_denormals,
            grouping: Grouping::Group,
            steps: 0,
            lane_steps: [0; N],
            samples: [0; N],
            outs: scratch.outs,
            decisions: [Decision::None; N],
            killed: Wide::none(),
            store,
            elem_store,
            queries: Vec::new(),
            group: Wide::first(lanes),
            retired: Wide::none(),
            lanes,
            wave: wave as usize,
            derivs: None,
        };
        let mut done = Dispatched::default();
        let mut result = Ok(());
        for id in ids {
            match runner.run_group(id, size, wave) {
                Ok(steps) => {
                    done.groups += 1;
                    done.steps += steps;
                    if let Some((global, hook)) = after.as_mut() {
                        let words = runner.store[*global].as_deref().unwrap_or(&[]);
                        if let Err(e) = hook(id, words) {
                            result = Err(format!("group {id:?}: {e}"));
                            break;
                        }
                    }
                }
                Err(e) => {
                    result = Err(format!("group {id:?}: {e}"));
                    break;
                }
            }
        }
        for (g, memory) in self.globals.iter().zip(runner.store) {
            if let (GlobalKind::Storage(id), Some(words)) = (&g.kind, memory) {
                buffers.insert(*id, words);
            }
        }
        for (g, held) in self.globals.iter().zip(runner.elem_store) {
            if let GlobalKind::StorageArray(id, _) = g.kind {
                for (k, words) in held.into_iter().enumerate() {
                    if let Some(words) = words {
                        elements.insert((id, k as u32), words);
                    }
                }
            }
        }
        result.map(|()| done)
    }
}

impl<'a, const N: usize> Runner<'a, N, Wide> {
    /// Run one thread group: `id` its position in the dispatch, `size` its `numthreads`. Returns the steps taken.
    fn run_group(&mut self, id: [u32; 3], size: [u32; 3], wave: u32) -> Result<u64, String> {
        let c: &'a Executable = self.c;
        let lanes = (size[0] * size[1] * size[2]) as usize;
        self.steps = 0;
        self.killed = Wide::none();
        self.retired = Wide::none();
        self.queries.clear();
        for (i, g) in c.globals.iter().enumerate() {
            if let GlobalKind::Workgroup(words) = g.kind {
                match &mut self.store[i] {
                    Some(v) => v.iter_mut().for_each(|w| *w = 0),
                    slot => *slot = Some(vec![0; words as usize]),
                }
            }
        }
        for l in 0..lanes {
            let local = [
                l as u32 % size[0],
                (l as u32 / size[0]) % size[1],
                l as u32 / (size[0] * size[1]),
            ];
            for (i, (slot, n)) in c.args.iter().enumerate() {
                if *n == 0 {
                    continue;
                }
                let value = match c.builtins.get(i).copied().flatten() {
                    Some(naga::BuiltIn::WorkGroupId) => [id[0], id[1], id[2], 0],
                    Some(naga::BuiltIn::LocalInvocationId) => [local[0], local[1], local[2], 0],
                    Some(naga::BuiltIn::GlobalInvocationId) => [
                        id[0] * size[0] + local[0],
                        id[1] * size[1] + local[1],
                        id[2] * size[2] + local[2],
                        0,
                    ],
                    Some(naga::BuiltIn::LocalInvocationIndex) => [l as u32, 0, 0, 0],
                    Some(naga::BuiltIn::SubgroupInvocationId) => [l as u32 % wave, 0, 0, 0],
                    Some(naga::BuiltIn::SubgroupSize) => [wave, 0, 0, 0],
                    Some(naga::BuiltIn::SubgroupId) => [l as u32 / wave, 0, 0, 0],
                    Some(naga::BuiltIn::NumSubgroups) => [(lanes as u32).div_ceil(wave), 0, 0, 0],
                    other => return Err(format!("compute entry argument {i} carries {other:?}")),
                };
                self.regs[*slot as usize * N + l] = value;
            }
            for w in 0..c.local_words {
                self.mem[Self::local(w, l)] = 0;
            }
        }
        self.block(&c.body, self.group)?;
        Ok(self.steps)
    }
}
