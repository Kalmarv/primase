//! Every texture binding of every MaterialTemplate of Riders Republic's base and USNAT forges with their patches
//! (build 25394987) that links a TextureObject has its TextureSelector read ([`texture_parameter`]), none shared by
//! two bindings of one template; a binding may link a ShaderCustomTextureOperator, whose own selector is read the same
//! way ([`template::custom_texture`]); the others link another Operator or nothing.
//! Against the notes' worked examples (material-spec §27.0): MAD_Screens_Simple's t76 (slot 0, channel 1, the
//! Pixels_Screen_A set and its DiffuseMap), MAD_Tex1_Graffiti_Mode_DecalFX's t76 (slot 11, channel 2, the bare
//! texture GFX_Jammer_NormalMap) and MAD_Screens_Simple's t75 (slot 0, channel 0, no default).
use std::collections::BTreeSet;

use anvil_formats::archive::{Archive, At};
use anvil_formats::template::{self, BindingKind, MATERIAL_TEMPLATE, OperatorDefault, texture_parameter};
use primase_testdata::{Game, install};

#[test]
fn every_texture_binding_has_its_parameter() {
    let Some(dir) = install(Game::Riders) else {
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
                    None => {
                        // it links no TextureObject: another Operator, or none
                        assert!(matches!(
                            template::operator(&body, b).expect("reads"),
                            None | Some(OperatorDefault::Other(_))
                        ));
                        missing.push(format!("{} t{}", n.name, 75 + shader));
                    }
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
        "{templates} templates, {bindings} texture bindings, {defaults} with a default texture; by (channel, set, texture) {kinds:?}; missing {missing:?}"
    );
    // 8 texture bindings link nothing (the character screens' generic two); the 34 that link a custom texture
    // operator are read
    assert_eq!(missing.len(), 8);
    // no channel-0 record names a default; a channel-1 default is a set and its texture; a channel-2 one a bare
    // texture (the resolution's steps, material-spec §27.0)
    assert!(kinds.keys().all(|&(channel, set, texture)| match channel {
        0 => !set && !texture,
        1 => set == texture,
        2 => !set,
        _ => false,
    }));
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

/// The first TextureParameter marker at or after where the binding's linked id first occurs (but in itself): the
/// notes' first reading (material-spec §27.0), kept here to check the link's reading against.
fn searched(body: &[u8], b: &template::Binding) -> Option<usize> {
    let id = b.link()?.to_le_bytes();
    let object = body
        .windows(8)
        .enumerate()
        .position(|(at, w)| w == id && at != b.at + 26)?;
    let marker = template::TEXTURE_PARAMETER.to_le_bytes();
    body[object..]
        .windows(4)
        .position(|w| w == marker)
        .map(|k| object + k)
}

/// Every binding's linked Operator read (registry-named-fields §12), its kind by the binding's; each texture binding's
/// TextureSelector by the link where the notes' search finds it; the registry's worked examples (DataPC bodies).
#[test]
fn every_binding_links_its_operator() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut classes: std::collections::BTreeMap<(u32, u32), u64> = Default::default();
    let (mut selectors, mut agree) = (0u64, 0u64);
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
            for b in &all {
                let op = template::operator(&body, b).expect("the operator reads");
                let class = match op {
                    None => 0,
                    Some(OperatorDefault::Float(_)) => template::CONSTANT_FLOAT,
                    Some(OperatorDefault::Vector(_)) => template::CONSTANT_VECTOR,
                    Some(OperatorDefault::Integer(_)) => template::CONSTANT_INTEGER,
                    Some(OperatorDefault::Uv { .. }) => template::UV_TRANSFORM_OPERATOR,
                    Some(OperatorDefault::Texture { selector, .. }) => {
                        selectors += 1;
                        agree += u64::from(searched(&body, b) == Some(selector.at));
                        template::TEXTURE_OBJECT
                    }
                    Some(OperatorDefault::CustomTexture { selector, .. }) => {
                        selectors += 1;
                        agree += u64::from(searched(&body, b) == Some(selector.at));
                        template::SHADER_CUSTOM_TEXTURE_OPERATOR
                    }
                    Some(OperatorDefault::Other(c)) => c,
                };
                *classes.entry((b.f[3], class)).or_default() += 1;
            }
        }
    }
    println!(
        "(Type, Operator class): {classes:x?}; selectors {selectors}, where the search found them {agree}"
    );
    assert_eq!(agree, selectors, "the link reaches the record the search found");
    // each kind links its Operator: scalars a ConstantFloatValue, vectors a ConstantVectorValue, UV transforms a
    // UVTransformOperator (or, on a few, a custom operator, an oscillator or an animation: no default read)
    for &(kind, class) in classes.keys() {
        let expected: &[u32] = match kind {
            0 => &[template::CONSTANT_FLOAT],
            2 => &[template::CONSTANT_VECTOR],
            1 => &[template::CONSTANT_INTEGER],
            6 => &[template::UV_TRANSFORM_OPERATOR],
            3 => &[template::TEXTURE_OBJECT],
            _ => &[],
        };
        let custom = !matches!(
            class,
            template::CONSTANT_FLOAT
                | template::CONSTANT_VECTOR
                | template::CONSTANT_INTEGER
                | template::UV_TRANSFORM_OPERATOR
                | template::TEXTURE_OBJECT
        );
        assert!(
            expected.contains(&class) || custom || class == 0,
            "Type {kind} linking class {class:08x}"
        );
    }
    // the registry's examples
    let mut archive = Archive::open(&[dir.join("DataPC.forge")]).expect("archive");
    let mut at = |entry: usize, link: u64| -> OperatorDefault {
        let body = archive.resource(At { forge: 0, entry }).expect("template").body;
        let b = template::bindings(&body)
            .expect("bindings")
            .into_iter()
            .find(|b| b.link() == Some(link))
            .expect("the binding");
        template::operator(&body, &b)
            .expect("reads")
            .expect("an operator")
    };
    assert_eq!(
        at(4236, 0x1C5_3BAE_D307),
        OperatorDefault::Vector([16.0, 9.0, 6.5, 0.0])
    );
    assert_eq!(at(4240, 0x1C5_3BAE_4C34), OperatorDefault::Float(5.0));
    assert_eq!(at(62685, 0x1D4_34FD_8FEE), OperatorDefault::Integer(0));
    assert_eq!(
        at(16159, 0x1F7_9C49_1129),
        OperatorDefault::Uv {
            floats: [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            flags: [0, 0, 0]
        }
    );
    let OperatorDefault::Texture {
        selector,
        sampler_mode,
    } = at(12659, 0x1F7_9C48_C840)
    else {
        panic!("a texture")
    };
    assert_eq!(
        (
            selector.channel,
            selector.slot,
            selector.set,
            selector.texture.map(|r| r.id),
            sampler_mode
        ),
        (2, 0, None, Some(0x18A_F99B_3770), 0)
    );
}
