//! The G-buffer the pixel programs of the shader database write (`PS_Deferred*`), read from the programs themselves:
//! every distinct program of every effect, the widths of its output targets, and what each lane of each target
//! responds to, found by evaluating the program ([`iridia_exec::evaluate_raw`], one pixel) at two operating points
//! and changing one class of input at a time: the textures, the constant buffers (every word), each lane of `COLOR0`
//! (the vertex colour) and the surface normal (`TEXCOORD1`, with a tangent and bitangent orthogonal to it).
//!
//! - A lane responds to a class when a change of that class changes it at either operating point.
//! - Target 1's xyz is held to the world normal biased, `n * 0.5 + 0.5`, with every texture flat (a texel of 0.5) and
//!   the constants zero. A program that discards that pixel stores no normal (the executor reports the lanes written:
//!   none), and is tried at a second operating point (an opaque vertex colour); a program that discards both is
//!   counted by family and must be one of the alpha-test and stipple programs.
//! - Target 3's z is the lane the emissive effects write: with every texel, constant word and vertex colour at 1 it is
//!   non-zero, and the effects of the programs where it is are listed.
//!
//! The counts are printed. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaStage};
use primase_gta::passes::gbuffer;
use primase_gta::passes::resolve_shaders::split_program_name;
use primase_gta::read::source::GtaSource;
use iridia_exec::ProgramShader;
use iridia_exec::{
    Grid, Input, Invocation, Layout, PixelShader, View, Views, evaluate::Constants, evaluate_raw,
};
use primase_testdata::{Game, install, keys};

/// Whether the lifted program has a `discard` anywhere in its body.
fn has_discard(bytecode: &[u8]) -> bool {
    fn block(b: &naga::Block) -> bool {
        use naga::Statement as S;
        b.iter().any(|s| match s {
            S::Kill => true,
            S::Block(b) => block(b),
            S::If { accept, reject, .. } => block(accept) || block(reject),
            S::Loop { body, continuing, .. } => block(body) || block(continuing),
            S::Switch { cases, .. } => cases.iter().any(|c| block(&c.body)),
            _ => false,
        })
    }
    primase_dxil::lift(bytecode).is_ok_and(|p| p.module.entry_points.iter().any(|e| block(&e.function.body)))
}

/// A program lifted and ready to run, or why not.
fn executor(bytecode: &[u8]) -> Result<ProgramShader, String> {
    let program = primase_dxil::lift(bytecode).map_err(|e| e.to_string())?;
    ProgramShader::new(program).map_err(|e| e.to_string())
}

/// What a pixel is evaluated with.
#[derive(Clone, Copy)]
struct Setup {
    /// Every texture's value in every channel.
    texel: f32,
    /// Every word of every constant buffer.
    word: f32,
    /// `COLOR0`.
    colour: [f32; 4],
    /// `TEXCOORD1`, a unit vector.
    normal: [f32; 3],
}

/// The input classes a lane is tried against.
const CLASSES: [&str; 7] = [
    "texture",
    "constants",
    "colour.x",
    "colour.y",
    "colour.z",
    "colour.w",
    "normal",
];

