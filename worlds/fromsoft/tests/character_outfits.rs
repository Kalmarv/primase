//! Installed initializer variants resolve their own protector parts and keep the associated lazy animation read.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use inputs::{Game as Data, Tool, install, keys, oodle};
use primase_core::convert::Reader;
use primase_fromsoft::sekiro::{CharacterReader, character_outfits};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn installed_initializer_catalogs_and_the_sekiro_outfit_reader_keep_authored_parts() -> Result<()> {
    for (game, data) in [(Game::Sekiro, Data::Sekiro), (Game::EldenRing, Data::Elden)] {
        let (Some(root), Some(folder), Some(library)) = (install(data), keys(data), oodle(Tool::Oodle6))
        else {
            continue;
        };
        let codec = common::oodle(library)?;
        let decoder = Decoder {
            oodle: Some(&codec),
            max_output: 1 << 30,
        };
        let installed = common::install(root, game, Keys::from_folder(folder, game, None)?);
        let archive = installed.open()?;
        let outfits = character_outfits(&installed, &archive, &decoder)?;
        assert!(!outfits.is_empty());
        let readable: Vec<_> = outfits.iter().filter(|outfit| outfit.readable()).collect();
        assert!(!readable.is_empty(), "{game:?}: no readable initializer outfit");
        for outfit in &readable {
            assert_eq!(outfit.animations_of, "c0000");
            for part in &outfit.parts {
                assert!(archive.contains(&format!("/parts/{part}.partsbnd.dcx")));
            }
        }
        println!(
            "{game:?}: {} initializer variants, {} readable",
            outfits.len(),
            readable.len()
        );
        if game == Game::Sekiro {
            let outfit = readable
                .iter()
                .find(|outfit| outfit.initializer == 9000)
                .ok_or("no initializer 9000")?;
            assert_eq!(
                outfit.parts,
                ["fc_m_0200_l", "bd_m_9040_l", "am_m_9000_l", "lg_m_9000_l"]
            );
            let mut reader = CharacterReader::outfit(installed, outfit, decoder)?;
            let (world, _) = reader.read().map_err(|error| error.to_string())?;
            assert!(world.iter::<primase_havok::cloth::HavokCloth>().count() >= 2);
            assert!(
                world
                    .iter::<primase_fromsoft::sekiro::ClipSource>()
                    .next()
                    .is_some()
            );
            assert!(
                world.iter::<primase_core::Clip>().next().is_none(),
                "tracks stay lazy until selected"
            );
        }
    }
    Ok(())
}
