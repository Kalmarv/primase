//! `bake-anvil-material` on every MAD_Tex1_Rocks Material of the USNAT forge (Riders Republic build 25394987),
//! against the notes' formulas for its G-buffer program (docs/material-spec.md §14.6, executed by the notes on WARP;
//! the program read in docs/shaders.md §6.2: roughness the detail normal's alpha), worked here in f64 from the same
//! sampled texels: the bake runs the game's program, the oracle the formulas.
use primase_anvil::components::{AnvilMaterial, AnvilTextureMap, MaterialBinding, MaterialValue};
use primase_anvil::passes::BakeAnvilMaterial;
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Texture, World};
use primase_image::sampling::{Address, Levels, Transfer, footprint_lod};
use primase_testdata::real_data;

const SIZE: u32 = 64;

/// A bound texture of `m` at `t(75 + slot)`: its levels and transfer.
fn bound(w: &World, m: &AnvilMaterial, slot: u32) -> (Levels, Transfer) {
    let key = m
        .bindings
        .iter()
        .find(|(_, b)| *b == MaterialBinding::Texture { slot })
        .expect("bound")
        .0;
    let e = m
        .values
        .iter()
        .find_map(|(k, v)| match v {
            MaterialValue::Texture { texture, .. } if *k == key => *texture,
            _ => None,
        })
        .expect("set");
    let transfer = if w.get::<AnvilTextureMap>(e).expect("map").field_25 == 1 {
        Transfer::SrgbColour
    } else {
        Transfer::Stored
    };
    (
        Levels::of(w.get::<Texture>(e).expect("texture")).expect("levels"),
        transfer,
    )
}

