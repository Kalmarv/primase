//! Skate guidance: a SWSkateGuidanceSystem built byte by byte read back with its chains, and level 01's guidance read
//! whole (`installs.shaunwhite`) and lowered to grind kinds.
use primase_core::pass::{Conservation, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Curve, World};
use primase_shaunwhite::components::Guidance;
use primase_shaunwhite::passes::register;
use primase_shaunwhite::read::guidance::{
    GUIDANCE_OBJECT, GuidanceSystem, NO_LINK, SW_GUIDANCE_OBJECT_DATA, SW_GUIDANCE_SYSTEM,
};
use primase_shaunwhite::read::level::read_level;
use primase_shaunwhite::read::scene::RESOURCE_REF;
use primase_testdata::{Game, install};

fn u32s(out: &mut Vec<u8>, v: &[u32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}
fn f32s(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// An edge: (start, end) points, (start link, end link), length.
type BuiltEdge = ((u32, u32), (u32, u32), f32);

/// A system of `edges` over `points` (encoded as the game encodes them).
fn system(edges: &[BuiltEdge], points: &[[f32; 3]]) -> Vec<u8> {
    let mut b = vec![0u8, 4];
    u32s(&mut b, &[0x1234, SW_GUIDANCE_SYSTEM, 0x1235, RESOURCE_REF]);
    b.push(0xff);
    u32s(&mut b, &[edges.len() as u32]);
    for ((i0, i1), (start, end), length) in edges {
        b.extend_from_slice(&[0, 0, 0, 0]);
        u32s(&mut b, &[GUIDANCE_OBJECT]);
        b.push(1);
        u32s(&mut b, &[0, *i0, *i1]);
        f32s(&mut b, &[0., 0., 1.]);
        u32s(&mut b, &[0]);
        f32s(&mut b, &[0., -1., 0.]);
        u32s(&mut b, &[0]);
        b.push(0);
        u32s(&mut b, &[0x2000, SW_GUIDANCE_OBJECT_DATA, 0, *end, *start]);
        f32s(&mut b, &[*length]);
    }
    u32s(&mut b, &[(points.len() * 3) as u32]);
    for p in points {
        for v in p {
            b.extend_from_slice(&((v * 200.).round() as i32 + 32767).to_le_bytes()[..2]);
        }
    }
    b
}

#[test]
fn a_built_system_reads_back_as_chains() {
    let points = [
        [0., 0., 0.4],
        [2., 0., 0.4],
        [4., 0., 0.4],
        [4., 3., 0.4],
        [-1.5, 7.25, 1.],
    ];
    // edges 0-1-2 chained (edge 1 runs backwards, 2 -> 1), edge 2 continues to point 3; edge 3 alone
    let edges = [
        ((0, 1), (NO_LINK, 1), 2.),
        ((2, 1), (2, 0), 2.),
        ((2, 3), (1, NO_LINK), 3.),
        ((4, 0), (NO_LINK, NO_LINK), 9.),
    ];
    let b = system(&edges, &points);
    let s = GuidanceSystem::parse(&b, 6).unwrap();
    assert_eq!(s.points, points.to_vec());
    assert_eq!(s.edges.len(), 4);
    assert_eq!(s.edges[1].links, [2, 0]);
    assert_eq!(s.edges[0].normals, [[0., 0., 1.], [0., -1., 0.]]);
    let chains = s.chains();
    let runs: Vec<(Vec<u32>, bool)> = chains.iter().map(|c| (c.points.clone(), c.closed)).collect();
    assert_eq!(runs, vec![(vec![0, 1, 2, 3], false), (vec![4, 0], false)]);

    // a link to an edge that does not hold the point it joins at is refused
    let bad = system(
        &[((0, 1), (NO_LINK, 1), 2.), ((2, 3), (NO_LINK, NO_LINK), 3.)],
        &points,
    );
    assert!(GuidanceSystem::parse(&bad, 6).is_err());
    // so is an index past the points
    let past = system(&[((0, 9), (NO_LINK, NO_LINK), 2.)], &points);
    assert!(GuidanceSystem::parse(&past, 6).is_err());
}

#[test]
fn a_loop_reads_closed() {
    let points = [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.]];
    let edges = [((0, 1), (2, 1), 1.), ((1, 2), (0, 2), 1.), ((2, 0), (1, 0), 1.4)];
    let s = GuidanceSystem::parse(&system(&edges, &points), 6).unwrap();
    let chains = s.chains();
    assert_eq!(chains.len(), 1);
    assert_eq!(
        (chains[0].points.clone(), chains[0].closed),
        (vec![0, 1, 2], true)
    );
}

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
    assert!(
        !report
            .iter()
            .any(|(_, t)| t.starts_with("guidance systems not read")),
        "{report:?}"
    );
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
    assert_eq!(edges, 6118);
    assert!(agree * 100 >= edges * 95, "{agree} of {edges}");

    run(&mut w, "lower-shaunwhite-grinds").unwrap();
    assert_eq!(w.iter::<Guidance>().count(), 0);
    let count = |k: &str| w.iter::<Curve>().filter(|(_, c)| c.kind == k).count();
    let (rail, ledge, coping) = (count("rail"), count("ledge"), count("coping"));
    println!("curves: rail {rail}, ledge {ledge}, coping {coping}");
    assert!(rail > 50 && ledge > 200 && coping > 10, "{rail} {ledge} {coping}");
}
