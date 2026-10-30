//! The costume steps on Studio's own `cosmetic-mesh-import` output (`oracles.baseline.skate/fbmod`, the files with
//! costume presets) and the game's donor textures (`installs.skate/Data`, `tools.oodle9`):
//! - `presets::repair` takes exactly the doubly listed pointer slot out of every preset's type-info fixups, leaving
//!   every value and every other fixup as it was;
//! - `textures::retexture` with colour and normal replacements: one asset each, every preset's import of a donor
//!   colour or normal texture array pointed at it, the masks kept; the new documents, headers and chunks as the module
//!   documentation states; the mod written reads back; a rebuild is byte-identical;
//! - `thumbnail::set_thumbnail`: the item's two thumbnail imports pointed at new CDN documents and texture assets, nothing
//!   else changed (see its test);
//! - `morphs::neutralise`: both target chunks (LODs 0-1 and 2-5), every section's six targets the mesh's own stored
//!   positions and tangent frames, every other byte of the chunks unchanged; the LOD 0 positions are the positions of
//!   Studio's own readback GLB of the import (`<name>.readback.glb` beside the file, when present).

#![allow(clippy::unwrap_used)]

mod inputs;

use frostbite_formats::ebx::{self, Pointer, Value};
use frostbite_formats::fbmod::{self, Body, Fbmod};
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::texture;
use inputs::{Game, OracleSet, Tool, install_file, oracle, tool};
use primase_core::TexFormat;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_reskate::costume::textures::{Donor, HeaderUnknowns, Replacement, guid_text, retexture};
use primase_reskate::costume::thumbnail::{CdnContentHash, Choices};
use primase_reskate::costume::{ADDITION, costume_presets, morphs, presets, thumbnail};
use primase_reskate::postcompile::meshset;
use primase_texcodec::{Bcdec, TexDecoder};
use sha1::Digest;

type R = Result<(), Box<dyn std::error::Error>>;

const DONOR: Donor = Donor {
    toc: "Win32/items.toc",
    bundle: "win32/characters/maincharacters/generic/cas/clothing/unlicensed/generic/apparel/fullbody/costume/isaacclarke/2025/gen_costume_isaacclarke_complex_dmpreset_cas_main_bundlereftable",
};

/// An 8x8 BC7 texture of 4 levels whose bytes say which role it is.
fn replacement(role: &str, fill: u8) -> Replacement {
    Replacement {
        role: role.into(),
        format: 67,
        width: 8,
        height: 8,
        levels: vec![
            vec![fill; 64],
            vec![fill + 1; 16],
            vec![fill + 2; 16],
            vec![fill + 3; 16],
        ],
    }
}

/// The Studio files with costume presets.
fn studio_files(oodle: &Oodle) -> Result<Vec<(String, Fbmod)>, Box<dyn std::error::Error>> {
    let Some(dir) = oracle(OracleSet::Baseline, Game::Skate, "fbmod") else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "fbmod") {
            let m = fbmod::read(&std::fs::read(&p)?)?;
            if !costume_presets(&m, Some(oodle))?.is_empty() {
                out.push((p.display().to_string(), m));
            }
        }
    }
    Ok(out)
}

