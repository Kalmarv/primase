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
use std::collections::BTreeMap;

use primase_shader::{Program, ResourceId, Stage, SystemValue};

use crate::{Backend, Element, Executable, ImageLoad, ImageQuery, ImageSample, Resources, Runner};

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
            if let primase_shader::ResourceKind::ConstantBuffer { layout } = &r.kind {
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
            textures: TextureUse::default(),
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
            textures: backend.textures,
            buffers: backend.buffers,
        })
    }
}

/// The backend of a run: the caller's buffers, and stand-ins for the textures, counted.
struct Answers<B> {
    buffers: B,
    textures: TextureUse,
}

impl<B: BufferReads> Backend for Answers<B> {
    fn sample(&mut self, _request: ImageSample<Element>) -> Result<[f32; 4], String> {
        self.textures.samples += 1;
        Ok(SAMPLE_STAND_IN)
    }
    fn load(&mut self, _request: ImageLoad<Element>) -> Result<[u32; 4], String> {
        self.textures.loads += 1;
        Ok(LOAD_STAND_IN)
    }
    fn query(&mut self, _request: ImageQuery<Element>) -> Result<[u32; 4], String> {
        self.textures.queries += 1;
        Ok(QUERY_STAND_IN)
    }
    fn read_buffer(&mut self, view: ResourceId, word: u32, count: u8) -> Result<[u32; 4], String> {
        self.buffers.read(view, word, count)
    }
    fn buffer_words(&mut self, view: ResourceId) -> Result<u32, String> {
        self.buffers.words(view)
    }
}
