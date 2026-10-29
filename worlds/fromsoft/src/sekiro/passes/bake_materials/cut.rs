//! The alpha test a program states, carried by the bake where the lowering could not carry it: the value its discard
//! compares with the material's reference (`tested < reference`, [`iridia::output_uses::alpha_tests`]), evaluated
//! texel by texel from what the program itself sampled, becomes the baked albedo's alpha, and the material a mask at
//! the reference. The program keeps its test off in the bake ([`crate::layer_bake::ALPHA_TEST_POLICY`]), so every
//! texel's colour is baked, and the cut is the game's own expression rather than one slot's alpha.
use crate::layer_bake::resolve;
use iridia::Interface;
use iridia::output_uses::{AlphaExpr, alpha_tests};
use iridia_exec::{
    AtomicAdd, Error, Execution, ImageAtomic, ImageLoad, ImageQuery, ImageSample, ImageStore, Load,
    PixelShader, ProgramShader, Sample, Sampler, Store,
};
use iridia_interface::{AlphaComparison, ResourceId};
use primase_core::shader::{ConstantRole, StageBinding};
use std::collections::BTreeMap;

/// What a material's notes and the bake's log count where the bake carries the cut ([`Compare`]).
pub const CARRIED_ALPHA_TEST: &str = "alpha test carried from the program's own compare: the value its discard \
     compares, evaluated from the program's samples with the alpha test off, is the baked albedo's alpha, cut at the \
     material's reference (quantized to UNORM8)";

/// The one test of a program against its alpha reference, ready to be evaluated: the tested expression, the reference
/// and where it lives, and the bind slot of every texture and constant buffer the expression reads.
#[derive(Debug)]
pub(super) struct Compare {
    tested: AlphaExpr,
    /// The material's reference: the mask's cutoff, and the value of the reference word wherever the expression reads it
    /// (the bake's constants hold the test-off value there).
    pub reference: f32,
    reference_word: (ResourceId, u32),
    /// The bind slot of each texture and constant buffer resource the expression reads.
    slots: BTreeMap<ResourceId, u32>,
    /// The output register the evaluation writes the tested value to: one past the program's own.
    pub register: u32,
}

impl Compare {
    /// The test a pixel stage's program makes of its alpha reference, where the binding states it as a comparison of a
    /// value the program writes nowhere ([`AlphaComparison::Unwritten`]) and above zero: `Ok(None)` where it states no
    /// such test. An error names why the program's tests of the reference are not one exact expression.
    pub(super) fn of(
        shader: &ProgramShader,
        interface: &Interface,
        stage: &StageBinding,
    ) -> Result<Option<Compare>, String> {
        let Some((reference, reference_word)) = stage.constants.iter().find_map(|c| match c.role {
            ConstantRole::AlphaReference {
                value,
                compared: AlphaComparison::Unwritten,
            } if value > 0. => Some((value, (c.resource, c.offset / 4))),
            _ => None,
        }) else {
            return Ok(None);
        };
        let graph = shader.program().graph()?;
        let mut tests = alpha_tests(&graph)
            .into_iter()
            .filter(|t| (t.buffer, t.word) == reference_word)
            .map(|t| t.tested);
        let tested = tests
            .next()
            .ok_or("no discard of the program compares the alpha reference")?;
        if let Some(other) = tests.next() {
            return Err(format!(
                "the program discards on two expressions against its alpha reference: {tested}; {other}"
            ));
        }
        if !tested.is_exact() {
            return Err(format!("the tested value {tested} is not an exact expression"));
        }
        let mut slots = BTreeMap::new();
        for leaf in tested.leaves() {
            let resource = match leaf {
                AlphaExpr::Texel { resource, .. } => resource,
                AlphaExpr::Word { buffer, .. } => buffer,
                _ => continue,
            };
            let r = interface
                .resource(resource)
                .ok_or_else(|| format!("resource {} of {tested} is not in the interface", resource.0))?;
            slots.insert(resource, resolve::register(r).map_err(|e| e.to_string())?);
        }
        let register = interface
            .outputs
            .iter()
            .filter_map(|o| o.provenance.d3d.map(|d| d.register))
            .max()
            .map_or(0, |r| r.saturating_add(1));
        Ok(Some(Compare {
            tested,
            reference,
            reference_word,
            slots,
            register,
        }))
    }

