use std::collections::BTreeMap;

use super::flat::{BinOp, CvtOp, Executable, GlobalKind, LevelOp, Loc, MathOp, Op, Ptr, Reg, St, UnOp};
use super::request::{Backend, ImageLoad, ImageLod, ImageQuery, ImageSample, ImageStore};
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
pub struct Runner<'a, const N: usize> {
    c: &'a Executable,
    b: &'a Bound<'a>,
    regs: Vec<Reg>,
    mem: Vec<u32>,
    sampler: &'a mut dyn Backend,
    ftz: bool,
    steps: u64,
    samples: [u64; N],
    outs: [Vec<Reg>; N],
    killed: u8,
    /// The derivatives of the arguments (scalar mode): per argument, d/dx then d/dy lanes.
    derivs: Option<&'a [[Reg; 2]]>,
}

impl<const N: usize> std::fmt::Debug for Runner<'_, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("lanes", &N)
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

#[derive(Default, Clone, Copy)]
struct Exits {
    brk: u8,
    cont: u8,
    ret: u8,
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

impl<'a, const N: usize> Runner<'a, N> {
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
            derivs,
        }
    }

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

    fn ops(&mut self, lo: u32, hi: u32, active: u8) -> Result<(), String> {
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
                        if active >> l & 1 != 0 {
                            self.regs[dst + l] = self.mem_get(*loc, l, 0, *n)?;
                        }
                    }
                }
                Op::LoadLanes { loc, offs, n } => {
                    for l in 0..N {
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
                            let x = self.dyn_extra(ptr, l)?;
                            self.regs[dst + l] = self.mem_get(ptr.loc, l, x, *n)?;
                        }
                    }
                }
                Op::Swizzle { src, pat, n } => {
                    for l in 0..N {
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
                            let s = self.regs[*src as usize * N + l];
                            self.regs[dst + l] = [s[*i as usize]; 4];
                        }
                    }
                }
                Op::Splat { src, n } => {
                    let _ = n;
                    for l in 0..N {
                        if active >> l & 1 != 0 {
                            let s = self.regs[*src as usize * N + l];
                            self.regs[dst + l] = [s[0]; 4];
                        }
                    }
                }
                Op::Compose { parts, np } => {
                    for l in 0..N {
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
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
                        if active != 15 {
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
                        if active != 15 {
                            return Err("a quad's lanes diverge at an implicit-LOD sample".into());
                        }
                        let uv: [[f32; 3]; 4] = std::array::from_fn(|l| c3(self, l, *coord));
                        let d = |a: usize| -> [f32; 3] { std::array::from_fn(|i| uv[a][i] - uv[0][i]) };
                        Some([d(1), d(2)])
                    } else {
                        None
                    };
                    for l in 0..N {
                        if active >> l & 1 == 0 {
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
                            texture: *tex,
                            sampler: *smp,
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
                        if active >> l & 1 == 0 {
                            continue;
                        }
                        let cd = self.regs[*coord as usize * N + l];
                        let one = |s: &Option<u32>| s.map(|s| self.regs[s as usize * N + l][0] as i32);
                        let req = ImageLoad {
                            texture: *tex,
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
                        if active >> l & 1 == 0 {
                            continue;
                        }
                        let level = level.map(|s| self.regs[s as usize * N + l][0]);
                        self.regs[dst + l] = self.sampler.query(ImageQuery {
                            texture: *tex,
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

    fn block(&mut self, b: &'a [St], active: u8) -> Result<Exits, String> {
        let mut ex = Exits::default();
        let mut active = active;
        for st in b {
            if active == 0 {
                break;
            }
            match st {
                St::Run(lo, hi) => self.ops(*lo, *hi, active)?,
                St::Store { loc, src, n } => {
                    for l in 0..N {
                        if active >> l & 1 != 0 {
                            let v = self.regs[*src as usize * N + l];
                            self.mem_put(*loc, l, 0, *n, v)?;
                        }
                    }
                }
                St::StoreDyn { ptr, src, n } => {
                    for l in 0..N {
                        if active >> l & 1 != 0 {
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
                        if active >> l & 1 != 0 {
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
                St::Atomic { ptr, value, result } => {
                    for l in 0..N {
                        if active >> l & 1 != 0 {
                            let Loc::Global(g, o) = ptr.loc else {
                                return Err("atomic on a local".into());
                            };
                            let GlobalKind::Storage(view) = self.c.globals[g as usize].kind else {
                                return Err("atomic on a read-only global".into());
                            };
                            let word = o + self.dyn_extra(ptr, l)?;
                            let old =
                                self.sampler
                                    .atomic_add(view, word, self.regs[*value as usize * N + l][0])?;
                            if let Some(r) = result {
                                self.regs[*r as usize * N + l] = [old; 4];
                            }
                        }
                    }
                }
                St::If { c, a, r } => {
                    let mut take = 0u8;
                    for l in 0..N {
                        if active >> l & 1 != 0 && self.regs[*c as usize * N + l][0] != 0 {
                            take |= 1 << l;
                        }
                    }
                    let e1 = self.block(a, take)?;
                    let e2 = self.block(r, active & !take)?;
                    let (b, cn, rt) = (e1.brk | e2.brk, e1.cont | e2.cont, e1.ret | e2.ret);
                    ex.brk |= b;
                    ex.cont |= cn;
                    ex.ret |= rt;
                    active &= !(b | cn | rt);
                }
                St::Loop { body, cont, break_if } => {
                    let mut lm = active;
                    loop {
                        self.steps += 1;
                        if self.steps > STEP_LIMIT {
                            return Err("execution budget exceeded".into());
                        }
                        let e = self.block(body, lm)?;
                        ex.ret |= e.ret;
                        lm &= !(e.ret | e.brk);
                        if lm == 0 {
                            break;
                        }
                        let e2 = self.block(cont, lm)?;
                        ex.ret |= e2.ret;
                        lm &= !(e2.ret | e2.brk);
                        if let Some(bi) = break_if {
                            for l in 0..N {
                                if lm >> l & 1 != 0 && self.regs[*bi as usize * N + l][0] != 0 {
                                    lm &= !(1 << l);
                                }
                            }
                        }
                        if lm == 0 {
                            break;
                        }
                    }
                    active &= !ex.ret;
                }
                St::Switch { sel, cases } => {
                    let mut chosen = [usize::MAX; N];
                    for (l, slot) in chosen.iter_mut().enumerate() {
                        if active >> l & 1 != 0 {
                            let v = self.regs[*sel as usize * N + l][0];
                            *slot = cases
                                .iter()
                                .position(|c| c.0 == Some(v))
                                .or_else(|| cases.iter().position(|c| c.0.is_none()))
                                .unwrap_or(usize::MAX);
                        }
                    }
                    let mut carry = 0u8;
                    for (i, (_, fall, body)) in cases.iter().enumerate() {
                        let mut enter = carry;
                        for (l, slot) in chosen.iter().enumerate() {
                            if *slot == i {
                                enter |= 1 << l;
                            }
                        }
                        carry = 0;
                        if enter == 0 {
                            continue;
                        }
                        let e = self.block(body, enter)?;
                        ex.cont |= e.cont;
                        ex.ret |= e.ret;
                        let done = e.brk | e.cont | e.ret;
                        if *fall {
                            carry = enter & !done;
                        }
                    }
                    active &= !(ex.cont | ex.ret);
                }
                St::Break => {
                    ex.brk |= active;
                    active = 0;
                }
                St::Continue => {
                    ex.cont |= active;
                    active = 0;
                }
                St::Return(result) => {
                    for l in 0..N {
                        if active >> l & 1 != 0 {
                            let mut members = Vec::with_capacity(result.len());
                            for (s, n) in result {
                                let mut r = [0u32; 4];
                                r[..*n as usize]
                                    .copy_from_slice(&self.regs[*s as usize * N + l][..*n as usize]);
                                members.push(r);
                            }
                            self.outs[l] = members;
                        }
                    }
                    ex.ret |= active;
                    active = 0;
                }
                St::Kill => {
                    self.killed |= active;
                    if N == 1 {
                        ex.ret |= active;
                        active = 0;
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
        self.killed = 0;
        for o in self.outs.iter_mut() {
            o.clear();
        }
        for l in 0..N {
            if args[l].len() < self.c.args.len() {
                return Err(format!(
                    "{} arguments given, the entry point takes {}",
                    args[l].len(),
                    self.c.args.len()
                ));
            }
            for (i, (s, _)) in self.c.args.iter().enumerate() {
                self.regs[*s as usize * N + l] = args[l][i];
            }
            for w in self.mem[l * self.c.local_words..(l + 1) * self.c.local_words].iter_mut() {
                *w = 0;
            }
        }
        let c: &'a Executable = self.c;
        self.block(&c.body, ((1u16 << N) - 1) as u8)?;
        let (steps, samples) = (self.steps, self.samples);
        Ok(std::array::from_fn(|l| Output {
            values: std::mem::take(&mut self.outs[l]),
            discarded: self.killed >> l & 1 != 0,
            steps,
            samples: samples[l],
        }))
    }
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
