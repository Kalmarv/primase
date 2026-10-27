//! The rider's slots, parts and variants as the customization BuildTables list them, on Riders Republic's forges.
use anvil_formats::archive::Archive;
use anvil_formats::rider::{BEARD_COLOURS, HAIR_COLOURS, RiderCatalog, colours, rider_catalog, stated};
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
        ("footw", 17, 16, 16),
        ("headw", 123, 122, 119),
        ("neckw", 2, 2, 2),
        ("eyew", 20, 20, 17),
        ("bp_activities", 1, 0, 0),
        ("outfits", 27, 27, 27),
        ("heads", 12, 0, 0),
        ("eyes", 1, 0, 0),
        ("hair_activator", 61, 0, 0),
        ("beard_activator", 94, 0, 0),
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
    // the hairs table lists no part itself: its activators are slots whose parts are the styles of each haircut and the
    // per-head tables of each beard (the hair materials table is no slot), and their weights are odds
    let hair = by("hair_activator");
    let named =
        |slot: &anvil_formats::rider::RiderSlot, name: &str| slot.parts.iter().any(|p| p.name == name);
    assert!(named(hair, "MAD_CHR_Hair_005_U_Full") && named(hair, "MAD_CHR_Hair_005_U_Cut"));
    assert_eq!(hair.table_name, "MAD_CHR_BODY_HAIR_ACTIVATOR");
    let beard = by("beard_activator");
    assert!(named(beard, "MAD_CHR_Beard_001_U_H000") && named(beard, "MAD_CHR_Beard_001_U_H006"));
    assert!(hair.stated.is_none() && beard.stated.is_none());
    assert!(!c.slots.iter().any(|s| s.table_name == "MAD_CHR_HAIR_Materials"));
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

/// The hair and beard colour tables list one row per colour, each row named by its own condition and linking the
/// Material of that colour (the beards' table the same colours' Materials).
#[test]
fn the_colour_tables_list_a_material_per_row_and_name_the_row_by_its_condition() {
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
    let hair = colours(&mut archive, HAIR_COLOURS).expect("hair colours");
    let beard = colours(&mut archive, BEARD_COLOURS).expect("beard colours");
    let distinct = |list: &[anvil_formats::rider::Colour]| {
        let mut conditions: Vec<u32> = list.iter().map(|c| c.condition).collect();
        conditions.sort_unstable();
        conditions.dedup();
        conditions.len()
    };
    // one hair condition is shared by two rows, which `Variant::Row` refuses
    assert_eq!((hair.len(), distinct(&hair)), (50, 49));
    assert_eq!((beard.len(), distinct(&beard)), (49, 49));
    assert_eq!(hair[0].name, "MAD_CHR_HairCut_V001_Blond");
    assert_eq!(beard[0].name, "MAD_CHR_HairCut_V001_Blond");
    assert_eq!(
        hair.last().map(|c| c.name.as_str()),
        Some("MAD_CHR_HairCut_V047_BlackRed")
    );
}
