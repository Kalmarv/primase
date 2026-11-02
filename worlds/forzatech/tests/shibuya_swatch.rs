//! The swatch tint over the whole Shibuya Scramble box of the Tokyo track (every prototype, as `shibuya_ground.rs`),
//! from the install the `GAME_DATA` configuration names: how many prototypes, primitives and material variants it
//! makes, and what `specialize-material-variants` then adds to the prepared map with the tint and without. Without
//! the install it skips.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    reason = "a test reading the files you point it at"
)]

mod inputs;
mod lowering;

use forzatech_formats::world::{Area, Season};
use inputs::{Game, install_file};
use primase_core::convert::Reader;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, Instance, LodChoice, Mesh, SelectLod, Semantic};
use primase_forzatech::components::{EmissionVariation, SwatchTint};
use primase_forzatech::passes::{ApplyForzaEmissionVariation, BakeForzaSwatchTint, SelectForzaMainView};
use primase_forzatech::read::convert::ForzaReader;
use primase_forzatech::read::open::Track;
use primase_forzatech::read::placements::Scope;
use primase_forzatech::read::textures::TextureScope;
use primase_image::{Beyond, CapMode, CapTextures, NormalDownscale};
use primase_meshops::{SpecializeMaterialVariants, UnusedVariants};
use std::collections::BTreeSet;

fn scope() -> Scope {
    Scope {
        area: Area::new(-1550.0, -5000.0, -1100.0, -13_700.0 / 3.0),
        season: Season::Summer,
        // every stored tier: the textures as the game stores them
        textures: TextureScope::All,
        terrain_textures: TextureScope::All,
    }
}

/// What the prepared map holds once the variants are specialized: the prototypes placed, and the triangles of their
/// meshes.
#[derive(Debug)]
struct Prepared {
    prototypes: usize,
    triangles: u64,
}

fn run(folder: &std::path::Path, bake: bool) -> Prepared {
    let track = Track::open(folder).unwrap();
    let mut reader = ForzaReader::new(track, scope());
    let (world, _) = reader.read().unwrap();
    let (mut part, _) = reader.load_all(world).unwrap();
    let mut log = PassLog::default();
    SelectLod {
        choice: LodChoice::AtMost(1),
    }
    .run(&mut part, &mut log)
    .unwrap();
    // what the main view does not draw is gone before materials are made of it
    SelectForzaMainView.run(&mut part, &mut log).unwrap();
    lowering::lower(&mut part, &mut log);
    if bake {
        for (what, n) in log
            .counts
            .iter()
            .chain(&log.approximations)
            .chain(&log.omitted)
            .filter(|(w, _)| w.contains("emi") || w.contains("uberlod"))
        {
            println!("lowering: {what}: {n}");
        }
    }
    // every material a primitive draws names its shader family (the undrawn Default.materialbin ones went with their
    // primitives)
    assert!(
        !log.omitted
            .contains_key("materials of shader family no shader family left unlowered"),
        "a drawn material has no shader family: {:?}",
        log.omitted
    );
    // the destination's cap, before the copies are baked
    CapTextures::new(
        256,
        CapMode {
            beyond: Beyond::Filter,
            normals: NormalDownscale::Box,
        },
    )
    .unwrap()
    .run(&mut part, &mut log)
    .unwrap();
    let mut log = PassLog::default();
    if bake {
        BakeForzaSwatchTint.run(&mut part, &mut log).unwrap();
        println!("bake counts: {:#?}", log.counts);
        println!("bake approximations: {:#?}", log.approximations);
        println!("bake omissions: {:#?}", log.omitted);
        assert_eq!(part.iter::<SwatchTint>().count(), 0);
        // the emission's instance variation, as material variants beside the tint's
        let mut log = PassLog::default();
        ApplyForzaEmissionVariation.run(&mut part, &mut log).unwrap();
        println!("emission variation counts: {:#?}", log.counts);
        println!("emission variation omissions: {:#?}", log.omitted);
        assert_eq!(part.iter::<EmissionVariation>().count(), 0);
    }
    for (_, mesh) in part.iter::<Mesh>() {
        assert!(
            mesh.primitives()
                .iter()
                .all(|p| !bake || !p.streams.contains_key(&Semantic::ShaderInput(0)))
        );
    }
    let mut log = PassLog::default();
    SpecializeMaterialVariants::new(UnusedVariants::Discard)
        .run(&mut part, &mut log)
        .unwrap();
    println!("specialize counts (bake {bake}): {:?}", log.counts);
    println!("specialize omissions (bake {bake}): {:?}", log.omitted);
    if bake {
        for (e, pbr) in part.iter::<PbrMaterial>() {
            pbr.verify(e, &part).unwrap();
        }
    }
    let placed: BTreeSet<Entity> = part.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    let mut triangles = 0u64;
    for &p in &placed {
        if let Some(mesh) = part.get::<Mesh>(p) {
            triangles += mesh
                .primitives()
                .iter()
                .map(|q| q.indices.as_ref().map_or(0, |i| i.len() as u64 / 3))
                .sum::<u64>();
        }
    }
    Prepared {
        prototypes: placed.len(),
        triangles,
    }
}

#[test]
fn shibuya_swatch() {
    let Some(folder) = install_file(Game::Fh6, "media/Tracks/Brio") else {
        return;
    };
    let without = run(&folder, false);
    let with = run(&folder, true);
    println!("prepared without the tint: {without:?}");
    println!("prepared with the tint: {with:?}");
    println!(
        "specialize adds for the tint: {} prototypes, {} triangles ({:.1}% of {})",
        with.prototypes - without.prototypes,
        with.triangles - without.triangles,
        100.0 * (with.triangles - without.triangles) as f64 / without.triangles as f64,
        without.triangles
    );
    assert!(with.prototypes >= without.prototypes);
}
