//! Real lakes (Riders Republic build 25394987, DataPC_MAD_USNAT): WaterVisualEntity_001_008 placed and lowered by
//! lower-anvil-water under the pass manager with its checks on, against the notes' counts (docs/material-spec.md
//! §24.3: 120 patches, levels 3 to 6 with 29, 41, 34 and 16, all at z 781) and the recipe's geometry (§24.6, §24.9).
mod inputs;

use anvil_formats::archive::Archive;
use inputs::{Game, install};
use primase_anvil::components::AnvilWater;
use primase_anvil::passes::{LowerAnvilWater, Shore};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{Instance, Mesh, Semantic, Stream, Texture, Transform, World};

#[test]
fn a_lake_becomes_its_patches() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = ["DataPC_MAD_USNAT_patch_01.forge", "DataPC_MAD_USNAT.forge"];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let names = &archive.forge(1).expect("forge").names;
    let entry = names
        .iter()
        .position(|n| n.name == "WaterVisualEntity_001_008" && n.type_id == anvil_formats::entity::ENTITY)
        .expect("the lake");
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    placer
        .place(anvil_formats::archive::At { forge: 1, entry }, &mut placed)
        .expect("place");
    assert_eq!((placed.lakes.len(), placed.lakes_unread), (1, 0));
    let lake = placed.lakes[0];
    let water = w.get::<AnvilWater>(lake).expect("water").clone();
    assert_eq!(water.patches.len(), 120);
    assert!(water.atlases.iter().all(Option::is_some));
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pass = LowerAnvilWater {
        rect: [-1e6, -1e6, 1e6, 1e6],
        depth: [(3, 2.0), (4, 1.0), (5, 0.5), (6, 0.5)].into(),
        river_depth: 1.0,
        colour_max: 0.15,
        roughness: 0.05,
        shore: Shore::Coverage,
    };
    let mut report = RunReport::default();
    manager
        .run(
            &mut w,
            &Pipeline {
                passes: vec![Box::new(pass)],
            },
            &mut report,
        )
        .expect("lower");
    let mesh = w.get::<Mesh>(lake).expect("the lake's mesh");
    // placed at its origin (its least corner rounded down to 64 m), its vertices local to it
    let placed = w
        .iter::<Instance>()
        .find(|(_, i)| i.prototype == lake)
        .map(|(e, _)| e)
        .expect("placed");
    let origin = w.get::<Transform>(placed).expect("transform").cols[3];
    assert!(origin.iter().all(|c| c % 64.0 == 0.0), "{origin:?}");
    let primitives = mesh.primitives();
    // a primitive a level, levels 3 to 6, four corners a patch
    let counts: Vec<usize> = primitives
        .iter()
        .map(|p| p.positions().expect("positions").len() / 4)
        .collect();
    assert_eq!(counts, [29, 41, 34, 16]);
    // every corner at the water level; each patch's first corner at its position, its texture coordinate (u0, v0)
    let mut levels = water.patches.clone();
    levels.sort_by_key(|p| p.level);
    let first_of = |level: u32| levels.iter().filter(move |p| p.level == level);
    for (k, prim) in primitives.iter().enumerate() {
        let level = 3 + k as u32;
        let positions: Vec<[f32; 3]> = prim
            .positions()
            .expect("positions")
            .iter()
            .map(|q| std::array::from_fn(|k| q[k] + origin[k]))
            .collect();
        assert!(positions.iter().all(|p| (p[2] - 781.0).abs() < 1e-3));
        let Some(Stream::F32x2(uvs)) = prim.streams.get(&Semantic::TexCoord(0)) else {
            panic!("texture coordinates")
        };
        let side = (512 >> level) as f32;
        for (i, p) in first_of(level).enumerate() {
            assert_eq!(positions[4 * i], [p.position[0], p.position[1], 781.0]);
            assert_eq!(
                positions[4 * i + 2],
                [p.position[0] + side, p.position[1] + side, 781.0]
            );
            assert_eq!(uvs[4 * i], [p.rect[0], p.rect[1]]);
            assert_eq!(uvs[4 * i + 2], [p.rect[2], p.rect[3]]);
        }
        let material = w
            .get::<PbrMaterial>(prim.material.expect("material"))
            .expect("pbr");
        assert_eq!((material.alpha_mode, material.metallic), (AlphaMode::Blend, 0.0));
        let texture = material.base_color_texture.expect("colour").texture;
        assert!(w.get::<Texture>(texture).is_some());
    }
    assert_eq!(
        w.iter::<Instance>().filter(|(_, i)| i.prototype == lake).count(),
        1
    );
}
