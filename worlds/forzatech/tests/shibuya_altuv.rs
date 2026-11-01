//! The `EmissiveAltUV` materials of the Shibuya Scramble box of the Tokyo track (x -1550..-1100, z -5000..-4566.67,
//! summer), over the whole box and not a sample, from the install the `GAME_DATA` configuration names: lowered
//! (`lower-forzatech-materials` keeps the emission map on `TEXCOORD4`), the textures capped to 256, drawn through
//! `TEXCOORD4` by `rebase-texcoord-set` and multiplied by `bake-forzatech-emission-product`. What it asserts: no mark is
//! left, no primitive is drawn with a slot on `TEXCOORD4`, and every `EmissiveAltUV` material drawn is accounted for
//! (rebased, folded into a factor, or left out by a counted reason); the counts it prints are the module documentation's.
//! Without the install it skips with a line naming its key.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test reading the files you point it at"
)]

mod inputs;

use forzatech_formats::world::{Area, Season};
use inputs::{Game, install_file};
use primase_core::convert::Reader;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, LodChoice, Mesh, SelectLod};
use primase_forzatech::components::EmissionByBaseColour;
use primase_forzatech::passes::{BakeForzaEmissionProduct, LowerForzaMaterials, SelectForzaMainView};
use primase_forzatech::read::convert::ForzaReader;
use primase_forzatech::read::open::Track;
use primase_forzatech::read::placements::Scope;
use primase_image::{Beyond, CapMode, CapTextures, NormalDownscale, RebaseTexcoordSet};
use std::collections::{BTreeMap, BTreeSet};

fn drawn(part: &primase_core::World) -> BTreeMap<Entity, u64> {
    let mut triangles: BTreeMap<Entity, u64> = BTreeMap::new();
    for (_, mesh) in part.iter::<Mesh>() {
        for p in mesh.primitives() {
            if let Some(m) = p.material {
                let corners = p.indices.as_ref().map_or_else(|| p.vertex_count(), Vec::len);
                *triangles.entry(m).or_default() += corners as u64 / 3;
            }
        }
    }
    triangles
}

#[test]
fn shibuya_altuv_emission() {
    let Some(folder) = install_file(Game::Fh6, "media/Tracks/Brio") else {
        return;
    };
    let scope = Scope {
        area: Area::new(-1550.0, -5000.0, -1100.0, -13_700.0 / 3.0),
        season: Season::Summer,
    };
    let mut reader = ForzaReader::new(Track::open(folder).unwrap(), scope);
    let (world, _) = reader.read().unwrap();
    let (mut part, _) = reader.load_all(world).unwrap();
    let mut log = PassLog::default();
    SelectLod {
        choice: LodChoice::AtMost(1),
    }
    .run(&mut part, &mut log)
    .unwrap();
    SelectForzaMainView.run(&mut part, &mut log).unwrap();
    LowerForzaMaterials.run(&mut part, &mut log).unwrap();

    let before = drawn(&part);
    let marked: BTreeSet<Entity> = part
        .iter::<EmissionByBaseColour>()
        .map(|(e, _)| e)
        .filter(|e| before.contains_key(e))
        .collect();
    let marked_triangles: u64 = marked.iter().map(|e| before[e]).sum();
    println!(
        "EmissiveAltUV materials drawn: {} materials, {marked_triangles} triangles",
        marked.len()
    );
    assert!(!marked.is_empty(), "the box draws EmissiveAltUV materials");

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
    let mut rebase = PassLog::default();
    RebaseTexcoordSet::new(4, 256)
        .unwrap()
        .run(&mut part, &mut rebase)
        .unwrap();
    let mut product = PassLog::default();
    BakeForzaEmissionProduct.run(&mut part, &mut product).unwrap();

    for (what, n) in rebase.counts.iter().chain(&product.counts) {
        println!("count: {what}: {n}");
    }
    for (what, n) in rebase.approximations.iter().chain(&product.approximations) {
        println!("approximation: {what}: {n}");
    }

    assert_eq!(part.iter::<EmissionByBaseColour>().count(), 0, "no mark is left");
    for (_, mesh) in part.iter::<Mesh>() {
        for p in mesh.primitives() {
            if let Some(pbr) = p.material.and_then(|m| part.get::<PbrMaterial>(m)) {
                assert!(
                    pbr.slots().all(|(_, r)| r.texcoord != 4),
                    "a primitive is drawn with a slot on TEXCOORD4"
                );
            }
        }
    }
    // every drawn material is accounted for: rebased or folded primitives, or left out for a counted reason
    let rebased = rebase.counts.get("primitives rebased").copied().unwrap_or(0);
    let folded = rebase
        .counts
        .get("primitives with their emission folded into a factor")
        .copied()
        .unwrap_or(0);
    let refused: u64 = rebase
        .counts
        .iter()
        .filter(|(what, _)| what.starts_with("primitives not rebased"))
        .map(|(_, n)| *n)
        .sum();
    let meshes_not_rebased: u64 = rebase
        .counts
        .iter()
        .filter(|(what, _)| what.starts_with("meshes not rebased"))
        .map(|(_, n)| *n)
        .sum();
    println!(
        "primitives: rebased {rebased}, folded {folded}, left as they are {refused}, meshes failing {meshes_not_rebased}"
    );
    assert!(rebased + folded > 0, "the pass draws something");
}