impl Setup {
    /// The setup with one class of input changed.
    fn changed(self, class: usize) -> Self {
        let mut s = self;
        match class {
            0 => s.texel = 0.9 - self.texel * 0.5,
            1 => s.word = 0.9 - self.word * 0.5,
            2..=5 => s.colour[class - 2] = 1.0 - self.colour[class - 2] * 0.7,
            _ => s.normal = unit([self.normal[2], -self.normal[0], self.normal[1] + 0.3]),
        }
        s
    }
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

/// One pixel of `shader`: the four words of each target, or why not.
fn run(shader: &ProgramShader, s: Setup) -> Result<Vec<[f32; 4]>, String> {
    run_written(shader, s).map(|(out, _)| out)
}

/// One pixel of `shader`: the four words of each target, and whether the program wrote any lane of any target. A pixel
/// the program discards before it stores anything (an alpha test, a stipple) writes none, and its words read 0.
fn run_written(shader: &ProgramShader, s: Setup) -> Result<(Vec<[f32; 4]>, bool), String> {
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
            .set_words(slot, 0, &vec![s.word.to_bits(); words as usize])
            .map_err(|e| e.to_string())?;
    }
    let textures: BTreeMap<u32, View> = PixelShader::textures(shader)
        .iter()
        .map(|&slot| (slot, View::Constant([s.texel; 4])))
        .collect();
    let textures = Views::refusing(textures);
    // a tangent orthogonal to the normal, and the bitangent
    let n = s.normal;
    let t = unit([-n[1], n[0], 0.0]);
    let b = [
        n[1] * t[2] - n[2] * t[1],
        n[2] * t[0] - n[0] * t[2],
        n[0] * t[1] - n[1] * t[0],
    ];
    let mut inputs = BTreeMap::new();
    for v in &PixelShader::interface(shader).inputs {
        let value = match (v.label.name.to_ascii_uppercase().as_str(), v.label.index) {
            ("SV_POSITION", _) => [0.0, 0.0, 0.0, 1.0],
            ("COLOR", 0) => s.colour,
            ("TEXCOORD", 0) => [0.3, 0.7, 0.0, 0.0],
            ("TEXCOORD", 1) => [n[0], n[1], n[2], 0.0],
            ("TEXCOORD", 4) => [t[0], t[1], t[2], 0.0],
            ("TEXCOORD", 5) => [b[0], b[1], b[2], 0.0],
            _ => [0.25; 4],
        };
        inputs.insert((v.label.name.clone(), v.label.index), Input::Constant(value));
    }
    let outputs = &PixelShader::interface(shader).outputs;
    let targets: Vec<(u32, u8)> = (0..outputs.len() as u32)
        .map(|i| {
            let width = outputs[i as usize].value.width;
            (i, ((1u16 << width) - 1) as u8)
        })
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
    .map_err(|e| e.to_string())?;
    let written = raw.pixels.iter().any(|p| p[0].1 != 0);
    Ok((
        raw.pixels
            .iter()
            .map(|p| {
                let (bits, _) = p[0];
                bits.map(f32::from_bits)
            })
            .collect(),
        written,
    ))
}

/// The group of the G-buffer pass's programs that write five targets.
const FIVE: &str = "PS_DeferredTextured* [4, 4, 4, 4, 2]";

/// What the lifted text of the five-target programs says.
#[derive(Default)]
struct StaticText {
    lifted: usize,
    /// Programs whose target 1 xyz are each a biased value.
    biased: usize,
    /// Programs whose target 3 z is not the constant 0.
    emission_written: usize,
    emission_effects: BTreeSet<String>,
    /// Programs the pass reads each of the new roles' lanes of: intensity and exponent stored as square roots, a
    /// Fresnel term, a motion vector.
    intensity_sqrt: usize,
    exponent_sqrt: usize,
    fresnel: usize,
    biased_read: usize,
    floored_read: usize,
    zero_read: usize,
    motion: usize,
    unread: BTreeMap<String, usize>,
    refused: BTreeMap<String, usize>,
    odd: Vec<String>,
}

/// How often a lane responds to each class, over the programs of a group.
#[derive(Default)]
struct Lane {
    programs: usize,
    responds: [usize; CLASSES.len()],
    /// Programs where the lane held the same value in every evaluation, by that value.
    fixed: BTreeMap<String, usize>,
}

