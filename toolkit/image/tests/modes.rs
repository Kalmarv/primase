//! `set-material-modes` through the pass manager, on worlds of masked, blended and double-sided materials worked by
//! hand: what changes, what is reported by name and as an approximation, and the options.
#![allow(clippy::unwrap_used)]

use primase_core::ledger::Conservation;
use primase_core::pass::{Options, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{Entity, Name, World};
use primase_image::SET_MATERIAL_MODES;

fn run(w: &mut World, alpha: &str, double_sided: &str) -> Result<RunReport, String> {
    let options: Options = [("alpha", alpha), ("double-sided", double_sided)]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![SET_MATERIAL_MODES.instance(&options)?],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn material(w: &mut World, name: Option<&str>, alpha_mode: AlphaMode, double_sided: bool) -> Entity {
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            alpha_mode,
            double_sided,
            ..PbrMaterial::default()
        },
    );
    if let Some(n) = name {
        w.insert(m, Name(n.into()));
    }
    m
}

/// Four materials: a masked double-sided one, a blended single-sided one, an opaque double-sided one and an opaque
/// single-sided one.
fn world() -> (World, [Entity; 4]) {
    let mut w = World::new();
    let ms = [
        material(&mut w, Some("hair"), AlphaMode::Mask { cutoff: 0.5 }, true),
        material(&mut w, Some("glass"), AlphaMode::Blend, false),
        material(&mut w, Some("cloth"), AlphaMode::Opaque, true),
        material(&mut w, None, AlphaMode::Opaque, false),
    ];
    (w, ms)
}

fn modes(w: &World, ms: &[Entity; 4]) -> Vec<(AlphaMode, bool)> {
    ms.iter()
        .map(|&m| {
            let m = w.get::<PbrMaterial>(m).unwrap();
            (m.alpha_mode, m.double_sided)
        })
        .collect()
}

#[test]
fn opaque_and_single_sided_everywhere_with_each_change_reported() {
    let (mut w, ms) = world();
    let report = run(&mut w, "opaque", "false").unwrap();
    assert_eq!(modes(&w, &ms), [(AlphaMode::Opaque, false); 4]);
    let log = &report.steps[0].log;
    assert_eq!(log.counts["material hair: masked made opaque"], 1);
    assert_eq!(log.counts["material glass: blended made opaque"], 1);
    assert_eq!(log.counts["material hair: made single-sided"], 1);
    assert_eq!(log.counts["material cloth: made single-sided"], 1);
    // glass was single-sided and the unnamed material is opaque and single-sided already: nothing for them
    assert_eq!(log.counts.len(), 4, "{:?}", log.counts);
    assert_eq!(
        log.approximations["masked materials made opaque (cut-out texels drawn)"],
        1
    );
    assert_eq!(
        log.approximations["blended materials made opaque (drawn without translucency)"],
        1
    );
    assert_eq!(
        log.approximations["double-sided materials made single-sided (back faces not drawn)"],
        2
    );
}

#[test]
fn keep_leaves_a_mode_and_double_sided_true_sets_the_flag() {
    let (mut w, ms) = world();
    let report = run(&mut w, "keep", "true").unwrap();
    assert_eq!(
        modes(&w, &ms),
        [
            (AlphaMode::Mask { cutoff: 0.5 }, true),
            (AlphaMode::Blend, true),
            (AlphaMode::Opaque, true),
            (AlphaMode::Opaque, true),
        ]
    );
    let log = &report.steps[0].log;
    assert_eq!(log.counts["material glass: made double-sided"], 1);
    assert_eq!(log.counts[&format!("material {:?}: made double-sided", ms[3])], 1);
    assert_eq!(
        log.approximations["single-sided materials made double-sided (back faces drawn)"],
        2
    );

    let (mut w, ms) = world();
    let before = modes(&w, &ms);
    let report = run(&mut w, "keep", "keep").unwrap();
    assert_eq!(modes(&w, &ms), before);
    assert!(report.steps[0].log.counts.is_empty() && report.steps[0].log.approximations.is_empty());
}

#[test]
fn both_options_are_required_and_checked() {
    let make = |o: &[(&str, &str)]| {
        let o: Options = o
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        SET_MATERIAL_MODES.instance(&o)
    };
    assert!(make(&[("alpha", "opaque"), ("double-sided", "keep")]).is_ok());
    assert!(make(&[("double-sided", "keep")]).err().unwrap().contains("alpha"));
    assert!(make(&[("alpha", "keep")]).err().unwrap().contains("double-sided"));
    assert!(
        make(&[("alpha", "mask"), ("double-sided", "keep")])
            .err()
            .unwrap()
            .contains("alpha=mask")
    );
    assert!(
        make(&[("alpha", "keep"), ("double-sided", "yes")])
            .err()
            .unwrap()
            .contains("double-sided=yes")
    );
    assert!(
        make(&[("alpha", "keep"), ("double-sided", "keep"), ("x", "1")])
            .err()
            .unwrap()
            .contains("x")
    );
}
