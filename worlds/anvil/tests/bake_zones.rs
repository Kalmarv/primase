//! `bake-anvil-material` on the character zones (`MAD_CHR_5Zones_*`): a glove Material of a rider baked at its mask's
//! size and held against the notes' formula (docs/material-spec.md section 18.2) evaluated by hand on the same texels, and the
//! whole rider lowered with the zones baked instead of drawn as placeholders.
use std::collections::BTreeMap;

use anvil_formats::archive::Archive;
use anvil_formats::build_table::BUILD_TABLE;
use anvil_formats::skeleton::SKELETON;
use primase_anvil::components::{AnvilMaterial, MaterialBinding, MaterialValue};
use primase_anvil::passes::BAKE_ANVIL_MATERIAL;
use primase_anvil::read::placements::{
    Gender, InstancedLevels, Placed, Placer, RiderPartChoice, RiderSpec, Variant,
};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog, PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Name, Texture, World};
use primase_foundations::{sampling::Address, sampling::Levels};
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
        ("unstated-address", "wrap"),
        ("unset", "white"),
    ]
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .into()
}

fn srgb8(linear: f32) -> f32 {
    (primase_foundations::color::linear_to_srgb(f64::from(linear.clamp(0.0, 1.0))) * 255.0).round() as f32
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
        primase_image::levels(w.get::<Texture>(e).expect("texture")).expect("levels")
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
    let pass = BAKE_ANVIL_MATERIAL.instance(&o).expect("options");
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
             view-cosine=0.7,camera-distance=1000000,unset=white,unstated-address=wrap}, \
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

/// The skin Material of a head baked with a tone and with white: the Material's own factors cancel (the mud window and
/// the Diffuse), leaving the tint the record gives, `lerp(1, tone, W)` with `W = Normal.b` (the exponent byte 255): held
/// texel by texel on the head's own masks. Without a tone the pass leaves the skin to `lower-anvil-material`.
#[test]
fn a_skin_material_bakes_with_the_chosen_tone_as_its_tint() {
    let Some(mut archive) = open() else {
        return;
    };
    let tone = [199u8, 144, 113];
    let bake = |archive: &mut Archive, skin: Option<[u8; 3]>| -> (World, Entity, AnvilMaterial, PassLog) {
        let mut w = rider(archive, &["MAD_CHR_HEAD_001_U"]);
        let (entity, m) = w
            .iter::<AnvilMaterial>()
            .find(|(_, m)| m.template_name.as_deref() == Some("MAD_CHR_Skin"))
            .map(|(e, m)| (e, m.clone()))
            .expect("the head's skin Material");
        let mut o = options();
        o.insert("size".into(), "256".into());
        if let Some([r, g, b]) = skin {
            o.insert("skin-tone".into(), format!("{r},{g},{b}"));
        }
        let pass = BAKE_ANVIL_MATERIAL.instance(&o).expect("options");
        assert_eq!(pass.options().contains_key("skin-tone"), skin.is_some());
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).expect("bake");
        (w, entity, m, log)
    };
    // no tone: not baked
    let (w, entity, _, log) = bake(&mut archive, None);
    assert!(w.get::<PbrMaterial>(entity).is_none());
    assert!(!log.counts.keys().any(|k| k.contains("MAD_CHR_Skin")));
    let (w_tone, e_tone, m, log) = bake(&mut archive, Some(tone));
    let (w_white, e_white, _, _) = bake(&mut archive, Some([255; 3]));
    assert_eq!(
        log.counts.get("MAD_CHR_Skin Materials baked"),
        Some(&1),
        "{:?}",
        log.counts
    );
    assert!(
        log.approximations
            .keys()
            .any(|k| k.contains("MAD_CHR_Skin Materials baked with the chosen skin-tone"))
    );
    let facial = m
        .values
        .iter()
        .find(|(k, _)| *k == crc("Use_Facial_Hair"))
        .map(|(_, v)| v.clone());
    assert_eq!(
        facial,
        Some(MaterialValue::Float(0.0)),
        "the head's facial hair is off: {facial:?}"
    );
    let base_of = |w: &World, e: Entity| {
        let t = w
            .get::<PbrMaterial>(e)
            .and_then(|p| p.base_color_texture.as_ref())
            .map(|t| t.texture)
            .expect("base");
        w.get::<Texture>(t).expect("texture").levels[0].clone()
    };
    let (a, b) = (base_of(&w_tone, e_tone), base_of(&w_white, e_white));
    let normal = {
        let e = m
            .textures
            .iter()
            .find(|t| t.shader_slot == 1)
            .and_then(|t| t.texture)
            .expect("the Normal");
        primase_image::levels(w_tone.get::<Texture>(e).expect("normal")).expect("levels")
    };
    let lin = primase_foundations::color::srgb8_to_linear;
    let (mut compared, mut off) = (0u64, 0u64);
    for y in (0..256i64).step_by(4) {
        for x in (0..256i64).step_by(4) {
            // the normal's texel under the bake's (256 px over the normal's own size)
            let nx = x * i64::from(normal.width) / 256;
            let ny = y * i64::from(normal.height) / 256;
            let nz = f64::from(normal.texel(0, nx, ny, [Address::Wrap; 2])[2]) / 255.0;
            let at = (y as usize * 256 + x as usize) * 4;
            for c in 0..3 {
                let white = lin(b[at + c]);
                if white < 0.02 {
                    continue;
                }
                let t = f64::from(tone[c]) / 255.0;
                let expected = (white * (1.0 + nz * (t - 1.0))).clamp(0.0, 1.0);
                let got = lin(a[at + c]);
                compared += 1;
                if (got - expected).abs() > 0.03 * expected.max(0.05) + 0.01 {
                    off += 1;
                }
            }
        }
    }
    println!("skin: {compared} channel values compared, {off} off the tint formula");
    assert!(compared > 5000);
    assert!(off * 50 < compared, "{off} of {compared}");
}