    /// The tested value of one invocation: its leaves the words of `buffers` (the reference word the material's
    /// reference) and the lanes of what the invocation sampled (`seen`, by texture slot).
    fn value(&self, buffers: &[(u32, &[[u32; 4]])], seen: &BTreeMap<u32, Seen>) -> Result<f32, String> {
        let mut leaf = |e: &AlphaExpr| -> Result<f32, String> {
            match *e {
                AlphaExpr::Word { buffer, word } if (buffer, word) == self.reference_word => {
                    Ok(self.reference)
                }
                AlphaExpr::Word { buffer, word } => {
                    let slot = self.slots.get(&buffer).ok_or("a word of an unbound buffer")?;
                    buffers
                        .iter()
                        .find(|(s, _)| s == slot)
                        .and_then(|(_, rows)| rows.get((word / 4) as usize))
                        .and_then(|row| row.get((word % 4) as usize))
                        .map(|bits| f32::from_bits(*bits))
                        .ok_or_else(|| format!("word {word} of cb{slot} is not bound"))
                }
                AlphaExpr::Texel { resource, lane, .. } => {
                    let slot = self
                        .slots
                        .get(&resource)
                        .ok_or("a sample of an unbound texture")?;
                    match seen.get(slot) {
                        Some(Seen::Once(texel)) => texel
                            .get(usize::from(lane))
                            .copied()
                            .ok_or_else(|| format!("lane {lane} of t{slot}")),
                        Some(Seen::Differing) => Err(format!(
                            "t{slot} is sampled more than once with different values: which sample {} reads is not known",
                            self.tested
                        )),
                        None => Err(format!("t{slot}, which {} reads, was not sampled", self.tested)),
                    }
                }
                _ => Err(format!("{e} is not a leaf")),
            }
        };
        evaluate(&self.tested, &mut leaf)
    }
}

/// `e` evaluated in the program's own single-precision arithmetic, its leaves by `leaf`.
fn evaluate(e: &AlphaExpr, leaf: &mut dyn FnMut(&AlphaExpr) -> Result<f32, String>) -> Result<f32, String> {
    Ok(match e {
        AlphaExpr::Literal(n) => *n as f32,
        AlphaExpr::Word { .. } | AlphaExpr::Texel { .. } => leaf(e)?,
        AlphaExpr::Negate(a) => -evaluate(a, leaf)?,
        AlphaExpr::Add(a, b) => evaluate(a, leaf)? + evaluate(b, leaf)?,
        AlphaExpr::Subtract(a, b) => evaluate(a, leaf)? - evaluate(b, leaf)?,
        AlphaExpr::Multiply(a, b) => evaluate(a, leaf)? * evaluate(b, leaf)?,
        AlphaExpr::Other(what) => return Err(format!("{what} is not evaluated")),
    })
}

/// What one invocation sampled from a texture slot: one texel, or several that differ (two sample instructions at
/// different coordinates, which a slot alone does not tell apart).
#[derive(Clone, Copy, Debug)]
enum Seen {
    Once([f32; 4]),
    Differing,
}

/// The sampler of one invocation, answering through `inner` and keeping what each texture slot answered.
struct Recorder<'a> {
    inner: &'a mut dyn Sampler,
    seen: BTreeMap<u32, Seen>,
}

impl Recorder<'_> {
    fn keep(&mut self, slot: u32, texel: [f32; 4]) -> [f32; 4] {
        let seen = match self.seen.get(&slot) {
            None => Seen::Once(texel),
            Some(Seen::Once(was)) if was.map(f32::to_bits) == texel.map(f32::to_bits) => Seen::Once(texel),
            Some(_) => Seen::Differing,
        };
        self.seen.insert(slot, seen);
        texel
    }
}

impl Sampler for Recorder<'_> {
    fn sample(&mut self, request: Sample) -> Result<[f32; 4], String> {
        let texel = self.inner.sample(request)?;
        Ok(self.keep(request.texture, texel))
    }
    fn sample_image(&mut self, request: ImageSample<u32>) -> Result<[f32; 4], String> {
        let slot = request.texture;
        let texel = self.inner.sample_image(request)?;
        Ok(self.keep(slot, texel))
    }
    fn load(&mut self, request: Load) -> Result<[u32; 4], String> {
        self.inner.load(request)
    }
    fn store(&mut self, request: Store) -> Result<(), String> {
        self.inner.store(request)
    }
    fn atomic_add(&mut self, request: AtomicAdd) -> Result<u32, String> {
        self.inner.atomic_add(request)
    }
    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> Result<u32, String> {
        self.inner.structured(slot, element, offset)
    }
    fn buffer_elements(&mut self, slot: u32) -> Option<u32> {
        self.inner.buffer_elements(slot)
    }
    fn load_image(&mut self, request: ImageLoad<u32>) -> Result<[u32; 4], String> {
        self.inner.load_image(request)
    }
    fn query_image(&mut self, request: ImageQuery<u32>) -> Result<[u32; 4], String> {
        self.inner.query_image(request)
    }
    fn atomic_image(&mut self, request: ImageAtomic<u32>) -> Result<u32, String> {
        self.inner.atomic_image(request)
    }
    fn store_image(&mut self, request: ImageStore<u32>) -> Result<(), String> {
        self.inner.store_image(request)
    }
}

