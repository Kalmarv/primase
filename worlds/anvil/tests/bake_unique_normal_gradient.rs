//! `bake-anvil-material` on every MAD_Tex1_UniqueNormalGradient Material of the USNAT forge and its patch (Riders
//! Republic build 25394987), against the notes' formulas for its G-buffer program (docs/material-spec.md §27.3,
//! executed by the notes on WARP and on primase's evaluator; the rows read from the program), worked here in f64 from
//! the same sampled texels with the camera far (the medium albedo): the bake runs the game's program, the oracle the
//! formulas.
use primase_anvil::components::{AnvilMaterial, AnvilTextureMap, MaterialBinding, MaterialValue};
use primase_anvil::passes::BAKE_ANVIL_MATERIAL;
use primase_anvil::passes::formulas::{sat, tint};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Texture, World};
use primase_image::sampling::{Address, Levels, Transfer, footprint_lod};
use primase_testdata::{Game, install};

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

#[test]
fn unique_normal_gradient_bakes_as_the_notes_formulas() {
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
            if archive.forge(t.forge).expect("forge").names[t.entry].name == "MAD_Tex1_UniqueNormalGradient"
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    println!("UniqueNormalGradient Materials: {}", ids.len());
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
        .get("MAD_Tex1_UniqueNormalGradient Materials baked")
        .copied()
        .unwrap_or(0);
    // the camera distance is the program's stated choice, counted per Material baked with it
    assert_eq!(
        log.approximations
            .get("Materials baked with the camera at the stated camera-distance 1000000 m (their mix by distance)")
            .copied(),
        (baked > 0).then_some(baked),
        "{log:?}"
    );
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
        let (medium_n, medium_a, obj_n, obj_m, dirt, micro_n) = (
            bound(&w, m, 0),
            bound(&w, m, 1),
            bound(&w, m, 2),
            bound(&w, m, 3),
            bound(&w, m, 4),
            bound(&w, m, 5),
        );
        let x = |r: u32| row(m, r)[0];
        let (dirt_tiling, base_tiling, micro_tiling) = (x(1), x(2), x(7));
        let (erode, value, hue, saturation) = (x(12), x(18), x(19), x(20));
        // per zone: falloff, width, threshold rows and its colour row
        let zones = [(3, 4, 5, 15), (6, 14, 13, 17), (11, 10, 9, 16)];
        let sample = |(l, t): &(Levels, Transfer), uv: [f64; 2], scale: f64| {
            let s = f64::from(SIZE);
            let lod = footprint_lod(l.width, l.height, [[scale / s, 0.0], [0.0, scale / s]], 0.0);
            l.trilinear([uv[0] * scale, uv[1] * scale], lod, [Address::Wrap; 2], *t)
                .expect("sample")
                .map(|c| f64::from(c as f32))
        };
        for (px, py) in [(0, 0), (5, 9), (31, 32), (47, 13), (63, 63), (20, 50)] {
            let uv = [(f64::from(px) + 0.5) / 64.0, (f64::from(py) + 0.5) / 64.0];
            // the camera far: the medium albedo
            let c = sample(&medium_a, uv, base_tiling);
            let tinted = tint([c[0], c[1], c[2]], hue, saturation, value);
            let mask = sample(&obj_m, uv, 1.0);
            let g = sat(sample(&dirt, uv, dirt_tiling)[2] + erode) * mask[1];
            let mut zoned = tinted;
            for (falloff, width, threshold, colour) in zones {
                let (f, wd, th) = (x(falloff), x(width), g + x(threshold));
                let k = 50.0 * (1.01 - f);
                let band = sat((-th + wd + 0.5) * k + 0.5) * sat((th + wd - 0.5) * k + 0.5);
                let col = row(m, colour);
                for ch in 0..3 {
                    zoned[ch] *= 1.0 + band * (col[ch] - 1.0);
                }
            }
            let albedo: [f64; 3] = std::array::from_fn(|ch| zoned[ch] + mask[0] * (tinted[ch] - zoned[ch]));
            let xy = |n: [f64; 4]| [n[0] * 2.0 - 1.0, n[1] * 2.0 - 1.0];
            let z = |d: [f64; 2]| (1.0 - d[0] * d[0] - d[1] * d[1]).max(0.0).sqrt();
            let (dm, dob) = (
                xy(sample(&medium_n, uv, base_tiling)),
                xy(sample(&obj_n, uv, 1.0)),
            );
            let n1 = [dm[0] + dob[0], dm[1] + dob[1], z(dm) * z(dob)];
            let l1 = (n1[0] * n1[0] + n1[1] * n1[1] + n1[2] * n1[2]).sqrt();
            let dmi = xy(sample(&micro_n, uv, micro_tiling));
            let n = [n1[0] / l1 + dmi[0], n1[1] / l1 + dmi[1], n1[2] / l1 * z(dmi)];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let i = ((py * SIZE + px) * 4) as usize;
            for ch in 0..3 {
                let srgb = (primase_image::linear_to_srgb(albedo[ch].clamp(0.0, 1.0)) * 255.0).round() as u8;
                worst[0] = worst[0].max(srgb.abs_diff(base_px[i + ch]));
                let nb = ((n[ch] / len * 0.5 + 0.5) * 255.0).round() as u8;
                worst[2] = worst[2].max(nb.abs_diff(normal[i + ch]));
            }
            worst[1] = worst[1].max(255u8.abs_diff(rough[i + 1]));
            checked += 1;
        }
    }
    println!(
        "baked {baked} of {}, {checked} texels checked, worst byte differences (albedo, roughness, normal) {worst:?}",
        ids.len()
    );
    assert!(baked > 0, "{log:?}");
    // roughness is the constant 1; albedo and normal round at byte boundaries (the normal also through the G-buffer's
    // 9-bit octahedral store)
    assert!(worst[0] <= 1 && worst[1] == 0 && worst[2] <= 2, "{worst:?}");
}
