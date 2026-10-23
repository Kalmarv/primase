//! What the G-buffer programs of a campus do with the engine's per-frame, per-view and per-draw buffers (`cb0`, `cb1`,
//! `cb2`), over the programs `resolve-anvil-shaders` binds on a small rectangle: per stage and buffer row, how many
//! distinct programs read each lane (`USE stage cbN[row]: lanes x y z w`), and with `PRIMASE_DUMP` set to a folder the
//! emitted WGSL of the smallest vertex and pixel program (`vertex.wgsl`, `pixel.wgsl`), so the statements in
//! `resolve_shaders::engine_reason` can be read off the code.
//!
//! Needs `installs.riders`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use primase_anvil::passes::ResolveAnvilShaders;
use primase_anvil::read::convert::{
    AnvilOptions, AnvilReader, CellSelection, Edition, ForgeFile, GridSelection, Levels,
};
use primase_anvil::read::placements::{InstancedCollision, InstancedLevels};
use primase_core::convert::Reader;
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{Shader, ShaderBinding, Stage};
use primase_core::{Instance, World};
use primase_shader::ConstantReads;
use primase_testdata::{Game, install};

#[test]
fn how_the_programs_read_the_engine_buffers() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let options = AnvilOptions {
        forges: [
            ("DataPC_MAD_USNAT_patch_01.forge", Edition::Patch),
            ("DataPC_MAD_USNAT.forge", Edition::Base),
            ("DataPC_patch_01.forge", Edition::Patch),
            ("DataPC.forge", Edition::Base),
        ]
        .map(|(f, edition)| ForgeFile {
            path: dir.join(f),
            edition,
        })
        .to_vec(),
        frame: SourceFrame::ENU,
        grids: ["ShortRange", "GRN_Template_TerrainWorldSmall"]
            .iter()
            .map(|g| GridSelection {
                grid: (*g).into(),
                cells: CellSelection::Over {
                    rect: [-3584.0, 1152.0, -3456.0, 1280.0],
                    levels: Levels::Finest,
                },
            })
            .collect(),
        instanced_levels: InstancedLevels::Every,
        instanced_collision: InstancedCollision::PerInstance,
        terrain: None,
    };
    let mut reader = AnvilReader::open(&options).unwrap();
    let (read, _) = reader.read().unwrap();
    let roots: BTreeSet<_> = read.iter::<Instance>().map(|(e, _)| e).collect();
    let (mut world, _): (World, _) = reader.load(&read, &roots).unwrap();
    let pass = ResolveAnvilShaders::from_options(&Options::from([(
        "unset-textures".to_owned(),
        "white".to_owned(),
    )]))
    .unwrap();
    pass.run(&mut world, &mut PassLog::default()).unwrap();
    let used: BTreeSet<_> = world
        .iter::<ShaderBinding>()
        .flat_map(|(_, b)| b.stages.iter().map(|s| (s.stage, s.shader)))
        .collect();
    // (stage, register, row) -> programs reading each lane
    let mut lanes: BTreeMap<(Stage, u32, u32), [usize; 4]> = BTreeMap::new();
    let mut smallest: BTreeMap<Stage, (usize, Vec<u8>)> = BTreeMap::new();
    let mut programs = 0usize;
    for (stage, e) in used {
        let Some(shader) = world.get::<Shader>(e) else {
            continue;
        };
        let Ok(lifted) = primase_dxbc::lift(&shader.bytecode) else {
            continue;
        };
        programs += 1;
        let reads = ConstantReads::of_module(lifted.module, |b| (b.group, b.binding));
        for register in 0..3 {
            let (rows, _) = reads.rows_of(0, register);
            for row in rows {
                let entry = lanes.entry((stage, register, row)).or_default();
                for lane in reads.lanes_of(0, register, row) {
                    entry[lane as usize] += 1;
                }
            }
        }
        if smallest
            .get(&stage)
            .is_none_or(|(n, _)| shader.bytecode.len() < *n)
        {
            smallest.insert(stage, (shader.bytecode.len(), shader.bytecode.clone()));
        }
    }
    println!("{programs} distinct programs");
    for ((stage, register, row), l) in &lanes {
        println!(
            "USE {stage:?} cb{register}[{row}]: lanes x{} y{} z{} w{}",
            l[0], l[1], l[2], l[3]
        );
    }
    if let Ok(dump) = std::env::var("PRIMASE_DUMP") {
        for (stage, (_, bytes)) in smallest {
            let lifted = primase_dxbc::lift(&bytes).unwrap();
            let text = primase_shader::emit::wgsl(&lifted).unwrap();
            let name = if stage == Stage::Vertex { "vertex" } else { "pixel" };
            std::fs::write(format!("{dump}/{name}.wgsl"), text).unwrap();
        }
    }
    assert!(programs > 10);
}
