//! Skate guidance: level 01's guidance read whole (`installs.shaunwhite`) and lowered to grind kinds.
mod inputs;

use anvil_formats::shaunwhite::guidance::Class;
use inputs::{Game, install};
use primase_core::pass::{Conservation, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Curve, World};
use primase_shaunwhite::components::Guidance;
use primase_shaunwhite::passes::register;
use primase_shaunwhite::read::level::read_level;

fn run(w: &mut World, text: &str) -> Result<RunReport, String> {
    let mut reg = PassRegistry::new();
    register(&mut reg);
    let pipeline: Pipeline = reg.parse(text).map_err(|e| e.to_string())?;
    let mut report = RunReport::default();
    PassManager::new(reg)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(w, &pipeline, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report)
}

#[test]
fn level_01_guidance_reads_and_lowers() {
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    let (mut w, report) = read_level(&folder.join("DataPC_01.forge"), &[]).unwrap();
    for (who, what) in report.iter().filter(|(_, t)| t.contains("guidance")) {
        println!("{who}: {what}");
    }
    // every system of the four classes reads, and ends where the next component starts
    for bad in ["guidance systems not read", "guidance systems not followed"] {
        assert!(
            !report.iter().any(|(_, t)| t.starts_with(bad)),
            "{bad}: {report:?}"
        );
    }
    let systems = |class: Class| -> usize {
        let line = format!("guidance systems: {}: ", class.name());
        report
            .iter()
            .find_map(|(_, t)| t.strip_prefix(&line).and_then(|n| n.parse().ok()))
            .unwrap_or(0)
    };
    let by_class = Class::ALL.map(systems);
    println!("systems by class (plain, free-form, scale, morph): {by_class:?}");
    assert_eq!(by_class, [714, 95, 25, 5]);
    // every edge: its world length (rigid placements) is its stored length
    let (mut edges, mut agree) = (0usize, 0usize);
    for (e, g) in w.iter::<Guidance>() {
        let c = w.get::<Curve>(e).unwrap();
        let mut pts = c.points.clone();
        if c.closed {
            pts.push(pts[0]);
        }
        assert_eq!(pts.len(), g.lengths.len() + 1, "{}", c.kind);
        for (k, l) in g.lengths.iter().enumerate() {
            let d = (0..3)
                .map(|j| (pts[k + 1][j] - pts[k][j]).powi(2))
                .sum::<f32>()
                .sqrt();
            edges += 1;
            agree += usize::from((d - l).abs() < 0.02);
        }
    }
    println!("guidance edges: {edges}, world length = stored length: {agree}");
    assert_eq!(edges, 6993);
    assert_eq!(agree, edges);

    run(&mut w, "lower-shaunwhite-grinds").unwrap();
    assert_eq!(w.iter::<Guidance>().count(), 0);
    let count = |k: &str| w.iter::<Curve>().filter(|(_, c)| c.kind == k).count();
    let (rail, ledge, coping) = (count("rail"), count("ledge"), count("coping"));
    println!("curves: rail {rail}, ledge {ledge}, coping {coping}");
    assert!(rail > 50 && ledge > 200 && coping > 10, "{rail} {ledge} {coping}");
}
