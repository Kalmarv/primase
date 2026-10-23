//! What target 1's w carries, from the consumer's side: the lighting passes decode the G-buffer normal from all four
//! channels of the target (`textureLoad` of the normal target, then in the lifted text
//! `fract(w * 7.984375)`, `fract(w * 63.875)` and `fract(w * 0.9980469)` beside `x * 256 - 128`).
//!
//! - **The decode.** The programs that decode the normal this way are counted (the constants occur together in each),
//!   and the decode is worked through for every byte `b` of w and some bytes of x, y and z: it gives
//!   `256 x - 128 + ix / 8`, `256 y - 128 + iy / 8` and `256 z - 128 + iz / 4` with `ix = b >> 5`, `iy = (b >> 2) & 7`
//!   and `iz = b & 3`, the fractional steps of the three components in 3, 3 and 2 bits, to within an eighth of a step
//!   (the constants are 255.5/256 times the exact ones, which centres each bin): w is the payload of the normal's
//!   sub-step bits, one byte.
//! - **The producers.** The `*_twiddle_tnt` effects write that payload (`floor(128 n + 128) / 256` in xyz, the packed
//!   fractions in w): over 400 normals, the lighting decode of the 8-bit target is closer to the normal on average
//!   with w than with w forced to 0. The plain programs write `saturate(128 z - 127)` there, which is 0 for every
//!   normal with z below 0.992 and otherwise a payload of less than one step that the consumer reads as fraction
//!   bits: near +z it moves the decoded normal by up to half a degree, no more than the 8-bit target's own
//!   quantisation does (0.38 degrees at worst).
//!
//! So the lane is not a role of its own in the vocabulary: it is the low-order bits of the normal's lanes, left Unknown
//! with this evidence. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaStage};
use primase_gta::passes::resolve_shaders::split_program_name;
use primase_gta::read::source::GtaSource;
use primase_shader_exec::ProgramShader;
use primase_shader_exec::{
    Grid, Input, Invocation, Layout, PixelShader, Texture, evaluate::Constants, evaluate_raw,
};
use primase_testdata::{Game, install, keys};

/// A program lifted and ready to run, or why not.
fn executor(bytecode: &[u8]) -> Result<ProgramShader, String> {
    let program = primase_dxil::lift(bytecode).map_err(|e| e.to_string())?;
    ProgramShader::new(program).map_err(|e| e.to_string())
}

fn world() -> Option<World> {
    let (dir, keys) = (install(Game::Gta)?, keys(Game::Gta)?);
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    Some(world)
}

