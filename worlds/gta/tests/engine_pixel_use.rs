//! The lanes of the engine constants the pixel and vertex programs read and `resolve-gta-shaders` gives no role (the
//! remainder after it binds only what a program reads): per constant and stage, how many programs read each lane, which
//! effects' programs they are, and the smallest reader. The emitted WGSL and constant table of the smallest reader of
//! each lane is written under the scratch root's `gta-engine-use` folder, so the statements in the role table's docs can be read off the
//! code.
//!
//! What the smallest readers show (read off their WGSL; the roles' reasons in `engine_reason` state it):
//!
//! - `gStippleAlpha` and `gTemporalAntiGhosting` (row 24 of `PS_DeferredTextured_6b2704e5`) are words of the state byte
//!   written to target 0 w: `((stipple << 6) & 192) | select(7, 23, antighosting as float > 0.5)`.
//! - `gDynamicBakesAndWetness` z multiplies an up-facing, wetness-weighted term (`(1 - globalScalars2.z) *
//!   saturate((n.z - 0.35) * 1.54) * z`) that darkens the albedo and raises the specular in the parallax programs, and
//!   blends a dirt texture's channels in `PS_VehicleTexturedUnlit`; x and y are the two coefficients of
//!   `y * (0.5 + 0.5 n.z) + x` in the ped programs.
//! - `gLightArtificialIntAmbient0` is read whole only by the lighting passes (with five other ambient colours); the
//!   G-buffer programs read its w, added to `globalScalars.z * vertex colour x` in target 3 x.
//! - `globalScalars` x multiplies the alpha before the alpha test; `globalScalars2` z is `1 - z` in a wetness term and
//!   `floor(128 z)` in the state byte.
//!
//! The counts are printed (`USE name stage: lanes, effects, smallest reader`) and the lane counts the reasons rest on are
//! asserted. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use inputs::{Game, install, keys};
use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaShaderTable};
use primase_gta::passes::resolve_shaders::constant_reads_of;
use primase_gta::passes::resolve_shaders::value_bytes;
use primase_gta::read::source::GtaSource;

const NAMES: [&str; 12] = [
    "gDynamicBakesAndWetness",
    "gStippleAlpha",
    "gLightArtificialIntAmbient0",
    "gTemporalAntiGhosting",
    "gTransparencyAASamples",
    "gUmGlobalPrevTimer",
    "POMFlags",
    "globalHeightScale",
    "globalFogParams",
    "globalScalars",
    "globalScalars2",
    "gClothVertexStride",
];

/// One constant of one stage: the programs reading each lane, their effects, the smallest reader.
#[derive(Default)]
struct Use {
    lanes: [usize; 4],
    effects: BTreeMap<String, usize>,
    smallest: Option<(usize, String)>,
    /// The smallest reader of each lane: bytecode size, program name, entity.
    by_lane: [Option<(usize, String, primase_core::Entity)>; 4],
}

