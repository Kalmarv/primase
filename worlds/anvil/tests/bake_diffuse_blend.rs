//! `bake-anvil-material` on every MAD_Tex1_UniqueNormal_DiffuseBlend Material of the USNAT forge and its patch
//! (Riders Republic build 25394987), against the notes' formulas for its G-buffer program (docs/material-spec.md
//! §27.8, executed by the notes on WARP; the rows read from the program), worked here in f64 from the same sampled
//! texels. A Material whose occlusion reads a mask it leaves unset is refused (counted), the others baked.
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

/// A bound texture of `m` at `t(75 + slot)`: its levels and transfer.
fn bound(w: &World, m: &AnvilMaterial, slot: u32) -> (Levels, Transfer) {
    // the texture the binding resolves to (the Material's record, its set's slot or the template's default)
    let e = m
        .textures
        .iter()
        .find(|t| t.shader_slot == slot)
        .expect("bound")
        .texture
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
fn unique_normal_diffuse_blend_bakes_as_the_notes_formulas() {
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
            if archive.forge(t.forge).expect("forge").names[t.entry].name
                == "MAD_Tex1_UniqueNormal_DiffuseBlend"
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    println!("DiffuseBlend Materials: {}", ids.len());
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
    let pass = BakeAnvilMaterial::from_options(&Options::from(
        [
            ("size", "64"),
            ("normal-green", "up"),
            ("shader-uv", "texcoord"),
            ("mip-bias", "0"),
            ("terrain-ao", "0.5"),
            ("sun-elevation", "45"),
            ("view-cosine", "0.7"),
            ("camera-distance", "1000000"),
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
        .get("MAD_Tex1_UniqueNormal_DiffuseBlend Materials baked")
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
        let tiling: Vec<f64> = m
            .bindings
            .iter()
            .find_map(|(k, b)| match b {
                MaterialBinding::UvTransform { vec4: 2 } => m.values.iter().find_map(|(key, v)| match v {
                    MaterialValue::Floats(f) if key == k => Some(f.iter().map(|&x| f64::from(x)).collect()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("tiling");
        let (albedo, base_n, detail_n) = (bound(&w, m, 0), bound(&w, m, 1), bound(&w, m, 2));
        let masks = (use_ao != 0.0).then(|| (bound(&w, m, 3), bound(&w, m, 4)));
        let sample = |(l, t): &(Levels, Transfer), uv: [f64; 2], scale: [f64; 2]| {
            let s = f64::from(SIZE);
            let lod = footprint_lod(l.width, l.height, [[scale[0] / s, 0.0], [0.0, scale[1] / s]], 0.0);
            l.trilinear(uv, lod, [Address::Wrap; 2], *t)
                .expect("sample")
                .map(|c| f64::from(c as f32))
        };
        for (px, py) in [(0, 0), (5, 9), (31, 32), (47, 13), (63, 63), (20, 50)] {
            let uv = [(f64::from(px) + 0.5) / 64.0, (f64::from(py) + 0.5) / 64.0];
            let (su, sv, tu, tv) = (tiling[0], tiling[1], tiling[2], tiling[3]);
            let duv = [su * uv[0] + tu, sv * uv[1] + tv];
            let a = sample(&albedo, uv, [1.0; 2]);
            let b = sample(&base_n, uv, [1.0; 2]);
            let d = sample(&detail_n, duv, [su, sv]);
            let ao = masks.as_ref().map_or(1.0, |(t78, t79)| {
                1.0 + use_ao * (sample(t78, uv, [1.0; 2])[2] * sample(t79, duv, [su, sv])[2] - 1.0)
            });
            let xy = |n: [f64; 4]| [n[0] * 2.0 - 1.0, n[1] * 2.0 - 1.0];
            let z = |q: [f64; 2]| (1.0 - q[0] * q[0] - q[1] * q[1]).max(0.0).sqrt();
            let (db, dd) = (xy(b), xy(d));
            let n = [db[0] + dd[0], db[1] + dd[1], z(db) * z(dd)];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let i = ((py * SIZE + px) * 4) as usize;
            let byte = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
            for ch in 0..3 {
                let srgb = (primase_image::linear_to_srgb(a[ch].clamp(0.0, 1.0)) * 255.0).round() as u8;
                worst[0] = worst[0].max(srgb.abs_diff(base_px[i + ch]));
                let nb = ((n[ch] / len * 0.5 + 0.5) * 255.0).round() as u8;
                worst[2] = worst[2].max(nb.abs_diff(normal[i + ch]));
            }
            worst[1] = worst[1]
                .max(byte(d[3]).abs_diff(rough[i + 1]))
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
    assert!(baked > 0, "{log:?}");
    assert!(
        worst[0] <= 1 && worst[1] <= 1 && worst[2] <= 2 && worst[3] <= 1,
        "{worst:?}"
    );
}
