//! The shader tables of every MaterialTemplate of the six forges (Riders Republic build 25394987): both stages kept,
//! each stage's keys unique within a template and each key finding its program, every program a stored DXBC container
//! whose shader type is its stage's. Counted: 53,180 pixel and 124,398 vertex records (177,578, the notes' count) naming
//! 8,040 distinct pixel and 5,750 distinct vertex programs. All 13,790 programs were disassembled by Microsoft's
//! `D3DDisassemble` (d3dcompiler_47): 8,040 `ps_5_0` and 5,750 `vs_5_0`, the stage each was kept under.
//!
//! The game's shader pack (`ShaderDataDX11.bin`, the `oracles.d3d-disassembler.riders/shader-pack` folder) is another
//! population: of these 13,790 programs one is byte-identical to a container in it.
use std::collections::BTreeSet;

use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::template::{self, MATERIAL_TEMPLATE, Stage};
use primase_testdata::{Game, install};

#[test]
fn both_stages_are_kept() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut records = [0usize; 2];
    let mut programs: [BTreeSet<Vec<u8>>; 2] = [BTreeSet::new(), BTreeSet::new()];
    let mut templates = 0;
    for forge in [
        "DataPC.forge",
        "DataPC_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
        "DataPC_MAD_WhiteRoom_patch_01.forge",
    ] {
        let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
        let entries: Vec<usize> = {
            let names = &archive.forge(0).expect("forge").names;
            (0..names.len())
                .filter(|&k| names[k].type_id == MATERIAL_TEMPLATE)
                .collect()
        };
        for entry in entries {
            let body = archive.resource(At { forge: 0, entry }).expect("template").body;
            let read = template::permutations(&body).expect("permutations");
            let tables = template::shader_tables(&read, template::feature_node(&body).expect("node"));
            templates += 1;
            for (stage, table) in [(Stage::Pixel, &tables.pixel), (Stage::Vertex, &tables.vertex)] {
                let keys: BTreeSet<u32> = table.iter().map(|&(k, _)| k).collect();
                assert_eq!(keys.len(), table.len(), "{forge}: a key twice in one stage");
                assert_eq!(table.len(), read.iter().filter(|p| p.stage == stage).count());
                let side = usize::from(stage == Stage::Vertex);
                records[side] += table.len();
                for &(key, index) in table {
                    let program = &tables.programs[index];
                    let found = match stage {
                        Stage::Pixel => tables.pixel(key),
                        Stage::Vertex => tables.vertex(key),
                    };
                    assert_eq!(found, Some(program.as_slice()));
                    let chunks = primase_dxbc::chunks(program).expect("container");
                    let code = chunks
                        .iter()
                        .find(|c| &c.kind == b"SHEX" || &c.kind == b"SHDR")
                        .expect("code");
                    let kind = u32::from_le_bytes(code.data[..4].try_into().expect("4")) >> 16;
                    assert_eq!(kind, if stage == Stage::Pixel { 0 } else { 1 });
                    programs[side].insert(program.clone());
                }
            }
        }
    }
    assert_eq!(templates, 432);
    assert_eq!(records, [53_180, 124_398]);
    assert_eq!([programs[0].len(), programs[1].len()], [8_040, 5_750]);
}
