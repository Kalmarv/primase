//! Real CompiledMips read as stored (Riders Republic build 25394987): the four a separate read-only decode checked
//! (games/riders-republic docs/recon/compiled-mip-reader-request.md, f9925f0).
use std::fs::File;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::mip::{COMPILED_MIP, StoredMip};
use primase_anvil::read::shapes::MAX_RECORD;
use primase_testdata::real_data;

#[test]
fn four_mips_read_their_envelopes() {
    let Some(path) = real_data("PRIMASE_ANVIL_FORGE_BASE") else {
        return;
    };
    let forge = Forge::open(&path).expect("forge");
    let mut file = File::open(&path).expect("open");
    // entry, name, index id, body bytes, image bytes, parent, level
    let fixtures = [
        (
            373,
            "MAD_TEX_TIL_Metal_Painted_A_DiffuseMap_PC_Scarlett_Prospero_Mip0",
            0x01BB_340C_9D36_u64,
            524_306,
            524_288,
            0x01A2_7983_E447_u64,
            0,
        ),
        (
            375,
            "MAD_TEX_TIL_Concrete_CleanPainted_A_DiffuseMap_PC_Scarlett_Prospero_Mip0",
            0x01C7_C843_E760,
            2_097_170,
            2_097_152,
            0x01C7_C843_E75E,
            0,
        ),
        (
            376,
            "MAD_TEX_TIL_Concrete_CleanPainted_A_DiffuseMap_PC_Scarlett_Prospero_Mip1",
            0x01C7_C843_E761,
            524_306,
            524_288,
            0x01C7_C843_E75E,
            1,
        ),
        (
            383,
            "MAD_TEX_TIL_Metal_Painted_A_Mask1Map_PC_Scarlett_Prospero_Mip0",
            0x01C7_C84E_7FD1,
            524_306,
            524_288,
            0x01C7_C84E_7FCF,
            0,
        ),
    ];
    for (entry, name, id, body, image, parent, level) in fixtures {
        assert_eq!(forge.names[entry].name, name);
        assert_eq!(forge.index[entry].id, id);
        let payload = forge.payload(&mut file, entry).expect("payload");
        let block = read_entry(&payload, MAX_RECORD).expect("entry");
        let resource = Resource::parse(&block.resource.data).expect("resource");
        assert_eq!((resource.type_id, resource.body.len()), (COMPILED_MIP, body));
        let mip = StoredMip::parse(&resource.body).expect("mip");
        assert_eq!((mip.image.len(), mip.parent, mip.level), (image, parent, level));
        assert_eq!(
            Some(mip.image.as_slice()),
            resource
                .body
                .get(StoredMip::IMAGE_AT..StoredMip::IMAGE_AT + image)
        );
    }
}
