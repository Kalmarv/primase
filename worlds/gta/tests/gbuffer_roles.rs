//! The roles `resolve-gta-shaders` gives target 2 and target 4 of the G-buffer, against the programs' own behaviour and
//! tables rather than the reading of their stores that grants them (`passes::gbuffer`):
//!
//! - Target 2's x and y decode to the material's specular parameters: on the program the dxc disassembly of notes
//!   8fe53305 reads (`normal_spec_decal`, `PS_DeferredTextured_0174add1`), with the specular texture's channels 1, x is
//!   `sqrt(SpecularColor * (mask.x + mask.y + mask.z))` and y is `sqrt(Specular / 512)`; the material's cbuffer
//!   (register 10, space 1) holds Fresnel at word 0, Specular at 1, SpecularColor at 2 and the mask at words 4 to 6,
//!   and every other constant word is 0. The decoded values of the `ScalarEncoding`s the pass binds are the
//!   parameters, on three parameter sets.
//! - Target 4's motion vector is the displacement to the previous frame: the pixel program stores
//!   `TEXCOORD10.xy / TEXCOORD10.w - SV_Position.xy + ...`, and the vertex program's `TEXCOORD10` reads, of the
//!   matrices buffer (register 2, space 0), the four rows of one of the engine's previous-frame matrices
//!   (`gPrevWorldViewProj` or `gRelativePrevWorldViewProj`, the row ranges the program's own table gives them),
//!   possibly with rows of `gWorld`; a program that reads no row of the buffer is counted, not claimed.
//!
//! The counts are printed. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::shader::{Label, ScalarEncoding, Shader};
use primase_gta::components::{GtaEffect, GtaShaderTable, GtaStage};
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

#[test]
fn target_two_decodes_to_the_materials_specular_parameters() {
    let Some(world) = world() else { return };
    let entity = world
        .iter::<GtaEffect>()
        .find(|(_, e)| e.name == "normal_spec_decal")
        .and_then(|(_, e)| {
            e.programs
                .iter()
                .find(|p| p.stage == GtaStage::Pixel && p.name == "PS_DeferredTextured_0174add1_Wrapped")
                .and_then(|p| p.shader)
        })
        .unwrap();
    let sh = world.get::<Shader>(entity).unwrap();
    let shader = executor(&sh.bytecode).unwrap();
    let intensity_encoding = ScalarEncoding::square_root_of(1.0);
    let exponent_encoding = ScalarEncoding::square_root_of(512.0);
    for (specular, colour, mask) in [
        (100.0f32, 0.3f32, [1.0f32, 0.0, 0.0]),
        (400.0, 0.8, [0.5, 0.25, 0.25]),
        (12.0, 0.05, [1.0, 1.0, 0.0]),
    ] {
        let buffers: Vec<(u32, u32)> = PixelShader::constant_buffers(&shader)
            .iter()
            .map(|&(slot, rows)| (slot, rows.saturating_mul(4)))
            .collect();
        let mut constants = Constants::new(&Layout {
            buffers,
            fields: BTreeMap::new(),
        });
        constants
            .set_words(10, 1, &[specular.to_bits(), colour.to_bits()])
            .unwrap();
        constants.set_words(10, 4, &mask.map(f32::to_bits)).unwrap();
        let textures: BTreeMap<u32, Texture> = PixelShader::textures(&shader)
            .iter()
            .map(|&slot| (slot, Texture::Constant([1.0; 4])))
            .collect();
        let mut inputs = BTreeMap::new();
        for v in &PixelShader::interface(&shader).inputs {
            let value = match (v.label.name.to_ascii_uppercase().as_str(), v.label.index) {
                ("SV_POSITION", _) => [0.0, 0.0, 0.0, 1.0],
                ("TEXCOORD", 0) => [0.3, 0.7, 0.0, 0.0],
                ("TEXCOORD", 1) => [0.0, 0.0, 1.0, 0.0],
                ("TEXCOORD", 4) => [1.0, 0.0, 0.0, 0.0],
                ("TEXCOORD", 5) => [0.0, 1.0, 0.0, 0.0],
                _ => [0.5; 4],
            };
            inputs.insert((v.label.name.clone(), v.label.index), Input::Constant(value));
        }
        let raw = evaluate_raw(
            &shader,
            &constants,
            &textures,
            &inputs,
            Grid {
                size: 1,
                period: [1.0; 2],
            },
            &[(2, 0b0011)],
            Invocation::Scalar,
        )
        .unwrap();
        let [x, y, ..] = raw.pixels[0][0].0.map(f32::from_bits);
        let (intensity, exponent) = (intensity_encoding.decode(x), exponent_encoding.decode(y));
        let wanted = colour * (mask[0] + mask[1] + mask[2]);
        println!(
            "specular {specular}, colour {colour}, mask {mask:?}: intensity {intensity} (wanted {wanted}), exponent {exponent}"
        );
        assert!(
            (intensity - wanted).abs() < 1e-3 * wanted.max(1.0),
            "intensity {intensity} for {wanted}"
        );
        assert!(
            (exponent - specular).abs() < 1e-3 * specular,
            "exponent {exponent} for {specular}"
        );
    }
}

