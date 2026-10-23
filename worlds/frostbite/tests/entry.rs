//! What a data folder lists and what an entry reads into a world: EBX documents as JSON (the committed synthetic
//! fixtures, hand-worked values), the texture format table against core's layouts and the texture reader's sizes, a
//! depot's component; and, on the game's data (`installs.skate/Data`, `tools.oodle9`), the listing of
//! `Win32/items.toc` (every entry's count as its manifest states it, an archive for every entry) and one EBX
//! document, one texture and one ShaderBlockDepot read into a world.

#![allow(clippy::unwrap_used)]

use primase_compress::oodle::{Oodle, Version};
use primase_core::{ColorSpace, Component, Texture, World};
use primase_frostbite::bundle::Kind;
use primase_frostbite::components::{FrostbiteDepot, FrostbiteEbx, FrostbiteEntry, FrostbiteTexture};
use primase_frostbite::entry::{format_colour_space, tex_format};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::listing::resource_type_name;
use primase_frostbite::{depot, ebx, texture};
use primase_testdata::{Game, Tool, install_file, tool};
use serde_json::json;

type R = Result<(), Box<dyn std::error::Error>>;

const REFL: &[u8] = include_bytes!("fixtures/ebx/refl.ebx");
const RFL2: &[u8] = include_bytes!("fixtures/ebx/rfl2.ebx");

#[test]
fn an_ebx_document_shows_its_values_as_json() -> R {
    let j = ebx::read(REFL)?.to_json()?;
    assert_eq!(j["file_guid"], "a0".repeat(16));
    assert_eq!(j["root_type"], "Root");
    assert_eq!(j["imports"], json!([]));
    let root = &j["instances"][0];
    assert_eq!(root["type"], "Root");
    assert_eq!(root["exported"], true);
    assert_eq!(root["guid"], "ab".repeat(16));
    assert_eq!(root["values"]["flags"], true);
    assert_eq!(root["values"]["id"], -7);
    assert_eq!(root["values"]["scale"], 1.5);
    assert_eq!(root["values"]["label"], "hello");
    assert_eq!(root["values"]["child"], json!({ "instance": 1 }));
    assert_eq!(root["values"]["ids"], json!([1, 2, 3]));
    let child = &j["instances"][1];
    assert_eq!(child["type"], "Child");
    assert_eq!(child["exported"], false);
    assert_eq!(child["values"]["count"], 42);
    assert_eq!(child["values"]["items"][1]["values"]["value"], 200);
    // the document with an import and a boxed value
    let j = ebx::read(RFL2)?.to_json()?;
    assert_eq!(
        j["imports"][0],
        json!({ "file_guid": "b1".repeat(16), "class_guid": "b2".repeat(16) })
    );
    assert_eq!(j["instances"][0]["values"]["boxed"]["boxed"]["data_offset"], 112);
    Ok(())
}

/// A string that is not UTF-8 shows as its bytes, and a float JSON cannot hold is an error, not a null.
#[test]
fn a_value_json_cannot_hold_is_not_dropped() -> R {
    let mut doc = ebx::read(REFL)?;
    let object = doc.instances[0].object.as_mut().unwrap();
    *object.field_mut("label").unwrap() = ebx::Value::Str(vec![0xff, 0x41]);
    assert_eq!(
        doc.to_json()?["instances"][0]["values"]["label"],
        json!({ "bytes": "ff41" })
    );
    let object = doc.instances[0].object.as_mut().unwrap();
    *object.field_mut("scale").unwrap() = ebx::Value::Float(f64::NAN);
    assert!(doc.to_json().is_err());
    Ok(())
}

/// A Frostbite format ordinal has a core layout exactly where the texture reader knows its storage, and the two
/// agree on a level's size.
#[test]
fn texture_formats_map_to_core_layouts() {
    for format in 0..128 {
        assert_eq!(
            tex_format(format).is_some(),
            texture::block(format).is_some(),
            "format {format}"
        );
        if let (Some(core), Some(storage)) = (tex_format(format), texture::block(format)) {
            for (w, h) in [(1, 1), (4, 4), (5, 3), (64, 32)] {
                assert_eq!(
                    texture::level_size(storage, w, h).map(|s| s as usize),
                    Some(core.level_size(w, h)),
                    "format {format} at {w}x{h}"
                );
            }
        }
    }
    assert_eq!(tex_format(66), tex_format(67));
    // a colour space exactly where there is a layout: the sRGB ordinals are the odd member of each pair of the table
    for format in 0..128 {
        assert_eq!(
            format_colour_space(format).is_some(),
            tex_format(format).is_some(),
            "format {format}"
        );
    }
    let srgb: Vec<u32> = (0..128)
        .filter(|&f| format_colour_space(f) == Some(ColorSpace::Srgb))
        .collect();
    assert_eq!(srgb, [20, 24, 55, 57, 59, 61, 67]);
    assert_eq!(resource_type_name(primase_frostbite::TEXTURE), Some("Texture"));
    assert_eq!(resource_type_name(depot::RTYPE), Some("ShaderBlockDepotResource"));
    assert_eq!(resource_type_name(0), None);
}

