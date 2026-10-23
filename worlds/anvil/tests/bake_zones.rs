//! `bake-anvil-material` on the character zones (`MAD_CHR_5Zones_*`): a glove Material of a rider baked at its mask's
//! size and held against the notes' formula (docs/material-spec.md section 18.2) evaluated by hand on the same texels, and the
//! whole rider lowered with the zones baked instead of drawn as placeholders.
use std::collections::BTreeMap;

use primase_anvil::components::{AnvilMaterial, MaterialBinding, MaterialValue};
use primase_anvil::passes::BakeAnvilMaterial;
use primase_anvil::read::archive::Archive;
use primase_anvil::read::build_table::BUILD_TABLE;
use primase_anvil::read::placements::{
    Gender, InstancedLevels, Placed, Placer, RiderPartChoice, RiderSpec, Variant,
};
use primase_anvil::read::skeleton::SKELETON;
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassLog, PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Name, Texture, World};
use primase_image::sampling::{Address, Levels};
use primase_testdata::{Game, install};

fn crc(name: &str) -> u32 {
    let mut hash = 0xFFFF_FFFFu32;
    for b in name.bytes() {
        hash ^= u32::from(b);
        for _ in 0..8 {
            hash = if hash & 1 != 0 {
                (hash >> 1) ^ 0xEDB8_8320
            } else {
                hash >> 1
            };
        }
    }
    !hash
}

fn open() -> Option<Archive> {
    let dir = install(Game::Riders)?;
    let forges = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    Archive::open(&forges.map(|f| dir.join(f))).ok()
}

fn id_of(a: &Archive, type_id: u32, name: &str) -> u64 {
    (0..5)
        .find_map(|f| {
            a.forge(f)?
                .entries()
                .find(|(_, n)| n.type_id == type_id && n.name == name)
                .map(|(i, _)| i.id)
        })
        .unwrap_or_else(|| panic!("no {name}"))
}

/// The rider's parts placed with their Materials and shaders read.
fn rider(archive: &mut Archive, parts: &[&str]) -> World {
    let spec = RiderSpec {
        name: "rider".to_owned(),
        skeletons: ["MAD_CHR_Skel_U", "MAD_CHR_SkelAddon_U", "MAD_CHR_SkelAddon_M"]
            .iter()
            .map(|n| id_of(archive, SKELETON, n))
            .collect(),
        gender: Gender::Male,
        parts: parts
            .iter()
            .map(|n| RiderPartChoice {
                table: id_of(archive, BUILD_TABLE, n),
                variant: Variant::Stated,
            })
            .collect(),
    };
    let mut w = World::new();
    let mut placed = Placed::default();
    Placer::new(archive, &mut w, SourceFrame::WORLD, InstancedLevels::Selector)
        .with_materials()
        .with_shaders()
        .place_rider(&spec, &mut placed);
    w
}

fn options() -> Options {
    [
        ("size", "1024"),
        ("normal-green", "up"),
        ("shader-uv", "texcoord"),
        ("mip-bias", "0"),
        ("terrain-ao", "0.5"),
        ("sun-elevation", "51.29"),
        ("view-cosine", "0.7"),
        ("camera-distance", "1000000"),
        ("unset", "white"),
    ]
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .into()
}

fn srgb8(linear: f32) -> f32 {
    (primase_image::linear_to_srgb(f64::from(linear.clamp(0.0, 1.0))) * 255.0).round() as f32
}

