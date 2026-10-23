//! Every metaparam of the install's shader bundles (`/shader/shaderbdle.shaderbdlebnd.dcx`, 248 bundles): 232 read, the
//! 16 of version 4 (the old lightmap shaders and `sfx_AMSN_cloth`) refused as another version, with the counts of
//! what they declare. The 232 files were compared field by field with the third-party SoulsFormatsNEXT reader
//! (`METAPARAM`, built from its source; it takes Sekiro's colour parameters, type 12 where it says 13, only with that
//! one value allowed): textures, render passes, CAParams and MtdParams, no difference. Each program's extension agrees
//! with the shader type of its DXBC container, counted by stage.
//!
//! Needs `PRIMASE_SEKIRO_DIR` and `PRIMASE_OODLE6`.
#![allow(clippy::unwrap_used)]
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::metaparam::Metaparam;
use primase_fromsoft::{Archive, Binder};
use primase_testdata::real_data;
use std::collections::BTreeMap;
mod common;

#[test]
fn every_metaparam_of_the_install() {
    let (Some(root), Some(dll)) = (real_data("PRIMASE_SEKIRO_DIR"), real_data("PRIMASE_OODLE6")) else {
        return;
    };
    let dll = common::oodle(dll).unwrap();
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let archive = Archive::open(root).unwrap();
    let outer = Binder::read(
        &archive
            .read("/shader/shaderbdle.shaderbdlebnd.dcx", &decoder)
            .unwrap(),
        &decoder,
    )
    .unwrap();
    assert_eq!(outer.files.len(), 248);
    let (mut read, mut refused) = (0, 0);
    let mut declared = [0usize; 4];
    let mut stages: BTreeMap<(String, u32), usize> = BTreeMap::new();
    for bundle in &outer.files {
        for f in Binder::read(&bundle.bytes, &decoder).unwrap().files {
            let name = f.name.as_deref().unwrap();
            let extension = name.rsplit_once('.').unwrap().1.to_owned();
            if extension == "metaparam" {
                match Metaparam::read(&f.bytes) {
                    Ok(m) => {
                        read += 1;
                        declared[0] += m.textures.len();
                        declared[1] += m.passes.len();
                        declared[2] += m.ca_params.len();
                        declared[3] += m.mtd_params.len();
                    }
                    Err(e) => {
                        assert!(e.to_string().contains("version"), "{name}: {e}");
                        refused += 1;
                    }
                }
                continue;
            }
            let chunks = primase_dxbc::chunks(&f.bytes).unwrap();
            let code = chunks
                .iter()
                .find(|c| &c.kind == b"SHEX" || &c.kind == b"SHDR")
                .unwrap();
            let shader_type = u32::from_le_bytes(code.data[..4].try_into().unwrap()) >> 16;
            *stages.entry((extension, shader_type)).or_default() += 1;
        }
    }
    assert_eq!((read, refused), (232, 16));
    assert_eq!(declared, [1352, 1160, 773, 1535]);
    // .ppo pixel (0), .vpo vertex (1), .hpo hull (3), .dpo domain (4)
    let stages: Vec<_> = stages.into_iter().collect();
    assert_eq!(
        stages,
        [
            (("dpo".into(), 4), 37),
            (("hpo".into(), 3), 37),
            (("ppo".into(), 0), 10_607),
            (("vpo".into(), 1), 5_747)
        ]
    );
}
