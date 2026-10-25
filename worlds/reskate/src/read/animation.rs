//! Light animation sources: EBX timing and skeletons, then one selected ECLP. No mesh or texture resources.
use primase_core::{Entity, World};
use primase_frostbite::{bundle, folder::{EbxEntry, Source}};
use serde::{Deserialize, Serialize};
use crate::game::{clip::{clip_from, insert_clip, timing_from}, rig::insert_rig, skeleton::read_skeleton};

pub const RIG_TOC: &str = "Win32/globals.toc";
pub const RIG_BUNDLE: &str = "win32/configurations/gameconfigurations/delmargameconfiguration";
pub const RIG_NAME: &str = "animation/dingo/animbase_default_rig";
pub const RENDER_TOC: &str = "Win32/levels/game/bam_levelroot/bam_levelroot.toc";
pub const RENDER_NAME: &str = "characters/maincharacters/common/animbase/animbase_default_renderskeleton";

/// The location is part of the identity: equal names in different bundles are not merged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipEntry {
    pub key: String,
    pub toc: String,
    pub bundle: String,
    pub name: String,
    pub duration: Option<f64>,
    pub looping: bool,
    /// A controller with unsupported timing/codec stays visible as a named refusal, rather than disappearing.
    pub refusal: Option<String>,
}

fn key(toc: &str, bundle: &str, name: &str) -> Result<String, String> {
    serde_json::to_string(&(toc.to_ascii_lowercase(), bundle.to_ascii_lowercase(), name.to_ascii_lowercase()))
        .map_err(|e| e.to_string())
}

/// Visit each EBX sequentially; only controller documents survive as descriptors. Does not call resource() or
/// decode ECLP, read a MeshSet, material, texture or animation channel map. The caller chooses which TOCs to list.
pub fn list_clips(src: &Source<'_>, toc: &str) -> Result<Vec<ClipEntry>, String> {
    let table = src.folder.toc(toc).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (name, region) in &table.bundles {
        let (files, inline) = bundle::read_region(region).map_err(|e| e.to_string())?;
        let Some((manifest, first, _)) = src.manifest(&files, &inline).map_err(|e| e.to_string())? else { continue; };
        for (i, asset) in manifest.ebx.iter().enumerate() {
            let entry = EbxEntry { name: asset.name.clone(), file: *files.get(first + i)
                .ok_or_else(|| format!("{toc}/{name}: EBX file table is truncated"))?, size: asset.size as usize };
            let doc = src.read_ebx(&entry).map_err(|e| format!("{toc}/{name}/{}: {e}", asset.name))?;
            if doc.root_type != "ClipControllerAsset" { continue; }
            let timing = timing_from(&doc);
            out.push(ClipEntry { key: key(toc, name, &asset.name)?, toc: toc.into(), bundle: name.clone(),
                name: asset.name.clone(), duration: timing.as_ref().ok().map(|t| t.duration()),
                looping: timing.as_ref().is_ok_and(|t| t.looping), refusal: timing.err() });
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Skeletons { pub animation: Entity, pub render: Entity }

/// The authored animation rig and render skeleton, not a costume. Decodes no clip payload.
pub fn read_skeletons(src: &Source<'_>, world: &mut World) -> Result<Skeletons, String> {
    let rig = super::game::read_rig(src, RIG_TOC, RIG_BUNDLE, RIG_NAME)?;
    let (_, doc) = src.ebx(RENDER_TOC, RENDER_NAME).map_err(|e| e.to_string())?;
    let animation = insert_rig(&rig, RIG_NAME, world)?;
    let render = read_skeleton(&doc, RENDER_NAME, world)?;
    Ok(Skeletons { animation, render })
}

/// Decode this exact listed location, not the first occurrence of its name in the TOC. The caller caches by key,
/// chooses interpolation in lower-skate-clips, and maps animation -> render using game::rig::render_map + map-clip.
pub fn decode_clip(src: &Source<'_>, entry: &ClipEntry, animation: Entity, world: &mut World) -> Result<Entity, String> {
    if entry.key != key(&entry.toc, &entry.bundle, &entry.name)? { return Err("clip descriptor key does not match its location".into()); }
    let index = src.ebx_index(&entry.toc, &entry.bundle).map_err(|e| e.to_string())?;
    let mut found = index.values().filter(|e| e.name.eq_ignore_ascii_case(&entry.name));
    let selected = found.next().ok_or_else(|| format!("listed clip {} is no longer in {}", entry.name, entry.bundle))?;
    if found.next().is_some() { return Err(format!("{} is ambiguous in {}", entry.name, entry.bundle)); }
    let doc = src.read_ebx(selected).map_err(|e| e.to_string())?;
    let clip = clip_from(&entry.name, &doc,
        &|rid| src.resource(&entry.toc, &entry.bundle, rid).map(|r| (r.rtype, r.bytes)).map_err(|e| e.to_string()),
        &|d, v, what| super::game::import(src, &index, d, v, what), animation)?;
    primase_core::Component::verify(&clip, animation, world)?;
    Ok(insert_clip(clip, world))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clip_identity_distinguishes_bundle_and_separator_boundaries() {
        assert_ne!(key("a", "b", "c/d").expect("key"), key("a", "b/c", "d").expect("key"));
        assert_eq!(key("A", "B", "C").expect("key"), key("a", "b", "c").expect("key"));
    }
}
