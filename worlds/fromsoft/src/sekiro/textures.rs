use crate::{Archive, Binder, Result, dcx::Decoder, invalid, materials::stem, tpf::Package};
use std::collections::{BTreeMap, BTreeSet};
fn package_stem(path: &str) -> String {
    let lower = path.to_lowercase();
    stem(lower.strip_suffix(".dcx").unwrap_or(&lower))
}
pub(super) enum Location {
    Split(String),
    Inline(Vec<u8>),
}
#[derive(Default)]
pub(super) struct Catalog {
    pub entries: BTreeMap<String, Location>,
}
pub(super) fn has(archive: &Archive, path: &str) -> bool {
    archive.entries.contains_key(&primase_hash::fromsoft_path(path))
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
                .entry(t.name.to_lowercase())
                .or_insert(Location::Inline(t.dds));
        }
        Ok(())
    }
    pub fn load(
        &self,
        archive: &Archive,
        decoder: &Decoder<'_>,
        wanted: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut out = BTreeMap::new();
        let mut groups = BTreeMap::<&str, BTreeSet<&str>>::new();
        for name in wanted {
            match self.entries.get(name) {
                Some(Location::Inline(bytes)) => {
                    out.insert(name.clone(), bytes.clone());
                }
                Some(Location::Split(path)) => {
                    groups.entry(path).or_default().insert(name);
                }
                None => return Err(invalid("Sekiro texture", 0, format!("missing {name}"))),
            }
        }
        for (path, names) in groups {
            let payload = format!("{}bdt", &path[..path.len() - 3]);
            let binder = Binder::read_split(
                &archive.read(path, decoder)?,
                &archive.read(&payload, decoder)?,
                decoder,
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
                    .find(|t| t.name.eq_ignore_ascii_case(&name))
                    .or_else(|| {
                        if package.textures.len() == 1 {
                            package.textures.first()
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| invalid("Sekiro texture", 0, format!("package name mismatch {name}")))?;
                out.insert(name, t.dds.clone());
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
