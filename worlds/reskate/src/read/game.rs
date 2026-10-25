//! The game's own assets read from its data folder ([`primase_frostbite::folder`]): the I/O of [`crate::game`], whose
//! modules read the documents this one finds (a rig and the documents it imports; a clip, its resource and its
//! channel map).

use std::collections::BTreeMap;

use primase_core::{Entity, World};
use primase_frostbite::ebx::{Document, Pointer, Value};
use primase_frostbite::folder::{EbxEntry, Source};

use crate::game::clip::{clip_from, insert_clip};
use crate::game::rig::{AnimRig, rig_from};

/// The document an external pointer `v` of `doc` names, from `index` (its bundle's documents by file GUID).
pub(super) fn import(
    src: &Source,
    index: &BTreeMap<[u8; 16], EbxEntry>,
    doc: &Document,
    v: &Value,
    what: &str,
) -> Result<Document, String> {
    let Value::Pointer(Pointer::External(k)) = v else {
        return Err(format!("{what} is not an import"));
    };
    let guid = doc
        .imports
        .get(*k)
        .ok_or_else(|| format!("{what}: import {k} does not exist"))?
        .file_guid;
    let entry = index
        .get(&guid)
        .ok_or_else(|| format!("{what}: the imported document is not in the bundle"))?;
    src.read_ebx(entry)
        .map_err(|e| format!("{what} ({}): {e}", entry.name))
}

/// The rig named `rig_name` in bundle `bundle` of TOC `toc` ([`crate::game::rig`]), its imports from that bundle.
pub fn read_rig(src: &Source, toc: &str, bundle: &str, rig_name: &str) -> Result<AnimRig, String> {
    let index = src.ebx_index(toc, bundle).map_err(|e| e.to_string())?;
    let entry = index
        .values()
        .find(|e| e.name.eq_ignore_ascii_case(rig_name))
        .ok_or_else(|| format!("{bundle} has no EBX {rig_name}"))?;
    let doc = src.read_ebx(entry).map_err(|e| e.to_string())?;
    rig_from(&doc, &|d, v, what| import(src, &index, d, v, what)).map_err(|e| format!("{rig_name}: {e}"))
}

/// The clip named `name` in TOC `toc` ([`crate::game::clip`]) into `world`, a clip of the animation skeleton
/// `skeleton`: its resource and imports from the bundle that holds it.
pub fn read_clip(
    src: &Source,
    toc: &str,
    name: &str,
    skeleton: Entity,
    world: &mut World,
) -> Result<Entity, String> {
    let (bundle, doc) = src.ebx(toc, name).map_err(|e| e.to_string())?;
    let index = src.ebx_index(toc, &bundle).map_err(|e| e.to_string())?;
    let clip = clip_from(
        name,
        &doc,
        &|rid| {
            src.resource(toc, &bundle, rid)
                .map(|r| (r.rtype, r.bytes))
                .map_err(|e| e.to_string())
        },
        &|d, v, what| import(src, &index, d, v, what),
        skeleton,
    )?;
    Ok(insert_clip(clip, world))
}
