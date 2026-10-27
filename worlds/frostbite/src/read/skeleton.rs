//! The skeleton assets of a scope read into a world ([`read_skeletons`]).

use std::collections::BTreeSet;

use primase_core::{Entity, World};

use super::entry::read_entry;
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::Source;
use frostbite_formats::scope::Scope;
use frostbite_formats::{Result, skeleton};

/// The skeleton assets of the bundles of `scope` as entities of `w`, each with its `frostbite.skeleton`: the EBX
/// documents named `*_skeleton` whose root type is [`skeleton::ROOT`] (the game's naming of them: an asset not so
/// named is not found). Where two bundles hold a document of one name the earlier bundle's is read, as ids and
/// names are looked up in a scope. A skinned mesh finds its skeleton among these ([`crate::passes`]); a mesh's bone
/// lists name no skeleton, so a world with none links none.
pub fn read_skeletons(src: &Source<'_>, scope: &Scope<'_>, w: &mut World) -> Result<Vec<Entity>> {
    let mut seen = BTreeSet::new();
    let mut made = Vec::new();
    for bundle in scope.bundles() {
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Ebx && e.name.ends_with("_skeleton"))
        {
            if !seen.insert(entry.name.clone()) {
                continue;
            }
            let document = frostbite_formats::ebx::read(&src.payload(entry.file, entry.size as usize)?)?;
            if document.root_type == skeleton::ROOT {
                made.extend(read_entry(src, bundle, &entry, w)?);
            }
        }
    }
    Ok(made)
}
