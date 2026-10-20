//! Real TextureMaps read as stored (Riders Republic build 25394987, base WhiteRoom forge): fields, references, each
//! embedded level's bytes, the external levels' CompiledMips, and the top level decoded by primase-texcodec, against a
//! separate read-only decode (games/riders-republic docs/recon/texture-format-validation.md, bff28dc and 0ad60f1).
use std::fs::File;

use primase_anvil::read::data::{Resource, read_entry};
use primase_anvil::read::forge::Forge;
use primase_anvil::read::mip::{COMPILED_MIP, StoredMip};
use primase_anvil::read::shapes::MAX_RECORD;
use primase_anvil::read::texture::{self, StoredTextureMap, TEXTURE_MAP};
use primase_core::TexFormat;
use primase_testdata::real_data;
use primase_texcodec::{Decoder, TexDecoder};
use sha2::{Digest, Sha256};

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

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
    // entry, format, gamma, top level's bytes' SHA-256 (the separate decode's), its RGBA decode's SHA-256
    let fixtures = [
        (
            217,
            TexFormat::Bc1,
            1,
            "3410c1ec4ee2b9729873c9e72b0d016d1040eb866ab54802adb18f4e4d34b66c",
            // primase-texcodec's BC1. Pillow's (b974d7c3..., the separate decode's) differs by at most 1 in 32,524 of
            // 131,072 RGB channels and in no alpha: the two decoders round expanded and interpolated colours
            // differently (measured with scratch bc1_compare.py, 2026-10-20). BC7 below matches exactly.
            "4b3ea6deedfaad1f74c2c8a7a25b9e6bd3c7c0f383ae61da14c9bab6cc9df043",
        ),
        (
            218,
            TexFormat::Bc7,
            0,
            "cd4c3a0baf940ad5d6b172ff016ac2ccfec3ec3066770cb33a1fcfc6505ff2bc",
            "365e4a520183cf462ee3bdbace600e6172f17ed32f47e30df647b4b9a2a39962",
        ),
    ];
    for (entry, format, gamma, top, rgba) in fixtures {
        let r = resource(entry);
        assert_eq!(r.type_id, TEXTURE_MAP);
        let t = StoredTextureMap::parse(&r.body).expect("texture map");
        assert_eq!((t.width, t.height, t.mip_count, t.gamma), (256, 128, 9, gamma));
        assert_eq!(texture::format(t.format), Some(format));
        assert!(t.references.is_empty());
        let levels = t.levels().expect("levels");
        assert_eq!(levels.len(), 9);
        assert_eq!(sha(levels[0]), top, "entry {entry}");
        let pixels = Decoder::default()
            .decode_rgba8(format, 256, 128, levels[0])
            .expect("decode");
        assert_eq!(sha(&pixels), rgba, "entry {entry}: top level decoded");
    }
    // a map whose first two levels are CompiledMips of its own
    let r = resource(31);
    let t = StoredTextureMap::parse(&r.body).expect("texture map");
    assert_eq!((t.width, t.height, t.mip_count), (1024, 1024, 11));
    assert_eq!(texture::format(t.format), Some(TexFormat::Bc7));
    let ids: Vec<u64> = t.references.iter().map(|m| m.id).collect();
    assert_eq!(ids, [1_797_335_016_525, 1_797_335_016_526]);
    assert_eq!((t.pixels.len(), t.levels().expect("levels").len()), (87_408, 9));
    for (level, id) in (0u32..).zip(ids) {
        let mip = resource(forge.find(id).expect("in this forge"));
        assert_eq!((mip.type_id, mip.class_id), (COMPILED_MIP, id));
        let mip = StoredMip::parse(&mip.body).expect("mip");
        assert_eq!((mip.parent, mip.level), (r.class_id, level));
        let (w, h) = t.level_size(level);
        assert_eq!(Some(mip.image.len()), TexFormat::Bc7.checked_level_size(w, h));
    }
}