/// The lighting passes' decode of the normal target, as the lifted text of `PS_directional_just_dir` has it (before
/// the normalisation).
fn decode_raw(c: [f32; 4]) -> [f32; 3] {
    let fract = |v: f32| v - v.floor();
    let e107 = fract(c[3] * 7.984375);
    let e108 = fract(c[3] * 63.875);
    [
        fract(c[3] * 0.9980469) - 128.0 + c[0] * 256.0 - e107 * 0.125,
        e107 - 128.0 + c[1] * 256.0 - e108 * 0.125,
        c[2] * 256.0 - 128.0 + e108,
    ]
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn angle(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (a, b) = (unit(a), unit(b));
    (a[0] * b[0] + a[1] * b[1] + a[2] * b[2])
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// A target value as the 8-bit target stores it.
fn quantise(c: [f32; 4]) -> [f32; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0)
}

fn target_one(shader: &ProgramShader, normal: [f32; 3]) -> [f32; 4] {
    let buffers: Vec<(u32, u32)> = PixelShader::constant_buffers(shader)
        .iter()
        .map(|&(slot, rows)| (slot, rows.saturating_mul(4)))
        .collect();
    let constants = Constants::new(&Layout {
        buffers,
        fields: BTreeMap::new(),
    });
    let textures: BTreeMap<u32, Texture> = PixelShader::textures(shader)
        .iter()
        .map(|&slot| (slot, Texture::Constant([0.5; 4])))
        .collect();
    let mut inputs = BTreeMap::new();
    for v in &PixelShader::interface(shader).inputs {
        let value = match (v.label.name.to_ascii_uppercase().as_str(), v.label.index) {
            ("SV_POSITION", _) => [0.0, 0.0, 0.0, 1.0],
            ("COLOR", 0) => [0.5; 4],
            ("TEXCOORD", 0) => [0.3, 0.7, 0.0, 0.0],
            ("TEXCOORD", 1) => [normal[0], normal[1], normal[2], 0.0],
            ("TEXCOORD", 4) => [1.0, 0.0, 0.0, 0.0],
            ("TEXCOORD", 5) => [0.0, 1.0, 0.0, 0.0],
            _ => [0.25; 4],
        };
        inputs.insert((v.label.name.clone(), v.label.index), Input::Constant(value));
    }
    let targets: Vec<(u32, u8)> = PixelShader::interface(shader)
        .outputs
        .iter()
        .enumerate()
        .map(|(i, v)| (i as u32, ((1u16 << v.value.width) - 1) as u8))
        .collect();
    let raw = evaluate_raw(
        shader,
        &constants,
        &textures,
        &inputs,
        Grid {
            size: 1,
            period: [1.0; 2],
        },
        &targets,
        Invocation::Scalar,
    )
    .unwrap();
    raw.pixels[1][0].0.map(f32::from_bits)
}

fn effect_program(world: &World, effect: &str, hash: &str) -> primase_core::Entity {
    world
        .iter::<GtaEffect>()
        .find(|(_, e)| e.name == effect)
        .and_then(|(_, e)| {
            e.programs
                .iter()
                .find(|p| {
                    p.stage == GtaStage::Pixel && p.name == format!("PS_DeferredTextured_{hash}_Wrapped")
                })
                .and_then(|p| p.shader)
        })
        .unwrap()
}

#[test]
fn the_lighting_passes_read_w_as_the_normals_sub_step_bits() {
    let Some(world) = world() else { return };
    // the programs with the decode's three constants, of the families that are not the G-buffer pass
    let mut seen = BTreeSet::new();
    let (mut decoding, mut partial) = (0usize, 0usize);
    for (_, e) in world.iter::<GtaEffect>() {
        for p in e.programs.iter().filter(|p| p.stage == GtaStage::Pixel) {
            let Some((stem, _)) = split_program_name(&p.name) else {
                continue;
            };
            let Some(entity) = p.shader.filter(|_| !stem.starts_with("PS_Deferred")) else {
                continue;
            };
            if !seen.insert(entity) {
                continue;
            }
            let sh = world.get::<Shader>(entity).unwrap();
            let Ok(text) = primase_dxil::lift(&sh.bytecode)
                .map_err(|e| e.to_string())
                .and_then(|p| p.text().map_err(|e| e.to_string()))
            else {
                continue;
            };
            let found = ["7.984375f", "63.875f", "0.9980469f"]
                .iter()
                .filter(|c| text.contains(**c))
                .count();
            match found {
                3 => decoding += 1,
                0 => {}
                _ => partial += 1,
            }
        }
    }
    println!(
        "{decoding} programs decode the normal target's w with the three constants; {partial} use only some"
    );
    assert!(decoding > 300, "{decoding}");
    assert_eq!(partial, 0);
    // the decode, worked through: the payload byte b of w is ix (3 bits), iy (3 bits), iz (2 bits)
    for b in 0..=255u32 {
        let w = b as f32 / 255.0;
        let (ix, iy, iz) = ((b >> 5) as f32, ((b >> 2) & 7) as f32, (b & 3) as f32);
        for (x, y, z) in [(100.0f32, 130.0, 255.0), (0.0, 255.0, 128.0), (200.0, 17.0, 90.0)] {
            let d = decode_raw([x / 255.0, y / 255.0, z / 255.0, w]);
            let want = [
                x * 256.0 / 255.0 - 128.0 + ix / 8.0,
                y * 256.0 / 255.0 - 128.0 + iy / 8.0,
                z * 256.0 / 255.0 - 128.0 + iz / 4.0,
            ];
            for k in 0..3 {
                assert!(
                    (d[k] - want[k]).abs() < 0.15,
                    "b {b} lane {k}: {} for {}",
                    d[k],
                    want[k]
                );
            }
        }
    }
}

#[test]
fn the_twiddle_programs_fill_w_and_the_plain_programs_do_not_need_it() {
    let Some(world) = world() else { return };
    let load = |effect: &str, hash: &str| {
        let sh = world.get::<Shader>(effect_program(&world, effect, hash)).unwrap();
        executor(&sh.bytecode).unwrap()
    };
    let twiddle = load("normal_spec_twiddle_tnt", "95313230");
    let plain = load("cpv_only", "6b2704e5");
    // normals over the hemisphere, some of them close to +z
    let normals: Vec<[f32; 3]> = (0..400)
        .map(|k| {
            let t = k as f32 * 0.0157;
            let up = if k % 4 == 0 {
                0.9995
            } else {
                0.6 + 0.4 * (t * 0.37).sin().abs()
            };
            unit([
                t.sin() * 0.8 * (1.0 - up * 0.9),
                t.cos() * 0.5 * (1.0 - up * 0.9),
                up,
            ])
        })
        .collect();
    let (mut with_w, mut without_w) = (0f32, 0f32);
    let mut plain_worst = 0f32;
    for &n in &normals {
        let q = quantise(target_one(&twiddle, n));
        let mut q0 = q;
        q0[3] = 0.0;
        with_w += angle(decode_raw(q), n);
        without_w += angle(decode_raw(q0), n);
        let q = quantise(target_one(&plain, n));
        let mut q0 = q;
        q0[3] = 0.0;
        plain_worst = plain_worst.max(angle(decode_raw(q), decode_raw(q0)));
    }
    let count = normals.len() as f32;
    println!(
        "twiddle: mean angle from the normal {:.4} deg with w, {:.4} deg with w forced to 0; plain: worst change from w {plain_worst:.4} deg",
        with_w / count,
        without_w / count
    );
    assert!(with_w < without_w * 0.8, "{with_w} {without_w}");
    assert!(plain_worst < 0.6, "{plain_worst}");
}
