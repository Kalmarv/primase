//! Synthetic gates on our own dxc-built fixtures (`tests/fixtures/*.hlsl`, compiled with the command at the top of
//! each): the lifted interface, execution (`iridia_exec::ProgramShader`) against outputs derived from the HLSL by
//! hand, every truncation refused, and a seeded mutation run that must never panic.
use primase_dxil::lift;
use iridia::{
    ConstantType, InterpMode, InterpSampling, ResourceKind, SampleType, ScalarType, SystemValue, TextureDim,
    Value,
};
use iridia_exec::ProgramShader;
use iridia_exec::{PixelShader, Sample};

const BLEND: &[u8] = include_bytes!("fixtures/blend.dxil");

/// The fixture lifted, validated and ready to run.
fn shader(bytes: &[u8]) -> ProgramShader {
    let program = lift(bytes).unwrap_or_else(|e| panic!("{e}"));
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));
    ProgramShader::new(program).unwrap_or_else(|e| panic!("{e}"))
}

fn f(x: f32) -> u32 {
    x.to_bits()
}
/// The constant buffer `Constants` (b3): scale, bias, flags.
fn constants() -> Vec<[u32; 4]> {
    vec![
        [f(2.), f(4.), f(1.5), 0],
        [f(0.5), f(0.25), f(0.1), 0],
        [5, 0, 0, 0],
    ]
}
fn inputs(front: bool) -> Vec<[u32; 4]> {
    vec![
        [0; 4],
        [f(0.25), f(0.75), f(0.4), 0],
        [7, if front { u32::MAX } else { 0 }, 0, 0],
    ]
}

#[test]
fn reflection_names_bindings_and_packing() {
    let s = shader(BLEND);
    let r = s.interface();
    // (semantic, index, system value, register, lanes, interpolation, used): every element of the signature; the
    // fixture's SV_Position is declared and never read
    let inputs: Vec<_> = r
        .inputs
        .iter()
        .map(|e| {
            let d3d = e.provenance.d3d.unwrap();
            (
                e.label.name.as_str(),
                e.label.index,
                e.system,
                d3d.register,
                d3d.mask,
                e.interp.map(|i| (i.mode, i.sampling)),
                e.used,
            )
        })
        .collect();
    let center = |mode| Some((mode, InterpSampling::Center));
    assert_eq!(
        inputs,
        [
            (
                "SV_Position",
                0,
                Some(SystemValue::FragCoord),
                0,
                0b1111,
                center(InterpMode::Linear),
                false
            ),
            (
                "TEXCOORD",
                0,
                None,
                1,
                0b0011,
                center(InterpMode::Perspective),
                true
            ),
            (
                "TEXCOORD",
                1,
                None,
                1,
                0b0100,
                center(InterpMode::Perspective),
                true
            ),
            ("TEXCOORD", 2, None, 2, 0b0001, center(InterpMode::Flat), true),
            (
                "SV_IsFrontFace",
                0,
                Some(SystemValue::FrontFacing),
                2,
                0b0010,
                center(InterpMode::Flat),
                true
            ),
        ]
    );
    let outputs: Vec<_> = r.outputs.iter().map(|e| (e.system, e.interp, e.value)).collect();
    let colour = Value {
        scalar: ScalarType::Float,
        width: 4,
    };
    assert_eq!(
        outputs,
        [
            (Some(SystemValue::Target(0)), None, colour),
            (Some(SystemValue::Target(1)), None, colour)
        ]
    );
    assert_eq!(s.input_count(), 3);
    assert_eq!(s.constant_buffers(), &[(3, 3)]);
    assert_eq!(s.textures(), &[2]);
    assert_eq!(s.samplers(), &[1]);
    let named: Vec<(&str, u32)> = r
        .resources
        .iter()
        .map(|x| (x.provenance.name.as_str(), x.provenance.d3d.unwrap().register))
        .collect();
    assert_eq!(named, [("layer", 2), ("Constants", 3), ("linear_wrap", 1)]);
    for resource in &r.resources {
        match (resource.provenance.name.as_str(), &resource.kind) {
            ("layer", kind) => assert_eq!(
                kind,
                &ResourceKind::Texture {
                    dim: TextureDim::D2,
                    arrayed: false,
                    multisampled: false,
                    sample: SampleType::Float
                }
            ),
            ("linear_wrap", kind) => assert_eq!(kind, &ResourceKind::Sampler { comparison: false }),
            ("Constants", ResourceKind::ConstantBuffer { layout }) => {
                let vars: Vec<(&str, u32, &ConstantType)> = layout
                    .constants
                    .iter()
                    .map(|c| (c.name.as_str(), c.offset, &c.ty))
                    .collect();
                let vector = |scalar| ConstantType::Vector(Value { scalar, width: 4 });
                assert_eq!(
                    vars,
                    [
                        ("scale", 0, &vector(ScalarType::Float)),
                        ("bias", 16, &vector(ScalarType::Float)),
                        ("flags", 32, &vector(ScalarType::Uint)),
                    ]
                );
            }
            other => panic!("unexpected resource {other:?}"),
        }
    }
}

