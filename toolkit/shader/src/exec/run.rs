use std::collections::BTreeMap;

use super::flat::{
    AtomOp, BinOp, CvtOp, Executable, GlobalKind, LevelOp, Loc, MathOp, Member, Op, Pick, Ptr, Reg, Res, St,
    UnOp, WaveGather, WaveKind, WaveOp, WaveScan,
};
use super::mask::{Mask, Wide};
use super::request::{
    Backend, Element, ImageAtomic, ImageLoad, ImageLod, ImageQuery, ImageSample, ImageStore,
};
use crate::ResourceId;

/// The most operations and loop turns one invocation may execute: a program that does not end is refused.
pub const STEP_LIMIT: u64 = 1 << 24;

/// Where a global's words are during a run.
#[derive(Debug)]
enum Words<'a> {
    Owned(Vec<u32>),
    /// A constant buffer as the caller holds it: rows of four lanes.
    Rows(&'a [[u32; 4]]),
    Slice(&'a [u32]),
    /// A buffer the backend answers reads of.
    Backend,
}

/// The buffers a program reads, ready for runs: built once per material and reused for every pixel.
#[derive(Debug)]
pub struct Bound<'a> {
    data: Vec<Words<'a>>,
}

/// What a run reads besides its inputs: constant buffers (rows of four lanes) and buffers (32-bit words), by resource.
#[derive(Clone, Debug, Default)]
pub struct Resources {
    pub constants: BTreeMap<ResourceId, Vec<[u32; 4]>>,
    pub buffers: BTreeMap<ResourceId, Vec<u32>>,
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
        let mut data = Vec::with_capacity(self.globals.len());
        for g in &self.globals {
            data.push(match &g.kind {
                GlobalKind::Constants(id) => Words::Rows(
                    constants(*id).ok_or_else(|| format!("no constant buffer for resource {}", id.0))?,
                ),
                GlobalKind::Private(words) => Words::Owned(words.clone()),
                GlobalKind::Storage(id) => buffers(*id).map_or(Words::Backend, Words::Slice),
                // thread-group memory is the run's own
                GlobalKind::Workgroup(_) => Words::Owned(Vec::new()),
            });
        }
        Ok(Bound { data })
    }

    /// [`Executable::bind`] over a [`Resources`].
    pub fn bind_resources<'a>(&self, resources: &'a Resources) -> Result<Bound<'a>, String> {
        self.bind(
            |id| resources.constants.get(&id).map(Vec::as_slice),
            |id| resources.buffers.get(&id).map(Vec::as_slice),
        )
    }
}

/// What one invocation produced.
#[derive(Clone, Debug)]
pub struct Output {
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
        }
    }
}

/// Runs an [`Executable`] for one invocation (`N` = 1) or a 2 x 2 quad in lockstep (`N` = 4: lanes 0 and 1 the top row,
/// 2 and 3 the bottom), as a GPU does: lanes that diverge are masked, a screen derivative is a difference inside the
/// quad, a lane that is discarded goes on as a helper. A runner is reused for any number of runs.
pub struct Runner<'a, const N: usize, M: Mask = u8> {
    c: &'a Executable,
    b: &'a Bound<'a>,
    regs: Vec<Reg>,
    mem: Vec<u32>,
    sampler: &'a mut dyn Backend,
    ftz: bool,
    steps: u64,
    samples: [u64; N],
    outs: [Vec<Reg>; N],
    killed: M,
    /// The memory a run changes, by global: thread-group memory and the buffers a compute run writes (empty for a
    /// pixel run, which hands its writes to the backend).
    store: Vec<Option<Vec<u32>>>,
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
        f.debug_struct("Runner")
            .field("lanes", &N)
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
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

impl<'a, const N: usize> Runner<'a, N, u8> {
    /// A runner of `exe` over `bound`, answering requests through `backend`. A scalar run (`N` = 1) that reads
    /// derivatives of its inputs takes them in `derivatives`; a quad takes its own.
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
            steps: 0,
            samples: [0; N],
            outs: std::array::from_fn(|_| Vec::new()),
            killed: 0,
            store: Vec::new(),
            group: 0,
            retired: 0,
            lanes: 0,
            wave: 0,
            derivs,
        }
    }
}

impl<'a, const N: usize, M: Mask> Runner<'a, N, M> {
    /// Give the registers back, to start the next runner of this program with.
    pub fn into_scratch(self) -> Scratch<N> {
        Scratch {
            regs: self.regs,
            mem: self.mem,
        }
    }

