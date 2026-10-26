//! Gates of the MeshSet reader and writer (`primase_frostbite::meshset`).
//!
//! - `the_games_own_meshsets_round_trip` (`installs.skate/Data`, `tools.oodle9`): every MeshSet of every TOC (all
//!   three LOD record forms, both headers, the development mesh with data after its relocation table) reads with its
//!   resource metadata and writes back byte-identical, the metadata too.
//! - Synthetic, on the first rigid mesh of `Win32/buildkititems.toc`: an edit of the typed fields and names is what is
//!   written and read back; a relocation table in another order reads the same, one naming a field that is no
//!   pointer, and metadata that misplaces it, are refused.

#![allow(clippy::unwrap_used)]

use primase_compress::oodle::{Oodle, Version};
use primase_frostbite::MESHSET;
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::meshset::{self, LOD_MAP, LOD_MASKS, LOD_PLAIN};
use primase_testdata::{Game, Tool, install_file, tool};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn oodle() -> Option<Oodle> {
    let p = tool(Tool::Oodle9)?;
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let o = unsafe { Oodle::load(p, Version::V9) };
    Some(o.expect("tools.oodle9 loads"))
}

/// A stored MeshSet: its name, bytes and resource metadata.
type Stored = (String, Vec<u8>, [u8; 16]);

/// Every MeshSet of the TOCs `only` names (all when empty), once per stored file.
fn meshsets(only: &[&str]) -> R<Option<Vec<Stored>>> {
    let (Some(data), Some(oodle)) = (install_file(Game::Skate, "Data"), oodle()) else {
        return Ok(None);
    };
    let folder = Folder::open(&data)?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for toc in folder
        .tocs
        .iter()
        .filter(|t| only.is_empty() || only.contains(&t.as_str()))
    {
        for b in &src.listing(toc)?.bundles {
            for (i, a) in b.manifest.res.iter().enumerate() {
                if a.rtype != MESHSET {
                    continue;
                }
                let e = b.resource(i);
                let (loc, at, _) = e.file;
                if seen.insert((loc.patch, loc.chunk, loc.archive, at)) {
                    out.push((e.name, src.payload(e.file, e.size as usize)?, a.rmeta));
                }
            }
        }
    }
    Ok(Some(out))
}

#[test]
fn the_games_own_meshsets_round_trip() -> R {
    let Some(all) = meshsets(&[])? else {
        return Ok(());
    };
    let mut forms = std::collections::BTreeMap::<(usize, usize, bool), usize>::new();
    let mut tails = 0;
    for (name, bytes, meta) in &all {
        let m = meshset::read_resource(bytes, meta).map_err(|e| format!("{name}: {e}"))?;
        let (again, again_meta) = meshset::write(&m).map_err(|e| format!("{name}: {e}"))?;
        if again != *bytes {
            let at = again
                .iter()
                .zip(bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(again.len().min(bytes.len()));
            let w = |b: &[u8]| format!("{:02x?}", &b[at.saturating_sub(8)..(at + 24).min(b.len())]);
            panic!(
                "{name}: written bytes differ from {at:#x} ({} written, {} stored)
  written {}
  stored  {}",
                again.len(),
                bytes.len(),
                w(&again),
                w(bytes)
            );
        }
        assert_eq!(again_meta, *meta, "{name}: metadata");
        // the reader alone reads the same records
        let plain = meshset::read(bytes)?;
        assert_eq!(plain.lods, m.lods, "{name}");
        *forms
            .entry((m.header.len(), m.lod_size(), m.bone_bounds.is_some()))
            .or_default() += 1;
        tails += usize::from(!m.tail.is_empty());
    }
    eprintln!(
        "{} MeshSets round trip; (header, LOD record, bone bounds): {forms:?}; {tails} with trailing data",
        all.len()
    );
    for lod in [LOD_PLAIN, LOD_MASKS, LOD_MAP] {
        assert!(forms.keys().any(|k| k.1 == lod), "LOD record {lod:#x} occurs");
    }
    assert!(tails > 0);
    Ok(())
}

#[test]
fn edits_are_written_and_bad_tables_refused() -> R {
    let Some(all) = meshsets(&["Win32/buildkititems.toc"])? else {
        return Ok(());
    };
    let (name, bytes, meta) = all
        .iter()
        .find(|(_, b, _)| meshset::read(b).is_ok_and(|m| m.lod_size() == LOD_PLAIN))
        .unwrap();
    let mut m = meshset::read_resource(bytes, meta)?;
    m.name = format!("{}_renamed", m.name);
    m.lods[0].name.push_str("_x");
    let s = &mut m.lods[0].sections[0];
    s.name = "Another_M".into();
    s.key ^= 0x5555;
    s.primitives -= 1;
    let (key, primitives) = (s.key, s.primitives);
    m.lods[0].lists[2] = vec![0, 0, 0];
    let (w, wm) = meshset::write(&m)?;
    let back = meshset::read_resource(&w, &wm)?;
    assert_eq!(back.name, m.name, "{name}");
    assert_eq!(back.lods[0].name, m.lods[0].name);
    let b = &back.lods[0].sections[0];
    assert_eq!(
        (b.name.as_str(), b.key, b.primitives),
        ("Another_M", key, primitives)
    );
    assert_eq!(back.lods[0].lists[2], [0, 0, 0]);
    // the relocation table in another order reads the same; a table naming a field that is no pointer, and metadata
    // naming another table, are refused
    let table = u32::from_le_bytes(meta[..4].try_into()?) as usize;
    let mut swapped = bytes.clone();
    swapped[table + 4..table + 8].copy_from_slice(&bytes[table..table + 4]);
    swapped[table..table + 4].copy_from_slice(&bytes[table + 4..table + 8]);
    assert_eq!(
        meshset::read_resource(&swapped, meta)?,
        meshset::read_resource(bytes, meta)?
    );
    let mut wrong = bytes.clone();
    let first = u32::from_le_bytes(bytes[table..table + 4].try_into()?);
    wrong[table..table + 4].copy_from_slice(&(first + 4).to_le_bytes());
    assert!(meshset::read_resource(&wrong, meta).is_err());
    let mut shifted = *meta;
    shifted[..4].copy_from_slice(&(table as u32 + 16).to_le_bytes());
    assert!(meshset::read_resource(bytes, &shifted).is_err());
    // a bone mask on a plain LOD record has nowhere to go
    let mut masked = meshset::read(bytes)?;
    masked.lods[0].bone_masks = Some(vec![[0; meshset::MASK]; masked.lods[0].sections.len()]);
    assert!(meshset::write(&masked).is_err());
    Ok(())
}
