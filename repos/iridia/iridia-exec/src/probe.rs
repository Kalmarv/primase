//! One vertex of a vertex program, run to see what it does with chosen constants: the clip position a probe reads to
//! learn what a constant is (a matrix the program places the vertex with, a table it reads the instance through).
//!
//! A probe sets the constants it asks about to values that make the answer show, gives the program a vertex, and
//! compares clip positions. What it does not ask about the program still reads: its buffers are the caller's to answer
//! ([`BufferReads`], a table probe watches which it reads), and its textures are answered here with stated stand-ins
//! ([`SAMPLE_STAND_IN`], [`LOAD_STAND_IN`], [`QUERY_STAND_IN`]), counted ([`TextureUse`]). A stand-in lets a program that
//! reads a texture of its own still show what it does with the camera; where it also moves the vertex by the texel, a
//! check of the clip position fails, and the probe names the stand-ins as the likely reason ([`TextureUse::stand_ins`])
//! rather than the constant it asked about.
//!
//! The answering is [`AnsweredReads`]: every texture read of a run answered by the caller ([`Answer`]) and recorded, so
//! a run shows what a program does with what it reads, apart from what any texture holds. The vertex probe answers with
//! the stand-ins ([`StandIns`]); a pixel program run through a [`crate::ProgramShader`] takes it as its [`Sampler`], and
//! [`affine_uv`] reads from its records the map a program applies to a texture's coordinate.
use std::collections::BTreeMap;

mod buffers;
pub use buffers::{AnsweredBuffers, BufferExtent, BufferRead};

use iridia::{Program, ResourceId, Stage, SystemValue};

use crate::{
    Backend, Dim, Element, Executable, ImageLoad, ImageQuery, ImageSample, Resources, Runner, Sample, Sampler,
};

/// What a texture sample of the program is answered with: mid grey in every lane.
pub const SAMPLE_STAND_IN: [f32; 4] = [0.5; 4];
/// What a texel load is answered with: zero in every lane.
pub const LOAD_STAND_IN: [u32; 4] = [0; 4];
/// What a texture size or level-count query is answered with: one texel, one level, one sample.
pub const QUERY_STAND_IN: [u32; 4] = [1; 4];

/// A program's buffers, as the caller answers them: `count` (1 to 4) words from word `word` of the buffer `view`, and a
/// buffer's length in words.
pub trait BufferReads {
    fn read(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String>;
    /// The buffer's length in words: long enough for any read, unless the caller says otherwise.
    fn words(&mut self, view: ResourceId) -> Result<u32, String> {
        let _ = view;
        Ok(1 << 20)
    }
}

/// Buffers of zeros, for a program whose buffers the probe does not ask about.
#[derive(Debug)]
pub struct Zeros;

impl BufferReads for Zeros {
    fn read(&mut self, _view: ResourceId, _word: u32, _count: u8) -> Result<[u32; 4], String> {
        Ok([0; 4])
    }
}

/// How the caller answers a run's texture reads ([`AnsweredReads`]), each by its request; `R` names the texture (a
/// program's resource [`Element`], or a bind slot). A load or a query is refused unless the answer gives one, so no read
/// is answered by a value nobody stated.
pub trait Answer<R> {
    fn sample(&mut self, request: &ImageSample<R>) -> Result<[f32; 4], String>;
    fn load(&mut self, request: &ImageLoad<R>) -> Result<[u32; 4], String> {
        let _ = request;
        Err("a texel load, which the answer does not give".into())
    }
    fn query(&mut self, request: &ImageQuery<R>) -> Result<[u32; 4], String> {
        let _ = request;
        Err("a size or level-count query, which the answer does not give".into())
    }
}

/// A closure answers samples only.
impl<R, F: FnMut(&ImageSample<R>) -> Result<[f32; 4], String>> Answer<R> for F {
    fn sample(&mut self, request: &ImageSample<R>) -> Result<[f32; 4], String> {
        self(request)
    }
}

/// The vertex probe's answer: [`SAMPLE_STAND_IN`], [`LOAD_STAND_IN`] and [`QUERY_STAND_IN`] to every read.
#[derive(Clone, Copy, Debug, Default)]
pub struct StandIns;

impl<R> Answer<R> for StandIns {
    fn sample(&mut self, _request: &ImageSample<R>) -> Result<[f32; 4], String> {
        Ok(SAMPLE_STAND_IN)
    }
    fn load(&mut self, _request: &ImageLoad<R>) -> Result<[u32; 4], String> {
        Ok(LOAD_STAND_IN)
    }
    fn query(&mut self, _request: &ImageQuery<R>) -> Result<[u32; 4], String> {
        Ok(QUERY_STAND_IN)
    }
}

/// A run's texture reads, each answered by `answer` and recorded in the order the program made them (a read the answer
/// refuses is not recorded; the run fails with its reason). As a [`Sampler`] (`R` a bind slot) it is a pixel program's
/// textures; stores, atomics and structured buffers are refused, as a sampler that gives none refuses them.
#[derive(Debug)]
pub struct AnsweredReads<R, A> {
    pub answer: A,
    pub samples: Vec<ImageSample<R>>,
    pub loads: Vec<ImageLoad<R>>,
    pub queries: Vec<ImageQuery<R>>,
}

impl<R, A: Answer<R>> AnsweredReads<R, A> {
    pub fn new(answer: A) -> Self {
        AnsweredReads {
            answer,
            samples: Vec::new(),
            loads: Vec::new(),
            queries: Vec::new(),
        }
    }