/// A depot component shows its keys, blocks and parameters, a texture parameter as its reference; it refuses a key
/// that names a block it does not have.
#[test]
fn a_depot_component_shows_blocks_and_parameters() -> R {
    let mut block = 2u32.to_le_bytes().to_vec();
    block.extend(0x1000u64.to_le_bytes());
    block.extend(0x2bb6_5c2du32.to_le_bytes());
    block.extend([1, 0, 0, 0, 1, 0, 0, 0]);
    block.extend(1.5f32.to_le_bytes());
    block.extend((0x79a3u64 << 48).to_le_bytes());
    block.extend(depot::TEXTURE.to_le_bytes());
    block.extend([1, 0, 0, 0]);
    block.push(7);
    block.extend([0xA1; 16]);
    block.extend([0xB2; 16]);
    let d = depot::Depot {
        head: 0x10,
        version: 3,
        entries: vec![(9, 0)],
        blocks: vec![depot::Block {
            hash: 0xAA,
            data: block,
        }],
    };
    let parameters = vec![depot::params(&d.blocks[0].data)?];
    let c = FrostbiteDepot { depot: d, parameters };
    let j = c.to_json()?.unwrap();
    assert_eq!(
        j["entries"][0],
        json!({ "key": "0x0000000000000009", "block": 0 })
    );
    let params = &j["blocks"][0]["parameters"];
    assert_eq!(params[0]["value"], "0000c03f");
    assert_eq!(params[1]["slot"], 0x79a3);
    assert_eq!(
        params[1]["value"],
        json!({ "flag": 7, "guid_a": "a1".repeat(16), "guid_b": "b2".repeat(16) })
    );
    let mut w = World::new();
    let e = w.spawn();
    let mut bad = c.clone();
    bad.depot.entries.push((1, 4));
    assert!(bad.verify(e, &w).is_err());
    assert!(c.verify(e, &w).is_ok());
    Ok(())
}

#[test]
fn the_stock_items_toc_lists_and_reads() -> R {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Ok(());
    };
    let folder = Folder::open(data)?;
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let listing = src.listing("Win32/items.toc")?;
    assert!(listing.unread.is_empty(), "{:?}", listing.unread);
    assert_eq!(listing.bundles.len(), src.bundle_names("Win32/items.toc")?.len());
    let (mut ebx_n, mut res_n, mut chunk_n) = (0, 0, 0);
    for b in &listing.bundles {
        let l: Vec<_> = b.entries().collect();
        assert_eq!(l.len(), b.manifest.count(), "{}", b.name);
        for e in &l {
            match e.kind {
                Kind::Ebx => ebx_n += 1,
                Kind::Res => res_n += 1,
                Kind::Chunk => chunk_n += 1,
            }
            assert!(
                folder.archive(e.file.0).is_some(),
                "{} {}: no archive for {:?}",
                e.bundle,
                e.name,
                e.file.0
            );
        }
    }
    println!(
        "items.toc: {} bundles, {ebx_n} ebx, {res_n} resources, {chunk_n} chunks",
        listing.bundles.len()
    );
    assert!(ebx_n > 50_000 && res_n > 20_000 && chunk_n > 10_000);

    // the first EBX document, the first texture of a form the reader reads, the first depot
    let all: Vec<_> = (listing.bundles.iter())
        .flat_map(|b| b.entries().map(move |e| (b, e)))
        .collect();
    let mut w = World::new();
    let (b, e) = all.iter().find(|(_, e)| e.kind == Kind::Ebx).unwrap();
    let doc = src.read_entry(b, e, &mut w)?[0];
    assert_eq!(w.get::<FrostbiteEntry>(doc).unwrap().name, e.name);
    let component = w.get::<FrostbiteEbx>(doc).unwrap();
    assert_eq!(src.ebx_type(e)?, component.document.root_type);
    assert!(component.to_json()?.is_some());

    let (b, e) = all
        .iter()
        .find(|(b, e)| {
            e.resource_type == Some(primase_frostbite::TEXTURE)
                && texture::read(&src.payload(e.file, e.size as usize).unwrap()).is_ok_and(|h| {
                    h.kind == 0
                        && h.slices <= 1
                        && tex_format(h.format).is_some()
                        && b.chunk(&src, &h.chunk).is_ok()
                })
        })
        .unwrap();
    let t = src.read_entry(b, e, &mut w)?[0];
    let pixels = w.get::<Texture>(t).unwrap();
    let header = &w.get::<FrostbiteTexture>(t).unwrap().header;
    assert_eq!(
        (pixels.width, pixels.height),
        (u32::from(header.width), u32::from(header.height))
    );
    assert_eq!(pixels.levels.len(), usize::from(header.mip_count));
    assert_eq!(
        pixels.levels.iter().map(Vec::len).sum::<usize>(),
        header.chunk_size as usize
    );

    let (b, e) = all
        .iter()
        .find(|(_, e)| e.resource_type == Some(depot::RTYPE))
        .unwrap();
    let d = src.read_entry(b, e, &mut w)?[0];
    let d = w.get::<FrostbiteDepot>(d).unwrap();
    assert_eq!(d.parameters.len(), d.depot.blocks.len());

    // a resource type no reader reads is refused by name, and leaves nothing in the world
    let before = w.iter::<FrostbiteEntry>().count();
    let (b, e) = all
        .iter()
        .find(|(_, e)| e.kind == Kind::Res && resource_type_name(e.resource_type.unwrap()).is_none())
        .unwrap();
    let err = src.read_entry(b, e, &mut w).unwrap_err().to_string();
    assert!(err.contains(&e.name) && err.contains("is not read"), "{err}");
    assert_eq!(w.iter::<FrostbiteEntry>().count(), before);
    Ok(())
}

