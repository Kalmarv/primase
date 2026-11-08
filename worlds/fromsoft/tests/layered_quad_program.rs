//! Core's composition of a layered quad (`core.particles` [`compose_layers`]), with the targets and operations the
//! effects lowering reads from a multi-texture billboard's words, checked against the game's own program: Elden Ring's
//! multi-texture effect pixel program (`GXFfxtessellateMultiTexture.ppo` of the install's `/shader/gxffxshader`
//! binder), lifted by iridia and run by its executor, one pixel per case.
//!
//! The program is run with its constants set by their reflected names: the three layer words (`g_ps_TexBlendType`,
//! `g_ps_TexBlendType2`, `g_ps_TexBlendType3`) as the action's `unk_ds3_f2_10`, `unk_ds3_f2_11` and `unk_ds3_f2_12` (the
//! lowering's reading), `g_ps_bPreAlphaMult` as its premultiply switch; the particle colour not applied
//! (`g_ps_ColorBlendType` 1), unlit, unshadowed, unfogged, at an exposure of 1, with no soft-sprite cutoff and no fade.
//! Each layer's texture answers one texel (its slot found by the reflected name `g_multiTexture0` to `2`) and each
//! layer's colour is an input: the first layer's three lanes of `TEXCOORD4` (the program reads no fourth), the second's
//! and third's `TEXCOORD5` and `TEXCOORD6`. The program writes the composed colour with red, green and blue at least 0
//! and opacity clamped to 0..1, which the comparison applies to core's.
//!
//! Every layer word (0 to 2), every operation word of each laid layer (0 to 2, and 3, which the program combines in no
//! way and the lowering leaves out), both premultiply settings and eight texel and colour sets are run: 768 pixels.
//! Not covered: the first layer's colour alpha (the program does not read one: a base tint's alpha other than 1 is not
//! checked), the texel alpha offset `TEXCOORD2.z` (0 here) and the particle colour's own blend words.
//!
//! Needs `installs.elden`, `keys.elden` and `tools.oodle6`.
mod common;
mod inputs;

use fromsoft_formats::{Binder, Game, dcx::Decoder};
use iridia_exec::{Dim, ImageSample, Lod, PixelShader, ProgramShader, Sample, Sampler};
use iridia_interface::{D3dClass, Interface, ResourceKind};
use primase_core::particles::{LayerOperation, LayerTarget, compose_layers};
use primase_fromsoft::sekiro::passes::effects::units;
use primase_fromsoft::sekiro::passes::effects::value::Notes;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const BINDER: &str = "/shader/gxffxshader.shaderbnd.dcx";
const PROGRAM: &str = "GXFfxtessellateMultiTexture.ppo";

/// The texture slots of the three layers, and the texel each answers.
struct Layers {
    slots: [u32; 3],
    texels: [[f32; 4]; 3],
}

impl Sampler for Layers {
    fn sample(&mut self, request: Sample) -> std::result::Result<[f32; 4], String> {
        let layer = self
            .slots
            .iter()
            .position(|s| *s == request.texture)
            .ok_or_else(|| format!("t{}: not a layer's texture", request.texture))?;
        Ok(self.texels[layer])
    }

    /// The volume textures (the shadow volume, the volumetric fog) read white: no shadow, and fog the fixture turns off.
    fn sample_image(&mut self, request: ImageSample<u32>) -> std::result::Result<[f32; 4], String> {
        if request.dim == Dim::D3 {
            return Ok([1.0; 4]);
        }
        self.sample(Sample {
            texture: request.texture,
            sampler: request.sampler,
            uv: [request.coord[0], request.coord[1]],
            layer: request.layer,
            lod: Lod::from(request.lod),
        })
    }
}

/// The register of the program's input named `name` and `index`.
fn input(interface: &Interface, name: &str, index: u32) -> Result<usize> {
    let v = interface
        .inputs
        .iter()
        .find(|v| v.label.name.eq_ignore_ascii_case(name) && v.label.index == index)
        .ok_or_else(|| format!("no input {name}{index}"))?;
    let d3d = v.provenance.d3d.as_ref().ok_or("an input without a register")?;
    Ok(d3d.register as usize)
}

/// The `t` slot of the texture named `name`.
fn texture(interface: &Interface, name: &str) -> Result<u32> {
    interface
        .resources
        .iter()
        .find(|r| r.provenance.name == name)
        .and_then(|r| r.provenance.d3d)
        .filter(|d| d.class == D3dClass::ShaderResource)
        .map(|d| d.register)
        .ok_or_else(|| format!("no texture {name}").into())
}

/// Every constant buffer the program reads, zeroed, by `b` slot, with its named constants' word offsets.
struct Buffers {
    rows: Vec<(u32, Vec<[u32; 4]>)>,
    names: Vec<(String, u32, u32)>,
}

impl Buffers {
    fn new(interface: &Interface, shader: &ProgramShader) -> Result<Self> {
        let mut rows = Vec::new();
        let mut names = Vec::new();
        for &(slot, lines) in shader.constant_buffers() {
            rows.push((slot, vec![[0u32; 4]; lines as usize]));
        }
        for r in &interface.resources {
            if let (ResourceKind::ConstantBuffer { layout }, Some(d3d)) = (&r.kind, r.provenance.d3d) {
                for c in &layout.constants {
                    names.push((c.name.clone(), d3d.register, c.offset / 4));
                }
            }
        }
        Ok(Self { rows, names })
    }

