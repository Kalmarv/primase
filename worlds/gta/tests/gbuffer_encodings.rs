//! The lane encodings `resolve-gta-shaders` grants beyond the main specular program (`gbuffer_roles.rs`), each with
//! evidence from a side other than the reading of the program's stores that grants it:
//!
//! - **Target 2's x and y are squares to the lighting passes.** In the programs of the directional, point, spot,
//!   capsule and light-tile families that read the G-buffer, the load's y is squared and multiplied by 512 (the
//!   exponent) and its x squared (the intensity), both of one `textureLoad`. This is the consumer's side of the lane:
//!   a G-buffer program that stores a square root there (or the constant 0) is read as the roles say, whatever it
//!   computes inside the square root.
//! - **A fixed falloff.** `cpv_only`'s `PS_DeferredTextured_6b2704e5` stores `sqrt(c * 250/512)` in y beside
//!   `sqrt(c * 0.5)` in x: decoded, the exponent is 250 times the coverage `c` and the intensity half of it, so
//!   `decode(y) = 500 * decode(x)` wherever the coverage varies.
//! - **Floored normals.** The `*_twiddle_tnt` effects store `floor(128 n + 128) / 256` in target 1's xyz and the
//!   fractional bits in w: stored * 256 - 128 is within 1 of 128 n.
//!
//! The counts are printed. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::shader::{ScalarEncoding, Shader};
use primase_gta::components::{GtaEffect, GtaStage};
use primase_gta::passes::gbuffer;
use primase_gta::passes::resolve_shaders::split_program_name;
use primase_gta::read::source::GtaSource;
use primase_shader::exec::ProgramShader;
use primase_shader::{Constants, Grid, Input, Invocation, Layout, PixelShader, Texture, evaluate_raw};
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

