//! Every MaterialTemplate of the six forges (Riders Republic build 25394987): its shader permutations read
//! (read::template::permutations), each record's UCodeHash naming exactly one of its programs. Against the notes'
//! counts (docs/material-spec.md §25: 432 templates, 177,578 records, every hash resolving) and the engine's key
//! composer for Rocks (docs/shaders.md §6.2): a plain static Rocks G-buffer draw's pixel key 0x400000 names the
//! program at 0x25A2D, its roughness the detail normal's alpha.
mod inputs;

use anvil_formats::archive::{Archive, At};
use anvil_formats::template::{self, MATERIAL_TEMPLATE, Stage};
use inputs::{Game, install};

#[test]
fn every_permutation_names_one_program() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let (mut templates, mut records) = (0, 0);
    for forge in [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
        "DataPC_MAD_WhiteRoom_patch_01.forge",
    ] {
        let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
        let entries: Vec<(usize, String)> = {
            let names = &archive.forge(0).expect("forge").names;
            (0..names.len())
                .filter(|&k| names[k].type_id == MATERIAL_TEMPLATE)
                .map(|k| (k, names[k].name.clone()))
                .collect()
        };
        for (entry, name) in entries {
            let body = archive.resource(At { forge: 0, entry }).expect("template").body;
            let p = template::permutations(&body).unwrap_or_else(|e| panic!("{forge} {name}: {e}"));
            templates += 1;
            records += p.len();
            if forge == "DataPC_MAD_USNAT.forge" && name == "MAD_Tex1_Rocks" {
                let plain: Vec<usize> = p
                    .iter()
                    .filter(|x| x.stage == Stage::Pixel && x.key == 0x40_0000)
                    .map(|x| x.at)
                    .collect();
                assert_eq!((p.len(), plain), (146, vec![0x2_5A2D]));
            }
        }
    }
    assert_eq!((templates, records), (432, 177_578));
}