/// The program run one invocation at a time, each run's samples kept, and the tested value written to the extra output
/// register [`Compare::register`] (its first lane): an evaluation of this shader's targets is the test's value per
/// texel, from the same inputs, constants and textures as the bake's.
#[derive(Debug)]
pub(super) struct Tested<'a> {
    pub program: &'a ProgramShader,
    pub compare: &'a Compare,
}

impl PixelShader for Tested<'_> {
    fn interface(&self) -> &Interface {
        self.program.interface()
    }
    fn input_count(&self) -> usize {
        self.program.input_count()
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        self.program.constant_buffers()
    }
    fn textures(&self) -> &[u32] {
        self.program.textures()
    }
    fn samplers(&self) -> &[u32] {
        self.program.samplers()
    }
    fn execute(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        let mut recorder = Recorder {
            inner: sampler,
            seen: BTreeMap::new(),
        };
        let run = self.program.execute(inputs, buffers, &mut recorder)?;
        self.with_value(run, buffers, &recorder.seen)
    }
    fn execute_with_derivatives(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        let mut recorder = Recorder {
            inner: sampler,
            seen: BTreeMap::new(),
        };
        let run = self
            .program
            .execute_with_derivatives(inputs, derivatives, buffers, &mut recorder)?;
        self.with_value(run, buffers, &recorder.seen)
    }
}