#[test]
fn evaluation_matches_the_hlsl() {
    let s = shader(BLEND);
    let cb = constants();
    for front in [false, true] {
        let mut requests = Vec::new();
        let mut sampler = |q: Sample| {
            requests.push(q);
            Ok([q.uv[0], q.uv[1], 0.5, 1.0])
        };
        let out = s.execute(&inputs(front), &[(3, &cb)], &mut sampler).unwrap();
        // uv * scale.xy + bias.xy
        assert_eq!(requests.len(), 1);
        assert_eq!((requests[0].texture, requests[0].sampler), (2, 1));
        assert_eq!(requests[0].uv, [1.0, 3.25]);
        // t = (1, 3.25, 0.5, 1); w = saturate(0.4 * 1.5 - 0.1) = 0.5; colour = lerp(t, (1, 0.5, 0.25, 1), w)
        let colour = out.outputs[0].map(f32::from_bits);
        let want = [1.0, 1.875, 0.375, 1.0];
        for (a, b) in colour.iter().zip(want) {
            assert!((a - b).abs() <= 1e-6, "colour {colour:?}");
        }
        // d = dot(t.xyz, (1, 2, 3)) = 9, negated on front faces; sqrt|d|; max(uv); id * 3 + flags.x
        let d = if front { -9.0 } else { 9.0 };
        assert_eq!(out.outputs[1].map(f32::from_bits), [d, 3.0, 0.75, 26.0]);
        assert_eq!(out.output_masks, [15, 15]);
        assert_eq!(out.samples, 1);
    }
}

#[test]
fn missing_bindings_are_errors() {
    let s = shader(BLEND);
    let mut sampler = |_: Sample| Ok([0.; 4]);
    assert!(
        s.execute(&inputs(true)[..2], &[(3, &constants())], &mut sampler)
            .is_err()
    );
    assert!(s.execute(&inputs(true), &[], &mut sampler).is_err());
    assert!(
        s.execute(&inputs(true), &[(3, &constants()[..2])], &mut sampler)
            .is_err()
    );
    let mut failing = |_: Sample| Err("no texture".to_owned());
    assert!(
        s.execute(&inputs(true), &[(3, &constants())], &mut failing)
            .is_err()
    );
}

#[test]
fn every_truncation_is_refused() {
    for n in 0..BLEND.len() {
        assert!(lift(&BLEND[..n]).is_err(), "prefix {n}");
    }
}

