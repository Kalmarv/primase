#![allow(clippy::unwrap_used)]
//! Invocations in lockstep against the same invocations one after another, on one HLSL fixture compiled to both
//! bytecodes (tests/fixtures/lanes.hlsl) whose pixels diverge: a branch only some take with a sample in it, a loop each
//! pixel turns its own number of times and some leave early, a discard some take. In lockstep
//! ([`ProgramShader::execute_lockstep`], a row of 20: a run of 16 and one of 4) every pixel's outputs, written lanes,
//! instructions, samples and discard must be those it has alone ([`PixelShader::execute_with_derivatives`]); a pixel
//! past the step limit must stop [`PixelShader::execute_lanes`] as it stops alone, the first such pixel named; and a
//! grid evaluation must be the one made one pixel after another.
use iridia_exec::evaluate::Constants;
use iridia_exec::{
    Error, Execution, Grid, Image, Input, Invocation, Layout, PixelShader, ProgramShader, Raw, Sampler, View,
    Views, evaluate_raw_with,
};
use primase_foundations::parallel::Threads;
use std::collections::BTreeMap;
use std::sync::Arc;

const ROW: u32 = 20;

/// A pixel's input registers, and the inputs' derivatives (per register, d/dx then d/dy).
type Registers = Vec<[u32; 4]>;
type Derivatives = Vec<[[u32; 4]; 2]>;

fn shaders() -> Vec<(&'static str, ProgramShader)> {
    let executor = |program: iridia::Program| {
        program.validate().unwrap_or_else(|p| panic!("{p:?}"));
        ProgramShader::new(program).unwrap()
    };
    vec![
        (
            "DXBC",
            executor(iridia_dxbc::lift(include_bytes!("fixtures/lanes.dxbc")).unwrap()),
        ),
        (
            "DXIL",
            executor(primase_dxil::lift(include_bytes!("fixtures/lanes.dxil")).unwrap()),
        ),
    ]
}

/// Returns the coordinates it is sampled at.
#[derive(Debug)]
struct Coordinates;
impl Image for Coordinates {
    fn sample(&self, uv: [f32; 2], _lod: iridia_exec::Lod) -> Result<[f32; 4], String> {
        Ok([uv[0], uv[1], 0.5, 1.])
    }
}

fn textures() -> Views {
    Views::refusing(BTreeMap::from([(0, View::Image(Arc::new(Coordinates)))]))
}

/// `threshold` (the sampling side) and `budget` (past which a pixel's loop does not end), as rows.
fn constants(shader: &ProgramShader, budget: f32) -> Constants {
    let mut c = Constants::new(&Layout::of(shader).unwrap());
    assert!(c.set("threshold", &[0.5]).unwrap());
    assert!(c.set("budget", &[budget]).unwrap());
    c
}

/// The input registers of a pixel at `uv` (TEXCOORD0 where its signature puts it), and the derivatives: U steps by
/// `1 / ROW` along x.
fn registers(shader: &ProgramShader, uv: [f32; 2]) -> (Registers, Derivatives) {
    let mut r = vec![[0u32; 4]; shader.input_count()];
    let mut d = vec![[[0u32; 4]; 2]; shader.input_count()];
    let v = shader
        .interface()
        .inputs
        .iter()
        .find(|v| v.label.name.eq_ignore_ascii_case("TEXCOORD") && v.label.index == 0)
        .unwrap();
    let sig = v.provenance.d3d.unwrap();
    let lanes: Vec<usize> = (0..4).filter(|l| sig.mask & (1 << l) != 0).collect();
    r[sig.register as usize][lanes[0]] = uv[0].to_bits();
    r[sig.register as usize][lanes[1]] = uv[1].to_bits();
    d[sig.register as usize][0][lanes[0]] = (1. / ROW as f32).to_bits();
    (r, d)
}

/// What a test compares of an execution.
fn seen(e: &Execution) -> (Vec<[u32; 4]>, Vec<u8>, usize, usize, bool) {
    (
        e.outputs.clone(),
        e.output_masks.clone(),
        e.instructions,
        e.samples,
        e.discarded,
    )
}

/// A row of pixels at `v`: U at the pixel centres.
fn row(shader: &ProgramShader, v: f32) -> (Vec<Registers>, Derivatives) {
    let mut derivatives = Vec::new();
    let inputs = (0..ROW)
        .map(|x| {
            let (r, d) = registers(shader, [(x as f32 + 0.5) / ROW as f32, v]);
            derivatives = d;
            r
        })
        .collect();
    (inputs, derivatives)
}