/// The entries `added` carry the user string, flags and handler of Studio's own additions in `original`: every
/// addition entry of the file (the user strings that start with `ADDITION`) has the same user string up to its `|`
/// suffix, and each added entry's string, flags and handler are those.
fn assert_added_as_studio(original: &Fbmod, added: &[fbmod::Entry], path: &str) {
    let studio: Vec<_> = original
        .entries
        .iter()
        .filter_map(|e| e.stored.as_ref())
        .filter(|s| s.user.starts_with(ADDITION))
        .collect();
    assert!(!studio.is_empty(), "{path}: no Studio additions");
    let user = studio[0].user.split('|').next().unwrap();
    for s in &studio {
        assert_eq!(s.user.split('|').next().unwrap(), user, "{path}");
        assert_eq!(
            (s.flags, s.handler),
            (studio[0].flags, studio[0].handler),
            "{path}"
        );
    }
    assert!(!added.is_empty());
    for e in added {
        let s = e.stored.as_ref().unwrap();
        assert_eq!(s.user, user, "{path}: {}", e.name);
        assert_eq!(
            (s.flags, s.handler),
            (studio[0].flags, studio[0].handler),
            "{path}: {}",
            e.name
        );
    }
}
#[test]
fn studio_presets_are_repaired_and_retextured() -> R {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Ok(());
    };
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let folder = Folder::open(data)?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let files = studio_files(&oodle)?;
    let donor_names: std::collections::BTreeMap<[u8; 16], String> = src
        .ebx_index(DONOR.toc, DONOR.bundle)?
        .into_iter()
        .map(|(g, e)| (g, e.name))
        .collect();
    let (c, ny) = (replacement("c_ta", 10), replacement("ny_ta", 20));
    for (path, original) in files {
        let before = costume_presets(&original, Some(&oodle))?;

        // the repair
        let mut m = original.clone();
        let repaired = presets::repair(&mut m, Some(&oodle))?;
        assert_eq!(repaired.len(), before.len(), "{path}");
        let after = costume_presets(&m, Some(&oodle))?;
        for ((_, a), (_, b)) in before.iter().zip(&after) {
            let gone: Vec<_> = a
                .type_info_offsets
                .iter()
                .filter(|o| !b.type_info_offsets.contains(o))
                .collect();
            assert_eq!(gone.len(), 1, "{path}");
            assert!(b.import_offsets.contains(gone[0]));
            assert!(!b.type_info_offsets.iter().any(|o| b.import_offsets.contains(o)));
            assert_eq!((&a.imports, &a.import_offsets), (&b.imports, &b.import_offsets));
            let objects = |d: &frostbite_formats::ebx::Document| {
                d.instances.iter().map(|i| i.object.clone()).collect::<Vec<_>>()
            };
            assert_eq!(objects(a), objects(b));
        }

        // the textures
        let pick = |name: &str| {
            if !name.contains("isaacclarke") {
                None
            } else if name.ends_with("_c_ta") {
                Some(&c)
            } else if name.ends_with("_ny_ta") {
                Some(&ny)
            } else {
                None
            }
        };
        let entries = m.entries.len();
        let report = retexture(
            &mut m,
            &src,
            Some(&oodle),
            &DONOR,
            "c1010_test",
            &pick,
            HeaderUnknowns::Zero,
        )?;
        assert_eq!(report.assets.len(), 2, "{path}: {report:?}");
        assert_eq!(m.entries.len(), entries + 6);
        assert_added_as_studio(&original, &m.entries[entries..], &path);
        assert!(
            report
                .kept
                .iter()
                .all(|n| !n.ends_with("_c_ta") && !n.ends_with("_ny_ta"))
        );
        assert!(report.kept.iter().any(|n| n.ends_with("_msk_ta")), "{report:?}");
        let presets_now = costume_presets(&m, Some(&oodle))?;
        for (asset, r) in [(&report.assets[0].0, &c), (&report.assets[1].0, &ny)] {
            assert!(asset.ends_with(&format!("/c1010_test_{}", r.role)), "{asset}");
            let ebx_entry = m
                .entries
                .iter()
                .find(|e| &e.name == asset && matches!(e.body, Body::Ebx))
                .unwrap();
            let doc = ebx::read(&m.payload(ebx_entry.index().unwrap(), Some(&oodle))?)?;
            let o = doc.instances[0].object.as_ref().unwrap();
            assert_eq!(o.field("Name"), Some(&Value::Str(asset.as_bytes().to_vec())));
            assert_eq!(o.field("AuthoredWidth"), Some(&Value::UInt(8)));
            let Some(Value::Resource(rid)) = o.field("Resource") else {
                panic!("Resource")
            };
            assert_eq!(rid & 1, 1);
            let res = m
                .entries
                .iter()
                .find(|e| &e.name == asset && matches!(&e.body, Body::Res(x) if x.rid == *rid))
                .unwrap();
            let h = texture::read(&m.payload(res.index().unwrap(), Some(&oodle))?)?;
            assert_eq!(
                (
                    h.format,
                    h.width,
                    h.height,
                    h.slices,
                    h.depth,
                    h.mip_count,
                    h.first_mip
                ),
                (67, 8, 8, 1, 1, 4, 0)
            );
            assert_eq!(h.kind, 3, "a texture array keeps its kind");
            assert_eq!(&h.mip_sizes[..5], &[64, 16, 16, 16, 0]);
            assert_eq!(
                (
                    h.chunk_size,
                    h.mip_block_offsets,
                    h.unknown_20[0],
                    h.unknown_20[1]
                ),
                (112, [0; 2], 0, 4)
            );
            let chunk = m.entries.iter().find(|e| e.name == guid_text(&h.chunk)).unwrap();
            assert_eq!(
                m.payload(chunk.index().unwrap(), Some(&oodle))?,
                r.levels.concat()
            );
            // every preset that imported a donor texture of this role imports the asset now, and no donor one
            for ((_, was), (_, now)) in before.iter().zip(&presets_now) {
                let had = was
                    .imports
                    .iter()
                    .any(|i| donor_names.get(&i.file_guid).is_some_and(|n| pick(n) == Some(r)));
                assert_eq!(
                    now.imports.iter().any(|i| i.file_guid == doc.file_guid),
                    had,
                    "{path}"
                );
                assert!(
                    !now.imports
                        .iter()
                        .any(|i| donor_names.get(&i.file_guid).is_some_and(|n| pick(n) == Some(r)))
                );
            }
        }
        // the chunk entries are added as Studio adds its own: range, logical offset and h32 zero, logical size the
        // chunk's
        for e in &original.entries {
            if let Body::Chunk(ci) = &e.body {
                assert_eq!(
                    (ci.range_start, ci.range_end, ci.logical_offset, ci.h32),
                    (0, 0, 0, 0),
                    "{}",
                    e.name
                );
                assert_eq!(
                    u64::from(ci.logical_size),
                    e.stored.as_ref().unwrap().size,
                    "{}",
                    e.name
                );
            }
        }
        for e in &m.entries[entries..] {
            if let Body::Chunk(ci) = &e.body {
                assert_eq!(
                    (ci.range_start, ci.range_end, ci.logical_offset, ci.h32),
                    (0, 0, 0, 0)
                );
                assert_eq!(u64::from(ci.logical_size), e.stored.as_ref().unwrap().size);
            }
        }
        let bytes = fbmod::write(&m)?;
        assert_eq!(fbmod::read(&bytes)?, m);

        assert_eq!(report.header_unknowns, HeaderUnknowns::Zero);
        // the other choice carries the donor's: the template is the first donor texture redirected to the asset
        let mut donor_run = original.clone();
        let d = retexture(
            &mut donor_run,
            &src,
            Some(&oodle),
            &DONOR,
            "c1010_test",
            &pick,
            HeaderUnknowns::Donor,
        )?;
        let (asset, from) = &d.assets[0];
        let (_, template_doc) = src.ebx(DONOR.toc, &from[0])?;
        let Some(Value::Resource(template_rid)) = template_doc.instances[0]
            .object
            .as_ref()
            .unwrap()
            .field("Resource")
            .cloned()
        else {
            panic!("Resource")
        };
        let template = texture::read(&src.resource(DONOR.toc, DONOR.bundle, template_rid)?.bytes)?;
        let res = donor_run
            .entries
            .iter()
            .find(|e| &e.name == asset && matches!(e.body, Body::Res(_)))
            .unwrap();
        let h = texture::read(&donor_run.payload(res.index().unwrap(), Some(&oodle))?)?;
        assert_eq!(
            (h.mip_block_offsets, h.unknown_20[0]),
            (template.mip_block_offsets, template.unknown_20[0])
        );
        assert_eq!(h.unknown_20[1], 4);

        // a rebuild makes the same bytes
        let mut again = original.clone();
        presets::repair(&mut again, Some(&oodle))?;
        retexture(
            &mut again,
            &src,
            Some(&oodle),
            &DONOR,
            "c1010_test",
            &pick,
            HeaderUnknowns::Zero,
        )?;
        assert_eq!(fbmod::write(&again)?, bytes);
        println!("{path}: {} presets repaired; {:?}", repaired.len(), report);
    }
    Ok(())
}