/// One evaluation of a G-buffer program with every texel `texel`, every constant word `word`, vertex colour `colour`
/// and the surface normal `normal`: the four words of each target.
fn evaluate(shader: &ProgramShader, texel: f32, word: f32, colour: f32, normal: [f32; 3]) -> Vec<[f32; 4]> {
    let buffers: Vec<(u32, u32)> = PixelShader::constant_buffers(shader)
        .iter()
        .map(|&(slot, rows)| (slot, rows.saturating_mul(4)))
        .collect();
    let mut constants = Constants::new(&Layout {
        buffers: buffers.clone(),
        fields: BTreeMap::new(),
    });
    for &(slot, words) in &buffers {
        constants
            .set_words(slot, 0, &vec![word.to_bits(); words as usize])
            .unwrap();
    }
    let textures: BTreeMap<u32, Texture> = PixelShader::textures(shader)
        .iter()
        .map(|&slot| (slot, Texture::Constant([texel; 4])))
        .collect();
    let mut inputs = BTreeMap::new();
    for v in &PixelShader::interface(shader).inputs {
        let value = match (v.label.name.to_ascii_uppercase().as_str(), v.label.index) {
            ("SV_POSITION", _) => [0.0, 0.0, 0.0, 1.0],
            ("COLOR", 0) => [colour; 4],
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
    raw.pixels.iter().map(|p| p[0].0.map(f32::from_bits)).collect()
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

/// The name before `.lane * ` in a line, as `_e123`.
fn base_of(line: &str, lane: char) -> Option<String> {
    let at = line.find(&format!(".{lane} * "))?;
    let name = line[..at].rsplit(['(', ' ']).next()?;
    line.contains(&format!("{name}.{lane} * {name}.{lane}"))
        .then(|| name.to_owned())
}

#[test]
fn the_lighting_passes_read_target_two_as_squares() {
    let Some(world) = world() else { return };
    let (mut reading, mut decoding) = (0usize, 0usize);
    let mut seen = BTreeSet::new();
    let mut missed = Vec::new();
    for (_, e) in world.iter::<GtaEffect>() {
        for p in e.programs.iter().filter(|p| p.stage == GtaStage::Pixel) {
            let Some((stem, _)) = split_program_name(&p.name) else {
                continue;
            };
            let lighting = [
                "PS_directional_",
                "PS_pointCM_",
                "PS_spotCM_",
                "PS_capsule_",
                "PS_lightTile_",
            ]
            .iter()
            .any(|f| stem.starts_with(f));
            let Some(entity) = p.shader.filter(|_| lighting) else {
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
            let loads: BTreeSet<String> = text
                .lines()
                .filter(|l| l.contains("textureLoad("))
                .filter_map(|l| {
                    l.trim()
                        .strip_prefix("let ")?
                        .split(' ')
                        .next()
                        .map(str::to_owned)
                })
                .collect();
            // `* 512f` of a square of a load's y
            let exponents: BTreeSet<String> = text
                .lines()
                .filter(|l| l.contains("* 512f"))
                .filter_map(|l| base_of(l.trim(), 'y'))
                .filter(|m| loads.contains(m))
                .collect();
            if exponents.is_empty() {
                continue;
            }
            reading += 1;
            let squares_x: BTreeSet<String> = text.lines().filter_map(|l| base_of(l.trim(), 'x')).collect();
            if exponents.iter().any(|m| squares_x.contains(m)) {
                decoding += 1;
            } else if missed.len() < 5 {
                missed.push(format!("{} {exponents:?}", p.name));
            }
        }
    }
    println!(
        "{reading} lighting programs read an exponent as y * y * 512 of a load; {decoding} also square x of that load"
    );
    assert!(reading > 100, "{reading}");
    assert_eq!(reading, decoding, "{missed:?}");
}

#[test]
fn a_fixed_falloff_program_decodes_to_two_hundred_and_fifty_times_the_coverage() {
    let Some(world) = world() else { return };
    let entity = effect_program(&world, "cpv_only", "6b2704e5");
    let sh = world.get::<Shader>(entity).unwrap();
    let writes = gbuffer::writes(&sh.bytecode).unwrap();
    assert!(writes.specular_intensity_encoded && writes.specular_exponent_encoded);
    let shader = executor(&sh.bytecode).unwrap();
    let intensity = ScalarEncoding::square_root_of(1.0);
    let exponent = ScalarEncoding::square_root_of(512.0);
    let mut nonzero = 0;
    for (colour, word) in [(1.0f32, 0.5f32), (0.8, 0.3), (0.5, 0.9), (1.0, 0.0)] {
        let out = evaluate(&shader, 0.5, word, colour, [0.0, 0.0, 1.0]);
        let (x, y) = (intensity.decode(out[2][0]), exponent.decode(out[2][1]));
        println!("colour {colour}, word {word}: intensity {x}, exponent {y}");
        assert!((y - 500.0 * x).abs() <= 1e-3 * y.max(1.0), "{x} {y}");
        nonzero += usize::from(x > 0.0);
    }
    assert!(nonzero >= 2, "the coverage never varied");
}

#[test]
fn the_floored_normals_are_the_normal_to_a_step() {
    let Some(world) = world() else { return };
    let entity = effect_program(&world, "normal_spec_twiddle_tnt", "95313230");
    let sh = world.get::<Shader>(entity).unwrap();
    let writes = gbuffer::writes(&sh.bytecode).unwrap();
    assert!(writes.normal_floored && !writes.normal_biased);
    let shader = executor(&sh.bytecode).unwrap();
    for n in [[0.0f32, 0.0, 1.0], [0.48, 0.6, 0.64], [-0.3, 0.2, 0.9327]] {
        let out = evaluate(&shader, 0.5, 0.0, 0.5, n);
        for k in 0..3 {
            let steps = out[1][k] * 256.0 - 128.0;
            assert!((steps - 128.0 * n[k]).abs() < 1.0, "{n:?} lane {k}: {steps}");
        }
        assert!((0.0..=1.0).contains(&out[1][3]));
    }
}
