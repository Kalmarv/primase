//! The rider's slots, parts and variants as the customization BuildTables list them, on Riders Republic's forges.
use primase_anvil::read::archive::Archive;
use primase_anvil::read::rider::{RiderCatalog, rider_catalog, stated};
use primase_testdata::{Game, install};

/// (slot key, parts, parts listing variants, parts whose variants state one) of every slot.
fn counts(c: &RiderCatalog) -> Vec<(String, usize, usize, usize)> {
    c.slots
        .iter()
        .map(|s| {
            (
                s.key.clone(),
                s.parts.len(),
                s.parts.iter().filter(|p| !p.variants.is_empty()).count(),
                s.parts.iter().filter(|p| stated(&p.variants).is_some()).count(),
            )
        })
        .collect()
}

#[test]
fn the_slot_tables_list_parts_and_their_variants_and_state_a_default_where_they_do() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let forges = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&forges.map(|f| dir.join(f))).expect("archive");
    let c = rider_catalog(&mut archive).expect("catalog");
    let found = counts(&c);
    println!("{found:?}");
    let expected = [
        ("ubody", 150, 149, 144),
        ("lbody", 91, 89, 87),
        ("headw", 123, 122, 119),
        ("neckw", 2, 2, 2),
        ("eyew", 20, 20, 17),
        ("bp_activities", 1, 0, 0),
        ("outfits", 27, 27, 27),
        ("heads", 12, 0, 0),
        ("eyes", 1, 0, 0),
        ("mad_chr_innermouth_u", 2, 0, 0),
    ];
    assert_eq!(
        found,
        expected
            .iter()
            .map(|(k, a, b, c)| ((*k).to_owned(), *a, *b, *c))
            .collect::<Vec<_>>()
    );
    // the slot tables' weights are odds and state no part; the heads' table states one, the outfits are not in the
    // default composition
    let by = |key: &str| c.slots.iter().find(|s| s.key == key).expect("slot");
    assert!(
        ["ubody", "lbody", "headw", "neckw", "eyew"]
            .iter()
            .all(|k| by(k).stated.is_none())
    );
    let heads = by("heads");
    let head = heads
        .parts
        .iter()
        .find(|p| Some(p.table) == heads.stated)
        .expect("a stated head");
    assert_eq!(head.name, "MAD_CHR_HEAD_000_U");
    assert!(by("ubody").in_default && !by("outfits").in_default);
    // a part's stated variant is a listed variant whose weight is 1.0
    let upper = by("ubody")
        .parts
        .iter()
        .find(|p| p.name == "MAD_CHR_UBODY_001_U_1001")
        .expect("part");
    assert_eq!(upper.variants.len(), 33);
    assert_eq!(
        stated(&upper.variants).map(|v| v.name.as_str()),
        Some("MAD_CHR_UBODY_001_U_1001_T001_V001")
    );
}