#[test]
fn a_zones_material_bakes_to_the_notes_formula() {
    let Some(mut archive) = open() else {
        return;
    };
    // the gloves' variant: NoVWind, no details, no motifs, no decals
    let mut w = rider(&mut archive, &["MAD_CHR_UBODY_001_U_1001"]);
    let (entity, m): (Entity, AnvilMaterial) = w
        .iter::<AnvilMaterial>()
        .find(|(e, _)| {
            w.get::<Name>(*e)
                .is_some_and(|n| n.0 == "MAD_CHR_GEN_Gloves_002_MF_T001_V004")
        })
        .map(|(e, m)| (e, m.clone()))
        .expect("the gloves' Material");
    assert_eq!(
        m.template_name.as_deref(),
        Some("MAD_CHR_5Zones_NoVWind_NoDetailsNoMotifsNoDecals")
    );
    let value = |name: &str| -> MaterialValue {
        let key = crc(name);
        m.values
            .iter()
            .find(|(k, _)| *k == key)
            .unwrap_or_else(|| panic!("no property {name}"))
            .1
            .clone()
    };
    let float4 = |name: &str| match value(name) {
        MaterialValue::Float4(v) => v,
        other => panic!("{name}: {other:?}"),
    };
    let float = |name: &str| match value(name) {
        MaterialValue::Float(v) => v,
        other => panic!("{name}: {other:?}"),
    };
    let texture_of = |name: &str| -> Levels {
        let key = crc(name);
        let e = m
            .textures
            .iter()
            .find(|t| t.key == key)
            .and_then(|t| t.texture)
            .unwrap_or_else(|| panic!("no texture {name}"));
        Levels::of(w.get::<Texture>(e).expect("texture")).expect("levels")
    };
    let mask1 = texture_of("Textures-Mask1");
    let mask2 = texture_of("Textures-Mask2");
    assert_eq!((mask1.width, mask1.height), (mask2.width, mask2.height));
    let size = mask1.width;
    assert!(
        m.bindings
            .iter()
            .any(|(_, b)| matches!(b, MaterialBinding::Vector { .. }))
    );

    let mut o = options();
    o.insert("size".into(), size.to_string());
    let pass = BakeAnvilMaterial::from_options(&o).expect("options");
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("bake");
    assert!(
        log.counts
            .iter()
            .any(|(k, n)| k.contains("5Zones_NoVWind_NoDetailsNoMotifsNoDecals Materials baked") && *n >= 1),
        "{:?} {:?}",
        log.counts,
        log.omitted
    );
    let pbr = w.get::<PbrMaterial>(entity).expect("baked Material");
    let base_entity = pbr.base_color_texture.as_ref().expect("base").texture;
    let rough_entity = pbr
        .metallic_roughness_texture
        .as_ref()
        .expect("roughness")
        .texture;
    let base = w.get::<Texture>(base_entity).expect("base texture");
    let rough = w.get::<Texture>(rough_entity).expect("roughness texture");
    assert_eq!((base.width, base.height), (size, size));

    let zone_colours: Vec<([f32; 4], [f32; 4], f32, f32)> = (1..=5)
        .map(|z| {
            (
                float4(&format!("Z{z}-Base-Albedo")),
                float4(&format!("Z{z}-Variance-Albedo")),
                float(&format!("Z{z}-Variance-Albedo-Blend-Mode")),
                float(&format!("Z{z}-Variance-Albedo-Invert")),
            )
        })
        .collect();
    let zone_roughness: Vec<(f32, f32, f32, f32)> = (1..=5)
        .map(|z| {
            (
                float(&format!("Z{z}-Base-Roughness")),
                float(&format!("Z{z}-Variance-Roughness")),
                float(&format!("Z{z}-Variance-Roughness-Blend-Mode")),
                float(&format!("Z{z}-Variance-Roughness-Invert")),
            )
        })
        .collect();
    let (mut compared, mut off, mut rough_off) = (0u64, 0u64, 0u64);
    let mut zones_seen: BTreeMap<usize, u64> = BTreeMap::new();
    let step = (size / 64).max(1) as i64;
    let mut y = 0;
    while y < i64::from(size) {
        let mut x = 0;
        while x < i64::from(size) {
            let [r, g, _, a] = mask1
                .texel(0, x, y, [Address::Wrap; 2])
                .map(|c| f32::from(c) / 255.0);
            let [_, _, _, a2] = mask2.texel(0, x, y, [Address::Wrap; 2]);
            let zone = if g > 0.75 {
                4
            } else if g > 0.25 {
                3
            } else if r <= 0.25 {
                0
            } else if r <= 0.75 {
                1
            } else {
                2
            };
            // away from the thresholds the zone is unambiguous in 8 bits
            let near = [r, g]
                .iter()
                .any(|v| [0.25f32, 0.75].iter().any(|t| (v - t).abs() < 0.01));
            if !near {
                *zones_seen.entry(zone).or_default() += 1;
                let (b, var, mode, invert) = zone_colours[zone];
                let vv = a + invert * (1.0 - 2.0 * a);
                let expected: [f32; 3] = std::array::from_fn(|c| {
                    let (b, v) = (b[c], vv * var[c]);
                    let x = if mode == 2.0 {
                        v
                    } else {
                        mode * (b + v - b * v) + b * v
                    };
                    srgb8(b + vv * (x - b))
                });
                let at = (y as usize * size as usize + x as usize) * 4;
                let got = &base.levels[0][at..at + 3];
                compared += 1;
                if got
                    .iter()
                    .zip(expected)
                    .any(|(g, e)| (f32::from(*g) - e).abs() > 1.5)
                {
                    off += 1;
                }
                // roughness from the zones (section 19.1: the permutations whose roughness is the zone's, Mask2 read for the
                // normal only): the zone's base roughness varied by Mask1.a through its mode and invert
                let (rb, rv, rmode, rinvert) = zone_roughness[zone];
                let rvv = a + rinvert * (1.0 - 2.0 * a);
                let rx = if rmode.floor() == 2.0 {
                    rv.clamp(0.0, 1.0)
                } else {
                    (rmode.floor() * (rb + rv - rb * rv) + rb * rv).clamp(0.0, 1.0)
                };
                let expected_rough = (rb + rvv * (rx - rb)).clamp(0.0, 1.0) * 255.0;
                if (f32::from(rough.levels[0][at + 1]) - expected_rough).abs() > 1.5 {
                    rough_off += 1;
                    if rough_off < 4 {
                        println!(
                            "zone {zone}: expected roughness {expected_rough}, baked {}, Mask2.a {a2}",
                            rough.levels[0][at + 1]
                        );
                    }
                }
            }
            x += step;
        }
        y += step;
    }
    println!(
        "{size} px masks: {compared} texels compared ({zones_seen:?} by zone), {off} albedo texels off by more than 1.5 of 255, {rough_off} roughness"
    );
    assert!(compared > 1000 && zones_seen.len() >= 2);
    assert_eq!((off, rough_off), (0, 0));
}