/// xorshift64*: deterministic mutations without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Mutate the fixture (bytes flipped, words overwritten with boundary values, spans zeroed or duplicated) and
/// require that parsing and, when it parses, evaluating return without panicking.
#[test]
fn mutations_never_panic() {
    let runs: usize = primase_testdata::debug("dxil.fuzz_runs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut rng = Rng(0x5eed_d0c5_1a7e_c0de);
    let (mut parsed, mut evaluated) = (0, 0);
    for _ in 0..runs {
        let mut bytes = BLEND.to_vec();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(bytes.len());
            match rng.below(4) {
                0 => bytes[at] ^= 1 << rng.below(8),
                1 => {
                    let value: u32 =
                        [0, 1, 0x7fff_ffff, u32::MAX, 0x8000_0000, rng.next() as u32][rng.below(6)];
                    let at = at.min(bytes.len().saturating_sub(4));
                    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
                }
                2 => {
                    let end = (at + rng.below(32)).min(bytes.len());
                    bytes[at..end].fill(0);
                }
                _ => {
                    let len = rng.below(16).min(bytes.len() - at);
                    let span = bytes[at..at + len].to_vec();
                    let to = rng.below(bytes.len() - len + 1);
                    bytes[to..to + len].copy_from_slice(&span);
                }
            }
        }
        if let Some(s) = lift(&bytes).ok().and_then(|p| ProgramShader::new(p).ok()) {
            parsed += 1;
            let rows: Vec<(u32, Vec<[u32; 4]>)> = s
                .constant_buffers()
                .iter()
                .map(|&(slot, n)| (slot, vec![[0x3f00_0000; 4]; n.min(4096) as usize]))
                .collect();
            let refs: Vec<(u32, &[[u32; 4]])> = rows.iter().map(|(s, v)| (*s, v.as_slice())).collect();
            let mut sampler = |q: Sample| Ok([q.uv[0], q.uv[1], 0.5, 1.0]);
            let n = s.input_count().min(64);
            if s.execute(&vec![[0x3e80_0000; 4]; n], &refs, &mut sampler).is_ok() {
                evaluated += 1;
            }
        }
    }
    println!("{runs} mutations: {parsed} parsed, {evaluated} evaluated");
    assert!(
        parsed > 0 && evaluated > 0,
        "{runs} mutations: {parsed} parsed, {evaluated} evaluated; the run must reach the evaluator"
    );
}

const OPS: &[u8] = include_bytes!("fixtures/ops.dxil");

#[test]
fn integer_rounding_and_unary_operations_match_the_hlsl() {
    let s = shader(OPS);
    // a = (2.5, 3, 8, 4); n = (-7, 5, 0, 0)
    let values = vec![[f(2.5), f(3.), f(8.), f(4.)], [(-7i32) as u32, 5, 0, 0]];
    assert_eq!(s.constant_buffers(), &[(0, 2)]);
    let mut sampler = |_: Sample| Err("no textures".to_owned());
    for (t, w) in [([0.75f32, 0.25], 8.0f32), ([0.25, 0.75], 0.5)] {
        let inputs = vec![[0; 4], [f(t[0]), f(t[1]), 0, 0]];
        let out = s.execute(&inputs, &[(0, &values)], &mut sampler).unwrap();
        // frac(2.75), exp2(3), log2(8), and rsqrt(4) unless t.x > t.y gives max(-7, 5) + min(5, 3)
        assert_eq!(out.outputs[0].map(f32::from_bits), [0.75, 8.0, 3.0, w]);
        // round to nearest even, floor, ceil, truncate
        assert_eq!(out.outputs[1].map(f32::from_bits), [2.0, -3.0, 3.0, -2.0]);
        // -7 / 2 = -3, -7 % 3 = -1, 5 << 3, -7 >> 1 = -4 (arithmetic)
        assert_eq!(
            out.outputs[2],
            [(-3i32) as u32, (-1i32) as u32, 40, (-4i32) as u32]
        );
        assert_eq!(out.output_masks, [15, 15, 15]);
    }
}

/// What the fixture uses is what its lift declares: three four-lane outputs, and the program validates.
#[test]
fn the_ops_fixture_lifts_to_the_outputs_it_writes() {
    let program = lift(OPS).unwrap();
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));
    let outputs: Vec<_> = program
        .interface
        .outputs
        .iter()
        .map(|o| (o.system, o.value.width))
        .collect();
    assert_eq!(
        outputs,
        [
            (Some(SystemValue::Target(0)), 4),
            (Some(SystemValue::Target(1)), 4),
            (Some(SystemValue::Target(2)), 4)
        ]
    );
}
