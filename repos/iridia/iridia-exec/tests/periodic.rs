#![allow(clippy::unwrap_used)]
//! `evaluate_periodic` with a synthetic program whose output is a function of the grid's U and V: exact for a program
//! already periodic, the program's value outside the cross-faded first cell, seamless across the wrap, and its size
//! limits.
use iridia::{D3dSignature, Interface, Label, ScalarType, Stage, Value, Varying, VaryingProvenance};
use iridia_exec::{
    Execution, Input, Layout, PixelShader, Sampler, Views, evaluate::Constants, evaluate_periodic,
};
use std::collections::BTreeMap;

/// o0 = (f(U, V), U, V, 1), U and V read from TEXCOORD0.xy.
#[derive(Debug)]
struct Uv(Interface, fn(f32, f32) -> f32);
impl PixelShader for Uv {
    fn interface(&self) -> &Interface {
        &self.0
    }
    fn input_count(&self) -> usize {
        1
    }
    fn constant_buffers(&self) -> &[(u32, u32)] {
        &[]
    }
    fn textures(&self) -> &[u32] {
        &[]
    }
    fn samplers(&self) -> &[u32] {
        &[]
    }
    fn execute(
        &self,
        inputs: &[[u32; 4]],
        _: &[(u32, &[[u32; 4]])],
        _: &mut dyn Sampler,
    ) -> Result<Execution, iridia_exec::Error> {
        let (u, v) = (f32::from_bits(inputs[0][0]), f32::from_bits(inputs[0][1]));
        Ok(Execution {
            discarded: false,
            outputs: vec![[(self.1)(u, v), u, v, 1.].map(f32::to_bits)],
            output_masks: vec![15],
            instructions: 1,
            samples: 0,
        })
    }
}

fn shader(f: fn(f32, f32) -> f32) -> Uv {
    Uv(
        Interface {
            stage: Stage::Pixel,
            inputs: vec![Varying {
                label: Label {
                    name: "TEXCOORD".into(),
                    index: 0,
                },
                system: None,
                value: Value {
                    scalar: ScalarType::Float,
                    width: 2,
                },
                interp: None,
                used: true,
                provenance: VaryingProvenance {
                    d3d: Some(D3dSignature {
                        register: 0,
                        mask: 3,
                        read_write_mask: 3,
                    }),
                },
            }],
            outputs: Vec::new(),
            resources: Vec::new(),
            facts: Default::default(),
        },
        f,
    )
}

/// The bake of `f` over `cells` cells of `texels`: its first target's first component, row-major.
fn bake(f: fn(f32, f32) -> f32, cells: u32, texels: u32) -> Vec<f32> {
    let s = shader(f);
    let inputs = BTreeMap::from([(("TEXCOORD".to_owned(), 0), Input::GridUv)]);
    let e = evaluate_periodic(
        &s,
        &Constants::new(&Layout::default()),
        &Views::refusing(BTreeMap::new()),
        &inputs,
        cells,
        texels,
        &[(0, 1)],
    )
    .unwrap();
    let side = (cells * texels) as usize;
    let t = e.targets[0].as_ref().unwrap();
    assert_eq!(t.len(), side * side);
    assert_eq!(e.instructions, u64::from((cells + 1) * texels).pow(2));
    t.iter().map(|v| v[0]).collect()
}

/// The program's value at pixel (x, y) of a grid whose U and V run one per cell of `texels`.
fn at(f: fn(f32, f32) -> f32, texels: u32, x: usize, y: usize) -> f32 {
    f((x as f32 + 0.5) / texels as f32, (y as f32 + 0.5) / texels as f32)
}

/// A program already periodic with period K bakes exactly, the first cells included.
#[test]
fn a_periodic_program_bakes_exactly() {
    fn f(u: f32, v: f32) -> f32 {
        (std::f32::consts::TAU * u / 3.).sin() + (std::f32::consts::TAU * v / 3.).cos()
    }
    let (cells, texels) = (3, 4);
    let side = (cells * texels) as usize;
    let b = bake(f, cells, texels);
    for y in 0..side {
        for x in 0..side {
            let want = at(f, texels, x, y);
            assert!(
                (b[y * side + x] - want).abs() < 1e-4,
                "({x}, {y}): {} vs {want}",
                b[y * side + x]
            );
        }
    }
}

