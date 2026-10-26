//! Level 01 read whole (`installs.shaunwhite`): its cells' entities placed with their meshes, materials and textures,
//! its start and spawn markers read, the content selection run over it, and the material and collision lowerings. The counts
//! are printed; the checks are what must hold of any level read.
use primase_core::frame::SourceFrame;
use primase_core::pass::{Conservation, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading};
use primase_core::{
    Collider, Collision, Environment, Instance, Light, LightKind, Marker, MaterialUsage, Mesh, Morphs, Name,
    Surfaces, Texture, Transform, VertexColor, World,
};
use primase_shaunwhite::components::{
    CollisionPlacedBy, CollisionSource, CollisionSurfaces, LightShape, LightSource, MaterialRef, MeshFormat,
    PlacedBy, Sampler, SkySettings,
};
use primase_shaunwhite::passes::register;
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
    // every Mesh of level 01 reads: the further vertex streams are morph targets, NaN texture coordinates are zeroed
    assert!(
        !report.iter().any(|(_, t)| t.starts_with("meshes not read")),
        "{report:?}"
    );
    // the classes a Visual shows that draw nothing are named, not just counted by hash (placements of each in level 01)
    for (class, why, n) in [
        ("0xc3120087", "free-form shape hull", 140),
        ("0xcaaa7ee8", "particle system", 290),
        ("0xf4bbbb9e", "dynamic mesh", 20),
    ] {
        let want = format!("visuals showing class {class}, a {why}");
        let line = report.iter().find(|(_, t)| t.starts_with(&want));
        assert_eq!(
            line.and_then(|(_, t)| t.rsplit(": ").next()?.parse::<usize>().ok()),
            Some(n),
            "{want}"
        );
    }
    let morphs: Vec<usize> = w.iter::<Morphs>().map(|(_, m)| m.targets.len()).collect();
    println!(
        "morphs: {} meshes, {} targets",
        morphs.len(),
        morphs.iter().sum::<usize>()
    );
    assert_eq!((morphs.len(), morphs.iter().sum::<usize>()), (26, 30));
    let placements = w.iter::<Instance>().count();
    let points = w
        .iter::<LightSource>()
        .filter(|(_, l)| matches!(l.shape, LightShape::Point { .. }))
        .count();
    let spots = w
        .iter::<LightSource>()
        .filter(|(_, l)| matches!(l.shape, LightShape::Spot { .. }))
        .count();
    assert_eq!((points, spots, w.iter::<SkySettings>().count()), (110, 79, 1));
    let sky = w.iter::<SkySettings>().next().unwrap().1.clone();
    // Root entry's native SkyLight fields were separately inspected: source vector and AmbientColor retained.
    assert_eq!(sky.direction, [0.46, 0.27, 0.85]);
    assert_eq!(sky.sun_colour, 0xFFF9_F1DE);
    run(
        &mut w,
        "lower-shaunwhite-lighting{point-scale=100,sun-scale=1,spot-axis=y,sun-vector=to-sun,colour=linear}",
    )
    .unwrap();
    let env = w.resource::<Environment>().unwrap();
    let sun = w.get::<Light>(env.sun.unwrap()).unwrap();
    assert_eq!(sun.kind, LightKind::Directional);
    assert_eq!(sun.intensity, 2.5);
    assert!(env.ambient.is_none());
    println!(
        "lights: {points} omni, {spots} spot, {} core lights, one named sun",
        w.iter::<Light>().count()
    );
    let meshes = w.iter::<Mesh>().count();
    let textures = w.iter::<Texture>().count();
    assert!(
        placements > 5000 && meshes > 1500 && textures > 500,
        "{placements} {meshes} {textures}"
    );
    for (e, i) in w.iter::<Instance>() {
        assert!(w.has::<Transform>(e));
        assert!(w.has::<Mesh>(i.prototype) || w.has::<CollisionSource>(i.prototype));
        assert!(w.has::<PlacedBy>(e) || w.has::<CollisionPlacedBy>(e));
    }
    // every compiled mesh states its vertex stride and flags; the three trees' meshes are made by the reader
    let formats = w.iter::<MeshFormat>().count();
    assert!(
        w.iter::<MeshFormat>().all(|(e, _)| w.has::<Mesh>(e)) && formats + 3 == meshes,
        "{formats} of {meshes}"
    );

    // every material states its fields (a body of another length is the exception), every slot its texture and the
    // texture how it is sampled
    let materials: Vec<&MaterialRef> = w.iter::<MaterialRef>().map(|(_, m)| m).collect();
    let stated = materials.iter().filter(|m| m.state.is_some()).count();
    let blended = materials
        .iter()
        .filter(|m| m.state.is_some_and(|s| s.blended))
        .count();
    let tested = materials
        .iter()
        .filter(|m| m.state.is_some_and(|s| s.alpha_test))
        .count();
    println!(
        "materials: {} read, {stated} with fields, {blended} blended, {tested} alpha-tested, {} with a layer, {} unread slots",
        materials.len(),
        materials
            .iter()
            .filter(|m| m.slots.contains_key("diffuse_b"))
            .count(),
        materials.iter().map(|m| m.unread.len()).sum::<usize>()
    );
    assert!(stated + 2 >= materials.len() && blended > 20 && tested > 20);
    for m in &materials {
        for s in m.slots.values() {
            assert!(
                w.has::<Sampler>(s.texture),
                "{}: a texture without its sampler",
                m.name
            );
            assert!(s.scale.iter().chain(&s.offset).all(|x| x.is_finite()));
        }
    }

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

    let collision_placements = w.iter::<CollisionPlacedBy>().count();
    assert!(collision_placements > 2000, "{collision_placements}");
    run(&mut w, "lower-shaunwhite-collision").unwrap();
    assert_eq!(w.iter::<CollisionSource>().count(), 0);
    assert!(w.resource::<CollisionSurfaces>().is_none());
    let surfaces = w.resource::<Surfaces>().unwrap();
    for name in ["Concrete", "ConcreteVert", "ConcreteNonRidable", "WoodDeck"] {
        assert!(surfaces.0.iter().any(|s| s.name == name), "{name}");
    }
    // Unresolved source zero is preserved by name, not assigned a guessed destination material.
    assert!(
        surfaces
            .0
            .iter()
            .any(|s| s.name == "source:0x00000000" && s.class.is_none())
    );
    let mut placed_triangles = 0;
    let mut multi_children = 0;
    for (e, source) in w.iter::<CollisionPlacedBy>() {
        let instance = w.get::<Instance>(e).unwrap();
        let mesh = w.get::<Collision>(instance.prototype);
        assert!(mesh.is_some() || w.has::<Collider>(instance.prototype));
        if let Some(mesh) = mesh {
            placed_triangles += mesh.triangles.len();
        }
        // These two independently inspected MultiInert children share a shape prototype, but keep their own poses.
        if [0x3BB4_9511, 0x3BB4_9512].contains(&source.entity) {
            assert_eq!(source.shape, 0xB310_25C9);
            // Translation floats read separately from the source bytes in Cell01439. The containing MultiInert
            // is at the origin, so these also distinguish child poses from an incorrect container pose.
            let bits = if source.entity == 0x3BB4_9511 {
                [0x41F7_F520, 0x4071_169E, 0x4237_FBB0]
            } else {
                [0x41F7_F516, 0x4071_5DB2, 0x424F_DD33]
            };
            assert_eq!(w.get::<Transform>(e).unwrap().cols[3].map(f32::to_bits), bits);
            multi_children += 1;
        }
    }
    assert_eq!(multi_children, 2);
    assert!(placed_triangles > 100_000, "{placed_triangles}");
    println!(
        "collision: {collision_placements} placements, {placed_triangles} placed triangles, {} source surface names",
        surfaces.0.len()
    );

    // the content selection: keeping everything changes nothing; dropping the classes leaves no placement a row names
    let all_keep = "select-shaunwhite-content{helpers=keep,effects=keep,characters=keep,missions=keep,influence=restored}";
    let kept = run(&mut w, all_keep).unwrap();
    // restored keeps both Visuals of the 36 tree Entities: nothing goes
    let log = &kept.steps[0].log;
    assert_eq!(
        log.counts["tree entities with a RealTreeMesh and a stored Mesh"],
        36
    );
    assert_eq!(log.counts["tree entities with both Visuals kept (restored)"], 36);
    assert!(!log.counts.contains_key("placements dropped"));
    assert_eq!(w.iter::<Instance>().count(), placements);
    let drop = run(
        &mut w,
        "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=keep,influence=restored}",
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
        if let Some(format) = w.get::<MeshFormat>(i.prototype) {
            assert!(!format.skinned());
            let n = &w.get::<Name>(e).unwrap().0;
            assert!(!n.contains("OutOfBound") && !n.starts_with("FPP_Blocker"), "{n}");
        } else {
            // a tree (a Mesh the reader made, with no vertex format) or a collision placement
            assert!(w.has::<CollisionPlacedBy>(e) || w.has::<Mesh>(i.prototype));
        }
    }
    assert_eq!(w.iter::<Marker>().count(), markers.len());
    let missions = run(
        &mut w,
        "select-shaunwhite-content{helpers=drop,effects=drop,characters=drop,missions=drop,influence=restored}",
    )
    .unwrap();
    println!("missions: {:?}", missions.steps[0].log.counts);
    assert!(w.iter::<PlacedBy>().all(|(_, p)| p.cell.starts_with("Cell")));
    assert!(w.iter::<PlacedBy>().count() > 3000);

    // Draw-content selection preserves authored collision and its prototypes, including mission/helper names.
    assert_eq!(w.iter::<CollisionPlacedBy>().count(), collision_placements);
    for (e, _) in w.iter::<CollisionPlacedBy>() {
        let p = w.get::<Instance>(e).unwrap().prototype;
        assert!(w.has::<Collision>(p) || w.has::<Collider>(p));
    }
    let harbour = w
        .iter::<MaterialRef>()
        .find(|(_, m)| m.name == "Gen_GFX_WaterPort_Mat")
        .map(|(e, m)| (e, m.clone()))
        .expect("harbour water material");
    assert!(harbour.1.state.unwrap().water);
    assert!(!harbour.1.slots.contains_key("diffuse"));
    assert!(harbour.1.slots.contains_key("normal") && harbour.1.slots.contains_key("specular"));
    let water_meshes: Vec<_> = w
        .iter::<Mesh>()
        .filter(|(_, m)| m.primitives().iter().any(|p| p.material == Some(harbour.0)))
        .map(|(e, m)| (e, m.clone()))
        .collect();
    assert_eq!(water_meshes.len(), 1);
    let water_placements: Vec<_> = w
        .iter::<Instance>()
        .filter(|(_, i)| i.prototype == water_meshes[0].0)
        .map(|(e, _)| (e, *w.get::<Transform>(e).unwrap()))
        .collect();
    assert_eq!(water_placements.len(), 1);
    // Height read separately from the Cell05460 harbour Entity: source Z=-10.999994, core Y unchanged.
    assert!((water_placements[0].1.cols[3][1] + 10.999_994).abs() < 0.000_01);
    run(&mut w, "lower-shaunwhite-water{roughness=0.05}").unwrap();
    assert_eq!(w.get::<MaterialUsage>(harbour.0), Some(&MaterialUsage::Water));
    assert_eq!(
        w.get::<PbrMaterial>(harbour.0).unwrap().base_color,
        harbour.1.state.unwrap().color
    );
    for (e, m) in &water_meshes {
        assert_eq!(w.get::<Mesh>(*e), Some(m));
    }
    for (e, t) in &water_placements {
        assert_eq!(w.get::<Transform>(*e), Some(t));
    }
    run(&mut w, "lower-shaunwhite-layers{influence=restored}").unwrap();
    let lowered = run(&mut w, "lower-shaunwhite-material").unwrap();
    assert_eq!(w.get::<MaterialUsage>(harbour.0), Some(&MaterialUsage::Water));
    assert_eq!(w.get::<PbrMaterial>(harbour.0).unwrap().roughness, 0.05);
    assert_eq!(w.iter::<MaterialRef>().count(), 0);
    assert!(w.iter::<PbrMaterial>().count() > 500);
    // the lowered materials' alpha modes and tiling follow the stored fields; every one verifies
    let pbr: Vec<&PbrMaterial> = w.iter::<PbrMaterial>().map(|(_, p)| p).collect();
    let mode = |f: fn(&AlphaMode) -> bool| pbr.iter().filter(|p| f(&p.alpha_mode)).count();
    // tiling lives on the material's base colour, or on the core layer of a blend's layer (its material untiled)
    let tiled = pbr
        .iter()
        .filter(|p| p.base_color_texture.is_some_and(|t| t.transform.is_some()))
        .count()
        + w.iter::<primase_core::LayeredMaterial>()
            .flat_map(|(_, l)| l.layers.iter().flatten())
            .filter(|l| l.transform.is_some())
            .count();
    println!(
        "lowered: {} materials, {} blend, {} mask, {} unlit, {tiled} with a UV transform; {:?}",
        pbr.len(),
        mode(|m| matches!(m, AlphaMode::Blend)),
        mode(|m| matches!(m, AlphaMode::Mask { .. })),
        pbr.iter().filter(|p| p.shading == Shading::Unlit).count(),
        lowered.steps[0].log.counts
    );
    assert!(
        mode(|m| matches!(m, AlphaMode::Blend)) > 20 && mode(|m| matches!(m, AlphaMode::Mask { .. })) > 20
    );
    assert!(tiled > 10, "{tiled} tiled");
    let emissive = pbr
        .iter()
        .filter(|p| p.emissive_strength != 1.0 || p.emissive_texture.is_some())
        .count();
    let specular = pbr
        .iter()
        .filter(|p| p.specular.is_some_and(|s| s.color_texture.is_some()))
        .count();
    println!(
        "lowered: {emissive} emissive, {specular} with a specular map, roughness {:.2} to {:.2}",
        pbr.iter().map(|p| p.roughness).fold(1.0, f32::min),
        pbr.iter().map(|p| p.roughness).fold(0.0, f32::max)
    );
    assert!(emissive > 5 && specular > 100);
    let ignored = w.iter::<VertexColor>().count();
    println!("lowered: {ignored} materials ignore their vertex colour");
    assert!(ignored > 50);
    assert!(w.verify_components().is_empty());
}