    pub fn sample(&mut self, request: ImageSample<R>) -> Result<[f32; 4], String> {
        let value = self.answer.sample(&request)?;
        self.samples.push(request);
        Ok(value)
    }

    pub fn load(&mut self, request: ImageLoad<R>) -> Result<[u32; 4], String> {
        let value = self.answer.load(&request)?;
        self.loads.push(request);
        Ok(value)
    }

    pub fn query(&mut self, request: ImageQuery<R>) -> Result<[u32; 4], String> {
        let value = self.answer.query(&request)?;
        self.queries.push(request);
        Ok(value)
    }

    /// How many reads of each kind were answered.
    pub fn counts(&self) -> TextureUse {
        let n = |len: usize| u32::try_from(len).unwrap_or(u32::MAX);
        TextureUse {
            samples: n(self.samples.len()),
            loads: n(self.loads.len()),
            queries: n(self.queries.len()),
        }
    }
}

impl<A: Answer<u32>> Sampler for AnsweredReads<u32, A> {
    /// A plain 2D sample, answered as the image sample it is.
    fn sample(&mut self, request: Sample) -> Result<[f32; 4], String> {
        let [u, v] = request.uv;
        AnsweredReads::sample(
            self,
            ImageSample {
                texture: request.texture,
                sampler: request.sampler,
                dim: Dim::D2,
                coord: [u, v, 0.],
                layer: request.layer,
                lod: request.lod.into(),
                offset: [0; 3],
                gather: None,
                depth_ref: None,
                depth: false,
            },
        )
    }
    fn sample_image(&mut self, request: ImageSample<u32>) -> Result<[f32; 4], String> {
        AnsweredReads::sample(self, request)
    }
    fn load_image(&mut self, request: ImageLoad<u32>) -> Result<[u32; 4], String> {
        AnsweredReads::load(self, request)
    }
    fn query_image(&mut self, request: ImageQuery<u32>) -> Result<[u32; 4], String> {
        AnsweredReads::query(self, request)
    }
}

/// The map a program applies to a texture coordinate, `sampled = scale * input + offset` on each axis ([`affine_uv`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvMap {
    pub scale: [f64; 2],
    pub offset: [f64; 2],
}