    fn mem_get(&mut self, loc: Loc, lane: usize, extra: u32, n: u8) -> Result<Reg, String> {
        let mut r = [0u32; 4];
        match loc {
            Loc::Local(o) => {
                let base = lane * self.c.local_words + (o + extra) as usize;
                let s = self
                    .mem
                    .get(base..base + n as usize)
                    .ok_or("local index out of range")?;
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
                let base = lane * self.c.local_words + (o + extra) as usize;
                let s = self
                    .mem
                    .get_mut(base..base + n as usize)
                    .ok_or("local index out of range")?;
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
        self.steps += (hi - lo) as u64;
        if self.steps > STEP_LIMIT {
            return Err("execution budget exceeded".into());
        }
        let c = self.c;
        let ftz = self.ftz;
        for k in lo..hi {
            let (dst, op) = &c.ops[k as usize];
            let dst = *dst as usize * N;
            match op {
                Op::Load { loc, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            self.regs[dst + l] = self.mem_get(*loc, l, 0, *n)?;
                        }
                    }
                }
                Op::LoadLanes { loc, offs, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            let mut r = [0u32; 4];
                            for i in 0..*n as usize {
                                r[i] = self.mem_get(*loc, l, offs[i] as u32, 1)?[0];
                            }
                            self.regs[dst + l] = r;
                        }
                    }
                }
                Op::LoadDyn { ptr, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            let x = self.dyn_extra(ptr, l)?;
                            self.regs[dst + l] = self.mem_get(ptr.loc, l, x, *n)?;
                        }
                    }
                }
                Op::Swizzle { src, pat, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            let s = self.regs[*src as usize * N + l];
                            let mut r = [0u32; 4];
                            for i in 0..*n as usize {
                                r[i] = s[pat[i] as usize];
                            }
                            self.regs[dst + l] = r;
                        }
                    }
                }
                Op::Lane { src, i } => {
                    for l in 0..N {
                        if active.has(l) {
                            let s = self.regs[*src as usize * N + l];
                            self.regs[dst + l] = [s[*i as usize]; 4];
                        }
                    }
                }
                Op::Splat { src, n } => {
                    let _ = n;
                    for l in 0..N {
                        if active.has(l) {
                            let s = self.regs[*src as usize * N + l];
                            self.regs[dst + l] = [s[0]; 4];
                        }
                    }
                }
                Op::Compose { parts, np } => {
                    for l in 0..N {
                        if active.has(l) {
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
                        }
                    }
                }
                Op::Un { op, src, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            let s = self.regs[*src as usize * N + l];
                            let mut r = [0u32; 4];
                            for i in 0..*n as usize {
                                r[i] = match op {
                                    UnOp::NegF => s[i] ^ 0x8000_0000,
                                    UnOp::NegI => (s[i] as i32).wrapping_neg() as u32,
                                    UnOp::Not => (s[i] == 0) as u32,
                                    UnOp::BitNot => !s[i],
                                };
                            }
                            self.regs[dst + l] = r;
                        }
                    }
                }
                Op::Bin { op, a, b, n, a1, b1 } => {
                    for l in 0..N {
                        if active.has(l) {
                            let x = self.regs[*a as usize * N + l];
                            let y = self.regs[*b as usize * N + l];
                            let mut r = [0u32; 4];
                            for i in 0..*n as usize {
                                r[i] = bin(*op, x[if *a1 { 0 } else { i }], y[if *b1 { 0 } else { i }], ftz)?;
                            }
                            self.regs[dst + l] = r;
                        }
                    }
                }
                Op::Select { c: cs, a, b, n, c1 } => {
                    for l in 0..N {
                        if active.has(l) {
                            let cc = self.regs[*cs as usize * N + l];
                            let x = self.regs[*a as usize * N + l];
                            let y = self.regs[*b as usize * N + l];
                            let mut r = [0u32; 4];
                            for i in 0..*n as usize {
                                r[i] = if cc[if *c1 { 0 } else { i }] != 0 {
                                    x[i]
                                } else {
                                    y[i]
                                };
                            }
                            self.regs[dst + l] = r;
                        }
                    }
                }
                Op::Cvt { op, src, n } => {
                    for l in 0..N {
                        if active.has(l) {
                            let s = self.regs[*src as usize * N + l];
                            let mut r = [0u32; 4];
                            for i in 0..*n as usize {
                                r[i] = match op {
                                    CvtOp::F2I => (f(s[i]) as i32) as u32,
                                    CvtOp::F2U => f(s[i]) as u32,
                                    CvtOp::I2F => (s[i] as i32 as f32).to_bits(),
                                    CvtOp::U2F => (s[i] as f32).to_bits(),
                                };
                            }
                            self.regs[dst + l] = r;
                        }
                    }
                }
                Op::Math {
                    f: mf,
                    a,
                    b,
                    c: cc,
                    n,
                } => {
                    for l in 0..N {
                        if active.has(l) {
                            let x = self.regs[*a as usize * N + l];
                            let y = self.regs[*b as usize * N + l];
                            let z = self.regs[*cc as usize * N + l];
                            self.regs[dst + l] = math(*mf, x, y, z, *n, ftz);
                        }
                    }
                }
                Op::ArrLen { gid, elem_words } => {
                    let bound = self.b;
                    let words = match &bound.data[*gid as usize] {
                        _ if matches!(self.store.get(*gid as usize), Some(Some(_))) => {
                            self.store[*gid as usize].as_ref().map_or(0, Vec::len) as u32
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
                    if N == 4 {
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
                        self.regs[dst] = r;
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
                    let footprint = if N == 4 && matches!(level, LevelOp::Auto | LevelOp::Bias(_)) {
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
        if !matches!(res.pick, Pick::One) && res.count != crate::Resource::UNBOUNDED && index >= res.count {
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
                            self.sampler.store_image(ImageStore {
                                view: *tex,
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
                } => {
                    for l in 0..N {
                        if active.has(l) {
                            let cd = self.regs[*coord as usize * N + l];
                            self.sampler.atomic_image(ImageAtomic {
                                view: *tex,
                                dim: *dim,
                                texel: std::array::from_fn(|i| if i < *cn as usize { cd[i] } else { 0 }),
                                layer: layer.map(|s| self.regs[s as usize * N + l][0]),
                                op: *op,
                                signed: *signed,
                                value: self.regs[*value as usize * N + l][0],
                            })?;
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
                            let Loc::Global(g, o) = ptr.loc else {
                                return Err("atomic on a local".into());
                            };
                            let word = (o + self.dyn_extra(ptr, l)?) as usize;
                            let v = self.regs[*value as usize * N + l][0];
                            let expected = compare.map_or(0, |c| self.regs[c as usize * N + l][0]);
                            let (old, swapped) = if let Some(Some(d)) = self.store.get_mut(g as usize) {
                                let cell = d.get_mut(word).ok_or("atomic past the end of the memory")?;
                                let old = *cell;
                                let (new, swapped) = atomic(*fun, *signed, old, v, expected);
                                *cell = new;
                                (old, swapped)
                            } else {
                                let GlobalKind::Storage(view) = self.c.globals[g as usize].kind else {
                                    return Err("atomic on a read-only global".into());
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
                        self.steps += 1;
                        if self.steps > STEP_LIMIT {
                            return Err("execution budget exceeded".into());
                        }
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
                            let mut members = Vec::with_capacity(result.len());
                            for member in result {
                                match *member {
                                    Member::Reg(s, n) => {
                                        let mut r = [0u32; 4];
                                        r[..n as usize]
                                            .copy_from_slice(&self.regs[s as usize * N + l][..n as usize]);
                                        members.push(r);
                                    }
                                    Member::Mem { word, count, width } => {
                                        for k in 0..count as usize {
                                            let at =
                                                l * self.c.local_words + word as usize + k * width as usize;
                                            let mut r = [0u32; 4];
                                            r[..width as usize]
                                                .copy_from_slice(&self.mem[at..at + width as usize]);
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
                    if N == 1 {
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
        self.steps = 0;
        self.samples = [0; N];
        self.killed = M::none();
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
            for w in self.mem[l * self.c.local_words..(l + 1) * self.c.local_words].iter_mut() {
                *w = 0;
            }
            for &(at, off, count, width) in &self.c.array_args {
                for k in 0..count as usize {
                    let base = l * self.c.local_words + off as usize + k * width as usize;
                    self.mem[base..base + width as usize].copy_from_slice(&args[l][at + k][..width as usize]);
                }
            }
        }
        let c: &'a Executable = self.c;
        self.block(&c.body, M::first(N))?;
        let (steps, samples) = (self.steps, self.samples);
        Ok(std::array::from_fn(|l| Output {
            values: std::mem::take(&mut self.outs[l]),
            discarded: self.killed.has(l),
            steps,
            samples: samples[l],
        }))
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
            MathOp::Dot | MathOp::PackHalf | MathOp::UnpackHalf => unreachable!(),
        };
    }
    r
}

// ---------------------------------------------------------------------------------------------------- compute

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
        let size = self.workgroup.ok_or("not a compute program")?;
        if wave == 0 {
            return Err("a wave of no lanes".into());
        }
        let ids = (0..groups[2])
            .flat_map(move |z| (0..groups[1]).flat_map(move |y| (0..groups[0]).map(move |x| [x, y, z])));
        match size[0] * size[1] * size[2] {
            0 => Err("a thread group of no invocations".into()),
            1..=64 => self.groups::<64>(bound, backend, buffers, ids, size, wave),
            65..=256 => self.groups::<256>(bound, backend, buffers, ids, size, wave),
            257..=1024 => self.groups::<1024>(bound, backend, buffers, ids, size, wave),
            _ => Err("a thread group of more than 1024 invocations".into()),
        }
    }

    fn groups<const N: usize>(
        &self,
        bound: &Bound<'_>,
        backend: &mut dyn Backend,
        buffers: &mut BTreeMap<ResourceId, Vec<u32>>,
        ids: impl Iterator<Item = [u32; 3]>,
        size: [u32; 3],
        wave: u32,
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
        let scratch = Scratch::<N>::new(self);
        let mut runner = Runner::<N, Wide> {
            c: self,
            b: bound,
            regs: scratch.regs,
            mem: scratch.mem,
            sampler: backend,
            ftz: self.flush_denormals,
            steps: 0,
            samples: [0; N],
            outs: std::array::from_fn(|_| Vec::new()),
            killed: Wide::none(),
            store,
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
            for w in self.mem[l * c.local_words..(l + 1) * c.local_words].iter_mut() {
                *w = 0;
            }
        }
        self.block(&c.body, self.group)?;
        Ok(self.steps)
    }
}
