//! Real TextureMaps read as stored (Riders Republic build 25394987, base WhiteRoom forge): fields, references, every
//! embedded level's bytes and the external levels' images (SHA-256s from the third-party RRAssetStudio's read in
//! games/riders-republic docs/recon/texture-format-validation.md, bff28dc), and the top levels decoded.
use std::fs::File;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::mip::{COMPILED_MIP, StoredMip};
use primase_anvil::read::shapes::MAX_RECORD;
use primase_anvil::read::texture::{self, StoredTextureMap, TEXTURE_MAP};
use primase_core::TexFormat;
use primase_testdata::real_data;
use primase_texcodec::{Bcdec, Decoder, TexDecoder};
use sha2::{Digest, Sha256};

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Map 217's (BC1) nine levels, largest first.
const LEVELS_217: [&str; 9] = [
    "3410c1ec4ee2b9729873c9e72b0d016d1040eb866ab54802adb18f4e4d34b66c",
    "e2ac07a7540f515f3506d16cdecba11e9c35be12c73babe23968a24ff6c223fd",
    "95a3f3d4dcf7f0159a1156a42359338ea93a6240aed1c591a630deff71b77b98",
    "e12deac376309d16847a41acdfd2350ab632f5cb6fb5114e2d90642952da8db0",
    "dcafc09016a99b6a92672ef078976872d61e5002bee882c4e09056e4299725b8",
    "fbb48d0da954f3eee9be7fb9169b01c3aa13ad69c61c57ba7734faa6a0ff4376",
    "98ab78005b4994163870d3934fc1428e58f6c2e1a0984fa80696728026b95951",
    "4380837fad8e03d7df1472905fd616aa1991954646ebd3b6a57f7eb08897904f",
    "4380837fad8e03d7df1472905fd616aa1991954646ebd3b6a57f7eb08897904f",
];

/// Map 218's (BC7) nine levels, largest first.
const LEVELS_218: [&str; 9] = [
    "cd4c3a0baf940ad5d6b172ff016ac2ccfec3ec3066770cb33a1fcfc6505ff2bc",
    "333e8757d65ae732ac911a7664535f7ce42a1730ddf0de8c2100d7a7e74d7828",
    "1acbcda2e84271d8a9f95021808221faf3953c87a77946c1f4d934e84ac03384",
    "3a8874042eebd8e7ed5ee48d0543ef5fbac15a0372c22839d8b4569c375618de",
    "59272c2b754db1abaf9e2631172d814c7496830b215833d573ebf0e8a51c6a35",
    "e50dd769eb857072584a64d3a180e24369f9847e872d224bc72d2e8fbf360926",
    "1fd4a88c869913049250540385182ebbbcaa0e0b80bfeaf81338c3efd0c51035",
    "7a705c52e2518c3182dbe3b541dff3ab5b456639f3771bd1d8f8b0f770baa79f",
    "61f9b435523d1e252c655792bd0d1cd6f52d43bacce1d1a2f701deacaeded5cc",
];

#[test]
fn texture_maps_read_their_levels_and_decode() {
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
    for (entry, format, field_25, levels) in [
        (217, TexFormat::Bc1, 1, LEVELS_217),
        (218, TexFormat::Bc7, 0, LEVELS_218),
    ] {
        let r = resource(entry);
        assert_eq!(r.type_id, TEXTURE_MAP);
        let t = StoredTextureMap::parse(&r.body).expect("texture map");
        assert_eq!(
            (t.width, t.height, t.mip_count, t.field_25),
            (256, 128, 9, field_25)
        );
        assert_eq!(texture::format(t.format), Some(format));
        assert!(t.references.is_empty());
        let held: Vec<String> = t.levels().expect("levels").into_iter().map(sha).collect();
        assert_eq!(held, levels, "entry {entry}");
    }
    // The top levels decoded. BC7 (218): primase-texcodec's bcdec decode equals the separate decode by the
    // independent texture2ddecoder (SHA-256 365e4a52..., 0ad60f1); BC7 is bit-exact by the D3D11.3 functional spec
    // (19.5.2). BC1 (217): the spec allows a tolerance (19.5.2), and primase-texcodec's own BC1 is gated against it
    // with Pillow in its gates; here it equals the third-party bcdec on every byte.
    let mut top = |entry: usize| {
        let r = resource(entry);
        let t = StoredTextureMap::parse(&r.body).expect("texture map");
        t.levels().expect("levels")[0].to_vec()
    };
    let bc7 = Decoder::default()
        .decode_rgba8(TexFormat::Bc7, 256, 128, &top(218))
        .expect("decode");
    assert_eq!(
        sha(&bc7),
        "365e4a520183cf462ee3bdbace600e6172f17ed32f47e30df647b4b9a2a39962"
    );
    let bc1 = top(217);
    let ours = Decoder::default()
        .decode_rgba8(TexFormat::Bc1, 256, 128, &bc1)
        .expect("decode");
    let bcdec = Bcdec
        .decode_rgba8(TexFormat::Bc1, 256, 128, &bc1)
        .expect("decode");
    assert_eq!(ours, bcdec, "BC1 against bcdec");
    // a map whose first two levels are CompiledMips of its own
    let r = resource(31);
    let t = StoredTextureMap::parse(&r.body).expect("texture map");
    assert_eq!((t.width, t.height, t.mip_count), (1024, 1024, 11));
    assert_eq!(texture::format(t.format), Some(TexFormat::Bc7));
    let ids: Vec<u64> = t.references.iter().map(|m| m.id).collect();
    assert_eq!(ids, [1_797_335_016_525, 1_797_335_016_526]);
    assert_eq!((t.pixels.len(), t.levels().expect("levels").len()), (87_408, 9));
    let images = [
        "e99078168a3e1c07d2b3382d35b17e3b35745963249807fa5fde2d42b913c80c",
        "dae3e543e373aa331763bf6c601266149e0647a6a40a3ec1f1d926b35a68fa2f",
    ];
    for ((level, id), image) in (0u32..).zip(ids).zip(images) {
        let mip = resource(forge.find(id).expect("in this forge"));
        assert_eq!((mip.type_id, mip.class_id), (COMPILED_MIP, id));
        let mip = StoredMip::parse(&mip.body).expect("mip");
        assert_eq!((mip.parent, mip.level), (r.class_id, level));
        let (w, h) = t.level_size(level);
        assert_eq!(Some(mip.image.len()), TexFormat::Bc7.checked_level_size(w, h));
        assert_eq!(sha(&mip.image), image, "level {level}");
    }
}