/// Every primitive's positions of a GLB, meshes in order (float3 accessors, tightly packed).
fn glb_positions(b: &[u8]) -> Vec<Vec<[f32; 3]>> {
    let word = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    let json_len = word(12);
    let j: serde_json::Value = serde_json::from_slice(&b[20..20 + json_len]).unwrap();
    let bin = 20 + json_len + 8;
    let mut out = Vec::new();
    for m in j["meshes"].as_array().unwrap() {
        for p in m["primitives"].as_array().unwrap() {
            let a = &j["accessors"][p["attributes"]["POSITION"].as_u64().unwrap() as usize];
            let v = &j["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
            let at = bin
                + v["byteOffset"].as_u64().unwrap_or(0) as usize
                + a["byteOffset"].as_u64().unwrap_or(0) as usize;
            let n = a["count"].as_u64().unwrap() as usize;
            out.push(
                b[at..at + 12 * n]
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .map(|c| {
                        std::array::from_fn(|k| f32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap()))
                    })
                    .collect(),
            );
        }
    }
    out
}

#[test]
fn body_shape_targets_are_made_neutral() -> R {
    let Some(dll) = tool(Tool::Oodle9) else {
        return Ok(());
    };
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    for (path, original) in studio_files(&oodle)? {
        let mut m = original.clone();
        let report = morphs::neutralise(&mut m, Some(&oodle))?;
        assert_eq!(report.chunks.len(), 2, "{path}");
        let payload = |m: &Fbmod, name: &str| {
            let e = m.entries.iter().find(|e| e.name == name).unwrap();
            m.payload(e.index().unwrap(), Some(&oodle)).unwrap()
        };
        let mesh = m
            .entries
            .iter()
            .find(|e| matches!(&e.body, Body::Res(r) if r.res_type == frostbite_formats::MESHSET))
            .unwrap();
        let set = meshset::read(&m.payload(mesh.index().unwrap(), Some(&oodle))?)?;
        let lods: Vec<Vec<u8>> = set
            .lods
            .iter()
            .map(|l| payload(&m, &guid_text(&l.chunk)))
            .collect();
        let mut sections = 0;
        for (name, n) in &report.chunks {
            let (before, after) = (payload(&original, name), payload(&m, name));
            assert_eq!(before.len(), after.len());
            let spans = morphs::targets(&after)?.unwrap();
            assert_eq!(spans.len(), *n);
            assert_eq!(morphs::targets(&before)?.unwrap(), spans);
            let mut outside = vec![true; after.len()];
            for t in &spans {
                outside[t.span.clone()].iter_mut().for_each(|x| *x = false);
                let sec = &set.lods[t.lod].sections[t.section];
                let v = t.vertices;
                // position stream 0 (12 bytes); the tangent frame its own 4-byte stream
                let pos = &lods[t.lod][sec.vertex_offset as usize..][..12 * v];
                let tf = sec.decl.elements.iter().find(|e| e.usage == 52).unwrap();
                let before_tf: usize = sec.decl.strides[..usize::from(tf.stream)]
                    .iter()
                    .map(|&s| usize::from(s))
                    .sum();
                let frames = &lods[t.lod][sec.vertex_offset as usize + before_tf * v..][..4 * v];
                let span = &after[t.span.clone()];
                for k in 0..6 {
                    assert_eq!(&span[k * 12 * v..(k + 1) * 12 * v], pos);
                    assert_eq!(&span[72 * v + k * 4 * v..72 * v + (k + 1) * 4 * v], frames);
                }
                sections += 1;
            }
            assert!(
                before
                    .iter()
                    .zip(&after)
                    .zip(&outside)
                    .all(|((a, b), o)| !o || a == b),
                "{name}"
            );
        }
        assert_eq!(sections, set.lods.iter().map(|l| l.sections.len()).sum::<usize>());
        // LOD 0 is what Studio read back from the import
        let readback = std::path::Path::new(&path).with_extension("readback.glb");
        if readback.exists() {
            // Studio's readback writes `"targets": []`, which glTF forbids (and primase's reader refuses): its
            // positions are read here by their accessors alone
            let prims = glb_positions(&std::fs::read(&readback)?);
            assert!(prims.len() >= set.lods[0].sections.len());
            for (sec, want) in set.lods[0].sections.iter().zip(&prims) {
                let got: Vec<[f32; 3]> = lods[0][sec.vertex_offset as usize..][..12 * sec.vertices as usize]
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .map(|c| {
                        std::array::from_fn(|k| f32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap()))
                    })
                    .collect();
                assert_eq!(&got, want, "{path}");
            }
        }
        println!(
            "{path}: {report:?}, {sections} sections; readback compared: {}",
            readback.exists()
        );
    }
    Ok(())
}

