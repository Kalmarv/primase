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
        for (reference, (want, target)) in stored.references.into_iter().zip(references) {
            let id = reference.expect("a reference").id;
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

/// TextureSet 163 (base WhiteRoom) read whole: its first two of 19 references are maps 217 and 218, as its leading
/// references read above; the census reads every TextureSet of the game the same way.
#[test]
fn a_texture_set_reads_all_19_references() {
    let Some(path) = real_data("PRIMASE_ANVIL_FORGE_BASE") else {
        return;
    };
    let forge = Forge::open(&path).expect("forge");
    let payload = forge
        .payload(&mut File::open(&path).expect("open"), 163)
        .expect("payload");
    let block = read_entry(&payload, MAX_RECORD).expect("entry");
    let r = Resource::parse(&block.resource.data).expect("resource");
    let t = primase_anvil::read::material::StoredTextureSet::parse(&r.body).expect("texture set");
    let ids: Vec<Option<u64>> = t.references.iter().map(|x| x.map(|x| x.id)).collect();
    assert_eq!(ids[..2], [Some(0x01D3_970D_C221), Some(0x01D3_970D_C22A)]);
}

/// Material 26 (base WhiteRoom) read whole: its ten property records' keys and (class, type) as RRAssetStudio reads
/// them (the Riders Republic material report, 2026-10-20), and each texture record's second reference a TextureMap
/// of the same forge.
#[test]
fn a_material_reads_its_property_records() {
    use primase_anvil::read::material::{Payload, StoredMaterial};
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
    let m = StoredMaterial::parse(&resource(26).body).expect("material");
    let records: Vec<(u32, u32, u32)> = m.properties.iter().map(|p| (p.key, p.class, p.kind)).collect();
    assert_eq!(
        records,
        [
            (0x6338_B6C4, 0, 10),
            (0x7091_389B, 0xC52E_2125, 19),
            (0x89D1_84DE, 0, 10),
            (0x6577_AE58, 0, 10),
            (0xEE58_1D0E, 0, 10),
            (0xC7C3_A6D7, 0, 10),
            (0xFD62_B19F, 0, 10),
            (0x443B_F980, 0x7D08_460D, 19),
            (0xE4B6_CC48, 0x7D08_460D, 19),
            (0x95DD_8185, 0, 10),
        ]
    );
    let maps: Vec<u64> = m
        .properties
        .iter()
        .filter_map(|p| match &p.payload {
            Payload::Texture { references, .. } => references[1].map(|r| r.id),
            _ => None,
        })
        .collect();
    assert_eq!(maps.len(), 2);
    for id in maps {
        let t = resource(forge.find(id).expect("in this forge"));
        assert_eq!((t.type_id, t.class_id), (TEXTURE_MAP, id));
    }
}
