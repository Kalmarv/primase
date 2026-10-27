//! The textures of a material's depot block, read into a world: [`TextureIndex`] is built from the TOCs a consumer
//! chooses, and [`resolve_textures`] gives every texture parameter of every `frostbite.material` of a world its
//! texture entity.
//!
//! **The chain.** A texture parameter of a depot block holds a flag and two GUIDs
//! ([`frostbite_formats::depot::Param::texture_ref`]). The first matches no EBX document of the install (0 of 987 references on
//! a sample of 15 bundles). The second is the file GUID of an EBX document (987 of 987; a `TextureAsset`, or for 294 a
//! `TextureArrayAsset`) whose `Resource` field is the id of a `Texture` resource (987 of 987), and that resource is
//! read as any texture resource is ([`read_entry`]: a core texture of the levels the header's chunk holds, and its header). The
//! streamable texture set of a material does not hold the link (neither the resource id nor the GUID occurs in its
//! bytes, 0 of 987): it lists the material's texture parameters ([`frostbite_formats::lookup`]).
//!
//! **Where the assets lie.** The material's own bundle holds the asset and its resource for 353 of the 987, and the
//! other 634 only in other bundles of the install (`items.toc`, `buildkititems.toc`, `customization.toc`), so a
//! lookup limited to the bundle, the game configuration's and the level root's leaves most references unresolved. The
//! index covers the TOCs it is built from, in the order given; a GUID or a resource id is found at its first place in
//! that order.
//!
//! **Which documents the index reads.** A texture document has the name of its `Texture` resource (987 of 987 on the
//! sample), so the index reads the file GUID of the EBX documents whose name is the name of a `Texture` resource of
//! the TOCs, not of every document. A texture document named otherwise is not found, and the reference is `Missing`
//! with that reason.

use std::collections::{BTreeMap, HashMap, HashSet};

use primase_core::{Entity, World};

use super::entry::read_entry;
use crate::components::{FrostbiteMaterial, MaterialTexture, TextureLink};
use frostbite_formats::bundle::Kind;
use frostbite_formats::ebx::Value;
use frostbite_formats::folder::Source;
use frostbite_formats::listing::Bundle;
use frostbite_formats::{Result, TEXTURE, ebx};

/// A texture parameter of a material: its name hash and the second GUID of its reference.
type Reference = (u64, [u8; 16]);

/// A place in the index: a bundle and the entry's place among the manifest's entries of its kind.
type Place = (u32, u32);

/// The texture assets and `Texture` resources of some TOCs: file GUID of an EBX `TextureAsset` to its place, and
/// resource id of a `Texture` resource to its place.
#[derive(Debug, Default)]
pub struct TextureIndex {
    bundles: Vec<Bundle>,
    assets: HashMap<[u8; 16], Place>,
    textures: HashMap<u64, Place>,
}

impl TextureIndex {
    /// The index of the TOCs `tocs` (paths relative to the data folder), in that order. Opens every bundle of each and
    /// reads the file GUID of every EBX document named like a `Texture` resource of them.
    pub fn build(src: &Source<'_>, tocs: &[&str]) -> Result<TextureIndex> {
        let mut bundles = Vec::new();
        for toc in tocs {
            bundles.extend(src.listing(toc)?.bundles);
        }
        let mut textures: HashMap<u64, Place> = HashMap::new();
        let mut names: HashSet<&str> = HashSet::new();
        for (b, bundle) in bundles.iter().enumerate() {
            for (i, a) in bundle.manifest.res.iter().enumerate() {
                if a.rtype == TEXTURE {
                    textures.entry(a.rid).or_insert((b as u32, i as u32));
                    names.insert(a.name.as_str());
                }
            }
        }
        let mut assets: HashMap<[u8; 16], Place> = HashMap::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for (b, bundle) in bundles.iter().enumerate() {
            for (i, a) in bundle.manifest.ebx.iter().enumerate() {
                if !names.contains(a.name.as_str()) || !seen.insert(a.name.as_str()) {
                    continue;
                }
                let entry = super::ebx(bundle, i)?;
                let guid = ebx::read_file_guid(&src.payload(entry.file, entry.size as usize)?)?;
                assets.entry(guid).or_insert((b as u32, i as u32));
            }
        }
        Ok(TextureIndex {
            bundles,
            assets,
            textures,
        })
    }

    /// The bundles the index holds.
    pub fn bundles(&self) -> usize {
        self.bundles.len()
    }

    /// The EBX documents found by file GUID.
    pub fn assets(&self) -> usize {
        self.assets.len()
    }

    /// The `Texture` resources found by id.
    pub fn textures(&self) -> usize {
        self.textures.len()
    }
}

/// What [`resolve_textures`] did: the texture references of the world's materials, how many became a texture
/// entity, and for the others the reason (the same reason the reference's [`TextureLink::Missing`] states).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextureReport {
    pub references: usize,
    pub linked: usize,
    /// The distinct textures read into the world.
    pub textures: usize,
    pub missing: BTreeMap<String, usize>,
}