/// The map from an input coordinate to the coordinate a program sampled at, from `points` (input, sampled) of runs at
/// known inputs: on each axis a scale and an offset, fitted to the two points farthest apart on that axis and confirmed by
/// every other point (at least four points, the inputs not on one line, so each axis is confirmed apart from the other).
/// A point is on the map within two roundings of an f32 multiply and add (`4 * f32::EPSILON` of the sum of the
/// magnitudes of the product, the offset and the sampled value), the most a program's `input * scale + offset` in f32
/// is off the exact map. Where the points are not on such a map, a 2D affine map of three of them tells why: one that
/// every point is on is a rotation or a shear (an axis takes the other); none, and the coordinate is not affine in the
/// input (a warp, or one made from another input or from time). Each is refused with its reason.
pub fn affine_uv(points: &[([f32; 2], [f32; 2])]) -> Result<UvMap, String> {
    if points.len() < 4 {
        return Err(format!(
            "{} points: a map is fitted to some and confirmed by a fourth",
            points.len()
        ));
    }
    let points: Vec<([f64; 2], [f64; 2])> = points
        .iter()
        .map(|(i, o)| (i.map(f64::from), o.map(f64::from)))
        .collect();
    let on_map = |predicted: f64, terms: f64, sampled: f64| {
        (predicted - sampled).abs() <= 4. * f64::from(f32::EPSILON) * (terms + sampled.abs())
    };
    // three inputs not on one line, for the 2D map
    let (p0, _) = points[0];
    let triangle = (1..points.len()).find_map(|i| {
        (i + 1..points.len()).find_map(|j| {
            let (pi, pj) = (points[i].0, points[j].0);
            let det = (pi[0] - p0[0]) * (pj[1] - p0[1]) - (pi[1] - p0[1]) * (pj[0] - p0[0]);
            (det != 0.).then_some((i, j, det))
        })
    });
    let Some((i, j, det)) = triangle else {
        return Err("the inputs are on one line: neither axis is confirmed apart from the other".into());
    };
    let mut scale = [0.; 2];
    let mut offset = [0.; 2];
    let mut separable = true;
    for axis in 0..2 {
        let low = points
            .iter()
            .min_by(|a, b| a.0[axis].total_cmp(&b.0[axis]))
            .expect("points");
        let high = points
            .iter()
            .max_by(|a, b| a.0[axis].total_cmp(&b.0[axis]))
            .expect("points");
        let a = (high.1[axis] - low.1[axis]) / (high.0[axis] - low.0[axis]);
        let c = low.1[axis] - a * low.0[axis];
        separable &= points.iter().all(|(input, sampled)| {
            on_map(
                a * input[axis] + c,
                (a * input[axis]).abs() + c.abs(),
                sampled[axis],
            )
        });
        (scale[axis], offset[axis]) = (a, c);
    }
    if separable {
        return Ok(UvMap { scale, offset });
    }
    // sampled[axis] = a * u + b * v + c on the three points (Cramer's rule)
    let (q0, qi, qj) = (points[0], points[i], points[j]);
    let solve = |axis: usize| {
        let (du_i, dv_i, ds_i) = (qi.0[0] - q0.0[0], qi.0[1] - q0.0[1], qi.1[axis] - q0.1[axis]);
        let (du_j, dv_j, ds_j) = (qj.0[0] - q0.0[0], qj.0[1] - q0.0[1], qj.1[axis] - q0.1[axis]);
        let a = (ds_i * dv_j - dv_i * ds_j) / det;
        let b = (du_i * ds_j - ds_i * du_j) / det;
        [a, b, q0.1[axis] - a * q0.0[0] - b * q0.0[1]]
    };
    let maps = [solve(0), solve(1)];
    let affine = points.iter().all(|(input, sampled)| {
        (0..2).all(|axis| {
            let [a, b, c] = maps[axis];
            let terms = (a * input[0]).abs() + (b * input[1]).abs() + c.abs();
            on_map(a * input[0] + b * input[1] + c, terms, sampled[axis])
        })
    });
    if affine {
        Err(format!(
            "an axis takes the other (a rotation or a shear): u from v by {}, v from u by {}",
            maps[0][1], maps[1][0]
        ))
    } else {
        Err("not affine in the input coordinate (a warp, or a coordinate made from another input or from time)".into())
    }
}

/// How many texture reads of a run were answered with a stand-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureUse {
    pub samples: u32,
    pub loads: u32,
    /// Size and level-count queries, as the lifted program asks them: one `GetDimensions` of a size and a level count
    /// may be two.
    pub queries: u32,
}

