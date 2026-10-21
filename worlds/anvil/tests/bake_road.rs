//! `bake-anvil-material` on MAD_Road (Riders Republic build 25394987; docs/material-spec.md §27.4, executed by the
//! notes on WARP and on primase's evaluator; its rows read from its program): its lane lines, which its shader computes
//! per pixel, baked over one period of its own UV, against the notes' formula worked here (the stripes in f32, as the
//! program compares them; the rest in f64) at every texel, and its texture references carrying its UV transform over
//! the period.
use primase_anvil::components::{AnvilMaterial, AnvilTextureMap, MaterialBinding, MaterialValue};
use primase_anvil::passes::BakeAnvilMaterial;
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::material::{MATERIAL, StoredMaterial};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{PbrMaterial, UvTransform};
use primase_core::{Entity, Texture, World};
use primase_image::sampling::{Address, Levels, Transfer, footprint_lod};
use primase_testdata::real_data;

const SIZE: u32 = 128;

fn sat(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// The value bound at `cb3` row `row`: a scalar's, or a UV transform's eight floats.
fn row(m: &AnvilMaterial, row: u32) -> Vec<f32> {
    m.bindings
        .iter()
        .find_map(|(k, b)| match b {
            MaterialBinding::Scalar { vec4 } | MaterialBinding::UvTransform { vec4 } if *vec4 == row => {
                m.values.iter().find_map(|(key, v)| match v {
                    MaterialValue::Float(x) if key == k => Some(vec![*x]),
                    MaterialValue::Floats(x) if key == k => Some(x.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("bound and set")
}

#[test]
fn road_bakes_its_lane_lines_over_a_period() {
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
    let mut ids = Vec::new();
    for f in 0..4 {
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
            if archive.forge(t.forge).expect("forge").names[t.entry].name == "MAD_Road" && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    println!("Road Materials: {}", ids.len());
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
            ("size", "128"),
            ("normal-green", "up"),
            ("shader-uv", "texcoord"),
            ("mip-bias", "0"),
            ("terrain-ao", "0.5"),
            ("sun-elevation", "45"),
            ("view-cosine", "0.7"),
            ("unset", "white"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned())),
    ))
    .expect("options");
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("run");
    println!("{log:?}");
    let baked = log.counts.get("MAD_Road Materials baked").copied().unwrap_or(0);
    assert!(baked > 0, "{log:?}");
    let (mut worst, mut paint_texels) = ([0u8; 3], 0);
    for (&e, m) in materials.iter().zip(&before) {
        let Some(pbr) = w.get::<PbrMaterial>(e) else {
            continue;
        };
        let x = |r: u32| f64::from(row(m, r)[0]);
        let (hue, saturate, luminosity, metalness) = (x(3), x(4), x(5), x(1));
        let f = |r: u32| row(m, r)[0];
        let (line_offset, line_width, line_gap, line_frequency, lane_width, lane_offset) =
            (f(7), f(8), f(9), f(10), f(11), f(12));
        let period = [1.0 / line_frequency, 1.0 / lane_width];
        // the references: the Material's tiling over the period
        let tiling = row(m, 13);
        let want = UvTransform {
            offset: [tiling[2] / period[0], tiling[3] / period[1]],
            scale: [tiling[0] / period[0], tiling[1] / period[1]],
            rotation: 0.0,
        };
        let refs = [
            &pbr.base_color_texture,
            &pbr.metallic_roughness_texture,
            &pbr.normal_texture,
        ];
        for r in refs {
            let t = r
                .as_ref()
                .expect("reference")
                .transform
                .unwrap_or(UvTransform::IDENTITY);
            assert_eq!(t, want, "{tiling:?} over {period:?}");
        }
        let level =
            |t: &primase_core::pbr::TextureRef| w.get::<Texture>(t.texture).expect("baked").levels[0].clone();
        let (base_px, rough, normal) = (
            level(pbr.base_color_texture.as_ref().expect("base")),
            level(pbr.metallic_roughness_texture.as_ref().expect("rough")),
            level(pbr.normal_texture.as_ref().expect("normal")),
        );
        let key = m
            .bindings
            .iter()
            .find(|(_, b)| *b == MaterialBinding::Texture { slot: 0 })
            .expect("bound")
            .0;
        let map = m
            .values
            .iter()
            .find_map(|(k, v)| match v {
                MaterialValue::Texture { texture, .. } if *k == key => *texture,
                _ => None,
            })
            .expect("set");
        let transfer = if w.get::<AnvilTextureMap>(map).expect("map").field_25 == 1 {
            Transfer::SrgbColour
        } else {
            Transfer::Stored
        };
        let levels = Levels::of(w.get::<Texture>(map).expect("texture")).expect("levels");
        let ramp = |h: f64| {
            [
                sat((6.0 * h - 3.0).abs() - 1.0),
                sat(2.0 - (6.0 * h - 2.0).abs()),
                sat(2.0 - (6.0 * h - 4.0).abs()),
            ]
        };
        let col = ramp(sat(hue)).map(|c| 1.0 + (c - 1.0) * sat(saturate));
        let s = SIZE as f32;
        for py in 0..SIZE {
            for px in 0..SIZE {
                // the grid's point in the pattern's UV, in f32 as the evaluator places it
                let u = (px as f32 + 0.5) / s * period[0];
                let v = (py as f32 + 0.5) / s * period[1];
                let lane = ((v + lane_offset) * lane_width).fract() >= 1.0 - line_width.clamp(0.0, 1.0);
                let dash = ((u + line_offset) * line_frequency).fract() >= line_gap;
                let paint = if lane && dash { 1.0f64 } else { 0.0 }.max(sat(luminosity));
                paint_texels += usize::from(lane && dash);
                let lod = footprint_lod(
                    levels.width,
                    levels.height,
                    [
                        [f64::from(period[0]) / f64::from(s), 0.0],
                        [0.0, f64::from(period[1]) / f64::from(s)],
                    ],
                    0.0,
                );
                let n = levels
                    .trilinear([f64::from(u), f64::from(v)], lod, [Address::Wrap; 2], transfer)
                    .expect("sample")
                    .map(|c| f64::from(c as f32));
                let i = ((py * SIZE + px) * 4) as usize;
                for ch in 0..3 {
                    let albedo = (col[ch] * paint).powf(2.2);
                    let b = (primase_image::linear_to_srgb(albedo.clamp(0.0, 1.0)) * 255.0).round() as u8;
                    worst[0] = worst[0].max(b.abs_diff(base_px[i + ch]));
                }
                let r = (n[3].clamp(0.0, 1.0) * 255.0).round() as u8;
                let metal = (sat(n[2] * metalness) * 255.0).round() as u8;
                worst[1] = worst[1]
                    .max(r.abs_diff(rough[i + 1]))
                    .max(metal.abs_diff(rough[i + 2]));
                let d = [n[0] * 2.0 - 1.0, n[1] * 2.0 - 1.0];
                let nz = (1.0 - d[0] * d[0] - d[1] * d[1]).max(0.0).sqrt();
                let len = (d[0] * d[0] + d[1] * d[1] + nz * nz).sqrt();
                for (ch, c) in [d[0], d[1], nz].into_iter().enumerate() {
                    let b = ((c / len * 0.5 + 0.5) * 255.0).round() as u8;
                    worst[2] = worst[2].max(b.abs_diff(normal[i + ch]));
                }
            }
        }
    }
    println!(
        "baked {baked} of {}, {paint_texels} paint texels, worst byte differences (albedo, roughness/metal, normal) \
         {worst:?}",
        ids.len()
    );
    assert!(paint_texels > 0, "no lane line baked");
    // the paint exact; the sampled maps within a byte: the bake's level of detail is the quad's f32 differences of a
    // coordinate spanning two units, the oracle's the exact period over the size (a rounding boundary apart)
    assert!(worst[0] <= 1 && worst[1] <= 1 && worst[2] <= 2, "{worst:?}");
}