#[test]
fn how_the_programs_use_the_remaining_engine_constants() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let mut seen = BTreeSet::new();
    let mut uses: BTreeMap<(String, String), Use> = BTreeMap::new();
    // the programs whose reads are not known, by stage and why (the message's first words)
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    for (_, effect) in world.iter::<GtaEffect>() {
        for p in &effect.programs {
            let Some(e) = p.shader else { continue };
            if !seen.insert(e) {
                continue;
            }
            let (Some(sh), Some(table)) = (
                world.get::<Shader>(e),
                world.get::<GtaShaderTable>(e).map(|t| &t.0),
            ) else {
                continue;
            };
            let reads = match constant_reads_of(&sh.bytecode) {
                Ok(r) => r,
                Err(e) => {
                    let why: String = e.chars().take(80).collect();
                    *refused.entry(format!("{:?}: {why}", p.stage)).or_default() += 1;
                    continue;
                }
            };
            let stage = format!("{:?}", p.stage);
            for (i, r) in table.resources.iter().enumerate() {
                if r.constants == 0 {
                    continue;
                }
                for c in table.constants_of(i) {
                    let Some(name) = table.name_of(c.name) else {
                        continue;
                    };
                    if !NAMES.contains(&name) {
                        continue;
                    }
                    let (sp, rg) = (u32::from(r.space), u32::from(r.register));
                    let Some(bytes) = value_bytes(c.flags) else {
                        continue;
                    };
                    let offset = u32::from(c.offset);
                    let words: Vec<u32> = (0..bytes.min(16) / 4)
                        .filter(|k| reads.reads_at(sp, rg, offset + 4 * k, 4))
                        .collect();
                    if words.is_empty() {
                        continue;
                    }
                    let u = uses.entry((name.to_owned(), stage.clone())).or_default();
                    for k in words {
                        // the lane of the constant, counted from its first word
                        u.lanes[k as usize] += 1;
                        let slot = &mut u.by_lane[k as usize];
                        if slot.as_ref().is_none_or(|(n, _, _)| sh.bytecode.len() < *n) {
                            *slot = Some((sh.bytecode.len(), p.name.clone(), e));
                        }
                    }
                    *u.effects.entry(effect.name.clone()).or_default() += 1;
                    if u.smallest.as_ref().is_none_or(|(n, _)| sh.bytecode.len() < *n) {
                        u.smallest = Some((sh.bytecode.len(), p.name.clone()));
                    }
                }
            }
        }
    }
    let dump = inputs::scratch().join("gta-engine-use");
    std::fs::create_dir_all(&dump).unwrap();
    for (why, n) in &refused {
        println!("REFUSED {n:6} {why}");
    }
    for ((name, stage), u) in &uses {
        let mut effects: Vec<_> = u.effects.iter().collect();
        effects.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let stems: Vec<String> = effects.iter().take(6).map(|(e, n)| format!("{e}:{n}")).collect();
        let (size, program) = u.smallest.as_ref().unwrap();
        println!(
            "USE {name} {stage}: lanes x{} y{} z{} w{}, {} effects ({}), smallest {program} ({size} bytes)",
            u.lanes[0],
            u.lanes[1],
            u.lanes[2],
            u.lanes[3],
            u.effects.len(),
            stems.join(" ")
        );
        {
            for (lane, reader) in u.by_lane.iter().enumerate() {
                let Some((_, program, entity)) = reader else {
                    continue;
                };
                let bytes = &world.get::<Shader>(*entity).unwrap().bytecode;
                let Ok(lifted) = iridia_dxil::lift(bytes) else {
                    continue;
                };
                let Ok(text) = iridia::emit::wgsl(&lifted) else {
                    continue;
                };
                let stem = format!("{}/{name}.{stage}.lane{lane}", dump.display());
                std::fs::write(format!("{stem}.wgsl"), format!("// {program}\n{text}")).unwrap();
                // the constants that reader reads, by buffer and offset: which row of the WGSL is which constant
                let table = world.get::<GtaShaderTable>(*entity).unwrap();
                let reads = constant_reads_of(bytes).unwrap();
                let mut lines = String::new();
                for (i, r) in table.0.resources.iter().enumerate() {
                    for c in table.0.constants_of(i) {
                        let (sp, rg) = (u32::from(r.space), u32::from(r.register));
                        let size = value_bytes(c.flags).unwrap_or(4);
                        let words: Vec<u32> = (0..size.min(16) / 4)
                            .filter(|k| reads.reads_at(sp, rg, u32::from(c.offset) + 4 * k, 4))
                            .collect();
                        if !words.is_empty() {
                            lines += &format!(
                                "cb{rg} space {sp} offset {} (row {} lane {}) {}: words {words:?}\n",
                                c.offset,
                                c.offset / 16,
                                c.offset % 16 / 4,
                                table.0.name_of(c.name).unwrap_or("?")
                            );
                        }
                    }
                }
                std::fs::write(format!("{stem}.table.txt"), lines).unwrap();
            }
        }
    }
    let lanes = |name: &str, stage: &str| uses[&(name.to_owned(), stage.to_owned())].lanes;
    assert!(lanes("gStippleAlpha", "Pixel")[0] > 1_000);
    assert_eq!(lanes("gStippleAlpha", "Pixel")[1..], [0, 0, 0]);
    assert!(lanes("gTemporalAntiGhosting", "Pixel")[0] > 300);
    let wet = lanes("gDynamicBakesAndWetness", "Pixel");
    assert!(wet[0] > 300 && wet[1] > 300 && wet[2] > 1_500 && wet[3] == 0);
    let ambient = lanes("gLightArtificialIntAmbient0", "Pixel");
    assert!(ambient[..3].iter().all(|n| *n > 2_000) && ambient[3] > 1_000);
    let scalars = lanes("globalScalars", "Pixel");
    assert!(scalars[..3].iter().all(|n| *n > 4_000));
    assert!(lanes("globalScalars2", "Pixel")[2] > 4_000);
    assert!(lanes("gTransparencyAASamples", "Pixel")[0] > 50);
}
