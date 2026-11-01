//! `lower-forzatech-materials` over the Shibuya Scramble box of the Tokyo track (x -1550..-1100, z -5000..-4566.67,
//! summer), on every 20th prototype as `families.rs` samples it, from the install the `GAME_DATA` configuration names:
//! how many materials and triangles each family's rule lowers, what it leaves out by name, and what the packed
//! occlusion-roughness-metallic maps hold. Without the install it skips with a line naming its key.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    reason = "a test reading the files you point it at"
)]

mod inputs;

use forzatech_formats::world::{Area, Season};
use inputs::{Game, install_file};
use primase_core::Component;
use primase_core::convert::Reader;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{ColorSpace, Entity, Instance, Mesh, TexFormat, TexRole, Texture};
use primase_forzatech::components::{ForzaMaterial, MaterialTextures, Value};
use primase_forzatech::passes::LowerForzaMaterials;
use primase_forzatech::read::convert::ForzaReader;
use primase_forzatech::read::open::Track;
use primase_forzatech::read::placements::Scope;
use std::collections::{BTreeMap, BTreeSet};

fn scope() -> Scope {
    Scope {
        area: Area::new(-1550.0, -5000.0, -1100.0, -13_700.0 / 3.0),
        season: Season::Summer,
    }
}

#[test]
fn shibuya_materials() {
    let Some(folder) = install_file(Game::Fh6, "media/Tracks/Brio") else {
        return;
    };
    let track = Track::open(folder).unwrap();
    let mut reader = ForzaReader::new(track, scope());
    let (world, _) = reader.read().unwrap();
    let prototypes: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    let sample: BTreeSet<Entity> = prototypes.iter().copied().step_by(20).collect();
    let (mut part, _) = reader.load(&world, &sample).unwrap();

    let mut triangles: BTreeMap<Entity, u64> = BTreeMap::new();
    for (_, mesh) in part.iter::<Mesh>() {
        for p in mesh.primitives() {
            if let Some(m) = p.material {
                *triangles.entry(m).or_default() += p.indices.as_ref().map_or(0, |i| i.len() as u64 / 3);
            }
        }
    }
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut part, &mut log).unwrap();

    // triangles by family, lowered or not
    let mut lowered: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut unlowered: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (e, m) in part.iter::<ForzaMaterial>() {
        let family = m
            .shader
            .as_ref()
            .map_or("no shader family", |s| s.family.as_str());
        let t = triangles.get(&e).copied().unwrap_or(0);
        let map = if part.has::<PbrMaterial>(e) {
            &mut lowered
        } else {
            &mut unlowered
        };
        let entry = map.entry(family.to_owned()).or_default();
        entry.0 += 1;
        entry.1 += t;
    }
    println!("lowered (materials, triangles): {lowered:?}");
    println!("unlowered (materials, triangles): {unlowered:?}");
    println!("counts: {:#?}", log.counts);
    println!("approximations: {:#?}", log.approximations);
    println!("omitted: {:#?}", log.omitted);

    // the four families with a rule are lowered whole, bar the foliage billboard and impostor
    let summary = |m: &BTreeMap<String, (u64, u64)>| -> Vec<(String, u64, u64)> {
        m.iter().map(|(f, (n, t))| (f.clone(), *n, *t)).collect()
    };
    assert_eq!(
        summary(&lowered),
        [
            ("environment".to_owned(), 758, 55_464),
            ("projection_lod".to_owned(), 59, 10_002),
            ("uber_branch".to_owned(), 6, 8_213),
            ("uber_foliage".to_owned(), 16, 13_670),
        ]
    );
    assert_eq!(unlowered["uber_foliage"], (2, 4));
    for (family, (n, _)) in &lowered {
        assert_eq!(log.counts[&format!("{family} materials lowered")], *n);
    }

    // every lowered material holds the ranges and references `pbr.material` documents
    for (e, pbr) in part.iter::<PbrMaterial>() {
        pbr.verify(e, &part).unwrap();
    }

    // the environment's packed maps: R occlusion, G roughness, B metallic, one map in both slots
    let mut metal = (0u64, 0u64);
    let mut maps = 0;
    for (e, pbr) in part.iter::<PbrMaterial>() {
        if part
            .get::<ForzaMaterial>(e)
            .and_then(|m| m.shader.as_ref())
            .map(|s| s.family.as_str())
            != Some("environment")
        {
            continue;
        }
        let Some(r) = pbr.metallic_roughness_texture else {
            continue;
        };
        assert_eq!(pbr.occlusion_texture, Some(r), "{e:?}");
        let t = part.get::<Texture>(r.texture).unwrap();
        assert_eq!((t.role, t.color_space), (TexRole::Mask, ColorSpace::Linear));
        if t.levels.len() == 1 || maps % 25 == 0 {
            let px = &t.levels[0];
            let extreme = px.chunks(4).filter(|p| p[2] < 16 || p[2] > 239).count() as u64;
            metal.0 += extreme;
            metal.1 += (px.len() / 4) as u64;
        }
        maps += 1;
    }
    // projection_lod's signed roughness-metalness maps: how much of the negative half they use (a decoded byte below 120
    // is a value below -0.06)
    let (mut negative, mut channels, mut signed_maps) = (0u64, 0u64, BTreeSet::new());
    for (e, m) in part.iter::<ForzaMaterial>() {
        if m.shader.as_ref().map(|s| s.family.as_str()) != Some("projection_lod") {
            continue;
        }
        let hash = m
            .resolved()
            .into_iter()
            .find_map(|(_, p)| match (&p.name, &p.value) {
                (Some(n), Value::Texture { path_hash, .. }) if n == "RoughnessMetalnessATexture" => {
                    Some(*path_hash)
                }
                _ => None,
            });
        let texture = part
            .get::<MaterialTextures>(e)
            .and_then(|b| b.0.iter().find(|t| Some(t.path_hash) == hash))
            .and_then(|t| t.texture);
        let Some(texture) = texture else { continue };
        let t = part.get::<Texture>(texture).unwrap();
        if t.format == TexFormat::Bc5Snorm && signed_maps.insert(texture) {
            let image = primase_image::texture_level_rgba8(t, 0).unwrap();
            let primase_image::Samples::U8(px) = &image.samples else {
                panic!("not 8-bit")
            };
            for texel in px.chunks(4) {
                channels += 2;
                negative += u64::from(texel[0] < 120) + u64::from(texel[1] < 120);
            }
        }
    }
    println!(
        "signed roughness-metalness maps: {}; channels below -0.06: {negative}/{channels}",
        signed_maps.len()
    );
    // the negative half is unused: 19 of the 36 maps' 34,701,312 channels
    assert!(negative * 100_000 < channels, "{negative}/{channels}");
    println!(
        "packed maps bound: {maps}; metallic texels at the extremes: {}/{}",
        metal.0, metal.1
    );
}