/// The value bound at `cb3` row `row`: a scalar's, or a UV transform's scale and translation where it starts.
fn row(m: &AnvilMaterial, row: u32) -> Vec<f32> {
    m.bindings
        .iter()
        .find_map(|(k, b)| match b {
            MaterialBinding::Scalar { vec4 }
            | MaterialBinding::Vector { vec4 }
            | MaterialBinding::UvTransform { vec4 }
                if *vec4 == row =>
            {
                m.values.iter().find_map(|(key, v)| match v {
                    MaterialValue::Float(x) if key == k => Some(vec![*x]),
                    MaterialValue::Float4(x) if key == k => Some(x.to_vec()),
                    MaterialValue::Floats(x) if key == k => Some(x.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("bound and set")
}

/// RGB to HSV and back, the textbook forms.
fn hsv(c: [f64; 3]) -> [f64; 3] {
    let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let d = max - min;
    if d == 0.0 {
        return [0.0, 0.0, max];
    }
    let h = if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    [h / 6.0, d / max, max]
}
fn rgb([h, s, v]: [f64; 3]) -> [f64; 3] {
    let hue = [
        ((h * 6.0 - 3.0).abs() - 1.0).clamp(0.0, 1.0),
        (2.0 - (h * 6.0 - 2.0).abs()).clamp(0.0, 1.0),
        (2.0 - (h * 6.0 - 4.0).abs()).clamp(0.0, 1.0),
    ];
    hue.map(|c| ((c - 1.0) * s + 1.0) * v)
}

#[test]
fn rocks_bake_as_the_notes_formulas() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    // the Rocks Materials of the USNAT forge and its patch, by their template's name (an id once)
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
            if archive.forge(t.forge).expect("forge").names[t.entry].name == "MAD_Tex1_Rocks"
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    println!("Rocks Materials: {}", ids.len());

    let mut w = World::new();
    let mut placed = Placed::default();
    let materials: Vec<Entity> = {
        let mut placer =
            Placer::new(&mut archive, &mut w, SourceFrame::WORLD, InstancedLevels::Slot).with_shaders();
        ids.iter()
            .map(|&id| placer.material(id, &mut placed).expect("read"))
            .collect()
    };
    assert!(placed.refused.is_empty(), "{:?}", placed.refused);
    let before: Vec<AnvilMaterial> = materials
        .iter()
        .map(|&e| w.get::<AnvilMaterial>(e).expect("material").clone())
        .collect();
    let pass = BakeAnvilMaterial::from_options(&Options::from(
        [
            ("size", "64"),
            ("normal-green", "up"),
            ("shader-uv", "texcoord"),
            ("mip-bias", "0"),
            ("terrain-ao", "0.3"),
            ("sun-elevation", "45"),
            ("view-cosine", "0.7"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned())),
    ))
    .expect("options");
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("run");
    println!("{log:?}");
    let baked = log
        .counts
        .get("MAD_Tex1_Rocks Materials baked")
        .copied()
        .unwrap_or(0);
    let mut worst = [0u8; 3];
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
        assert!(pbr.occlusion_texture.is_none() && pbr.metallic == 0.0);
        let (detail_n, detail_a, obj_n, obj_m, dirt) = (
            bound(&w, m, 0),
            bound(&w, m, 1),
            bound(&w, m, 2),
            bound(&w, m, 3),
            bound(&w, m, 4),
        );
        let (tiling, dirt_tiling) = (row(m, 9), row(m, 12));
        let [hue, sat, value] = [row(m, 5)[0], row(m, 6)[0], row(m, 4)[0]].map(f64::from);
        let colour = |r: u32| -> [f64; 3] { std::array::from_fn(|c| f64::from(row(m, r)[c])) };
        let (ca, cb, cc) = (colour(2), colour(3), colour(1));
        let sample = |(l, t): &(Levels, Transfer), uv: [f64; 2], scale: [f64; 2]| {
            let s = f64::from(SIZE);
            let lod = footprint_lod(l.width, l.height, [[scale[0] / s, 0.0], [0.0, scale[1] / s]], 0.0);
            l.trilinear(uv, lod, [Address::Wrap; 2], *t)
                .expect("sample")
                .map(|c| f64::from(c as f32))
        };
        for (x, y) in [(0, 0), (5, 9), (31, 32), (47, 13), (63, 63), (20, 50)] {
            let uv = [(f64::from(x) + 0.5) / 64.0, (f64::from(y) + 0.5) / 64.0];
            let tiled = |t: &[f32]| {
                let [su, sv, tu, tv] = [t[0], t[1], t[2], t[3]].map(f64::from);
                ([su * uv[0] + tu, sv * uv[1] + tv], [su, sv])
            };
            let (duv, dscale) = tiled(&tiling);
            let (muv, mscale) = tiled(&dirt_tiling);
            let mask = sample(&obj_m, uv, [1.0; 2]);
            let g = mask[1] as f32;
            let zone = |lo: f32, hi: f32| f64::from(u8::from(g >= lo && hi >= g));
            let d = sample(&dirt, muv, mscale)[2];
            let (wa, wb, wc) = (
                zone(0.0, 0.333) * d,
                zone(0.333, 0.666) * d,
                zone(0.666, 0.999) * d,
            );
            let albedo = sample(&detail_a, duv, dscale);
            let a = [albedo[0], albedo[1], albedo[2]];
            let base: [f64; 3] = std::array::from_fn(|c| {
                a[c] * (1.0 + wc * (cc[c] - 1.0)) * (1.0 + wb * (cb[c] - 1.0)) * (1.0 + wa * (ca[c] - 1.0))
            });
            let [h, s, v] = hsv(a);
            let shifted = rgb([(h * hue).fract(), s * sat, v * value]);
            let mixed: [f64; 3] = std::array::from_fn(|c| base[c] + mask[0] * (shifted[c] - base[c]));
            // the terrain's ambient occlusion (stated 0.3) mixed in by Use Terrain Albedo Mul: albedo (ao + 0.5)
            let terrain = f64::from(row(m, 7)[0]);
            let expect: [f64; 3] = std::array::from_fn(|c| mixed[c] + terrain * (mixed[c] * 0.8 - mixed[c]));
            let dn = sample(&detail_n, duv, dscale);
            let on = sample(&obj_n, uv, [1.0; 2]);
            let xy = |n: [f64; 4]| [n[0] * 2.0 - 1.0, n[1] * 2.0 - 1.0];
            let (d1, d2) = (xy(dn), xy(on));
            let z = |d: [f64; 2]| (1.0 - d[0] * d[0] - d[1] * d[1]).max(0.0).sqrt();
            let n = [d1[0] + d2[0], d1[1] + d2[1], z(d1) * z(d2)];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let i = ((y * SIZE + x) * 4) as usize;
            for c in 0..3 {
                let srgb = (primase_image::linear_to_srgb(expect[c].clamp(0.0, 1.0)) * 255.0).round() as u8;
                worst[0] = worst[0].max(srgb.abs_diff(base_px[i + c]));
                let nb = ((n[c] / len * 0.5 + 0.5) * 255.0).round() as u8;
                worst[2] = worst[2].max(nb.abs_diff(normal[i + c]));
            }
            let r = (dn[3].clamp(0.0, 1.0) * 255.0).round() as u8;
            worst[1] = worst[1].max(r.abs_diff(rough[i + 1]));
            checked += 1;
        }
    }
    println!(
        "baked {baked} of {}, {checked} texels checked, worst byte differences (albedo, roughness, normal) {worst:?}",
        ids.len()
    );
    assert!(baked > 0, "{log:?}");
    // roughness passes a sampled texel through; albedo and normal round at byte boundaries (the normal also through
    // the G-buffer's 9-bit octahedral store)
    assert!(worst[0] <= 1 && worst[1] == 0 && worst[2] <= 2, "{worst:?}");
}