impl TextureUse {
    /// Whether the run read no texture.
    pub fn none(&self) -> bool {
        *self == TextureUse::default()
    }

    /// The stand-ins the run's texture reads were answered with, as the part of a reason that names them; `None` when it
    /// read no texture.
    pub fn stand_ins(&self) -> Option<String> {
        if self.none() {
            return None;
        }
        let mut parts = Vec::new();
        for (n, what, stand_in) in [
            (self.samples, "samples", format!("{SAMPLE_STAND_IN:?}")),
            (self.loads, "loads", format!("{LOAD_STAND_IN:?}")),
            (
                self.queries,
                "size and level-count queries",
                format!("{QUERY_STAND_IN:?}"),
            ),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what} answered with {stand_in}"));
            }
        }
        Some(format!(
            "the program reads a texture, answered with stand-ins ({}): a vertex it moves by the texel is not where the \
             constants alone put it",
            parts.join(", ")
        ))
    }

    /// `why` a check of the run's clip position failed, with the stand-ins the run's texture reads were answered with
    /// where it read any ([`TextureUse::stand_ins`]).
    pub fn reason(&self, why: String) -> String {
        match self.stand_ins() {
            Some(stand_ins) => format!("{why}; {stand_ins}"),
            None => why,
        }
    }
}

/// One run's answer: the clip position, the texture reads answered with stand-ins and the caller's buffers as the run
/// left them.
#[derive(Debug)]
pub struct ProbeRun<B> {
    pub clip: [f32; 4],
    pub textures: TextureUse,
    pub buffers: B,
}

/// A vertex program made ready to run one vertex at a time.
#[derive(Debug)]
pub struct VertexProbe {
    program: Program,
    executable: Executable,
    position: usize,
}

