//! `bake-anvil-material` on the ice templates' Materials of the USNAT forge and its patch (Riders Republic build
//! 25394987): MAD_Hero_Ice and MAD_IceBrick against the notes' formulas for their G-buffer programs
//! (docs/material-spec.md §27.2, §27.6, executed by the notes; the rows read from the programs), worked here in f64
//! from the same sampled texels with the sun 30 degrees up and the eye at 0.7 to the normal (the colour's mix `f` not
//! 0), the parallax march seeing the surface straight on.
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
use primase_testdata::{Game, install};

const SIZE: u32 = 64;

const ORDER: [&str; 4] = [
    "DataPC_MAD_USNAT_patch_01.forge",
    "DataPC_MAD_USNAT.forge",
    "DataPC_patch_01.forge",
    "DataPC.forge",
];

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

/// The ice templates' Materials of the USNAT forge and its patch, by their template's name.
fn ice(archive: &mut Archive, template: &str) -> Vec<u64> {
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
            if archive.forge(t.forge).expect("forge").names[t.entry].name == template && !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

const SUN: f64 = 30.0;
const VIEW: f64 = 0.7;

/// The ice albedo (§27.2): the screen and multiply mixes of the surface and deep colours, mixed by `f` of the shaded
/// normal `n` against the sun (`SUN` degrees above +x) and the eye (`VIEW` to the normal, toward +x).
fn ice_albedo(surface: [f64; 3], deep: [f64; 3], n: [f64; 3], light_falloff: f64, fresnel: f64) -> [f64; 3] {
    let (sin, cos) = SUN.to_radians().sin_cos();
    let l = [cos, 0.0, sin];
    let v = [(1.0 - VIEW * VIEW).sqrt(), 0.0, VIEW];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let f = (1.0 - (dot(n, l) + light_falloff).clamp(0.0, 1.0)) * dot(n, v).abs().powf(fresnel).min(1.0);
    std::array::from_fn(|c| {
        let screen = 1.0 - (1.0 - surface[c]) * (1.0 - deep[c]);
        let mul = surface[c] * deep[c];
        screen + f * (mul - screen)
    })
}

fn bake(w: &mut World) -> PassLog {
    let pass = BakeAnvilMaterial::from_options(&Options::from(
        [
            ("size", "64"),
            ("normal-green", "up"),
            ("shader-uv", "texcoord"),
            ("mip-bias", "0"),
            ("terrain-ao", "0.5"),
            ("sun-elevation", "30"),
            ("view-cosine", "0.7"),
            ("camera-distance", "1000000"),
            ("unset", "white"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned())),
    ))
    .expect("options");
    let mut log = PassLog::default();
    pass.run(w, &mut log).expect("run");
    println!("{log:?}");
    log
}

/// The texture at `at` (its own coordinates), its level of detail that of `lod_scale` units across the texture a bake
/// texel.
fn sample_at((l, t): &(Levels, Transfer), at: [f64; 2], lod_scale: f64) -> [f64; 4] {
    let s = f64::from(SIZE);
    let lod = footprint_lod(
        l.width,
        l.height,
        [[lod_scale / s, 0.0], [0.0, lod_scale / s]],
        0.0,
    );
    l.trilinear(at, lod, [Address::Wrap; 2], *t)
        .expect("sample")
        .map(|c| f64::from(c as f32))
}

/// The texture at `uv` tiled `scale` times.
fn sample(texture: &(Levels, Transfer), uv: [f64; 2], scale: f64) -> [f64; 4] {
    sample_at(texture, [uv[0] * scale, uv[1] * scale], scale)
}

/// A baked Material's base colour, metallic-roughness and normal texels.
fn baked(w: &World, e: Entity) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let pbr = w.get::<PbrMaterial>(e)?;
    let level =
        |t: &primase_core::pbr::TextureRef| w.get::<Texture>(t.texture).expect("baked").levels[0].clone();
    Some((
        level(pbr.base_color_texture.as_ref().expect("base")),
        level(pbr.metallic_roughness_texture.as_ref().expect("rough")),
        level(pbr.normal_texture.as_ref().expect("normal")),
    ))
}

const TEXELS: [(u32, u32); 6] = [(0, 0), (5, 9), (31, 32), (47, 13), (63, 63), (20, 50)];

