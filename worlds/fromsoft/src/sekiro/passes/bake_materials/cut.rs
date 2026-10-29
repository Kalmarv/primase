//! The alpha test a program states, carried by the bake where the lowering could not carry it: the value its discard
//! compares with the material's reference (`tested < reference`, [`iridia::output_uses::alpha_tests`]), evaluated
//! texel by texel from what the program itself sampled, becomes the baked albedo's alpha, and the material a mask at
//! the reference. The program keeps its test off in the bake ([`crate::layer_bake::ALPHA_TEST_POLICY`]), so every
//! texel's colour is baked, and the cut is the game's own expression rather than one slot's alpha.
//!
//! The compare is evaluated in a run of the program with its test on (the reference word the material's), and it is
//! the cut only where it is the discard's own test: at every texel the program discards exactly where the value is
//! below the reference ([`Compare::carry`]). A value no texture the bake binds varies (the engine's dither noise at
//! its neutral grey, constants and literals) is one value over the surface: it keeps every texel or none, and is no
//! cut.
use super::pixels::BoundTexture;
use crate::layer_bake::resolve;
use iridia::Interface;
use iridia::output_uses::{AlphaExpr, alpha_tests};
use iridia_exec::evaluate::Constants;
use iridia_exec::{
    AtomicAdd, Error, Execution, ImageAtomic, ImageLoad, ImageQuery, ImageSample, ImageStore, Load,
    PixelShader, ProgramShader, Sample, Sampler, Store,
};
use iridia_interface::{AlphaComparison, ResourceId};
use primase_core::shader::{ConstantRole, StageBinding};
use std::collections::BTreeMap;

/// What a material's notes and the bake's log count where the bake carries the cut ([`Compare`]).
pub const CARRIED_ALPHA_TEST: &str = "alpha test carried from the program's own compare: the value its discard \
     compares, evaluated from the program's samples in a run at its reference whose discards it matches at every \
     texel, is the baked albedo's alpha, cut at the material's reference, each texel quantized to UNORM8 on the side \
     of it the program puts the texel";

/// What a material's notes and the bake's log count where the program's compare is one value over the surface and the
/// program, run at its reference, keeps every texel: the material is not cut, as the game cuts none of it.
pub const UNCUT_ALPHA_TEST: &str = "alpha-tested material not cut: the value its program's discard compares is one \
     value over the surface (no texture it reads varies), and the program, run at its reference, keeps every texel";

/// What the bake makes of the program's compare over its grid ([`Compare::carry`]).
#[derive(Debug, PartialEq)]
pub(super) enum Carried {
    /// The albedo's alpha per texel, on the side of the reference the program's discard puts it once quantized.
    Cut(Vec<f32>),
    /// One value over the surface that the program keeps everywhere: no cut.
    Uncut,
}

