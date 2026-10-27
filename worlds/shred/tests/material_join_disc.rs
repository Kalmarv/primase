//! Every disc submesh joins independently stored bank IDs. The ID-zero rule is
//! explicitly inferred from the exhaustive nonzero namespace join, not a native oracle.
mod inputs;

use primase_core::{Instance, Mesh, Texture, convert::Reader, frame::SourceFrame};
use primase_shred::{
    components::{EncodedTexture, MaterialId, MaterialTextures, ShredMaterial, SourceTexture},
    read::{
        convert::ShredReader,
        material_join::{MaterialInputs, TexturePreference},
    },
};
use inputs::{Game, install};
use rme_formats::pak::Pak;
use std::collections::BTreeSet;

#[test]
fn reader_keeps_material_texture_closure_for_charlie_oscar_and_mike() {
    let Some(root) = install(Game::Shred) else { return };
    let usrdir = root.join("PS3_GAME/USRDIR");
    let banks = Pak::open(usrdir.join("database/materials.pak")).unwrap();
    for name in ["charlie", "oscar", "mike"] {
        let base = Pak::open(usrdir.join(format!("levels/{name}.pak"))).unwrap();
        let layer = if name == "charlie" {
            Some(Pak::open(usrdir.join("levels/layers/00000004.pak")).unwrap())
        } else {
            None
        };
        let mut paks = vec![(name, &base)];
        if let Some(layer) = &layer {
            paks.push(("charlie-default", layer));
        }
        // Identity is a census frame only, not an assertion of physical metres or handedness.
        let mut reader = ShredReader::new(paks, SourceFrame::WORLD).with_materials(MaterialInputs {
            banks: &banks,
            assets: vec![],
            textures: TexturePreference::FullChainThenPrefetch,
        });
        let (plan, _) = reader.read().unwrap();
        assert!(plan.iter::<Texture>().next().is_none());
        assert_eq!(
            plan.iter::<ShredMaterial>().count(),
            plan.iter::<MaterialId>().count()
        );
        assert!(reader.material_report.unresolved_materials.is_empty());
        assert!(plan.verify_components().is_empty());
        let roots: BTreeSet<_> = plan.iter::<Instance>().take(20).map(|(e, _)| e).collect();
        assert!(!roots.is_empty());
        let (part, lines) = reader.load(&plan, &roots).unwrap();
        assert!(part.iter::<Mesh>().next().is_some());
        assert!(part.iter::<ShredMaterial>().next().is_some());
        assert!(part.iter::<Texture>().next().is_some());
        assert!(part.verify_components().is_empty());
        let (mut copies, mut prefetch, mut full) = (0, 0, 0);
        for (_, bindings) in part.iter::<MaterialTextures>() {
            for binding in &bindings.0 {
                let Some(e) = binding.texture else { continue };
                let source = part.get::<SourceTexture>(e).unwrap();
                if let Some(i) = source.preferred {
                    let chosen = &source.copies[i];
                    assert_eq!(
                        chosen.first_mip,
                        source
                            .copies
                            .iter()
                            .filter(|c| !c.faces.is_empty())
                            .map(|c| c.first_mip)
                            .min()
                            .unwrap()
                    );
                }
                for copy in &source.copies {
                    copies += 1;
                    if copy.first_mip == 0 {
                        full += 1;
                    } else {
                        prefetch += 1;
                    }
                    for &e in &copy.faces {
                        if let Some(texture) = part.get::<Texture>(e) {
                            assert_eq!(
                                texture.levels.len(),
                                usize::from(copy.header[61] - copy.first_mip)
                            );
                        } else {
                            assert!(part.get::<EncodedTexture>(e).is_some());
                        }
                    }
                }
            }
        }
        eprintln!(
            "SHRED reader {name}: {} material entities, {} decoded faces, {copies} bound-copy occurrences ({full} full/{prefetch} tail), {} unavailable texture paths; load={lines:?}",
            part.iter::<ShredMaterial>().count(),
            part.iter::<Texture>().count(),
            reader.material_report.unavailable_textures.len()
        );
    }
}
