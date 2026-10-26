//! Level 01 read whole (`installs.shaunwhite`): its cells' entities placed with their meshes, materials and textures,
//! its start and spawn markers read, the content selection run over it, and the material lowering after. The counts
//! are printed; the checks are what must hold of any level read.
use primase_core::frame::SourceFrame;
use primase_core::pass::{Conservation, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Marker, Mesh, Name, Texture, Transform, World};
use primase_shaunwhite::components::{MaterialRef, MeshFormat, PlacedBy};
use primase_shaunwhite::passes::{LowerShaunwhiteMaterial, register};
use primase_shaunwhite::read::level::read_level;
use primase_testdata::{Game, install};

/// Runs `text` with the manager's checks on: every component verifies after the pass, and the placements, primitives,
/// triangles and textures a pass drops are exactly the ones it declares.
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
fn level_01_reads_and_lowers() {
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    let shared: Vec<_> = ["DataPC_02.forge", "DataPC_04.forge"]
        .iter()
        .map(|f| folder.join(f))
        .collect();
    let (mut w, report) = read_level(&folder.join("DataPC_01.forge"), &shared).unwrap();
    for (who, what) in &report {
        println!("{who}: {what}");
    }
    let placements = w.iter::<Instance>().count();
    let meshes = w.iter::<Mesh>().count();
    let textures = w.iter::<Texture>().count();
    assert!(
        placements > 5000 && meshes > 1500 && textures > 500,
        "{placements} {meshes} {textures}"
    );
    for (e, i) in w.iter::<Instance>() {
        assert!(w.has::<Transform>(e) && w.has::<Mesh>(i.prototype));
        assert!(w.has::<PlacedBy>(e));
    }
    // every prototype states its vertex stride and flags
    assert!(w.iter::<Mesh>().all(|(e, _)| w.has::<MeshFormat>(e)));

    // markers: the start positions and spawn nodes, each at its Entity's matrix with its name
    let markers: Vec<(String, String)> = w
        .iter::<Marker>()
        .map(|(e, m)| (w.get::<Name>(e).unwrap().0.clone(), m.kind.clone()))
        .collect();
    let of = |kind: &str| markers.iter().filter(|(_, k)| k == kind).count();
    println!(
        "markers: {} start, {} spawn_point",
        of("start"),
        of("spawn_point")
    );
    assert!(of("start") > 20 && of("spawn_point") > 20, "{markers:?}");
    // every component of the world verifies, the markers among them (a transform, no ground position)
    let bad = w.verify_components();
    assert!(
        bad.is_empty(),
        "{} components fail their checks, first {:?}",
        bad.len(),
        &bad[..bad.len().min(5)]
    );
    // CH_GAME_StartPosition's matrix was read by hand from the Entity's bytes: translation (158.873, -64.894, -4.001)
    // in the source frame (x east, y north, z up), which core's frame turns to (x, z, -y)
    let (e, _) = w
        .iter::<Name>()
        .find(|(e, n)| n.0 == "CH_GAME_StartPosition" && w.has::<Marker>(*e))
        .expect("the game's start position");
    let t = w.get::<Transform>(e).unwrap();
    let want = SourceFrame::ENU.point([158.873, -64.894, -4.001]);
    assert!(
        t.cols[3].iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01),
        "{:?} against {want:?}",
        t.cols[3]
    );
    assert!(
        markers
            .iter()
            .any(|(n, k)| n == "Q01_Intro_FinalPlayerPos" && k == "start")
    );

    // the content selection: keeping everything changes nothing; dropping the classes leaves no placement a row names
    let all_keep = "select-shaunwhite-content{helpers=keep,effects=keep,characters=keep,missions=keep}";
    run(&mut w, all_keep).unwrap();
    assert_eq!(w.iter::<Instance>().count(), placements);
    let drop = run(
        &mut w,
        "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=keep}",
    )
    .unwrap();
    let log = &drop.steps[0].log;
    println!("select: {:?}", log.counts);
    for (k, n) in &log.drops {
        println!("select drop {k:?}: {n}");
    }
    let dropped = log.counts.get("placements dropped").copied().unwrap_or(0);
    assert!(dropped > 100, "{dropped} placements dropped");
    assert_eq!(w.iter::<Instance>().count() as u64, placements as u64 - dropped);
    for (e, i) in w.iter::<Instance>() {
        assert!(!w.get::<MeshFormat>(i.prototype).unwrap().skinned());
        let n = &w.get::<Name>(e).unwrap().0;
        assert!(!n.contains("OutOfBound") && !n.starts_with("FPP_Blocker"), "{n}");
    }
    assert_eq!(w.iter::<Marker>().count(), markers.len());
    let missions = run(
        &mut w,
        "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=drop}",
    )
    .unwrap();
    println!("missions: {:?}", missions.steps[0].log.counts);
    assert!(w.iter::<PlacedBy>().all(|(_, p)| p.cell.starts_with("Cell")));
    assert!(w.iter::<PlacedBy>().count() > 3000);

    let mut log = PassLog::default();
    LowerShaunwhiteMaterial.run(&mut w, &mut log).unwrap();
    assert_eq!(w.iter::<MaterialRef>().count(), 0);
    assert!(w.iter::<PbrMaterial>().count() > 500);
}