impl VertexProbe {
    /// The probe of `program`, a vertex program that writes a clip position and that the executor takes.
    pub fn new(program: Program) -> Result<Self, String> {
        if program.interface.stage != Stage::Vertex {
            return Err("the program is not a vertex program".into());
        }
        let position = program
            .interface
            .outputs
            .iter()
            .position(|o| o.system == Some(SystemValue::ClipPosition))
            .ok_or("the program writes no clip position")?;
        let executable =
            Executable::new(&program).map_err(|e| format!("the executor does not take the program: {e}"))?;
        Ok(VertexProbe {
            program,
            executable,
            position,
        })
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Runs one vertex: the constant buffers' rows by resource (a buffer not given is zeros), the program's inputs in
    /// the order of its interface, and its buffers as `buffers` answers them.
    pub fn run<B: BufferReads>(
        &self,
        constants: BTreeMap<ResourceId, Vec<[u32; 4]>>,
        inputs: &[[u32; 4]],
        buffers: B,
    ) -> Result<ProbeRun<B>, String> {
        let mut resources = Resources {
            constants,
            ..Resources::default()
        };
        for r in &self.program.interface.resources {
            if let iridia::ResourceKind::ConstantBuffer { layout } = &r.kind {
                resources
                    .constants
                    .entry(r.id)
                    .or_insert_with(|| vec![[0; 4]; layout.size.div_ceil(16).max(1) as usize]);
            }
        }
        let bound = self
            .executable
            .bind_resources(&resources)
            .map_err(|e| format!("the constants do not bind: {e}"))?;
        let mut backend = Answers {
            buffers,
            textures: AnsweredReads::new(StandIns),
        };
        let [out] = Runner::<1>::new(&self.executable, &bound, &mut backend, None)
            .run([inputs])
            .map_err(|e| format!("the program does not run: {e}"))?;
        let clip = out
            .values
            .get(self.position)
            .ok_or("the run gave no clip position")?
            .map(f32::from_bits);
        Ok(ProbeRun {
            clip,
            textures: backend.textures.counts(),
            buffers: backend.buffers,
        })
    }
}

/// The backend of a run: the caller's buffers, and stand-ins for the textures, recorded.
struct Answers<B> {
    buffers: B,
    textures: AnsweredReads<Element, StandIns>,
}

impl<B: BufferReads> Backend for Answers<B> {
    fn sample(&mut self, request: ImageSample<Element>) -> Result<[f32; 4], String> {
        self.textures.sample(request)
    }
    fn load(&mut self, request: ImageLoad<Element>) -> Result<[u32; 4], String> {
        self.textures.load(request)
    }
    fn query(&mut self, request: ImageQuery<Element>) -> Result<[u32; 4], String> {
        self.textures.query(request)
    }
    fn read_buffer(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String> {
        self.buffers.read(view, word, count)
    }
    fn buffer_words(&mut self, view: ResourceId) -> Result<u32, String> {
        self.buffers.words(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Lod;

    /// Points of `sampled = scale * input + offset`, the inputs not on one line.
    fn points(scale: [f32; 2], offset: [f32; 2], inputs: &[[f32; 2]]) -> Vec<([f32; 2], [f32; 2])> {
        inputs
            .iter()
            .map(|&[u, v]| ([u, v], [u * scale[0] + offset[0], v * scale[1] + offset[1]]))
            .collect()
    }

    const INPUTS: [[f32; 2]; 4] = [[0.1, 0.2], [0.7, 0.3], [0.4, 0.9], [0.55, 0.45]];

    /// A tiling and its centring offset are read back, within the stated rounding bound.
    #[test]
    fn an_affine_map_is_read_back_from_four_points() {
        let map = affine_uv(&points([20., 7.5], [-9.5, -3.25], &INPUTS)).unwrap();
        for (got, want) in map.scale.iter().chain(&map.offset).zip([20., 7.5, -9.5, -3.25]) {
            assert!((got - want).abs() < 1e-4, "{map:?}");
        }
    }

    /// Fewer than four points, inputs on one line, a point off the map and a map that turns an axis into the other are
    /// each refused with their own reason.
    #[test]
    fn a_map_that_cannot_be_confirmed_or_is_no_scale_and_offset_is_refused() {
        let three = points([2., 2.], [0., 0.], &INPUTS[..3]);
        assert!(affine_uv(&three).unwrap_err().contains("confirmed by a fourth"));
        let line: Vec<_> = (0..4)
            .map(|k| {
                let t = k as f32 * 0.25;
                ([t, t], [t, t])
            })
            .collect();
        assert!(affine_uv(&line).unwrap_err().contains("one line"));
        let warp: Vec<_> = INPUTS.iter().map(|&[u, v]| ([u, v], [u * u, v])).collect();
        assert!(affine_uv(&warp).unwrap_err().contains("not affine"));
        let turned: Vec<_> = INPUTS.iter().map(|&[u, v]| ([u, v], [v, -u])).collect();
        assert!(affine_uv(&turned).unwrap_err().contains("rotation or a shear"));
    }

    /// As a sampler, every read is answered by the caller and recorded: a plain 2D sample as the image sample it is; a
    /// load the answer does not give is refused and not recorded.
    #[test]
    fn answered_reads_answer_and_record_each_read() {
        let mut reads =
            AnsweredReads::new(|r: &ImageSample<u32>| Ok([r.coord[0], r.coord[1], r.texture as f32, 1.]));
        let plain = Sample {
            texture: 3,
            sampler: 1,
            uv: [0.25, 0.75],
            layer: None,
            lod: Lod::Level(2.),
        };
        assert_eq!(Sampler::sample(&mut reads, plain), Ok([0.25, 0.75, 3., 1.]));
        let load = ImageLoad {
            texture: 3,
            dim: Dim::D2,
            storage: false,
            texel: [0; 3],
            layer: None,
            sample: None,
            mip: None,
        };
        assert!(reads.load_image(load).unwrap_err().contains("texel load"));
        let [r] = reads.samples.as_slice() else {
            panic!("{:?}", reads.samples)
        };
        assert_eq!(
            (r.texture, r.sampler, r.dim, r.coord),
            (3, 1, Dim::D2, [0.25, 0.75, 0.])
        );
        assert_eq!(r.lod, crate::ImageLod::Level(2.));
        assert_eq!(
            reads.counts(),
            TextureUse {
                samples: 1,
                loads: 0,
                queries: 0
            }
        );
    }
}
