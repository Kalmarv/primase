//! Every texture binding of every MaterialTemplate of Riders Republic's base and USNAT forges with their patches
//! (build 25394987) has its TextureParameter record found and read ([`texture_parameter`]), none shared by two
//! bindings of one template; against the notes' worked examples (material-spec §27.0): MAD_Screens_Simple's t76 (slot
//! 0, channel 1, the Pixels_Screen_A set and its DiffuseMap; the character screens' two generic bindings link none), MAD_Tex1_Graffiti_Mode_DecalFX's t76 (slot 11, channel
//! 2, the bare texture GFX_Jammer_NormalMap) and MAD_Screens_Simple's t75 (slot 0, channel 0, no default).
use std::collections::BTreeSet;

use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::template::{self, BindingKind, MATERIAL_TEMPLATE, texture_parameter};
use primase_testdata::real_data;

#[test]
fn every_texture_binding_has_its_parameter() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let (mut templates, mut bindings, mut defaults) = (0u64, 0u64, 0u64);
    let mut missing: Vec<String> = Vec::new();
    let mut seen = Vec::new();
    for forge in [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ] {
        let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
        let names = archive.forge(0).expect("forge").names.clone();
        for (entry, n) in names.iter().enumerate() {
            if n.type_id != MATERIAL_TEMPLATE {
                continue;
            }
            let body = archive.resource(At { forge: 0, entry }).expect("template").body;
            let Ok(all) = template::bindings(&body) else {
                continue;
            };
            templates += 1;
            let mut at = BTreeSet::new();
            for b in &all {
                let BindingKind::Texture { slot: shader } = b.kind() else {
                    continue;
                };
                bindings += 1;
                match texture_parameter(&body, b).expect("the record reads") {
                    None => missing.push(format!("{} t{}", n.name, 75 + shader)),
                    Some(p) => {
                        assert!(
                            at.insert(p.at),
                            "{}: two bindings share the record at {}",
                            n.name,
                            p.at
                        );
                        defaults += u64::from(p.texture.is_some());
                        seen.push((n.name.clone(), shader, p));
                    }
                }
            }
        }
    }
    let mut kinds: std::collections::BTreeMap<(u32, bool, bool), u64> = Default::default();
    for (_, _, p) in &seen {
        *kinds
            .entry((p.channel, p.set.is_some(), p.texture.is_some()))
            .or_default() += 1;
    }
    println!(
        "{templates} templates, {bindings} texture bindings, {defaults} with a default texture; by (channel, set,          texture) {kinds:?}; missing {missing:?}"
    );
    // only the two generic bindings of the character screen templates, in both forges that hold them, link no object
    assert_eq!(missing.len(), 8);
    assert!(missing.iter().all(
        |m| m.starts_with("MAD_CHR_Classic_noVWIND_Screen") && (m.ends_with("t75") || m.ends_with("t76"))
    ));
    let find = |template: &str, shader: u32| {
        seen.iter()
            .find(|(n, s, _)| n == template && *s == shader)
            .map(|(_, _, p)| *p)
            .unwrap_or_else(|| panic!("{template} t{}", 75 + shader))
    };
    let simple = find("MAD_Screens_Simple", 1);
    assert_eq!(
        (
            simple.slot,
            simple.channel,
            simple.set.map(|r| r.id),
            simple.texture.map(|r| r.id)
        ),
        (0, 1, Some(1_903_543_813_427), Some(1_903_543_813_482))
    );
    let decal = find("MAD_Tex1_Graffiti_Mode_DecalFX", 1);
    assert_eq!(
        (decal.slot, decal.channel, decal.set, decal.texture.map(|r| r.id)),
        (11, 2, None, Some(253_896_160_623))
    );
    let video = find("MAD_Screens_Simple", 0);
    assert_eq!(
        (video.slot, video.channel, video.set, video.texture),
        (0, 0, None, None)
    );
}