#[test]
fn the_deferred_programs_write_one_g_buffer() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    // distinct programs by entity, with the effects that list them
    let mut programs: BTreeMap<primase_core::Entity, (String, BTreeSet<String>)> = BTreeMap::new();
    for (_, e) in world.iter::<GtaEffect>() {
        for p in e.programs.iter().filter(|p| p.stage == GtaStage::Pixel) {
            let (Some(s), Some((stem, _))) = (p.shader, split_program_name(&p.name)) else {
                continue;
            };
            if stem.starts_with("PS_Deferred") {
                programs
                    .entry(s)
                    .or_insert_with(|| (stem.to_owned(), BTreeSet::new()))
                    .1
                    .insert(e.name.clone());
            }
        }
    }
    let bases = [
        Setup {
            texel: 0.5,
            word: 0.5,
            colour: [0.5; 4],
            normal: unit([0.48, 0.6, 0.64]),
        },
        Setup {
            texel: 0.3,
            word: 0.2,
            colour: [0.8, 0.3, 0.6, 0.4],
            normal: unit([-0.3, 0.2, 0.9]),
        },
    ];
    let flat = Setup {
        texel: 0.5,
        word: 0.0,
        colour: [0.5; 4],
        normal: unit([0.48, 0.6, 0.64]),
    };
    let stress = Setup {
        texel: 1.0,
        word: 1.0,
        colour: [1.0; 4],
        normal: flat.normal,
    };
    // the group of a family: the pixel programs of the G-buffer pass by their targets, the others by family
    let group_of = |stem: &str, widths: &[u32]| -> String {
        let family = if stem.starts_with("PS_DeferredTextured") {
            "PS_DeferredTextured*"
        } else {
            stem.split("Textured").next().unwrap_or(stem)
        };
        format!("{family} {widths:?}")
    };
    let mut signatures: BTreeMap<(String, Vec<u32>), usize> = BTreeMap::new();
    let mut matrix: BTreeMap<(String, usize, usize), Lane> = BTreeMap::new();
    let mut normal: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut normal_misses: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut emission: BTreeMap<String, usize> = BTreeMap::new();
    let mut unevaluated: BTreeMap<String, usize> = BTreeMap::new();
    let mut static_text = StaticText::default();
    let mut biased_not_normal: Vec<String> = Vec::new();
    // programs whose pixel is discarded at every operating point tried, by program family
    let mut discarded_flat: BTreeMap<String, usize> = BTreeMap::new();
    for (entity, (stem, effects)) in &programs {
        let sh = world.get::<Shader>(*entity).unwrap();
        let widths: Vec<u32> = sh
            .interface
            .outputs
            .iter()
            .map(|v| u32::from(v.value.width))
            .collect();
        *signatures.entry((stem.clone(), widths.clone())).or_default() += 1;
        let group = group_of(stem, &widths);
        // the lifted program's own text, for the G-buffer pass's five targets: target 1's xyz are each `... + 0.5`
        // (a biased value), and target 3's z is the constant 0 or a computed value
        if group == FIVE {
            let text = primase_dxil::lift(&sh.bytecode)
                .map_err(|e| e.to_string())
                .and_then(|p| p.text().map_err(|e| e.to_string()));
            match text {
                Ok(text) => {
                    let line = |lane: &str| {
                        text.lines()
                            .find(|l| l.trim_start().starts_with(&format!("{lane} = ")))
                            .map(str::trim)
                    };
                    static_text.lifted += 1;
                    let biased = ["o1_.x", "o1_.y", "o1_.z"]
                        .iter()
                        .all(|l| line(l).is_some_and(|l| l.ends_with("+ 0.5f));")));
                    let emission = line("o3_.z").is_some_and(|l| !l.ends_with("bitcast<u32>(0f);"));
                    if biased {
                        static_text.biased += 1;
                    } else if static_text.odd.len() < 6 {
                        static_text.odd.push(format!(
                            "{stem} {:?}: {:?} {:?} {:?}",
                            effects,
                            line("o1_.x"),
                            line("o1_.y"),
                            line("o1_.z")
                        ));
                    }
                    if emission {
                        static_text.emission_written += 1;
                        static_text.emission_effects.extend(effects.iter().cloned());
                    }
                    // the pass's own reading of the stores says the same as the text
                    match gbuffer::writes(&sh.bytecode) {
                        Ok(reads) => {
                            static_text.intensity_sqrt += usize::from(reads.specular_intensity_encoded);
                            static_text.exponent_sqrt += usize::from(reads.specular_exponent_encoded);
                            static_text.fresnel += usize::from(reads.fresnel_written);
                            static_text.motion += usize::from(reads.motion_written);
                            // the text shows a lane stored from a plain expression; the reading also follows a
                            // variable assigned once, so it finds at least what the text shows
                            assert!(
                                (!biased || reads.normal_biased) && (!reads.emission_written || emission),
                                "{stem} {effects:?}: text {biased} {emission}, reading {} {}",
                                reads.normal_biased,
                                reads.emission_written
                            );
                            static_text.biased_read += usize::from(reads.normal_biased);
                            static_text.floored_read += usize::from(reads.normal_floored);
                            static_text.zero_read += usize::from(reads.normal_zero);
                        }
                        Err(why) => *static_text.unread.entry(why).or_default() += 1,
                    }
                }
                Err(why) => *static_text.refused.entry(why).or_default() += 1,
            }
        }
        // the lift and the executor do not cover every program
        let shader = match executor(&sh.bytecode) {
            Ok(s) => s,
            Err(why) => {
                *unevaluated.entry(why).or_default() += 1;
                continue;
            }
        };
        // each lane against each class
        let mut results = Vec::new();
        let mut failed = None;
        for base in bases {
            let mut row = vec![run(&shader, base)];
            row.extend((0..CLASSES.len()).map(|c| run(&shader, base.changed(c))));
            if let Some(Err(why)) = row.iter().find(|r| r.is_err()) {
                failed = Some(why.clone());
                break;
            }
            results.push(row.into_iter().map(Result::unwrap).collect::<Vec<_>>());
        }
        if let Some(why) = failed {
            *unevaluated.entry(format!("evaluation: {why}")).or_default() += 1;
            continue;
        }
        for (t, w) in widths.iter().enumerate() {
            for l in 0..*w as usize {
                let lane = matrix.entry((group.clone(), t, l)).or_default();
                lane.programs += 1;
                let mut moved = false;
                for (c, responds) in lane.responds.iter_mut().enumerate() {
                    if results.iter().any(|r| (r[0][t][l] - r[c + 1][t][l]).abs() > 1e-4) {
                        *responds += 1;
                        moved = true;
                    }
                }
                let first = results[0][0][t][l];
                let same = results.iter().flatten().all(|r| (r[t][l] - first).abs() < 1e-6);
                if !moved && same {
                    *lane.fixed.entry(format!("{first:.4}")).or_default() += 1;
                }
            }
        }
        // target 1's normal
        if widths.len() > 1 && widths[1] == 4 {
            // the flat pixel, or the first of a few operating points that is not discarded: a program with an alpha
            // test or a stipple can discard it, and a discarded pixel stores no normal to hold to anything
            'normal: {
                let kept = [
                    flat,
                    Setup {
                        colour: [0.5, 0.5, 0.5, 1.0],
                        ..flat
                    },
                ]
                .into_iter()
                .map(|s| run_written(&shader, s).unwrap())
                .find(|(_, written)| *written);
                let Some((flat_out, _)) = kept else {
                    *discarded_flat.entry(stem.to_owned()).or_default() += 1;
                    // it can only be a program with a discard in it
                    assert!(
                        has_discard(&sh.bytecode),
                        "{stem} {effects:?} writes nothing and has no discard"
                    );
                    break 'normal;
                };
                let t = normal.entry(group.clone()).or_default();
                t.1 += 1;
                let is_normal = (0..3).all(|k| (flat_out[1][k] - (flat.normal[k] * 0.5 + 0.5)).abs() < 1e-2);
                // a program the pass reads as storing target 1 biased holds the normal there
                if stem.starts_with("PS_DeferredTextured")
                    && gbuffer::writes(&sh.bytecode).is_ok_and(|w| w.normal_biased)
                    && !is_normal
                {
                    biased_not_normal.push(format!("{stem} {effects:?}: {:?}", flat_out[1]));
                }
                if is_normal {
                    t.0 += 1;
                } else {
                    let misses = normal_misses.entry(group.clone()).or_default();
                    if misses.len() < 3 {
                        misses.push(format!(
                            "{stem}: {:?} outputs {:?}",
                            flat_out[1],
                            sh.interface
                                .outputs
                                .iter()
                                .map(|v| (v.used, v.provenance.d3d.map(|d| (d.mask, d.read_write_mask))))
                                .collect::<Vec<_>>()
                        ));
                    }
                }
            }
        }
        if widths.len() > 3
            && let Ok(out) = run(&shader, stress)
            && out[3][2] != 0.0
        {
            for e in effects {
                *emission.entry(e.clone()).or_default() += 1;
            }
        }
    }
    println!("{} distinct PS_Deferred* programs", programs.len());
    for ((stem, widths), count) in &signatures {
        println!("  {count:>5} {stem} writes targets of widths {widths:?}");
    }
    println!("programs the executor does not cover, by reason: {unevaluated:?}");
    for ((group, t, l), lane) in &matrix {
        let responds: Vec<String> = CLASSES
            .iter()
            .zip(lane.responds)
            .filter(|(_, n)| *n > 0)
            .map(|(c, n)| format!("{c} {n}"))
            .collect();
        println!(
            "LANE {group} target {t}.{} of {} programs: responds to [{}]; fixed {:?}",
            ["x", "y", "z", "w"][*l],
            lane.programs,
            responds.join(", "),
            lane.fixed
        );
    }
    println!("target 1 xyz = n * 0.5 + 0.5 (programs that do / evaluated), by group: {normal:?}");
    println!("  the ones that do not: {normal_misses:?}");
    println!(
        "the lifted text of the {FIVE} programs: {} lifted, {} refused {:?}; target 1 xyz each biased by 0.5 in {}; target 3 z computed in {}, of the effects {:?}",
        static_text.lifted,
        static_text.refused.values().sum::<usize>(),
        static_text.refused,
        static_text.biased,
        static_text.emission_written,
        static_text.emission_effects
    );
    println!(
        "reads of target 2 and 4 over the {FIVE} programs: intensity stored as a square root or 0 {}, exponent stored as a square root or 0 {}, Fresnel written {}, motion written {}; unread {:?}",
        static_text.intensity_sqrt,
        static_text.exponent_sqrt,
        static_text.fresnel,
        static_text.motion,
        static_text.unread
    );
    println!(
        "target 1 of the {FIVE} programs, as the reading finds it: biased {}, floored {}, constant 0 {}, of {}",
        static_text.biased_read, static_text.floored_read, static_text.zero_read, static_text.lifted
    );
    println!("not biased: {:#?}", static_text.odd);
    println!("target 3 z nonzero under stress, by effect: {emission:?}");
    println!(
        "programs that discard the flat pixel at every operating point tried, by family: {discarded_flat:?}"
    );
    // the reading of the stores holds: every program read as storing target 1 biased holds the normal there, no
    // program of the G-buffer pass is refused, and the emission is written only by emissive effects
    assert!(biased_not_normal.is_empty(), "{biased_not_normal:?}");
    // a handful of programs: the alpha-test and stipple families, and two grass programs
    assert!(discarded_flat.values().sum::<usize>() < 60, "{discarded_flat:?}");
    assert!(static_text.refused.is_empty(), "{:?}", static_text.refused);
    assert!(static_text.unread.is_empty(), "{:?}", static_text.unread);
    assert!(
        static_text
            .emission_effects
            .iter()
            .all(|e| e.contains("emissive")),
        "{:?}",
        static_text.emission_effects
    );
}