/// A refusal's reason without the entry it names: the text after the last `"): "` of an unsupported-entry message
/// made by the entry reader.
fn category(why: &str) -> &str {
    why.rsplit_once("): ").map_or(why, |(_, rest)| rest)
}

/// Links every texture parameter of every `frostbite.material` of `w` to a texture entity: the asset of the
/// parameter's second GUID found in `index`, its `Resource` id's `Texture` resource read into `w` (once for each
/// resource, however many parameters name it). A reference that does not lead to a texture is a
/// [`TextureLink::Missing`] with the reason, and the longer text with the entry's name is in the material's
/// `unresolved`. The links of a material replace those of an earlier call.
pub fn resolve_textures(src: &Source<'_>, index: &TextureIndex, w: &mut World) -> TextureReport {
    let mut report = TextureReport::default();
    let mut by_resource: HashMap<u64, std::result::Result<Entity, String>> = HashMap::new();
    let mut by_asset: HashMap<[u8; 16], std::result::Result<u64, String>> = HashMap::new();
    let materials: Vec<(Entity, Vec<Reference>)> = w
        .iter::<FrostbiteMaterial>()
        .map(|(e, m)| {
            let refs = m.depot.as_ref().map_or_else(Vec::new, |d| {
                d.parameters
                    .iter()
                    .filter_map(|p| {
                        let t = p.texture_ref(&d.block.data)?;
                        Some((p.name, <[u8; 16]>::try_from(&t[17..33]).ok()?))
                    })
                    .collect()
            });
            (e, refs)
        })
        .collect();
    for (material, refs) in materials {
        let mut links = Vec::with_capacity(refs.len());
        let mut notes = Vec::new();
        for (name, guid) in refs {
            report.references += 1;
            let found = if guid == [0; 16] {
                Err((
                    "no texture is assigned (the second GUID is zero)".to_owned(),
                    None,
                ))
            } else {
                let rid = by_asset
                    .entry(guid)
                    .or_insert_with(|| asset_resource(src, index, &guid))
                    .clone();
                match rid {
                    Err(why) => Err((why, None)),
                    Ok(rid) => by_resource
                        .entry(rid)
                        .or_insert_with(|| read_texture(src, index, rid, w))
                        .clone()
                        .map_err(|why| (category(&why).to_owned(), Some(why))),
                }
            };
            links.push(MaterialTexture {
                name,
                guid,
                link: match found {
                    Ok(entity) => {
                        report.linked += 1;
                        TextureLink::Texture(entity)
                    }
                    Err((why, long)) => {
                        *report.missing.entry(why.clone()).or_default() += 1;
                        notes.push(format!(
                            "texture {name:#018x}: {}",
                            long.as_deref().unwrap_or(&why)
                        ));
                        TextureLink::Missing(why)
                    }
                },
            });
        }
        if let Some(m) = w.get_mut::<FrostbiteMaterial>(material) {
            m.unresolved.retain(|u| !u.starts_with("texture "));
            m.unresolved.extend(notes);
            m.textures = links;
        }
    }
    report.textures = by_resource.values().filter(|r| r.is_ok()).count();
    report
}

/// The `Resource` id of the texture document with file GUID `guid`, or why there is none.
fn asset_resource(
    src: &Source<'_>,
    index: &TextureIndex,
    guid: &[u8; 16],
) -> std::result::Result<u64, String> {
    let &(b, i) = index
        .assets
        .get(guid)
        .ok_or("the GUID is the file GUID of no texture document of the index")?;
    let entry = super::ebx(&index.bundles[b as usize], i as usize).map_err(|e| e.to_string())?;
    let doc = src
        .payload(entry.file, entry.size as usize)
        .and_then(|bytes| ebx::read(&bytes))
        .map_err(|e| format!("the texture asset {} is unreadable: {e}", entry.name))?;
    match doc
        .instances
        .first()
        .and_then(|i| i.object.as_ref())
        .and_then(|o| o.field("Resource"))
    {
        Some(Value::Resource(rid)) => Ok(*rid),
        _ => Err(format!("the document is a {} with no Resource", doc.root_type)),
    }
}

/// The `Texture` resource `rid` of the index read into `w`, or why it could not be.
fn read_texture(
    src: &Source<'_>,
    index: &TextureIndex,
    rid: u64,
    w: &mut World,
) -> std::result::Result<Entity, String> {
    let &(b, i) = index
        .textures
        .get(&rid)
        .ok_or("the asset's Texture resource is in no bundle of the index")?;
    let bundle = &index.bundles[b as usize];
    let entry = super::resource(bundle, i as usize).map_err(|e| e.to_string())?;
    debug_assert_eq!(entry.kind, Kind::Res);
    match read_entry(src, bundle, &entry, w) {
        Ok(made) => made
            .first()
            .copied()
            .ok_or_else(|| "the Texture resource made no entity".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::category;

    #[test]
    fn a_refusal_is_cut_to_what_it_says_of_the_texture() {
        assert_eq!(
            category("characters/hair/a_f (win32/hair/b): texture format 31"),
            "texture format 31"
        );
        assert_eq!(
            category("the asset's Texture resource is in no bundle"),
            "the asset's Texture resource is in no bundle"
        );
    }
}
