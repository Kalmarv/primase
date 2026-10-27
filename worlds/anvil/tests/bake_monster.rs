//! `bake-anvil-material` on every MAD_Tex1_Graffiti_Monster Material of the USNAT forge and its patch (Riders
//! Republic build 25394987), against the notes' formulas for its G-buffer program (docs/material-spec.md §27.14.2,
//! executed by the notes on primase's evaluator and WARP; the rows read from the program), worked here in f64 from the
//! same sampled texels: with the team and custom colours' alphas 0 and `EmissivePower` 0 (every placed Material: checked
//! here), albedo the diffuse, roughness and metalness the normal map's alpha and blue, the three normal maps summed,
//! AO `1 + Use AO (Mask1.b DetailsMask1.b - 1)`; a texture the Material resolves to none sampled white (`unset=white`).
//! A Material whose second tiling isn't whole (not periodic in the UV square) is left to `lower-anvil-material`.
use primase_anvil::components::{AnvilMaterial, AnvilTextureMap, MaterialBinding, MaterialValue};
use primase_anvil::passes::BAKE_ANVIL_MATERIAL;
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Texture, World};
use primase_foundations::{sampling::Address, sampling::Levels, sampling::Transfer, sampling::footprint_lod};
use primase_testdata::{Game, install};

const SIZE: u32 = 64;

/// The texture `m` resolves at `t(75 + slot)`: its levels and transfer, none where it resolves to none (white).
fn bound(w: &World, m: &AnvilMaterial, slot: u32) -> Option<(Levels, Transfer)> {
    let e = m
        .textures
        .iter()
        .find(|t| t.shader_slot == slot)
        .expect("bound")
        .texture?;
    let transfer = if w.get::<AnvilTextureMap>(e).expect("map").field_25 == 1 {
        Transfer::SrgbColour
    } else {
        Transfer::Stored
    };
    Some((
        primase_image::levels(w.get::<Texture>(e).expect("texture")).expect("levels"),
        transfer,
    ))
}

