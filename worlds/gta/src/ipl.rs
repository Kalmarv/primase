//! The start state of GTA's script-managed maps (IPLs): which the game's start scripts load.
//!
//! GTA's own answer is in its scripts (the building controller's state table, `REQUEST_IPL` calls), which are not read
//! yet. Until they are, the start state is the earlier skate exporter's (its collision export's `GroupDefaults`, which
//! read those scripts on game build 1.0.3889), in its order: North Yankton off; scenery a DLC patch split out
//! (`*_original`) on, or shared between the original and its variant (`*_shared`); its table ([`IPL_TABLE`]) by the
//! group's name, else the map's own; DLC collision fixes off; everything else off (only a script that requests it
//! loads it). That exporter also switched time- and weather-dependent groups by their manifest masks, which are not
//! read here yet either.

/// Script-managed map groups whose start state the earlier skate exporter's collision export fixed by hand
/// (`GroupDefaults` in its tools): (name, on at the start, why), as its building state table and script requests
/// read on game build 1.0.3889 say, and the online profile's multiplayer exteriors.
pub const IPL_TABLE: &[(&str, bool, &str)] = &[
    (
        "bh1_16_doors_shut",
        true,
        "Rockford Plaza doors, shut until the refurb state",
    ),
    ("bh1_16_refurb", false, "later story state"),
    ("bh1_47_joshhse_unburnt", true, "Josh's house before it burns"),
    ("bh1_47_joshhse_burnt", false, "later story state"),
    ("bnkheist_apt_norm", true, "Paleto bank block before the heist"),
    ("bnkheist_apt_dest", false, "later story state"),
    (
        "canyonriver01",
        true,
        "river under the rail bridge before the derailment",
    ),
    ("canyonriver01_traincrash", false, "later story state"),
    ("canyonrvrshallow", true, "normal river level"),
    ("canyonrvrdeep", false, "alternate river level"),
    ("cargoship", true, "docked cargo ship present until it is sunk"),
    ("sunkcargoship", false, "later story state"),
    ("carwash_with_spinners", true, "car wash, normal state"),
    ("carwash_without_spinners", false, "alternate state"),
    ("ch1_02_closed", true, "normal state"),
    ("ch1_02_open", false, "mission state"),
    ("chemgrill_grp1", true, "default prop state"),
    ("chop_props", false, "appears once Chop is available"),
    ("chophillskennel", false, "appears after Franklin moves house"),
    ("coronertrash", false, "appears after a mission"),
    ("crashed_cargoplane", false, "wreck appears after a mission"),
    (
        "cs1_02_cf_offmission",
        true,
        "Cluckin' Bell factory closed (off mission)",
    ),
    ("cs3_05_water_grp1", true, "default water state"),
    ("cs3_05_water_grp2", false, "alternate water state"),
    ("cs3_07_mpgates", false, "multiplayer / mission gates"),
    ("cs5_4_trains", true, "parked train, normal state"),
    ("des_farmhs_startimap", true, "farmhouse before destruction"),
    ("des_farmhs_endimap", false, "later story state"),
    ("des_protree_start", false, "North Yankton, prologue only"),
    ("des_protree_end", false, "North Yankton, prologue only"),
    ("des_smash2_startimap", true, "before destruction"),
    ("des_smash2_endimap", false, "later story state"),
    (
        "des_stilthouse_imapstart",
        true,
        "stilt house before it is pulled down",
    ),
    ("des_stilthouse_imapend", false, "later story state"),
    ("des_stilthouse_rebuild", false, "later story state"),
    ("dockcrane1", true, "dock crane, normal state"),
    (
        "dt1_03_gr_closed",
        true,
        "ground plate over the construction shaft, closed until the heist",
    ),
    ("dt1_03_shutter", true, "car park shutter, closed"),
    (
        "dt1_05_hc_req",
        false,
        "helicopter crash damage, later story state",
    ),
    ("dt1_05_hc_end", false, "later story state"),
    ("dt1_05_rubble", false, "later story state"),
    ("dt1_05_woffm", true, "FIB tower windows, off-mission state"),
    ("facelobbyfake", true, "closed Lifeinvader lobby shell"),
    ("fakeint", false, "closed car showroom, later story state"),
    ("fiblobbyfake", true, "closed FIB lobby shell"),
    ("farm", true, "O'Neil farm before it burns"),
    ("farmint_cap", true, "farm interior cap"),
    ("farm_burnt", false, "later story state"),
    ("fbi_colplug", true, "FIB tower collision plug"),
    (
        "fbi_repair",
        false,
        "FIB tower repaired after the raid, later story state",
    ),
    ("gasstation_ipl_group1", true, "gas station before the explosion"),
    ("gasstation_ipl_group2", false, "later story state"),
    ("id2_14_pre_no_int", true, "Lester's factory before the fire"),
    ("id2_14_post_no_int", false, "later story state"),
    ("jewel2fake", true, "closed jewellery store shell"),
    ("koriztempwalls", false, "mission state"),
    ("kt_carwash", true, "car wash, normal state"),
    ("kt_carwash_nobrush", false, "alternate state"),
    ("methtrailer_grp1", true, "trailer before the explosion"),
    ("methtrailer_grp2", false, "later story state"),
    ("methtrailer_grp3", false, "later story state"),
    ("mic3_chopper_debris", false, "mission state"),
    ("pcranecont", false, "mission prop (Scouting the Port)"),
    ("plane_crash_trench", false, "appears after a mission"),
    ("racetrack01", false, "only while the race is on"),
    ("railing_start", true, "bridge railing before the derailment"),
    ("railing_end", false, "later story state"),
    ("rc12b_default", true, "hospital default state"),
    ("rc12b_fixed", false, "later story state"),
    ("sc1_27_cut", false, "cutscene variant"),
    ("scafstartimap", true, "construction site before the mission"),
    ("scafendimap", false, "later story state"),
    ("smboat", true, "yacht anchored off Del Perro in story mode"),
    ("sp1_10_fake_interior", true, "closed stadium shell"),
    ("talklaugh_pipe", false, "mission prop"),
    ("tankercrash_grp1", false, "mission state"),
    ("tankercrash_grp2", false, "mission state"),
    ("tankerexp_grp0", true, "state before the tanker explosion"),
    ("tankerexp_grp1", false, "mission state"),
    ("tankerexp_grp3", false, "mission state"),
    ("trailerparka_grp1", true, "trailer park before it is destroyed"),
    ("trailerparkb_grp1", true, "trailer park before it is destroyed"),
    ("trailerparkc_grp1", true, "trailer park before it is destroyed"),
    ("trailerparkd_grp1", true, "trailer park before it is destroyed"),
    ("trailerparke_grp1", true, "trailer park before it is destroyed"),
    ("trailerparka_grp2", false, "later story state"),
    ("trailerparkb_grp2", false, "later story state"),
    ("trailerparkc_grp2", false, "later story state"),
    ("trailerparkd_grp2", false, "later story state"),
    ("trailerparke_grp2", false, "later story state"),
    ("ufo", false, "easter egg, only after 100% completion"),
    (
        "v_tunnel_hole_swap",
        true,
        "construction shaft tunnel, normal state",
    ),
    (
        "v_tunnel_hole",
        false,
        "construction shaft tunnel with the heist hole, later story state",
    ),
    ("shr_int", true, "car showroom, open at story start"),
    (
        "trevorstrailer",
        true,
        "Trevor's trailer, state before his first mission",
    ),
    ("trevorstrailertrash", false, "later story state"),
    ("trevorstrailertidy", false, "later story state"),
    ("rc12b_hospitalinterior", true, "hospital interior, normal state"),
    ("coroner_int_off", true, "coroner's office closed"),
    ("coroner_int_on", false, "mission interior"),
    (
        "post_hiest_unload",
        false,
        "jewellery store interior, heist state (jewel2fake shell is the default)",
    ),
    ("refit_unload", false, "later story state"),
    (
        "facelobby",
        false,
        "mission interior (facelobbyfake shell is the default)",
    ),
    (
        "fiblobby",
        false,
        "mission interior (fiblobbyfake shell is the default)",
    ),
    ("finbank", false, "mission interior"),
    ("farmint", false, "mission interior (farmint_cap is the default)"),
    ("id2_14_during1", false, "mission state"),
    ("id2_14_during2", false, "mission state"),
    ("hei_dlc_windows_casino", true, "casino exterior"),
    ("hei_dlc_casino_door", true, "casino exterior"),
    ("hei_dlc_vw_roofdoors_locked", true, "casino exterior"),
];