/// A streamed texture: its header describes the whole chain; its bundle's manifest holds only the levels from `first_mip`
/// on (a tail of the chunk, at the logical offset of the bytes of the levels before them) and its TOC's chunk table
/// the whole chain. `steel_02_2x2m_c` of the build kit's rails is 1024x1024 BC7 with 11 levels, the first three of
/// 1,376,256 bytes, and reads in full from the TOC; the manifest's 21,872-byte tail is the end of that chain.
#[test]
fn a_streamed_texture_reads_its_whole_chain_from_the_toc() -> R {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Ok(());
    };
    let folder = Folder::open(data)?;
    // SAFETY: the configured Oodle 9 library is the game's own.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let listing = src.listing("Win32/buildkititems.toc")?;
    let (bundle, entry) = listing
        .bundles
        .iter()
        .flat_map(|b| b.entries().map(move |e| (b, e)))
        .find(|(_, e)| {
            e.resource_type == Some(primase_frostbite::TEXTURE) && e.name.ends_with("/metal_steel_02_2x2m_c")
        })
        .unwrap();
    let mut w = World::new();
    let t = src.read_entry(bundle, &entry, &mut w)?[0];
    let header = w.get::<FrostbiteTexture>(t).unwrap().header.clone();
    assert_eq!(
        (header.width, header.height, header.mip_count, header.first_mip),
        (1024, 1024, 11, 3)
    );
    let pixels = w.get::<Texture>(t).unwrap();
    assert_eq!(
        (pixels.width, pixels.height, pixels.levels.len()),
        (1024, 1024, 11)
    );
    assert_eq!(pixels.levels.iter().map(Vec::len).sum::<usize>(), 1_398_128);
    // the manifest's chunk is the tail of the chain: the same bytes, from the logical offset of the three larger levels
    let skipped: u32 = header.mip_sizes[..3].iter().sum();
    let held = bundle
        .manifest
        .chunks
        .iter()
        .find(|c| c.guid == header.chunk)
        .unwrap();
    assert_eq!((held.loff, held.lsize), (skipped, 21_872));
    let (file, size) = bundle.chunk(&src, &header.chunk)?;
    let tail = src.payload(file, size)?;
    let rest: Vec<u8> = pixels.levels[3..].concat();
    assert_eq!(tail, rest);
    // every streamed and whole 2D texture of a sample of the TOC reads
    let (mut streamed, mut whole) = (0usize, 0usize);
    for (b, e) in listing
        .bundles
        .iter()
        .flat_map(|b| b.entries().map(move |e| (b, e)))
        .filter(|(_, e)| e.resource_type == Some(primase_frostbite::TEXTURE))
        .step_by(7)
    {
        let h = texture::read(&src.payload(e.file, e.size as usize)?)?;
        if h.kind != 0 || h.slices > 1 || tex_format(h.format).is_none() {
            continue;
        }
        let mut w = World::new();
        let t = src
            .read_entry(b, &e, &mut w)
            .map_err(|err| format!("{}: {err}", e.name))?[0];
        assert_eq!(
            w.get::<Texture>(t).unwrap().levels.len(),
            usize::from(h.mip_count),
            "{}",
            e.name
        );
        if h.first_mip > 0 {
            streamed += 1;
        } else {
            whole += 1;
        }
    }
    println!("{streamed} streamed and {whole} whole textures read in full");
    assert!(streamed > 0 && whole > 0);
    Ok(())
}