#[test]
fn invocations_in_lockstep_run_as_each_runs_alone() {
    let textures = textures();
    for (name, shader) in shaders() {
        let c = constants(&shader, 2.);
        let buffers: Vec<(u32, &[[u32; 4]])> = c.buffers().iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let mut sampler = |s: iridia_exec::Sample| textures_sample(&textures, s);
        let (mut sampled, mut discards, mut steps) = (0, 0, std::collections::BTreeSet::new());
        for y in 0..ROW {
            let (inputs, derivatives) = row(&shader, (y as f32 + 0.5) / ROW as f32);
            let lanes: Vec<&[[u32; 4]]> = inputs.iter().map(Vec::as_slice).collect();
            let together = shader
                .execute_lockstep(&lanes, &derivatives, &buffers, &mut sampler)
                .unwrap();
            assert_eq!(together.len(), ROW as usize, "{name}");
            let mut kinds = std::collections::BTreeSet::new();
            for (x, (one, t)) in inputs.iter().zip(&together).enumerate() {
                let alone = shader
                    .execute_with_derivatives(one, &derivatives, &buffers, &mut sampler)
                    .unwrap();
                assert_eq!(seen(t), seen(&alone), "{name}: pixel ({x}, {y})");
                kinds.insert((t.samples, t.discarded, t.instructions));
                sampled += t.samples;
                discards += usize::from(t.discarded);
                steps.insert(t.instructions);
            }
            // the row's pixels diverge: some sample and some do not, some discard, loops of different lengths
            assert!(kinds.len() >= 4, "{name}: row {y} does not diverge: {kinds:?}");
        }
        let pixels = (ROW * ROW) as usize;
        assert!(
            sampled > 0 && sampled < pixels,
            "{name}: {sampled} of {pixels} sample"
        );
        assert!(
            discards > 0 && discards < pixels,
            "{name}: {discards} of {pixels} discard"
        );
        assert!(steps.len() >= 8, "{name}: instruction counts {steps:?}");
    }
}

/// The [`Views`] as a [`Sampler`]: the fixture samples t0 only.
fn textures_sample(textures: &Views, s: iridia_exec::Sample) -> Result<[f32; 4], String> {
    match textures.views.get(&s.texture) {
        Some(View::Image(image)) => image.sample(s.uv, s.lod),
        _ => Err(format!("t{} has no texture", s.texture)),
    }
}

#[test]
fn a_pixel_past_the_step_limit_stops_the_run_as_it_stops_alone() {
    let textures = textures();
    for (name, shader) in shaders() {
        // pixels 12 and on loop without end: four of them in the first run of 16
        let c = constants(&shader, 0.6);
        let buffers: Vec<(u32, &[[u32; 4]])> = c.buffers().iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let mut sampler = |s: iridia_exec::Sample| textures_sample(&textures, s);
        // a row whose loops never leave early (z stays above V)
        let (inputs, derivatives) = row(&shader, 0.025);
        let lanes: Vec<&[[u32; 4]]> = inputs.iter().map(Vec::as_slice).collect();
        let alone: Error = shader
            .execute_with_derivatives(&inputs[12], &derivatives, &buffers, &mut sampler)
            .unwrap_err();
        assert!(
            alone.detail.contains("execution budget exceeded"),
            "{name}: {alone}"
        );
        let mut given = Vec::new();
        let (at, error) = shader
            .execute_lanes(&lanes, &derivatives, &buffers, &mut sampler, &mut |i, _| {
                given.push(i)
            })
            .unwrap_err();
        assert_eq!((at, error.to_string()), (12, alone.to_string()), "{name}");
        // the invocations before the first failing one, in order
        assert_eq!(given, (0..12).collect::<Vec<_>>(), "{name}");
        assert!(
            shader
                .execute_lockstep(&lanes, &derivatives, &buffers, &mut sampler)
                .is_err(),
            "{name}"
        );
    }
}

/// The shader with [`PixelShader::execute_lanes`]'s default: one invocation after another.
#[derive(Debug)]
struct OneByOne<'a>(&'a ProgramShader);
impl PixelShader for OneByOne<'_> {
    fn interface(&self) -> &iridia::Interface {
        self.0.interface()
    }
    fn input_count(&self) -> usize {
        self.0.input_count()
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        self.0.constant_buffers()
    }
    fn textures(&self) -> &[u32] {
        PixelShader::textures(self.0)
    }
    fn samplers(&self) -> &[u32] {
        self.0.samplers()
    }
    fn execute(
        &self,
        inputs: &[[u32; 4]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        self.0.execute(inputs, buffers, sampler)
    }
    fn execute_with_derivatives(
        &self,
        inputs: &[[u32; 4]],
        derivatives: &[[[u32; 4]; 2]],
        buffers: &[(u32, &[[u32; 4]])],
        sampler: &mut dyn Sampler,
    ) -> Result<Execution, Error> {
        self.0
            .execute_with_derivatives(inputs, derivatives, buffers, sampler)
    }
}

#[test]
fn a_grid_evaluates_in_lockstep_as_one_pixel_after_another() {
    let textures = textures();
    for (name, shader) in shaders() {
        let c = constants(&shader, 2.);
        let inputs = BTreeMap::from([(("TEXCOORD".to_owned(), 0), Input::GridUv)]);
        let grid = Grid {
            size: ROW,
            period: [1., 1.],
        };
        let run = |s: &dyn PixelShader| -> Raw {
            evaluate_raw_with(
                Threads::ONE,
                s,
                &c,
                &textures,
                &inputs,
                grid,
                &[(0, 15)],
                Invocation::Scalar,
            )
            .unwrap()
        };
        let (together, apart) = (run(&shader), run(&OneByOne(&shader)));
        assert_eq!(together.pixels, apart.pixels, "{name}");
        assert_eq!(
            (together.instructions, together.samples),
            (apart.instructions, apart.samples),
            "{name}"
        );
    }
}