/// Streaming suffixes a map's name carries after its group's (`_strm_3`, `_long_0`, `_lod`): the group name, as that
/// exporter strips them.
pub fn ipl_base(name: &str) -> &str {
    const SUFFIXES: [&str; 9] = [
        "_lod",
        "_slod",
        "_slod2",
        "_strm",
        "_long",
        "_critical",
        "_occl",
        "_lodlights",
        "_distantlights",
    ];
    let mut n = name;
    for _ in 0..4 {
        if let Some((head, tail)) = n.rsplit_once('_')
            && !tail.is_empty()
            && tail.bytes().all(|b| b.is_ascii_digit())
        {
            n = head;
        }
        match SUFFIXES.iter().find(|s| n.ends_with(*s)) {
            Some(s) => n = &n[..n.len() - s.len()],
            None => break,
        }
    }
    n
}

/// Whether script-managed map `name` is loaded at the game's start, and why (see the module documentation).
pub fn ipl_default(name: &str) -> (bool, &'static str) {
    let name_lower = name.to_lowercase();
    let base = ipl_base(&name_lower);
    if name_lower.starts_with("prologue") {
        return (false, "North Yankton, prologue only");
    }
    if base.ends_with("_original") {
        return (true, "original scenery split out by a DLC patch");
    }
    if base.ends_with("_shared") {
        return (true, "scenery shared by the original and a DLC variant");
    }
    for key in [name_lower.as_str(), base] {
        if let Some(&(_, on, why)) = IPL_TABLE.iter().find(|(n, _, _)| *n == key) {
            return (on, why);
        }
    }
    if ["legacy_fixes", "legacyfixes", "collision_fixes", "colfix"]
        .iter()
        .any(|p| name_lower.contains(p))
    {
        return (false, "DLC collision fix patch (it overlays base geometry)");
    }
    (false, "script-managed: off until a script loads it")
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use super::*;

    #[test]
    fn ipl_defaults_follow_the_rules_and_the_table() {
        assert_eq!(ipl_base("bh1_16_original_strm_3"), "bh1_16_original");
        assert_eq!(ipl_base("apa_stilt_ch2_04_ext1"), "apa_stilt_ch2_04_ext1");
        assert!(ipl_default("ch1_02_original_strm_0").0);
        assert!(ipl_default("cs1_shared").0);
        assert!(!ipl_default("prologue01_lod").0);
        // the table: the docked cargo ship, not the sunk one; the casino's exterior
        assert!(ipl_default("cargoship").0 && !ipl_default("sunkcargoship").0);
        assert!(ipl_default("hei_dlc_casino_door").0);
        assert!(!ipl_default("hei_showroom_closed").0);
        assert!(!ipl_default("mpstunt_collision_fixes").0);
        let mut names: Vec<&str> = IPL_TABLE.iter().map(|(n, _, _)| *n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), IPL_TABLE.len(), "a name twice in the table");
    }
}
