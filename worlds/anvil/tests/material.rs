//! Real Materials and a TextureSet read as stored, their references resolved within their forge (Riders Republic
//! build 25394987; games/riders-republic docs/recon/material-reference-envelope.md, e244d10).
use std::fs::File;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::material::{
    MATERIAL, MATERIAL_TEMPLATE, StoredReferences, TEXTURE_MAP, TEXTURE_SET,
};
use primase_anvil::read::shapes::MAX_RECORD;
use primase_testdata::real_data;

#[test]
fn materials_and_a_texture_set_read_their_references() {
    let Some(path) = real_data("PRIMASE_ANVIL_FORGE_BASE") else {
        return;
    };
    let forge = Forge::open(&path).expect("forge");
    let mut file = File::open(&path).expect("open");
    let mut resource = |entry: usize| {
        let payload = forge.payload(&mut file, entry).expect("payload");
        let block = read_entry(&payload, MAX_RECORD).expect("entry");
        Resource::parse(&block.resource.data).expect("resource")
    };
    // entry, its type, body bytes, the two references and what each resolves to in this forge (None: nothing)
    let fixtures = [
        (
            26,
            MATERIAL,
            452,
            [(0x016E_140F_E491, None), (0x01A2_7983_E454, Some(TEXTURE_SET))],
        ),
        (
            210,
            MATERIAL,
            879,
            [
                (0x016E_140F_F6C8, Some(MATERIAL_TEMPLATE)),
                (0x01C7_C843_E755, Some(TEXTURE_SET)),
            ],
        ),
        (
            162,
            MATERIAL,
            269,
            [(0x01C3_C9ED_9E7E, None), (0x01D3_970D_C220, Some(TEXTURE_SET))],
        ),
        (
            163,
            TEXTURE_SET,
            204,
            [
                (0x01D3_970D_C221, Some(TEXTURE_MAP)),
                (0x01D3_970D_C22A, Some(TEXTURE_MAP)),
            ],
        ),
    ];
    for (entry, type_id, body, references) in fixtures {
        let r = resource(entry);
        assert_eq!((r.type_id, r.body.len()), (type_id, body), "entry {entry}");
        let stored = StoredReferences::parse(&r.body).expect("references");
        assert_eq!(stored.rest.len(), body - 21);
        for (id, (want, target)) in stored.references.into_iter().zip(references) {
            assert_eq!(id, want, "entry {entry}");
            let found = forge.find(id).map(|k| {
                let t = resource(k);
                assert_eq!(t.class_id, id);
                t.type_id
            });
            assert_eq!(found, target, "entry {entry}: reference {id:#x}");
        }
    }
}
