//! The layering gate (`primase_reskate::postcompile::layers`) on a compiled fixture whose every case was looked at in
//! the game.
//!
//! Input: `oracles.studio.skate/layers/patch`, Studio's compile (with LODs) of a fixture map of fifteen columns, each a
//! layering case with a BEFORE half (the faulty layout) and mostly an AFTER half (the rule applied); and the game's
//! data and Oodle. Every verdict asserted here is what the game showed (two runs of the fixture, frames kept with the
//! private research): 0 mm opaque layers and cutouts fight, 0.5 and 4 mm do not; cutouts stored as half floats far
//! from their origin come apart; decals 0.5 and 2 mm under their base or under a cover 4 mm (wall) / 3.7 mm (floor)
//! above vanish near and show far; a decal over the sky never shows; overlapping decals whose boxes tie (identical
//! boxes, origins apart or not) change order with the view and the launch, while boxes padded apart along the normal
//! (`ChainOrder::Stacked`) hold one order; a decal over a base whose coarser LODs leave the surface breaks up; two
//! re-triangulated copies of an opaque surface fight, two bit-identical ones do not. The gate must fail every BEFORE
//! case and none of the AFTER ones.

#![allow(clippy::unwrap_used)]

mod common;

use common::R;
use primase_compress::oodle::{Oodle, Version};
use primase_reskate::postcompile::fb::folder::{Folder, Source};
use primase_reskate::postcompile::layers::{self, Check, Limits};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};

#[test]
fn layers_gate_on_the_fixture() -> R {
    let (Some(patch), Some(data), Some(dll)) = (
        oracle(OracleSet::Studio, Game::Skate, "layers/patch"),
        install_file(Game::Skate, "Data"),
        tool(Tool::Oodle9),
    ) else {
        return Ok(());
    };
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let base = Folder::open(&data)?;
    let fo = Folder::open(&patch)?;
    let src = Source {
        folder: &fo,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    let limits = Limits::default();
    let report = layers::gate(&src, &limits)?;
    println!("failing {:?}", report.failing(&limits));
    println!("triangles {:?}", report.triangles);
    println!("unresolved {:?}", report.unresolved);
    for (check, f) in &report.findings {
        println!("{check:?}: {:.4} m2 over {} pairs", f.area_m2, f.pairs);
        for (m, a) in &f.by_mesh {
            println!("  {a:.5} m2 {m}");
        }
    }
    // the area a check found on the meshes whose name holds `part` (the fixture's object names)
    let area = |check: Check, part: &str| -> f64 {
        report.findings.get(&check).map_or(0.0, |f| {
            f.by_mesh
                .iter()
                .filter(|(m, _)| m.contains(part))
                .map(|(_, a)| a)
                .sum()
        })
    };
    // the largest area one mesh holds (an allowance per mesh)
    let largest = |check: Check, part: &str| -> f64 {
        report.findings.get(&check).map_or(0.0, |f| {
            f.by_mesh
                .iter()
                .filter(|(m, _)| m.contains(part))
                .map(|(_, a)| *a)
                .fold(0.0, f64::max)
        })
    };
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    // column 0: opaque stripes 0 / 0.5 / 4 mm over opaque; only 0 mm fights (a 1.2 x 10 m floor strip, 1.2 x 3.6 m
    // wall strip). The AFTER decals at 0 mm pass.
    assert!(
        near(area(Check::DepthWriting, "c0_floor"), 12.0),
        "{}",
        area(Check::DepthWriting, "c0_floor")
    );
    assert!(
        near(area(Check::DepthWriting, "c0_wall"), 4.32),
        "{}",
        area(Check::DepthWriting, "c0_wall")
    );
    // column 1: cutouts at 0 mm and at 4 mm with their origin 40 m away fight; 4 mm with a near origin does not
    assert!(
        near(area(Check::DepthWriting, "c1_floor"), 24.0),
        "{}",
        area(Check::DepthWriting, "c1_floor")
    );
    assert!(
        near(area(Check::DepthWriting, "c1_wall"), 8.64),
        "{}",
        area(Check::DepthWriting, "c1_wall")
    );
    // columns 8 and 9: the far-origin cutouts on a leaning wall and a ramp cross their base; the near-origin ones pass
    for part in ["c8_w1_mask4mm_far37", "c9_r1_mask4mm_far37"] {
        assert!(area(Check::DepthWriting, part) > 0.01, "{part}");
    }
    for part in ["c8_w0_mask4mm_near", "c9_r0_mask4mm_near"] {
        assert_eq!(area(Check::DepthWriting, part), 0.0, "{part}");
    }
    // column 12: a re-triangulated copy fights (its 3 x 3 m); a bit-identical copy is a duplicate, not a fight
    assert!(near(area(Check::DepthWriting, "c12_a_dup"), 9.0));
    assert_eq!(area(Check::DepthWriting, "c12_b_dup"), 0.0);
    assert!(near(area(Check::Duplicate, "c12_b_dup"), 9.0));
    // column 2: decals 2 and 0.5 mm under their base are hidden near (every strip whole), +8 mm and the AFTER side pass
    for part in ["c2_b_f0", "c2_b_f1"] {
        assert!(near(area(Check::Behind, part), 12.0), "{part}");
    }
    for part in ["c2_b_w0", "c2_b_w1"] {
        assert!(near(area(Check::Behind, part), 4.32), "{part}");
    }
    // column 3: the covered halves of the BEFORE decals (floor 3.2 x 9 m half: 14.4; wall 1.6 x 3.6 m: 5.76)
    assert!(near(area(Check::Behind, "c3_b_decal_f"), 14.4));
    assert!(near(area(Check::Behind, "c3_b_decal_w"), 5.76));
    // column 6: a decal and a blended surface over the sky (each 10 m2) are invisible; over the wall they show
    assert!(near(area(Check::Unbacked, "c6_b_decal_sky"), 10.0));
    assert!(near(area(Check::Unbacked, "c6_b_blend_sky"), 10.0));
    // no AFTER layer, and no layer of the order columns (each on its wall), lies under, over or off its base
    for part in [
        "c0_a", "c1_a", "c2_a", "c3_a", "c6_a", "c4_", "c5_", "c11_", "c13_", "c14_",
    ] {
        for check in [Check::Behind, Check::Covered, Check::Floating] {
            assert_eq!(area(check, part), 0.0, "{check:?} {part}");
        }
        // the stacks' bounds pads are slivers behind the wall, under the per-mesh allowance
        assert!(largest(Check::Unbacked, part) < limits.unbacked, "{part}");
    }
    // order: ties (columns 4, 5: identical boxes; 13: origins apart, boxes identical) fail on both halves; boxes padded
    // apart along the normal (columns 11, 14) hold one order
    for part in ["c4_a", "c4_b", "c5_a", "c5_b", "c13_a", "c13_b"] {
        assert!(area(Check::Order, part) > 1.0, "{part}");
    }
    for part in ["c11_", "c14_"] {
        assert_eq!(area(Check::Order, part), 0.0, "{part}");
    }
    // LODs: the decals over the rippled wall (column 10: LOD1 +-20 mm) and the bumps (column 7: LOD2 +-8 mm) break up;
    // the quarter pipe's LOD2 stays within 0.64 mm, under the bias at 150 m, where that LOD draws
    assert!(area(Check::Lod, "c10_ripple_decal") > 0.5);
    assert!(area(Check::Lod, "c7_bumps_decal") > 1.0);
    assert_eq!(area(Check::Lod, "c7_qp_decal"), 0.0);
    assert!(!report.passed(&limits));
    Ok(())
}