/// A ramp in U and V (not periodic): outside the first cell of each axis the bake is the program's value; within it,
/// the cross-fade; and no pair of neighbours, across the wrap included, differs by more than the program's jump at the
/// wrap (K cells) spread over the fade cell's texels plus one step, where the program's own values jump by K.
#[test]
fn a_ramp_is_its_value_outside_the_fade_and_seamless_across_the_wrap() {
    fn f(u: f32, v: f32) -> f32 {
        u + v
    }
    let (cells, texels) = (4u32, 8u32);
    let (side, t) = ((cells * texels) as usize, texels as usize);
    let b = bake(f, cells, texels);
    let value = |x: usize, y: usize| b[y * side + x];
    for y in t..side {
        for x in t..side {
            assert!((value(x, y) - at(f, texels, x, y)).abs() < 1e-4, "({x}, {y})");
        }
    }
    // in the fade, along u only: s * E(x) + (1 - s) * E(x + K * texels)
    let (x, y) = (2, t + 3);
    let s = (x as f32 + 0.5) / texels as f32;
    let want = s * at(f, texels, x, y) + (1. - s) * at(f, texels, x + side, y);
    assert!((value(x, y) - want).abs() < 1e-4);
    // the program's own values jump by K at the wrap; the bake's neighbours, wrapped, by at most (K + 1) steps
    let step = 1. / texels as f32;
    assert!((at(f, texels, side - 1, t) - at(f, texels, 0, t)).abs() > cells as f32 - 1.);
    for y in 0..side {
        for x in 0..side {
            let right = value((x + 1) % side, y);
            let down = value(x, (y + 1) % side);
            assert!(
                (value(x, y) - right).abs() <= (cells + 1) as f32 * step,
                "({x}, {y}) to the right"
            );
            assert!(
                (value(x, y) - down).abs() <= (cells + 1) as f32 * step,
                "({x}, {y}) below"
            );
        }
    }
}

/// Fewer than two cells, cells of no texels, and a grid over 4096 ((K + 1) * texels) are refused; a target the
/// program does not write is `None`.
#[test]
fn sizes_are_checked_and_unwritten_targets_are_none() {
    let s = shader(|u, _| u);
    let inputs = BTreeMap::from([(("TEXCOORD".to_owned(), 0), Input::GridUv)]);
    let constants = Constants::new(&Layout::default());
    let textures = Views::refusing(BTreeMap::new());
    let run = |cells, texels| {
        evaluate_periodic(
            &s,
            &constants,
            &textures,
            &inputs,
            cells,
            texels,
            &[(0, 1), (1, 1)],
        )
    };
    // (2 + 1) * 1366 = 4098, (4095 + 1) * 2 = 8192
    for (cells, texels) in [(1, 8), (0, 8), (2, 0), (2, 1366), (4095, 2), (u32::MAX, 2)] {
        assert!(run(cells, texels).is_err(), "{cells} x {texels}");
    }
    let e = run(2, 3).unwrap();
    assert_eq!(e.targets[0].as_ref().unwrap().len(), 6 * 6);
    assert!(e.targets[1].is_none());
}

/// On several threads ([`evaluate_periodic_with`]) the bake is the one-thread bake, bit for bit, with the same counts.
#[test]
fn on_any_thread_count_the_bake_is_the_serial_one() {
    fn f(u: f32, v: f32) -> f32 {
        (u * 1.7).sin() * (v * 0.3).cos() + u * v
    }
    let s = shader(f);
    let inputs = BTreeMap::from([(("TEXCOORD".to_owned(), 0), Input::GridUv)]);
    let run = |n: usize| {
        let e = iridia_exec::evaluate_periodic_with(
            primase_foundations::parallel::Threads::new(n).unwrap(),
            &s,
            &Constants::new(&Layout::default()),
            &Views::refusing(BTreeMap::new()),
            &inputs,
            3,
            5,
            &[(0, 1)],
        )
        .unwrap();
        let bits: Vec<u32> = e.targets[0]
            .as_ref()
            .unwrap()
            .iter()
            .map(|v| v[0].to_bits())
            .collect();
        (bits, e.instructions, e.samples)
    };
    let one = run(1);
    for n in [2, 7] {
        assert_eq!(run(n), one, "{n} threads");
    }
}