fn lerp3(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 3] {
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

#[test]
fn hero_ice_bakes_as_the_notes_formulas() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    let ids = ice(&mut archive, "MAD_Hero_Ice");
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
    let log = bake(&mut w);
    let mut worst = [0u8; 3];
    let mut checked = 0;
    for (&e, m) in materials.iter().zip(&before) {
        let Some((base_px, rough, normal)) = baked(&w, e) else {
            continue;
        };
        let x = |r: u32| row(m, r)[0];
        let (global, fresnel, light_falloff, frost_tiling, frost_power) = (x(1), x(2), x(3), x(5), x(6));
        let (deep_a, deep_b, surface_a, surface_b) = (row(m, 7), row(m, 8), row(m, 10), row(m, 9));
        let t = |slot| bound(&w, m, slot);
        let (diffuse_1, object_n, roughness, height, ice_n, diffuse, micro) =
            (t(0), t(1), t(2), t(3), t(4), t(5), t(6));
        for (px, py) in TEXELS {
            let uv = [(f64::from(px) + 0.5) / 64.0, (f64::from(py) + 0.5) / 64.0];
            let surface = lerp3(surface_a, surface_b, sample(&diffuse, uv, global)[0]);
            let warp = sample(&micro, uv, frost_tiling)[1]
                * (1.0 - sample(&height, uv, global)[2].clamp(0.0, 1.0))
                * frost_power;
            let uv_d = [uv[0] * global + warp, uv[1] * global + warp];
            let deep = lerp3(deep_a, deep_b, sample_at(&diffuse_1, uv_d, global)[0]);
            let xy = |n: [f64; 4]| [n[0] * 2.0 - 1.0, n[1] * 2.0 - 1.0];
            let z = |d: [f64; 2]| (1.0 - d[0] * d[0] - d[1] * d[1]).max(0.0).sqrt();
            let (d1, d2) = (xy(sample(&ice_n, uv, global)), xy(sample(&object_n, uv, 1.0)));
            let n = [d1[0] + d2[0], d1[1] + d2[1], z(d1) * z(d2)];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = n.map(|c| c / len);
            let albedo = ice_albedo(surface, deep, n, light_falloff, fresnel);
            let i = ((py * SIZE + px) * 4) as usize;
            for ch in 0..3 {
                let srgb = (primase_image::linear_to_srgb(albedo[ch].clamp(0.0, 1.0)) * 255.0).round() as u8;
                worst[0] = worst[0].max(srgb.abs_diff(base_px[i + ch]));
                let nb = ((n[ch] * 0.5 + 0.5) * 255.0).round() as u8;
                worst[2] = worst[2].max(nb.abs_diff(normal[i + ch]));
            }
            let r = (sample(&roughness, uv, global)[0].clamp(0.0, 1.0) * 255.0).round() as u8;
            worst[1] = worst[1].max(r.abs_diff(rough[i + 1]));
            checked += 1;
        }
    }
    let baked = log
        .counts
        .get("MAD_Hero_Ice Materials baked")
        .copied()
        .unwrap_or(0);
    println!(
        "Hero_Ice: baked {baked} of {}, {checked} texels, worst byte differences {worst:?}",
        ids.len()
    );
    assert!(baked > 0, "{log:?}");
    assert!(worst[0] <= 1 && worst[1] <= 1 && worst[2] <= 2, "{worst:?}");
}

#[test]
fn ice_brick_bakes_as_the_notes_formulas() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&ORDER.map(|f| dir.join(f))).expect("archive");
    let ids = ice(&mut archive, "MAD_IceBrick");
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
    let log = bake(&mut w);
    let mut worst = [0u8; 3];
    let mut checked = 0;
    for (&e, m) in materials.iter().zip(&before) {
        let Some((base_px, rough, normal)) = baked(&w, e) else {
            continue;
        };
        let x = |r: u32| row(m, r)[0];
        let (global, fresnel, light_falloff, metal, frost_power, frost_tiling) =
            (x(1), x(2), x(4), x(5), x(6), x(7));
        let (deep_a, deep_b, surface_a, surface_b) = (row(m, 8), row(m, 9), row(m, 11), row(m, 10));
        let t = |slot| bound(&w, m, slot);
        let (diffuse_1, roughness, height, diffuse, micro) = (t(0), t(1), t(2), t(3), t(4));
        for (px, py) in TEXELS {
            let uv = [(f64::from(px) + 0.5) / 64.0, (f64::from(py) + 0.5) / 64.0];
            let surface = lerp3(surface_a, surface_b, sample(&diffuse, uv, global)[0]);
            let warp = sample(&micro, uv, frost_tiling)[1]
                * (1.0 - sample(&height, uv, global)[2].clamp(0.0, 1.0))
                * frost_power;
            let uv_d = [uv[0] * global + warp, uv[1] * global + warp];
            let deep = lerp3(deep_a, deep_b, sample_at(&diffuse_1, uv_d, global)[0]);
            let albedo = ice_albedo(surface, deep, [0.0, 0.0, 1.0], light_falloff, fresnel);
            let i = ((py * SIZE + px) * 4) as usize;
            for ch in 0..3 {
                let srgb = (primase_image::linear_to_srgb(albedo[ch].clamp(0.0, 1.0)) * 255.0).round() as u8;
                worst[0] = worst[0].max(srgb.abs_diff(base_px[i + ch]));
                let flat = [128u8, 128, 255][ch];
                worst[2] = worst[2].max(flat.abs_diff(normal[i + ch]));
            }
            let r = (sample(&roughness, uv, global)[0].clamp(0.0, 1.0) * 255.0).round() as u8;
            let mb = (metal.clamp(0.0, 1.0) * 255.0).round() as u8;
            worst[1] = worst[1]
                .max(r.abs_diff(rough[i + 1]))
                .max(mb.abs_diff(rough[i + 2]));
            checked += 1;
        }
    }
    let baked = log
        .counts
        .get("MAD_IceBrick Materials baked")
        .copied()
        .unwrap_or(0);
    println!(
        "IceBrick: baked {baked} of {}, {checked} texels, worst byte differences {worst:?}",
        ids.len()
    );
    assert!(baked > 0, "{log:?}");
    assert!(worst[0] <= 1 && worst[1] <= 1 && worst[2] <= 1, "{worst:?}");
}
