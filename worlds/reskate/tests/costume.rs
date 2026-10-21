//! The costume steps on Studio's own `cosmetic-mesh-import` output (`PRIMASE_RESKATE_FBMOD`, the files with costume
//! presets) and the game's donor textures (`PRIMASE_SKATE_DATA`, `PRIMASE_OODLE9`):
//! - `presets::repair` takes exactly the doubly listed pointer slot out of every preset's type-info fixups, leaving
//!   every value and every other fixup as it was;
//! - `textures::retexture` with colour and normal replacements: one asset each, every preset's import of a donor
//!   colour or normal texture array pointed at it, the masks kept; the new documents, headers and chunks as the module
//!   documentation states; the mod written reads back; a rebuild is byte-identical;
//! - `morphs::neutralise`: both target chunks (LODs 0-1 and 2-5), every section's six targets the mesh's own stored
//!   positions and tangent frames, every other byte of the chunks unchanged; the LOD 0 positions are the positions of
//!   Studio's own readback GLB of the import (`<name>.readback.glb` beside the file, when present).

#![allow(clippy::unwrap_used)]

use primase_compress::oodle::{Oodle, Version};
use primase_frostbite::ebx::{self, Value};
use primase_frostbite::fbmod::{self, Body, Fbmod};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::texture;
use primase_reskate::costume::textures::{Donor, HeaderUnknowns, Replacement, guid_text, retexture};
use primase_reskate::costume::{costume_presets, morphs, presets};
use primase_reskate::postcompile::meshset;
use primase_testdata::real_data;

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
    let Some(dir) = real_data("PRIMASE_RESKATE_FBMOD") else {
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

#[test]
fn studio_presets_are_repaired_and_retextured() -> R {
    let (Some(data), Some(dll)) = (real_data("PRIMASE_SKATE_DATA"), real_data("PRIMASE_OODLE9")) else {
        return Ok(());
    };
    // SAFETY: the caller names the game's own Oodle DLL in PRIMASE_OODLE9.
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
            let objects = |d: &primase_frostbite::ebx::Document| {
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
                (h.chunk_size, h.unknown_00, h.unknown_20[0], h.unknown_20[1]),
                (112, [0; 8], 0, 4)
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
            (h.unknown_00, h.unknown_20[0]),
            (template.unknown_00, template.unknown_20[0])
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
    let Some(dll) = real_data("PRIMASE_OODLE9") else {
        return Ok(());
    };
    // SAFETY: the caller names the game's own Oodle DLL in PRIMASE_OODLE9.
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
            .find(|e| matches!(&e.body, Body::Res(r) if r.res_type == primase_frostbite::MESHSET))
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
