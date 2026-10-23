//! A template's feature flags (`read::template::feature_node`, the body of its shader-state node) against the
//! ShaderKeys it stores, on all 432 MaterialTemplates of the six forges (Riders Republic build 25394987): the
//! engine's key composer (docs/shaders.md §6.2) sets a key bit only where the template's feature word `T` allows it,
//! and a template stores the permutations its flags can reach, so the byte of feature n (byte n + 1 of the node)
//! is set exactly where a stored key has the bit it gates:
//! - features 3 and 4 and bit `0x10` (both set), feature 3 and `0x400`/`0x800`, feature 4 and `0x200`;
//! - feature 9 and `0x1000`, feature 10 and `0x4` and `0x20` (pass 1), feature 11 and `0x8` (pass 2);
//! - feature 12 and `0x40`, feature 13 and `0x2`, feature 15 and `0x80` (pass 1).
//!
//! The census agreed on every template that has the pass; the test holds the templates to it and prints the counts.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::template::{self, MATERIAL_TEMPLATE};
use primase_testdata::{Game, install};

#[test]
fn feature_flags_are_set_where_the_template_stores_the_key_bit_they_gate() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut templates = 0;
    let mut checked = 0usize;
    for forge in [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
        "DataPC_MAD_WhiteRoom_patch_01.forge",
    ] {
        let mut archive = Archive::open(&[dir.join(forge)]).unwrap();
        let entries: Vec<usize> = {
            let names = &archive.forge(0).unwrap().names;
            (0..names.len())
                .filter(|&k| names[k].type_id == MATERIAL_TEMPLATE)
                .collect()
        };
        for entry in entries {
            let body = archive.resource(At { forge: 0, entry }).unwrap().body;
            let read = template::permutations(&body).unwrap();
            let node = template::feature_node(&body).unwrap();
            assert_eq!(node.len(), 54, "{forge} {entry}");
            let tables = template::shader_tables(&read, node);
            templates += 1;
            let has = |pass: u32, bit: u32| read.iter().any(|p| p.key >> 22 == pass && p.key & bit != 0);
            let has_any = |pass: u32| read.iter().any(|p| p.key >> 22 == pass);
            // (features that must all be set, key bit, pass)
            for (features, bit, pass) in [
                (&[3usize, 4][..], 0x10u32, 1u32),
                (&[3], 0x400, 1),
                (&[3], 0x800, 1),
                (&[4], 0x200, 1),
                (&[9], 0x1000, 1),
                (&[10], 0x4, 1),
                (&[10], 0x20, 1),
                (&[11], 0x8, 2),
                (&[12], 0x40, 1),
                (&[13], 0x2, 1),
                (&[15], 0x80, 1),
            ] {
                if !has_any(pass) {
                    continue;
                }
                let gate = features.iter().all(|&n| tables.feature(n) == Some(true));
                assert_eq!(
                    gate,
                    has(pass, bit),
                    "{forge} entry {entry}: features {features:?} and key bit {bit:#x}"
                );
                checked += 1;
            }
            // every flag a key rule asks for is 0 or 1
            for n in 0..16 {
                assert!(tables.feature(n).is_some(), "{forge} entry {entry}: feature {n}");
            }
        }
    }
    println!("{templates} templates, {checked} (template, feature, key bit) pairs agree");
    assert_eq!(templates, 432);
}
