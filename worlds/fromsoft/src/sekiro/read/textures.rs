use crate::invalid;
use fromsoft_formats::names::stem;
use fromsoft_formats::tpf::{self, Package, TextureKind};
use fromsoft_formats::{Archive, Binder, Result, dcx::Decoder};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A TPF texture's image: a 2D DDS through the shared container reader, retaining encoded mips and colour-space
/// metadata. Roles and normal conventions belong to bindings. Cube and volume images are refused.
pub fn tpf_image(texture: &tpf::Texture) -> Result<primase_core::Texture> {
    if texture.kind != TextureKind::Image2D {
        return Err(invalid("TPF", 0, "cube/volume image import is not supported"));
    }
    primase_dds::read(&texture.dds).map_err(|e| invalid("TPF DDS", 0, e))
}
fn package_stem(path: &str) -> String {
    let lower = fromsoft_formats::names::fold(path);
    stem(lower.strip_suffix(".dcx").unwrap_or(&lower))
}
pub(super) enum Location {
    Split(String),
    /// The DDS as the package stored it, shared with the payloads a load takes from it (the catalog outlives the
    /// load, so a copy made there is held twice until the read ends).
    Inline(Arc<[u8]>),
}
#[derive(Default)]
pub(super) struct Catalog {
    pub entries: BTreeMap<String, Location>,
}
pub(super) fn has(archive: &Archive, path: &str) -> bool {
    archive.contains(path)
}
impl Catalog {
    pub fn discover(archive: &Archive, decoder: &Decoder<'_>, area: &str) -> Result<Self> {
        let mut catalog = Self::default();
        let mut seen = BTreeSet::new();
        for area in [area, "m11", "m10", "m13", "m15", "m17", "m20", "m25"] {
            if !seen.insert(area) {
                continue;
            }
            for i in 0..10 {
                let path = format!("/map/{area}/{area}_{i:04}.tpfbhd");
                if !has(archive, &path) {
                    continue;
                }
                for name in Binder::split_names(&archive.read(&path, decoder)?)? {
                    let name = package_stem(&name);
                    if name.ends_with("_l") {
                        continue;
                    }
                    catalog
                        .entries
                        .entry(name)
                        .or_insert_with(|| Location::Split(path.clone()));
                }
            }
        }
        for path in [
            "/other/maptex.tpf.dcx",
            "/other/decaltex.tpf.dcx",
            "/other/systex.tpf.dcx",
        ] {
            if has(archive, path) {
                catalog.add(&archive.read(path, decoder)?)?;
            }
        }
        Ok(catalog)
    }
    pub fn add(&mut self, bytes: &[u8]) -> Result<()> {
        for t in Package::read(bytes)?.textures {
            self.entries
                .entry(fromsoft_formats::names::fold(&t.name))
                .or_insert_with(|| Location::Inline(t.dds.into()));
        }
        Ok(())
    }
    pub fn load(
        &self,
        archive: &Archive,
        decoder: &Decoder<'_>,
        wanted: &BTreeSet<String>,
    ) -> Result<crate::sekiro::source::Payloads> {
        let mut out = BTreeMap::new();
        let mut groups = BTreeMap::<&str, BTreeSet<&str>>::new();
        for name in wanted {
            match self.entries.get(name) {
                Some(Location::Inline(bytes)) => {
                    out.insert(name.clone(), Arc::clone(bytes));
                }
                Some(Location::Split(path)) => {
                    groups.entry(path).or_default().insert(name);
                }
                None => return Err(invalid("Sekiro texture", 0, format!("missing {name}"))),
            }
        }
        for (path, names) in groups {
            let payload = format!("{}bdt", &path[..path.len() - 3]);
            // only the packages this read binds are copied out of the payload: it holds hundreds
            let binder = Binder::read_split_where(
                &archive.read(path, decoder)?,
                &archive.read(&payload, decoder)?,
                decoder,
                &|name| names.contains(package_stem(name).as_str()),
            )?;
            for f in binder.files {
                let name = package_stem(f.name.as_deref().unwrap_or(""));
                if !names.contains(name.as_str()) {
                    continue;
                }
                let package = Package::read(&f.bytes)?;
                let t = package
                    .textures
                    .iter()
                    .find(|t| fromsoft_formats::names::fold(&t.name) == name)
                    .or_else(|| {
                        if package.textures.len() == 1 {
                            package.textures.first()
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| invalid("Sekiro texture", 0, format!("package name mismatch {name}")))?;
                out.insert(name, t.dds.as_slice().into());
            }
        }
        if out.len() != wanted.len() {
            return Err(invalid("Sekiro texture", 0, "missing indexed package"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_packages_keep_texture_identity() {
        assert_eq!(package_stem(r"N:\\tex\\Stone_A.tpf.dcx"), "stone_a");
        assert_eq!(package_stem("tex/stone_a.tpf"), "stone_a");
    }
}
