//! Bundles searched together: the way the game resolves a resource id or a name, which it looks up in every bundle
//! loaded. A bundle with a program database holds the permutations it adds and names the stock ones, which lie in
//! another bundle (the game's configuration bundle in `globals.toc`, a level's root).

use std::collections::HashMap;

use crate::inputlayout;
use crate::listing::{Bundle, Listed};
use crate::{binding, bytecode, lookup, permutation};

/// Bundles that ids and names are looked up in, with the index of what they hold built once: the permutations by type
/// and id, the CompiledBytecode resources by name. Where two bundles hold the same id or name, the earlier wins.
#[derive(Debug)]
pub struct Scope<'a> {
    bundles: &'a [Bundle],
    /// (type, id) -> (bundle, resource index in its manifest).
    permutations: HashMap<(u32, u64), (usize, usize)>,
    bytecode: HashMap<&'a str, (usize, usize)>,
}

impl<'a> Scope<'a> {
    pub fn new(bundles: &'a [Bundle]) -> Self {
        let (mut permutations, mut bytecode) = (HashMap::new(), HashMap::new());
        for (b, bundle) in bundles.iter().enumerate() {
            for (i, a) in bundle.manifest.res.iter().enumerate() {
                match a.rtype {
                    permutation::RASTER
                    | permutation::COMPUTE
                    | permutation::SHARED_RASTER
                    | permutation::SHARED_COMPUTE
                    | binding::BINDING_SET
                    | binding::PASS_BINDING
                    | inputlayout::RTYPE
                    | lookup::STREAMABLE_TEXTURE_SET => {
                        permutations
                            .entry((a.rtype, permutation::resource_id(&a.rmeta)))
                            .or_insert((b, i));
                    }
                    bytecode::RTYPE => {
                        bytecode.entry(a.name.as_str()).or_insert((b, i));
                    }
                    _ => {}
                }
            }
        }
        Scope {
            bundles,
            permutations,
            bytecode,
        }
    }

    /// The bundles, in lookup order.
    pub fn bundles(&self) -> &'a [Bundle] {
        self.bundles
    }

    /// The entry of the resource of type `rtype` with `id` in the first bundle of the scope that holds one (a
    /// permutation or a streamable texture set).
    pub fn resource_by_id(&self, rtype: u32, id: u64) -> Option<Listed> {
        let &(b, i) = self.permutations.get(&(rtype, id))?;
        Some(self.bundles[b].resource(i))
    }

    /// The bundle and entry of the resource of type `rtype` with `id`.
    pub fn resource_with_home(&self, rtype: u32, id: u64) -> Option<(&'a Bundle, Listed)> {
        let &(b, i) = self.permutations.get(&(rtype, id))?;
        Some((&self.bundles[b], self.bundles[b].resource(i)))
    }

    /// The bundle and entry of the permutation with `id`, a raster one first.
    pub fn permutation(&self, id: u64) -> Option<(&'a Bundle, Listed)> {
        let &(b, i) = [permutation::RASTER, permutation::COMPUTE]
            .iter()
            .find_map(|t| self.permutations.get(&(*t, id)))?;
        Some((&self.bundles[b], self.bundles[b].resource(i)))
    }

    /// The bundle and entry of the CompiledBytecode resource named `name`.
    pub fn bytecode(&self, name: &str) -> Option<(&'a Bundle, Listed)> {
        let &(b, i) = self.bytecode.get(name)?;
        Some((&self.bundles[b], self.bundles[b].resource(i)))
    }

    /// The bundle `entry` was listed from, among these.
    pub fn bundle_of(&self, entry: &Listed) -> Option<&'a Bundle> {
        self.bundles
            .iter()
            .find(|b| b.toc_path == entry.toc && b.name == entry.bundle)
    }
}