    fn set(&mut self, name: &str, lanes: &[u32]) -> Result<()> {
        let &(_, slot, word) = self
            .names
            .iter()
            .find(|(n, _, _)| n == name)
            .ok_or_else(|| format!("no constant {name}"))?;
        let (_, rows) = self
            .rows
            .iter_mut()
            .find(|(s, _)| *s == slot)
            .ok_or_else(|| format!("b{slot}: not read by the program"))?;
        for (i, v) in lanes.iter().enumerate() {
            let w = word as usize + i;
            rows[w / 4][w % 4] = *v;
        }
        Ok(())
    }
}

/// A small deterministic stream of values in 0..1.
struct Stream(u64);
impl Stream {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }
    fn four(&mut self) -> [f32; 4] {
        [self.next(), self.next(), self.next(), self.next()]
    }
}

#[test]
fn the_game_program_composes_layers_as_core_does() -> Result<()> {
    let (Some(dll), Some(archive)) = (
        inputs::oodle(inputs::Tool::Oodle6),
        common::archive(Game::EldenRing)?,
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let binder = Binder::read(&archive.read(BINDER, &decoder)?, &decoder)?;
    let member = binder
        .files
        .iter()
        .find(|m| m.name.as_deref().is_some_and(|n| n.ends_with(PROGRAM)))
        .ok_or_else(|| format!("{BINDER}: no {PROGRAM}"))?;
    let shader = ProgramShader::from_ir(primase_core::shader::lift_dxil(&member.bytes)?)?;
    let interface = shader.interface().clone();

    let slots = [
        texture(&interface, "g_multiTexture0")?,
        texture(&interface, "g_multiTexture1")?,
        texture(&interface, "g_multiTexture2")?,
    ];
    let colour_in = [
        input(&interface, "TEXCOORD", 4)?,
        input(&interface, "TEXCOORD", 5)?,
        input(&interface, "TEXCOORD", 6)?,
    ];
    let particle = input(&interface, "COLOR", 0)?;
    let screen = input(&interface, "TEXCOORD", 10)?;

    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let mut buffers = Buffers::new(&interface, &shader)?;
    buffers.set("g_ps_ColorBlendType", &[1])?;
    buffers.set("g_ps_bEnableLighting", &[0])?;
    buffers.set("g_ps_BillboardShadowIntensity", &bits(&[0.0]))?;
    buffers.set("g_ps_isDisableFog", &[1])?;
    buffers.set("g_ps_BlendingMode", &[0])?;
    buffers.set("g_ps_preExposure", &bits(&[1.0]))?;
    buffers.set("g_transductionAlphaToColorScale", &bits(&[1.0]))?;
    buffers.set("g_ps_softSpriteParam", &bits(&[0.0, -1.0, 0.0, 0.0]))?;

    let mut stream = Stream(9);
    let mut runs = 0;
    for set in 0..8 {
        let texels = [stream.four(), stream.four(), stream.four()];
        let mut tints = [stream.four(), stream.four(), stream.four()];
        tints[0][3] = 1.0;
        for targets_word in 0..=2 {
            for second in 0..=3 {
                for third in 0..=3 {
                    for premultiply in [false, true] {
                        // the program
                        buffers.set("g_ps_TexBlendType", &[targets_word, second, third])?;
                        buffers.set("g_ps_bPreAlphaMult", &[u32::from(premultiply)])?;
                        let mut inputs = vec![[0u32; 4]; shader.input_count()];
                        inputs[particle] = [1.0f32.to_bits(); 4];
                        inputs[screen] = [0.0, 0.0, 0.0, 1.0].map(f32::to_bits);
                        for (r, t) in colour_in.iter().zip(&tints) {
                            inputs[*r] = t.map(f32::to_bits);
                        }
                        let buffer_refs: Vec<(u32, &[[u32; 4]])> =
                            buffers.rows.iter().map(|(s, r)| (*s, r.as_slice())).collect();
                        let run = shader.execute(&inputs, &buffer_refs, &mut Layers { slots, texels })?;
                        let game = run.outputs[0].map(f32::from_bits);

                        // core, with the lowering's reading of the words
                        let mut notes = Notes::default();
                        let targets = units::layer_targets(targets_word as i32, &mut notes);
                        let sample =
                            |i: usize| -> [f32; 4] { std::array::from_fn(|c| texels[i][c] * tints[i][c]) };
                        let mut laid: Vec<(LayerTarget, LayerOperation, [f32; 4])> = Vec::new();
                        for (i, word) in [(1, second), (2, third)] {
                            if let Some(op) = units::layer_operation(word as i32, &mut notes) {
                                laid.push((targets[i - 1], op, sample(i)));
                            }
                        }
                        let ours = compose_layers(premultiply, sample(0), &laid);
                        let ours = [
                            ours[0].max(0.0),
                            ours[1].max(0.0),
                            ours[2].max(0.0),
                            ours[3].clamp(0.0, 1.0),
                        ];

                        let worst = game
                            .iter()
                            .zip(&ours)
                            .map(|(a, b)| (a - b).abs())
                            .fold(0.0f32, f32::max);
                        assert!(
                            worst < 1e-4,
                            "set {set}, words {targets_word} {second} {third}, premultiply {premultiply}: \
                             the game's program {game:?}, core {ours:?}"
                        );
                        runs += 1;
                    }
                }
            }
        }
    }
    println!("{runs} pixels of {PROGRAM} agree with core's composition");
    Ok(())
}