/// The value bound at `cb3` row `row`: a scalar's (in `.x`) or a vector's.
fn row(m: &AnvilMaterial, row: u32) -> [f64; 4] {
    m.bindings
        .iter()
        .find_map(|(k, b)| match b {
            MaterialBinding::Scalar { vec4 } | MaterialBinding::Vector { vec4 } if *vec4 == row => {
                m.values.iter().find_map(|(key, v)| match v {
                    MaterialValue::Float(x) if key == k => Some([f64::from(*x), 0.0, 0.0, 0.0]),
                    MaterialValue::Float4(x) if key == k => Some(x.map(f64::from)),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("bound and set")
}

/// The UV transform at `vec4`: `su, sv, tu, tv`.
fn tiling(m: &AnvilMaterial, at: u32) -> [f64; 4] {
    m.bindings
        .iter()
        .find_map(|(k, b)| match b {
            MaterialBinding::UvTransform { vec4 } if *vec4 == at => {
                m.values.iter().find_map(|(key, v)| match v {
                    MaterialValue::Floats(f) if key == k => Some([0, 1, 2, 3].map(|i| f64::from(f[i]))),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("tiling")
}

#[test]
fn graffiti_monster_bakes_as_the_notes_formulas() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let mut ids = Vec::new();
    for f in [0, 1] {
        let forge = archive.forge(f).expect("forge");
        let entries: Vec<(usize, u64)> = (0..forge.names.len())
            .filter(|&k| forge.names[k].type_id == MATERIAL)
            .map(|k| (k, forge.index[k].id))
            .collect();
        for (entry, id) in entries {
            let body = archive.resource(At { forge: f, entry }).expect("material").body;
            let m = StoredMaterial::parse(&body).expect("parse");
            let Some(t) = m.references[0].and_then(|t| archive.find(t.id)) else {
                continue;
            };
            if archive.forge(t.forge).expect("forge").names[t.entry].name == "MAD_Tex1_Graffiti_Monster"
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    println!("Graffiti_Monster Materials: {}", ids.len());
    let mut w = World::new();
    let mut placed = Placed::default();
    let materials: Vec<Entity> = {
        let mut placer =
            Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot).with_shaders();
        ids.iter()
            .map(|&id| placer.material(id, &mut placed).expect("read"))
            .collect()
    };
    let before: Vec<AnvilMaterial> = materials
        .iter()
        .map(|&e| w.get::<AnvilMaterial>(e).expect("material").clone())
        .collect();
    // the formulas below hold where the team and custom colours' alphas and EmissivePower are 0: on every Material
    for m in &before {
        assert_eq!((row(m, 5)[3], row(m, 6)[3], row(m, 2)[0]), (0.0, 0.0, 0.0));
    }
    let pass = BAKE_ANVIL_MATERIAL
        .instance(&Options::from(
            [
                ("size", "64"),
                ("normal-green", "up"),
                ("shader-uv", "texcoord"),
                ("mip-bias", "0"),
                ("terrain-ao", "0.5"),
                ("sun-elevation", "45"),
                ("view-cosine", "0.7"),
                ("camera-distance", "1000000"),
                ("unstated-address", "wrap"),
                ("unset", "white"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned())),
        ))
        .expect("options");
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("run");
    println!("{log:?}");
    let baked = log
        .counts
        .get("MAD_Tex1_Graffiti_Monster Materials baked")
        .copied()
        .unwrap_or(0);
    let mut worst = [0u8; 4];
    let mut checked = 0;
    for (&e, m) in materials.iter().zip(&before) {
        let Some(pbr) = w.get::<PbrMaterial>(e) else {
            continue;
        };
        let level =
            |t: &primase_core::pbr::TextureRef| w.get::<Texture>(t.texture).expect("baked").levels[0].clone();
        let (base_px, rough, normal) = (
            level(pbr.base_color_texture.as_ref().expect("base")),
            level(pbr.metallic_roughness_texture.as_ref().expect("rough")),
            level(pbr.normal_texture.as_ref().expect("normal")),
        );
        let occlusion = pbr.occlusion_texture.as_ref().map(level);
        let use_ao = row(m, 1)[0];
        let (t1, t2) = (tiling(m, 8), tiling(m, 11));
        let [albedo, base_n, detail, mask, detail_mask] = [0, 1, 2, 3, 4].map(|s| bound(&w, m, s));
        let sample = |t: &Option<(Levels, Transfer)>, uv: [f64; 2], scale: [f64; 2]| -> [f64; 4] {
            let Some((l, tr)) = t else {
                return [1.0; 4];
            };
            let s = f64::from(SIZE);
            let lod = footprint_lod(l.width, l.height, [[scale[0] / s, 0.0], [0.0, scale[1] / s]], 0.0);
            l.trilinear(uv, lod, [Address::Wrap; 2], *tr)
                .expect("sample")
                .map(|c| f64::from(c as f32))
        };
        for (px, py) in [(0, 0), (5, 9), (31, 32), (47, 13), (63, 63), (20, 50)] {
            let uv = [(f64::from(px) + 0.5) / 64.0, (f64::from(py) + 0.5) / 64.0];
            let at = |t: [f64; 4]| [t[0] * uv[0] + t[2], t[1] * uv[1] + t[3]];
            let a = sample(&albedo, uv, [1.0; 2]);
            let b = sample(&base_n, uv, [1.0; 2]);
            let d1 = sample(&detail, at(t1), [t1[0], t1[1]]);
            let d2 = sample(&detail, at(t2), [t2[0], t2[1]]);
            let ao = 1.0
                + use_ao
                    * (sample(&mask, uv, [1.0; 2])[2] * sample(&detail_mask, at(t1), [t1[0], t1[1]])[2]
                        - 1.0);
            let xy = |n: [f64; 4]| [n[0] * 2.0 - 1.0, n[1] * 2.0 - 1.0];
            let z = |q: [f64; 2]| (1.0 - q[0] * q[0] - q[1] * q[1]).max(0.0).sqrt();
            let (qb, q1, q2) = (xy(b), xy(d1), xy(d2));
            let n = [
                qb[0] + q1[0] + q2[0],
                qb[1] + q1[1] + q2[1],
                z(qb) * z(q1) * z(q2),
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let i = ((py * SIZE + px) * 4) as usize;
            let byte = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
            for ch in 0..3 {
                let srgb =
                    (primase_foundations::color::linear_to_srgb(a[ch].clamp(0.0, 1.0)) * 255.0).round() as u8;
                worst[0] = worst[0].max(srgb.abs_diff(base_px[i + ch]));
                let nb = ((n[ch] / len * 0.5 + 0.5) * 255.0).round() as u8;
                worst[2] = worst[2].max(nb.abs_diff(normal[i + ch]));
            }
            worst[1] = worst[1]
                .max(byte(b[3]).abs_diff(rough[i + 1]))
                .max(byte(b[2]).abs_diff(rough[i + 2]));
            worst[3] = worst[3].max(byte(ao).abs_diff(occlusion.as_ref().map_or(255, |o| o[i])));
            checked += 1;
        }
    }
    println!(
        "baked {baked} of {}, {checked} texels checked, worst byte differences (albedo, roughness/metal, normal, \
         occlusion) {worst:?}",
        ids.len()
    );
    // a second tiling that isn't whole isn't periodic in the UV square: left to lower-anvil-material, the others baked
    let aperiodic = log
        .omitted
        .get(
            "MAD_Tex1_Graffiti_Monster Materials not baked (left to lower-anvil-material): UV transform BEE89D40: a scale that isn't whole (not periodic in the square)",
        )
        .copied()
        .unwrap_or(0);
    assert_eq!((baked + aperiodic) as usize, ids.len(), "{log:?}");
    assert!(baked > 0);
    assert!(
        worst[0] <= 1 && worst[1] <= 1 && worst[2] <= 2 && worst[3] <= 1,
        "{worst:?}"
    );
}
