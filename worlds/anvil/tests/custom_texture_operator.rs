//! The ShaderCustomTextureOperators of Riders Republic's MaterialTemplates (build 25394987), their own TextureSelectors
//! read ([`template::custom_texture`]), and what resolves through them. All 36 operators of the base and USNAT forges
//! with their patches decode; Waterfall's selects its TextureSet's slot 3, a HeightMap. The Area 52 Tex2 Materials
//! m_MAD_MAT_TIL_Concrete_BareGraffitiBlend_A, Concrete_PanelsHeatDamagedBlend_A and Roof_WornBitumenBlend_A are read
//! and lowered: which bindings resolve, from where.
mod inputs;

use std::collections::BTreeMap;

use anvil_formats::archive::{Archive, At};
use anvil_formats::material::MATERIAL;
use anvil_formats::template::{self, MATERIAL_TEMPLATE, SHADER_CUSTOM_TEXTURE_OPERATOR};
use inputs::{Game, install};
use primase_anvil::components::{AnvilMaterial, TextureFrom};
use primase_anvil::passes::{AlphaChoice, Decals1Cutout, LowerAnvilMaterial, ShaderUv};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::layered::LayeredMaterial;
use primase_core::pass::{Pass, PassLog};
use primase_core::{Name, NormalGreen, World};

const ORDER: [&str; 4] = [
    "DataPC_MAD_USNAT_patch_01.forge",
    "DataPC_MAD_USNAT.forge",
    "DataPC_patch_01.forge",
    "DataPC.forge",
];

/// The three Area 52 Tex2 Materials, by name.
const AREA_52: [&str; 3] = [
    "MAD_MAT_TIL_Concrete_BareGraffitiBlend_A",
    "MAD_MAT_TIL_Concrete_PanelsHeatDamagedBlend_A",
    "MAD_MAT_TIL_Roof_WornBitumenBlend_A",
];

/// Every ShaderCustomTextureOperator in the templates of the four forges, read by its class id's place: each walked
/// ([`template::custom_texture`]), with its template's name and forge.
#[test]
fn all_36_operators_decode_and_waterfalls_selects_slot_3() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let class = SHADER_CUSTOM_TEXTURE_OPERATOR.to_le_bytes();
    let mut selects: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    let mut sampler: BTreeMap<u32, u64> = BTreeMap::new();
    let mut found = 0u64;
    let mut waterfall = None;
    let mut bindings = 0u64;
    for forge in ORDER {
        let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
        let names = archive.forge(0).expect("forge").names.clone();
        for (entry, n) in names.iter().enumerate() {
            if n.type_id != MATERIAL_TEMPLATE {
                continue;
            }
            let body = archive.resource(At { forge: 0, entry }).expect("template").body;
            for (at, w) in body.windows(4).enumerate() {
                if w != class {
                    continue;
                }
                let (selector, mode) = template::custom_texture(&body, at)
                    .unwrap_or_else(|e| panic!("{} in {forge}: {e}", n.name));
                found += 1;
                *selects.entry((selector.channel, selector.slot)).or_default() += 1;
                *sampler.entry(mode).or_default() += 1;
                if n.name == "MAD_Waterfall" {
                    waterfall = Some((selector.channel, selector.slot));
                }
            }
            let Ok(all) = template::bindings(&body) else {
                continue;
            };
            bindings += all
                .iter()
                .filter(|b| {
                    matches!(
                        template::operator(&body, b),
                        Ok(Some(template::OperatorDefault::CustomTexture { .. }))
                    )
                })
                .count() as u64;
        }
    }
    println!("{found} operators; selects {selects:?}; sampler modes {sampler:?}; {bindings} bindings");
    assert_eq!(found, 36);
    // an independent walk of the stored bytes (a script over the same bodies) finds these
    assert_eq!(
        selects,
        BTreeMap::from([
            ((0, 0), 2),
            ((0, 1), 6),
            ((0, 3), 2),
            ((0, 7), 2),
            ((0, 11), 10),
            ((2, 11), 12),
            ((2, 17), 2)
        ])
    );
    assert_eq!(sampler, BTreeMap::from([(0, 30), (1, 2), (2, 4)]));
    assert_eq!(
        bindings, 34,
        "the bindings that link one: two operators are linked by none"
    );
    assert_eq!(waterfall, Some((0, 3)));
}

/// The first Material called `name` in the forges (patches first), read by a placer.
fn material(archive: &mut Archive, name: &str) -> u64 {
    for f in 0..ORDER.len() {
        let forge = archive.forge(f).expect("forge");
        for (entry, n) in forge.names.iter().enumerate() {
            if n.type_id == MATERIAL && n.name == name {
                return forge.index.get(entry).map(|i| i.id).expect("id");
            }
        }
    }
    panic!("{name}");
}