impl Tested<'_> {
    fn with_value(
        &self,
        mut run: Execution,
        buffers: &[(u32, &[[u32; 4]])],
        seen: &BTreeMap<u32, Seen>,
    ) -> Result<Execution, Error> {
        let value = self.compare.value(buffers, seen).map_err(|detail| Error {
            bytecode: "alpha test",
            at: 0,
            detail,
        })?;
        let r = self.compare.register as usize;
        if run.outputs.len() <= r {
            run.outputs.resize(r + 1, [0; 4]);
            run.output_masks.resize(r + 1, 0);
        }
        if let (Some(out), Some(mask)) = (run.outputs.get_mut(r), run.output_masks.get_mut(r)) {
            *out = [value.to_bits(), 0, 0, 0];
            *mask = 1;
        }
        Ok(run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iridia_exec::evaluate::{Constants, Layout};
    use iridia_exec::{Grid, Input, Invocation, View, Views, evaluate_raw_with};
    use iridia_interface::{ShaderFormat, Stage};
    use primase_core::shader::ConstantBinding;
    use primase_foundations::parallel::Threads;

    /// A program whose discard tests the product of two textures' alphas, a value it writes to no target
    /// (`tests/fixtures/alpha_product.hlsl`), bound with its reference stated as such a test: the compare is that product
    /// against the reference, and the evaluation writes it per texel, from what each texel sampled: the first texture's
    /// alpha 0.2 and 1 in its two columns, the second's 0.5 everywhere. A comparison the binding states of an output
    /// alpha is the lowering's to carry, not this.
    #[test]
    fn the_carried_value_is_what_the_programs_discard_compares() {
        let shader = crate::layer_bake::program_executor(
            ShaderFormat::Dxbc,
            include_bytes!("../../../../tests/fixtures/alpha_product.dxbc"),
        )
        .unwrap();
        let interface = shader.interface().clone();
        let buffer = interface
            .resources
            .iter()
            .find(|r| r.provenance.name == "cbMtdParam")
            .unwrap()
            .id;
        let stage = |compared| StageBinding {
            constants: vec![ConstantBinding {
                resource: buffer,
                offset: 80,
                size: 4,
                name: Some("FC_AlphaRef".into()),
                role: ConstantRole::AlphaReference {
                    value: 0.25,
                    compared,
                },
                lines: None,
            }],
            ..StageBinding::new(Stage::Pixel, primase_core::World::default().spawn())
        };
        assert!(
            Compare::of(&shader, &interface, &stage(AlphaComparison::Output))
                .unwrap()
                .is_none()
        );
        let compare = Compare::of(&shader, &interface, &stage(AlphaComparison::Unwritten))
            .unwrap()
            .unwrap();
        assert_eq!(compare.reference, 0.25);
        assert_eq!(compare.register, 3);
        // the bake's constants: the colour factor one, the reference frozen below every alpha (the test off)
        let mut constants = Constants::new(&Layout::of(&shader).unwrap());
        constants.set_words(3, 0, &[1f32.to_bits(); 4]).unwrap();
        constants.set_words(3, 20, &[(-100f32).to_bits()]).unwrap();
        let first = crate::layer_bake::wrapped_texture(2, 1, &[255, 255, 255, 51, 255, 255, 255, 255], false)
            .unwrap();
        let views = Views::refusing(BTreeMap::from([
            (0, View::Image(std::sync::Arc::new(first))),
            (1, View::Constant([1., 1., 1., 0.5])),
        ]));
        let inputs = BTreeMap::from([
            (
                ("SV_Position".to_owned(), 0),
                Input::Constant([0.5, 0.5, 0.5, 1.]),
            ),
            (("TEXCOORD".to_owned(), 0), Input::GridUv),
        ]);
        let raw = evaluate_raw_with(
            Threads::ONE,
            &Tested {
                program: &shader,
                compare: &compare,
            },
            &constants,
            &views,
            &inputs,
            Grid {
                size: 2,
                period: [1.; 2],
            },
            &[(compare.register, 1)],
            Invocation::Scalar,
        )
        .unwrap();
        assert!(raw.discarded.iter().all(|d| !d));
        let tested: Vec<f32> = raw.pixels[0].iter().map(|(w, _)| f32::from_bits(w[0])).collect();
        assert_eq!(tested, [0.2 * 0.5, 0.5, 0.2 * 0.5, 0.5]);
    }

    /// The tested value is the expression's arithmetic on its leaves, single precision, and an expression with a part
    /// the graph does not follow is not evaluated.
    #[test]
    fn the_tested_value_is_the_expressions_arithmetic() {
        let word = |word: u32| AlphaExpr::Word {
            buffer: ResourceId(9),
            word,
        };
        // (dither - 0.5) * scale + a * b, the dither answered at its neutral 0.5: the product alone
        let e = AlphaExpr::Add(
            Box::new(AlphaExpr::Multiply(
                Box::new(word(21)),
                Box::new(AlphaExpr::Add(
                    Box::new(word(2)),
                    Box::new(AlphaExpr::Literal(-0.5)),
                )),
            )),
            Box::new(AlphaExpr::Multiply(Box::new(word(4)), Box::new(word(3)))),
        );
        let mut leaf = |l: &AlphaExpr| match *l {
            AlphaExpr::Word { word, .. } => Ok(match word {
                21 => 0.25,
                2 => 0.5,
                4 => 0.5,
                3 => 0.75,
                _ => f32::NAN,
            }),
            _ => Err("not a leaf".to_owned()),
        };
        assert_eq!(evaluate(&e, &mut leaf), Ok(0.5 * 0.75));
        assert_eq!(
            evaluate(
                &AlphaExpr::Negate(Box::new(AlphaExpr::Subtract(
                    Box::new(word(3)),
                    Box::new(word(2))
                ))),
                &mut leaf
            ),
            Ok(-0.25)
        );
        assert!(evaluate(&AlphaExpr::Other("a select".into()), &mut leaf).is_err());
    }

    /// A slot answered twice with the same texel is one value; with another, the slot alone no longer says which
    /// sample a leaf reads.
    #[test]
    fn a_slot_sampled_twice_differently_is_not_one_value() {
        let mut answer = |s: Sample| Ok([s.uv[0], 0., 0., 1.]);
        let mut recorder = Recorder {
            inner: &mut answer,
            seen: BTreeMap::new(),
        };
        let sample = |u: f32| Sample {
            texture: 3,
            sampler: 0,
            uv: [u, 0.],
            layer: None,
            lod: iridia_exec::Lod::Implicit,
        };
        recorder.sample(sample(0.25)).unwrap();
        recorder.sample(sample(0.25)).unwrap();
        assert!(matches!(recorder.seen[&3], Seen::Once([0.25, 0., 0., 1.])));
        recorder.sample(sample(0.5)).unwrap();
        assert!(matches!(recorder.seen[&3], Seen::Differing));
    }
}