#[test]
fn a_rider_lowers_with_its_zones_baked_and_its_skin_left_to_the_placeholder() {
    let Some(mut archive) = open() else {
        return;
    };
    let mut w = rider(
        &mut archive,
        &[
            "MAD_CHR_UBODY_001_U_1001",
            "MAD_CHR_LBODY_001_U_1001",
            "MAD_CHR_FOOTW_001_U",
            "MAD_CHR_HEADW_001_U",
            "MAD_CHR_HEAD_001_U",
        ],
    );
    let mut registry = PassRegistry::new();
    primase_anvil::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    let pipeline = manager
        .registry
        .parse(
            "lower-anvil-mesh{static16-normal=p}, lower-anvil-rig{own-transform=replace}, lower-anvil-skin, \
             bake-anvil-material{size=256,normal-green=up,shader-uv=texcoord,mip-bias=0,terrain-ao=0.5,sun-elevation=51.29,\
             view-cosine=0.7,camera-distance=1000000,unset=white}, \
             lower-anvil-material{normal-green=up,alpha=prelude,unset=white,shader-uv=texcoord,emissive-exposure=1,\
             underground-view=0.5,fake-river-s=0,decals1-cutout=decal}",
        )
        .expect("the pass text");
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).expect("lower");
    let mut baked = 0u64;
    let mut placeholders: BTreeMap<String, u64> = BTreeMap::new();
    let mut not_baked: BTreeMap<String, u64> = BTreeMap::new();
    for step in &report.steps {
        for (k, n) in &step.log.counts {
            if k.contains("Materials baked") {
                baked += n;
            }
        }
        for (k, n) in &step.log.omitted {
            if k.contains("not lowered: a neutral placeholder") {
                *placeholders.entry(k.clone()).or_default() += n;
            }
            if k.contains("not baked") {
                *not_baked.entry(k.clone()).or_default() += n;
            }
        }
    }
    println!("baked {baked}; placeholders {placeholders:?}; not baked {not_baked:?}");
    assert!(baked > 0);
    // the 5Zones are baked; no 5Zones template is drawn as a placeholder
    assert!(
        placeholders.keys().all(|k| !k.contains("5Zones")),
        "{placeholders:?}"
    );
}
