//! One `ShredReader` over MIKE with every input the caller can give: the level's meshes, the `.hkw` collision, the
//! source materials and the texture copies they bind. The read world holds the geometry, the material graph and the
//! collision with no texture pixels; `load` decodes the pixels of the part the roots reach; `load_all` decodes the
//! whole world in place.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use primase_core::{
    Collider, Collision, Instance, Mesh, Surfaces, Texture,
    convert::{self, Reader},
    frame::SourceFrame,
};
use primase_shred::{
    components::{DynamicShape, MaterialId, MaterialTextures, RigidBody, ShredMaterial},
    read::{
        collision::SurfaceTable,
        convert::ShredReader,
        material_join::{MaterialInputs, TexturePreference},
        pak::Pak,
    },
};
use primase_testdata::{Game, install};
use std::collections::BTreeSet;

#[test]
fn one_reader_reads_mike_with_meshes_collision_materials_and_textures() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let usrdir = root.join("PS3_GAME/USRDIR");
    let table = SurfaceTable::from_main_db(&std::fs::read(usrdir.join("database/main.db")).unwrap()).unwrap();
    let banks = Pak::open(usrdir.join("database/materials.pak")).unwrap();
    let pak = Pak::open(usrdir.join("levels/mike.pak")).unwrap();
    let mut reader = ShredReader::new(vec![("MIKE", &pak)], SourceFrame::WORLD)
        .with_collision(table)
        .with_materials(MaterialInputs {
            banks: &banks,
            assets: vec![],
            textures: TexturePreference::FullChainThenPrefetch,
        });
    let (world, lines) = reader.read().unwrap();
    convert::check_emits(&world, reader.emits(), "read").unwrap();
    assert!(world.verify_components().is_empty());
    for kind in ["shred geometry", "shred collision", "shred materials"] {
        assert!(lines.iter().any(|(k, _)| k == kind), "{kind}");
    }
    // geometry and the material graph are in the read world, the pixels are not
    assert_eq!(world.column::<Mesh>().map_or(0, |c| c.len()), reader.report.rigid);
    assert!(world.entities_named("shred.model_dependencies").is_empty());
    assert_eq!(
        world.iter::<ShredMaterial>().count(),
        world.iter::<MaterialId>().count()
    );
    assert!(reader.material_report.unresolved_materials.is_empty());
    assert!(world.iter::<MaterialTextures>().next().is_some());
    assert!(world.iter::<Texture>().next().is_none());
    // collision: one file, every leaf an entity
    assert_eq!(reader.collision.files.len(), 1);
    let soups = world.column::<Collision>().map_or(0, |c| c.len());
    let colliders = world.column::<Collider>().map_or(0, |c| c.len());
    let dynamic = world.column::<DynamicShape>().map_or(0, |c| c.len());
    assert_eq!(soups + colliders + dynamic, reader.collision.leaves);
    let origins = world
        .resource::<primase_shred::components::CollisionSurfaces>()
        .unwrap();
    assert!(
        world
            .resource::<Surfaces>()
            .is_some_and(|s| s.0.len() == origins.0.len() && s.0.len() >= 54)
    );

    // a selection of 20 placements and the collision loads with its pixels decoded
    let roots: BTreeSet<_> = world
        .iter::<Instance>()
        .take(20)
        .map(|(e, _)| e)
        .chain(world.iter::<RigidBody>().map(|(e, _)| e))
        .collect();
    let (part, load_lines) = reader.load(&world, &roots).unwrap();
    convert::check_load_emits(&world, &part, reader.emits()).unwrap();
    assert!(part.verify_components().is_empty());
    assert!(load_lines.iter().any(|(k, _)| k == "shred texture load"));
    assert!(part.iter::<Mesh>().next().is_some());
    assert!(part.iter::<ShredMaterial>().next().is_some());
    assert!(part.iter::<Texture>().next().is_some());
    assert_eq!(part.column::<Collision>().map_or(0, |c| c.len()), soups);
    assert!(part.resource::<Surfaces>().is_some());
    // every decoded texture belongs to a copy the part holds (nothing outside the roots' closure was decoded)
    assert!(part.iter::<Texture>().count() <= reader.material_report.copies * 6);

    // the whole world, in place
    let entities = world.entities().count();
    let (all, _) = reader.load_all(world).unwrap();
    assert_eq!(all.entities().count(), entities);
    assert!(all.iter::<Texture>().count() >= part.iter::<Texture>().count());
    assert!(all.verify_components().is_empty());
    eprintln!(
        "SHRED reader MIKE: {} meshes, {} collision leaves, {} materials; part {} textures, whole {} textures",
        all.column::<Mesh>().map_or(0, |c| c.len()),
        reader.collision.leaves,
        all.iter::<ShredMaterial>().count(),
        part.iter::<Texture>().count(),
        all.iter::<Texture>().count()
    );

    // without the caller's inputs the reader reads the geometry alone
    let mut bare = ShredReader::new(vec![("MIKE", &pak)], SourceFrame::WORLD);
    let (bare_world, bare_lines) = bare.read().unwrap();
    assert!(bare_world.column::<Collision>().is_none_or(|c| c.is_empty()));
    assert!(bare_world.iter::<ShredMaterial>().next().is_none());
    assert!(
        bare_lines
            .iter()
            .any(|(_, v)| v.contains("material inputs not supplied"))
    );
}