#[test]
fn guids_print_in_their_text_form() {
    let g = [
        0x3f, 0x54, 0x6b, 0x76, 0x28, 0x80, 0xea, 0x4f, 0x92, 0xe9, 0x0d, 0x9a, 0x5c, 0x25, 0x6d, 0x3e,
    ];
    assert_eq!(guid_text(&g), "766b543f-8028-4fea-92e9-0d9a5c256d3e");
}

/// A picture of `side` pixels square whose texels vary smoothly (so BC7 reproduces them closely), with a soft edge of
/// transparency.
fn picture(side: u32) -> thumbnail::Picture {
    let mut rgba = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let a = if x < 4 || y < 4 { 128 } else { 255 };
            rgba.extend_from_slice(&[
                (x * 255 / side) as u8,
                (y * 255 / side) as u8,
                ((x + y) * 127 / side) as u8,
                a,
            ]);
        }
    }
    thumbnail::Picture {
        width: side,
        height: side,
        rgba,
    }
}

/// The thumbnail step on Studio's own files (`oracles.baseline.skate/fbmod`) and the game's donor thumbnails
/// (`installs.skate/Data`, `tools.oodle9`):
/// - the game's own CDN documents state the `NameHash` rule: `name_hash` of the donor texture's `Name` is the donor
///   CDN document's `NameHash`, for every stock costume thumbnail;
/// - `set_thumbnail`: the item's `Thumbnail` and `ThumbnailLarge` imports point at the new CDN documents (they were
///   the donor's), which point at the new texture assets; the assets read (a new `Resource` id with bit 0 set, the
///   donor's format, size, crop and authored size, the chunk the header names); a third-party decoder (bcdec) reads
///   the chunk back as the picture, within BC7's error; the item's objects and every other import are as they were,
///   every other entry is unchanged; the mod written reads back and a rebuild is byte-identical;
/// - a picture of another size is refused by name and leaves the mod as it was.
#[test]
fn studio_item_gets_its_own_thumbnails() -> R {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Ok(());
    };
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let folder = Folder::open(data)?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let donor = Donor {
        toc: "Win32/items.toc",
        bundle: "win32/characters/customization/configs/cas_main_sharedbundle",
    };
    let index = src.ebx_index(donor.toc, donor.bundle)?;
    let name_of = |d: &ebx::Document| match d.instances[0].object.as_ref().unwrap().field("Name") {
        Some(Value::Str(s)) => String::from_utf8(s.clone()).unwrap(),
        other => panic!("Name: {other:?}"),
    };
    // the hash rule, against the game's stock documents
    let mut stock = 0;
    for e in index
        .values()
        .filter(|e| e.name.starts_with("thumbnail/cdn/img_own_costume_gen_"))
    {
        let cdn = src.read_ebx(e)?;
        let tex = src.read_ebx(&index[&cdn.imports[0].file_guid])?;
        assert_eq!(
            cdn.instances[0].object.as_ref().unwrap().field("NameHash"),
            Some(&Value::UInt(u64::from(thumbnail::name_hash(&name_of(&tex))))),
            "{}",
            e.name
        );
        stock += 1;
    }
    assert!(stock >= 2, "{stock} stock CDN documents");

    let pictures = thumbnail::Thumbnails {
        small: picture(256),
        large: picture(768),
    };
    let choices = Choices {
        header_unknowns: HeaderUnknowns::Zero,
        content_hash: CdnContentHash::PixelsSha1,
    };
    for (path, original) in studio_files(&oodle)? {
        let mut m = original.clone();
        let report = thumbnail::set_thumbnail(
            &mut m,
            &src,
            Some(&oodle),
            &donor,
            "c1010_test",
            &pictures,
            choices,
        )?;
        assert!(
            report.item.starts_with("items/cust_fullbodycostume/own_"),
            "{}",
            report.item
        );
        assert_eq!(report.thumbnails.len(), 2);
        assert_eq!(m.entries.len(), original.entries.len() + 8, "{path}");
        assert_added_as_studio(&original, &m.entries[original.entries.len()..], &path);

        let read = |m: &Fbmod, name: &str| -> ebx::Document {
            let e = m
                .entries
                .iter()
                .find(|e| e.name == name && matches!(e.body, Body::Ebx))
                .unwrap_or_else(|| panic!("{name}"));
            ebx::read(&m.payload(e.index().unwrap(), Some(&oodle)).unwrap()).unwrap()
        };
        let (was, now) = (read(&original, &report.item), read(&m, &report.item));
        let objects = |d: &ebx::Document| d.instances.iter().map(|i| i.object.clone()).collect::<Vec<_>>();
        assert_eq!(objects(&was), objects(&now), "{path}");
        let changed: Vec<usize> = (0..was.imports.len())
            .filter(|&i| was.imports[i] != now.imports[i])
            .collect();
        assert_eq!(changed.len(), 2, "{path}");

        for (t, side, pic, donor_cdn) in [
            (
                &report.thumbnails[0],
                256u32,
                &pictures.small,
                "thumbnail/cdn/img_own_costume_gen_isaacclarke_00001",
            ),
            (
                &report.thumbnails[1],
                768,
                &pictures.large,
                "thumbnail/cdn/img_own_costume_gen_isaacclarke_00001_lrg",
            ),
        ] {
            assert_eq!(
                (u32::from(t.width), u32::from(t.height), t.format),
                (side, side, 67)
            );
            let Some(Value::Pointer(Pointer::External(slot))) =
                now.instances[0].object.as_ref().unwrap().field(t.field).cloned()
            else {
                panic!("{}", t.field)
            };
            assert!(changed.contains(&slot));
            assert_eq!(index[&was.imports[slot].file_guid].name, donor_cdn);
            // the item's import is the new CDN document, which refers to the new texture asset
            let cdn = read(&m, &t.cdn);
            assert_eq!(now.imports[slot].file_guid, cdn.file_guid);
            assert_eq!(now.imports[slot].class_guid, cdn.instances[0].guid);
            let tex = read(&m, &t.texture);
            assert_eq!(cdn.imports[0].file_guid, tex.file_guid);
            assert_eq!(cdn.imports[0].class_guid, tex.instances[0].guid);
            let c = cdn.instances[0].object.as_ref().unwrap();
            assert_eq!(c.field("Name"), Some(&Value::Str(t.cdn.as_bytes().to_vec())));
            assert_eq!(
                c.field("NameHash"),
                Some(&Value::UInt(u64::from(thumbnail::name_hash(&t.texture))))
            );
            let o = tex.instances[0].object.as_ref().unwrap();
            assert_eq!(o.field("Name"), Some(&Value::Str(t.texture.as_bytes().to_vec())));
            let Some(Value::Resource(rid)) = o.field("Resource") else {
                panic!("Resource")
            };
            assert_eq!(rid & 1, 1);
            // the donor's crop and authored size stay (the slot's layout)
            let donor_tex = src.read_ebx(index.values().find(|e| e.name == t.donor).unwrap())?;
            let od = donor_tex.instances[0].object.as_ref().unwrap();
            for f in ["CropInfo", "AuthoredWidth", "AuthoredHeight"] {
                assert_eq!(o.field(f), od.field(f), "{f}");
            }
            let res = m
                .entries
                .iter()
                .find(|e| e.name == t.texture && matches!(&e.body, Body::Res(x) if x.rid == *rid))
                .unwrap();
            let h = texture::read(&m.payload(res.index().unwrap(), Some(&oodle))?)?;
            assert_eq!(
                (h.format, h.width, h.height, h.mip_count, h.slices),
                (67, side as u16, side as u16, 1, 1)
            );
            assert_eq!(h.chunk_size, side * side);
            let chunk = m.entries.iter().find(|e| e.name == guid_text(&h.chunk)).unwrap();
            let bytes = m.payload(chunk.index().unwrap(), Some(&oodle))?;
            assert_eq!(bytes.len() as u32, h.chunk_size);
            assert_eq!(
                c.field("ContentHash"),
                Some(&Value::Sha1(sha1::Sha1::digest(&bytes).into())),
                "the chosen ContentHash"
            );
            let px = Bcdec.decode_rgba8(TexFormat::Bc7, side, side, &bytes)?;
            let err: u64 = px
                .iter()
                .zip(&pic.rgba)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            let mean = err as f64 / px.len() as f64;
            assert!(mean < 2.0, "{}: mean error {mean}", t.texture);
            println!("{path}: {} mean abs error {mean:.3}", t.texture);
        }

        // nothing else changed
        for e in &original.entries {
            let x = m
                .entries
                .iter()
                .find(|x| x.name == e.name && x.kind() == e.kind())
                .unwrap_or_else(|| panic!("{} gone", e.name));
            assert_eq!((&x.body, &x.bundles), (&e.body, &e.bundles), "{}", e.name);
            if e.name != report.item {
                assert_eq!(
                    e.index().map(|i| original.payload(i, Some(&oodle)).unwrap()),
                    x.index().map(|i| m.payload(i, Some(&oodle)).unwrap()),
                    "{}",
                    e.name
                );
            }
        }
        assert_eq!(report.choices, choices); // the other choices: the donor's ContentHash kept, the header bytes the donor's (zero in both fields here)        let mut kept = original.clone();        let other = Choices {            header_unknowns: HeaderUnknowns::Donor,            content_hash: CdnContentHash::Donor,        };        let r = thumbnail::set_thumbnail(&mut kept, &src, Some(&oodle), &donor, "c1010_test", &pictures, other)?;        assert_eq!(r.choices, other);        for (t, want) in r.thumbnails.iter().zip(&report.thumbnails) {            let donor_cdn = index.values().find(|e| e.name == want.donor.replace("/tool/", "/cdn/img_")).unwrap();            let stock = src.read_ebx(donor_cdn)?;            let got = read(&kept, &t.cdn);            assert_eq!(                got.instances[0].object.as_ref().unwrap().field("ContentHash"),                stock.instances[0].object.as_ref().unwrap().field("ContentHash")            );            let header = |m: &Fbmod| {                let e = m.entries.iter().find(|e| e.name == t.texture && matches!(e.body, Body::Res(_))).unwrap();                m.payload(e.index().unwrap(), Some(&oodle)).unwrap()            };            let mut zero = texture::read(&header(&m))?;            let mut donor_h = texture::read(&header(&kept))?;            zero.chunk = [0; 16];            donor_h.chunk = [0; 16];            assert_eq!(zero, donor_h);        }
        let bytes = fbmod::write(&m)?;
        assert_eq!(fbmod::read(&bytes)?, m);

        // a rebuild makes the same bytes
        let mut again = original.clone();
        thumbnail::set_thumbnail(
            &mut again,
            &src,
            Some(&oodle),
            &donor,
            "c1010_test",
            &pictures,
            choices,
        )?;
        assert_eq!(fbmod::write(&again)?, bytes);

        // a picture of another size is refused, naming both sizes
        let mut wrong = original.clone();
        let bad = thumbnail::Thumbnails {
            small: picture(128),
            large: picture(768),
        };
        let e = thumbnail::set_thumbnail(
            &mut wrong,
            &src,
            Some(&oodle),
            &donor,
            "c1010_test",
            &bad,
            choices,
        )
        .unwrap_err();
        assert!(
            e.to_string().contains("128x128") && e.to_string().contains("256x256"),
            "{e}"
        );
        assert_eq!(wrong, original);
    }
    Ok(())
}