/// The one test of a program against its alpha reference, ready to be evaluated: the tested expression, the reference
/// and where it lives, and the bind slot of every texture and constant buffer the expression reads.
#[derive(Debug)]
pub(super) struct Compare {
    tested: AlphaExpr,
    /// The material's reference: the mask's cutoff, and the value of the reference word wherever the expression reads it
    /// (the bake's constants hold the test-off value there).
    pub reference: f32,
    reference_word: (ResourceId, u32),
    /// The bind slot of the reference word's buffer.
    reference_slot: u32,
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
        let buffer = interface
            .resource(reference_word.0)
            .ok_or("the alpha reference's buffer is not in the interface")?;
        let reference_slot = resolve::register(buffer).map_err(|e| e.to_string())?;
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
            reference_slot,
            slots,
            register,
        }))
    }

    /// The bake's constants with the test on: the reference word the material's reference, the rest as given.
    pub(super) fn test_on(&self, constants: &Constants) -> Result<Constants, String> {
        let mut on = constants.clone();
        on.set_words(
            self.reference_slot,
            self.reference_word.1,
            &[self.reference.to_bits()],
        )
        .map_err(|e| format!("the alpha reference's word: {e}"))?;
        Ok(on)
    }

    /// Whether the tested value is one value over the surface: every texture it reads bound to a single value
    /// (`textures`, by slot), its words constants.
    pub(super) fn constant(&self, textures: &BTreeMap<u32, BoundTexture>) -> bool {
        self.tested.leaves().into_iter().all(|leaf| match leaf {
            AlphaExpr::Texel { resource, .. } => self
                .slots
                .get(&resource)
                .and_then(|slot| textures.get(slot))
                .is_some_and(|t| matches!(t, BoundTexture::Constant(_))),
            _ => true,
        })
    }

    /// The cut the compare makes over the bake's grid, from its value per texel (`values`) and whether the program,
    /// run at its reference, discarded the texel (`discarded`), both in row-major order; `constant` where the value is
    /// one value over the surface ([`Compare::constant`]). The compare is the discard's test only where the two agree at
    /// every texel; otherwise the error says at how many they differ. One value kept everywhere is no cut, discarded
    /// everywhere an error. A cut's texels are moved to the side of the reference the program puts them on: a UNORM8
    /// alpha at or above the reference where it keeps the texel, below where it discards it, so the quantized mask is
    /// the program's.
    pub(super) fn carry(
        &self,
        values: &[f32],
        discarded: &[bool],
        constant: bool,
    ) -> Result<Carried, String> {
        if values.len() != discarded.len() {
            return Err(format!(
                "{} tested values for {} texels",
                values.len(),
                discarded.len()
            ));
        }
        let differ = values
            .iter()
            .zip(discarded)
            .filter(|&(&v, &d)| (v < self.reference) != d)
            .count();
        if differ > 0 {
            return Err(format!(
                "the program, run at its reference {}, does not discard where {} is below it at {differ} of {} \
                 texels: the compare is not its discard's test",
                self.reference,
                self.tested,
                values.len()
            ));
        }
        let discards = discarded.iter().filter(|&&d| d).count();
        if constant {
            return match (discards, values.first()) {
                (0, _) => Ok(Carried::Uncut),
                (_, Some(v)) => Err(format!(
                    "the program discards every texel: {} is {v} over the whole surface, below the reference {}",
                    self.tested, self.reference
                )),
                (_, None) => Err("no texel was tested".into()),
            };
        }
        // the lowest UNORM8 alpha a mask at the reference keeps
        let kept = (1..=255u8)
            .find(|&b| f32::from(b) / 255. >= self.reference)
            .ok_or_else(|| {
                format!(
                    "the reference {} is above every alpha a texture holds",
                    self.reference
                )
            })?;
        let (below, at) = (f32::from(kept - 1) / 255., f32::from(kept) / 255.);
        Ok(Carried::Cut(
            values
                .iter()
                .zip(discarded)
                .map(|(&v, &d)| if d { v.min(below) } else { v.max(at) })
                .collect(),
        ))
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
    /// alpha 0.2 and 1 in its two columns, the second's 0.5 everywhere. Run with its test on, the program discards the
    /// texels whose product is below the reference and no other, so the compare is its discard's test and the cut is
    /// carried. A comparison the binding states of an output alpha is the lowering's to carry, not this.
    #[test]
    fn the_carried_value_is_what_the_programs_discard_compares() {
        let shader = fixture();
        let interface = shader.interface().clone();
        let stage = |compared| fixture_stage(&interface, compared);
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
        // the test on: the program's own discards, where the product is below 0.25
        let on = evaluate_raw_with(
            Threads::ONE,
            &Tested {
                program: &shader,
                compare: &compare,
            },
            &compare.test_on(&constants).unwrap(),
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
        assert_eq!(on.discarded, [true, false, true, false]);
        let values: Vec<f32> = on.pixels[0].iter().map(|(w, _)| f32::from_bits(w[0])).collect();
        assert_eq!(values, tested);
        let textures = BTreeMap::from([
            (
                0,
                BoundTexture::Image(std::sync::Arc::new(
                    crate::layer_bake::wrapped_texture(1, 1, &[255; 4], false).unwrap(),
                )),
            ),
            (1, BoundTexture::Constant([1., 1., 1., 0.5])),
        ]);
        assert!(!compare.constant(&textures));
        assert_eq!(
            compare.carry(&values, &on.discarded, false),
            Ok(Carried::Cut(vec![0.2 * 0.5, 0.5, 0.2 * 0.5, 0.5]))
        );
    }

    /// The program of `tests/fixtures/alpha_product.hlsl`.
    fn fixture() -> ProgramShader {
        crate::layer_bake::program_executor(
            ShaderFormat::Dxbc,
            include_bytes!("../../../../tests/fixtures/alpha_product.dxbc"),
        )
        .unwrap()
    }

    /// The fixture's pixel stage, its reference 0.25 stated as a test `compared` so.
    fn fixture_stage(interface: &Interface, compared: AlphaComparison) -> StageBinding {
        let buffer = interface
            .resources
            .iter()
            .find(|r| r.provenance.name == "cbMtdParam")
            .unwrap()
            .id;
        StageBinding {
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
        }
    }

    /// A compare of `tested` against `reference`, for the cut's own rules.
    fn compare(tested: AlphaExpr, reference: f32) -> Compare {
        Compare {
            tested,
            reference,
            reference_word: (ResourceId(0), 4),
            reference_slot: 0,
            slots: BTreeMap::new(),
            register: 0,
        }
    }

    /// Each texel lands on the side of the reference the program's discard puts it, once quantized to UNORM8: a texel
    /// it discards just below a reference of 128/255 stays below (127/255), not rounded up onto it, and one it keeps at
    /// the reference stays at it. Where the program's discard and the compare differ at any texel, the compare is not
    /// the discard's test and nothing is carried.
    #[test]
    fn the_cut_is_the_side_the_programs_discard_puts_each_texel() {
        let reference = 128. / 255.;
        let c = compare(AlphaExpr::Literal(0.), reference);
        let values = [reference - 1e-4, reference, 0.9, 0.1];
        assert_eq!(
            c.carry(&values, &[true, false, false, true], false),
            Ok(Carried::Cut(vec![127. / 255., reference, 0.9, 0.1]))
        );
        let differ = c.carry(&values, &[false, false, false, true], false).unwrap_err();
        assert!(differ.contains("at 1 of 4 texels"), "{differ}");
        assert!(c.carry(&values, &[true, false], false).is_err());
        // a reference no UNORM8 alpha reaches keeps no texel a texture can hold
        assert!(
            compare(AlphaExpr::Literal(0.), 1.5)
                .carry(&[2.], &[false], false)
                .is_err()
        );
    }

    /// `M[AMSN_V][Mb4][3Mask][Ov_N2][beach]_Gbuf_[A]` discards where `FC_AlphaRef.y * (dither - 0.5) + 1 <
    /// FC_AlphaRef.x`, at a reference of 1 (`g_AlphaRef` 255): no texture of the surface reaches the value, and with
    /// the dither at its neutral grey it is 1 at every texel, which the program keeps. One value kept everywhere is no
    /// cut, not a mask at the reference; one discarded everywhere is refused.
    #[test]
    fn one_value_over_the_surface_kept_everywhere_is_no_cut() {
        // a texture's alpha the program samples, standing for the dither noise
        let shader = fixture();
        let interface = shader.interface().clone();
        let fixture = Compare::of(
            &shader,
            &interface,
            &fixture_stage(&interface, AlphaComparison::Unwritten),
        )
        .unwrap()
        .unwrap();
        let dither = fixture
            .tested
            .leaves()
            .into_iter()
            .find(|l| matches!(l, AlphaExpr::Texel { .. }))
            .unwrap();
        let AlphaExpr::Texel { resource, .. } = dither else {
            unreachable!("a texel")
        };
        let slot = fixture.slots[&resource];
        let tested = AlphaExpr::Add(
            Box::new(AlphaExpr::Multiply(
                Box::new(AlphaExpr::Word {
                    buffer: ResourceId(0),
                    word: 5,
                }),
                Box::new(AlphaExpr::Add(
                    Box::new(dither),
                    Box::new(AlphaExpr::Literal(-0.5)),
                )),
            )),
            Box::new(AlphaExpr::Literal(1.)),
        );
        let mut c = compare(tested, 1.);
        c.slots.insert(resource, slot);
        let neutral = BTreeMap::from([(slot, BoundTexture::Constant([0.5; 4]))]);
        assert!(c.constant(&neutral));
        assert_eq!(c.carry(&[1.; 4], &[false; 4], true), Ok(Carried::Uncut));
        let discards = c.carry(&[0.9; 4], &[true; 4], true).unwrap_err();
        assert!(discards.contains("discards every texel"), "{discards}");
        assert!(!c.constant(&BTreeMap::new()));
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