/// The Materials of the three Area 52 Tex2 blends have no ShaderCustomTextureOperator behind them: their template,
/// MAD_Tex2, binds ordinary TextureObjects, and what leaves each drawing a placeholder is a mask the Material records
/// with a null texture (t79, layer B's, and t80, layer A's) while its TextureSet's slot 7 is empty too. Under the
/// stated `unset=white` the blend lowers, the masks counted.
#[test]
fn the_area_52_tex2_blends_lower_with_their_unset_masks_white() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    let ids: Vec<u64> = AREA_52.iter().map(|n| material(&mut archive, n)).collect();
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut entities = Vec::new();
    {
        let mut placer = Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot);
        for &id in &ids {
            entities.push(placer.material(id, &mut placed).expect("read"));
        }
    }
    // t75 to t78 (both layers' normal maps and diffuses) are the Materials' own records; the masks t79 and t80 are
    // records with no texture, and nothing in the TextureSet or the template's record to fall back on
    let mut null_masks = 0;
    for &e in &entities {
        let m = w.get::<AnvilMaterial>(e).expect("material");
        assert_eq!(m.template_name.as_deref(), Some("MAD_Tex2"));
        for t in &m.textures {
            match t.shader_slot {
                0..=3 => assert!(t.texture.is_some() && t.from == TextureFrom::Record, "{t:?}"),
                _ => {
                    assert!(
                        matches!(t.from, TextureFrom::NullRecord { fallback: None }) || t.texture.is_some(),
                        "{t:?}"
                    );
                    null_masks += u64::from(t.texture.is_none());
                }
            }
        }
    }
    assert_eq!(null_masks, 5);
    let pass = LowerAnvilMaterial {
        normal_green: NormalGreen::Up,
        alpha: AlphaChoice::Opaque,
        shader_uv: ShaderUv::TexCoord,
        emissive_exposure: 1.0,
        underground_view: 0.5,
        fake_river_s: 0.0,
        decals1_cutout: Decals1Cutout::Surface,
    };
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("lower");
    println!("{log:?}");
    assert_eq!(
        log.counts
            .get("MAD_Tex2 Materials lowered (a height blend of two layers)"),
        Some(&3)
    );
    assert_eq!(log.counts.get("Tex2 masks unset: white (stated)"), Some(&5));
    assert!(
        !log.omitted.keys().any(|k| k.contains("Tex2 Materials missing")),
        "{:?}",
        log.omitted
    );
    for &e in &entities {
        let layered = w.get::<LayeredMaterial>(e).expect("a height blend");
        assert!(layered.layers.iter().all(Option::is_some), "both layers");
    }
}

/// A binding that links a ShaderCustomTextureOperator is read through the operator's own selector, as a TextureObject's
/// is: Waterfall's t75 names channel 0, slot 3, and its Material's texture there is its TextureSet's slot 3, the
/// HeightMap; Grass_AO's t76 names channel 0, slot 1, the cactus Material's NormalMap.
#[test]
fn custom_operator_bindings_name_the_set_slot_their_materials_texture_comes_from() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    let waterfall = material(&mut archive, "MAD_MAT_Waterfall");
    let cactus = material(&mut archive, "VEG-Des-CactusSkin01");
    let mut w = World::new();
    let mut placed = Placed::default();
    let (wf, cs) = {
        let mut placer = Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot);
        (
            placer.material(waterfall, &mut placed).expect("read"),
            placer.material(cactus, &mut placed).expect("read"),
        )
    };
    let name_of = |e| w.get::<Name>(e).map(|n| n.0.clone());
    for (e, shader_slot, want, set_slot, texture_name) in [
        (wf, 0, (3, 0), 3, "MAD_TEX_TIL_Waterfall_HeightMap"),
        (cs, 1, (1, 0), 1, "VEG-Des-CactusSkin01_NormalMap"),
    ] {
        let m = w.get::<AnvilMaterial>(e).expect("material");
        let t = m
            .textures
            .iter()
            .find(|t| t.shader_slot == shader_slot)
            .expect("bound");
        // (slot, channel) of the operator's selector
        assert_eq!(t.parameter, Some(want), "{t:?}");
        let texture = t.texture.expect("a texture");
        assert_eq!(
            m.texture_set.get(set_slot).copied().flatten(),
            Some(texture),
            "{t:?}"
        );
        assert_eq!(name_of(texture).as_deref(), Some(texture_name));
        assert!(
            matches!(t.from, TextureFrom::Record | TextureFrom::Set),
            "{:?}",
            t.from
        );
    }
}
