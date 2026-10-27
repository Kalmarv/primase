#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! Every component ped's variation info (`.ymt` beside its `.ydd`) read from the install: the info reads strictly
//! ([`PedVariations::from_root`]: every slot names its component, every component one slot), and each of its
//! drawables resolves to exactly one entry of the ped's drawable dictionary by the name the game builds
//! (`<slot>_<index>_<suffix>`, [`PedVariations::entries`]), with no entry left over: on the install, 4,316 entries of
//! 563 peds, 2,106 named `r`, 2,110 `u` and 100 `m` (measured). The counts are printed.
//!
//! Three `.ymt` files (`slod_human` among them) are not resources (plain binaries): counted, not read.
//!
//! Needs the GTA install and its key material.
use primase_testdata::{Game, install, keys};
use rage_engine_formats::archive::archive_set::Install;
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::archive::listing::Storage;
use rage_engine_formats::drawable::DrawableDictionary;
use rage_engine_formats::meta::Meta;
use rage_engine_formats::ped::PedVariations;

#[test]
fn every_ped_drawable_has_its_entry() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let keys = Keys::from_dir(&keys, None).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let (mut peds, mut entries, mut not_resources, mut without_dictionary) = (0usize, 0usize, 0usize, 0usize);
    let mut by_suffix = std::collections::BTreeMap::<char, usize>::new();
    for f in install
        .listing()
        .filter(|f| f.extension == "ymt" && f.archive.contains("componentpeds"))
    {
        if !matches!(f.storage, Storage::Resource { .. }) {
            not_resources += 1;
            continue;
        }
        let root = Meta::read(&install.resource(f.file, &keys).unwrap())
            .and_then(|m| m.root())
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        let info = PedVariations::from_root(&root).unwrap_or_else(|e| panic!("{}: {e}", f.path));
        let Some(ydd) = install.file(&f.path.replace(".ymt", ".ydd")) else {
            without_dictionary += 1;
            continue;
        };
        let dictionary = DrawableDictionary::read_resource(&install.resource(ydd, &keys).unwrap()).unwrap();
        let resolved = info
            .entries(&dictionary.hashes)
            .unwrap_or_else(|e| panic!("{}: {e}", f.path));
        peds += 1;
        entries += resolved.len();
        for e in resolved {
            *by_suffix.entry(e.suffix).or_default() += 1;
        }
    }
    println!(
        "{peds} peds, {entries} entries resolved {by_suffix:?}; {not_resources} ymt files that are no resource, {without_dictionary} peds without a dictionary"
    );
    assert!(peds > 0);
    assert_eq!(entries, by_suffix.values().sum::<usize>());
}