#[test]
fn the_motion_interpolant_is_the_previous_frames_clip_position() {
    let Some(world) = world() else { return };
    let label = Label {
        name: "TEXCOORD".into(),
        index: 10,
    };
    let mut checked = 0usize;
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut disagreed = Vec::new();
    let mut seen = BTreeSet::new();
    for (_, e) in world.iter::<GtaEffect>() {
        for p in e.programs.iter().filter(|p| p.stage == GtaStage::Vertex) {
            let Some((stem, _)) = split_program_name(&p.name) else {
                continue;
            };
            if stem != "VS_TransformD" && stem != "VS_TransformSkinD" {
                continue;
            }
            let Some(entity) = p.shader else { continue };
            if !seen.insert(entity) {
                continue;
            }
            let sh = world.get::<Shader>(entity).unwrap();
            if !sh.interface.outputs.iter().any(|v| v.label == label) {
                continue;
            }
            let rows = gbuffer::constant_rows(&sh.bytecode, &label).unwrap();
            let table = &world.get::<GtaShaderTable>(entity).unwrap().0;
            let buffer = table
                .resources
                .iter()
                .position(|r| r.register == 2 && r.space == 0 && r.constants > 0)
                .unwrap();
            // the rows of the matrices the table names in that buffer
            let rows_of = |name: &str| -> Option<BTreeSet<u32>> {
                let c = table
                    .constants_of(buffer)
                    .iter()
                    .find(|c| table.name_of(c.name) == Some(name))?;
                let first = u32::from(c.offset) / 16;
                Some((first..first + 4).collect())
            };
            let previous = ["gPrevWorldViewProj", "gRelativePrevWorldViewProj"].map(|n| rows_of(n).unwrap());
            let world_rows = rows_of("gWorld").unwrap();
            checked += 1;
            let read = rows.get(&2).cloned().unwrap_or_default();
            let class = if read.is_empty() {
                "no row of the matrices buffer"
            } else if previous.contains(&read) {
                "exactly a previous-frame matrix"
            } else if previous
                .iter()
                .any(|m| read.is_superset(m) && read.difference(m).all(|r| world_rows.contains(r)))
            {
                "a previous-frame matrix and rows of gWorld"
            } else {
                disagreed.push(format!("{} reads rows {read:?}", p.name));
                continue;
            };
            *classes.entry(class).or_default() += 1;
        }
    }
    println!("{checked} vertex programs write TEXCOORD10: {classes:?}");
    assert!(
        classes
            .get("exactly a previous-frame matrix")
            .copied()
            .unwrap_or(0)
            > 200,
        "{classes:?}"
    );
    assert!(disagreed.is_empty(), "{disagreed:?}");
}
