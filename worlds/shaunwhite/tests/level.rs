//! Level 01 read whole (`installs.shaunwhite`): its cells' entities placed with their meshes, materials and textures,
//! and the material lowering run over it. The counts are printed; the checks are what must hold of any level read.
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Mesh, Texture, Transform};
use primase_shaunwhite::components::MaterialRef;
use primase_shaunwhite::passes::LowerShaunwhiteMaterial;
use primase_shaunwhite::read::level::read_level;
use primase_testdata::{Game, install};

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
    }
    let mut log = PassLog::default();
    LowerShaunwhiteMaterial.run(&mut w, &mut log).unwrap();
    assert_eq!(w.iter::<MaterialRef>().count(), 0);
    assert!(w.iter::<PbrMaterial>().count() > 500);
}
