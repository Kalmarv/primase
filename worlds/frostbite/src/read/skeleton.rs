//! The skeleton assets of a scope read into a world ([`Source::read_skeletons`]).

use std::collections::BTreeSet;

use primase_core::{Entity, World};

use super::folder::Source;
use super::program::Scope;
use crate::bundle::Kind;
use crate::{Result, skeleton};

impl Source<'_> {
    /// The skeleton assets of the bundles of `scope` as entities of `w`, each with its `frostbite.skeleton`: the EBX
    /// documents named `*_skeleton` whose root type is [`skeleton::ROOT`] (the game's naming of them: an asset not so
    /// named is not found). Where two bundles hold a document of one name the earlier bundle's is read, as ids and
    /// names are looked up in a scope. A skinned mesh finds its skeleton among these ([`crate::passes`]); a mesh's bone
    /// lists name no skeleton, so a world with none links none.
    pub fn read_skeletons(&self, scope: &Scope<'_>, w: &mut World) -> Result<Vec<Entity>> {
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
                let document = crate::ebx::read(&self.payload(entry.file, entry.size as usize)?)?;
                if document.root_type == skeleton::ROOT {
                    made.extend(self.read_entry(bundle, &entry, w)?);
                }
            }
        }
        Ok(made)
    }
}
