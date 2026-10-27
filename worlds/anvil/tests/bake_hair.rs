//! `bake-anvil-material` on the character hair (`MAD_CHR_Hair_Anisotropic`): the Materials of two hair colours of a rider's
//! hair baked at their maps' size and held against the program's arithmetic (its dump read: the zones' colour by the
//! strand map, the roughness, the occlusion and the normal) evaluated by hand on the same texels, the colours told
//! apart as the names say, and the Materials that need what the bake states of the vertex colour counted.
use std::collections::BTreeMap;

use anvil_formats::archive::Archive;
use anvil_formats::build_table::BUILD_TABLE;
use anvil_formats::rider::{HAIR_COLOURS, colours};
use anvil_formats::skeleton::SKELETON;
use iridia_exec::{
    Grid, Input, Invocation, Layout, Lod, PixelShader, ProgramShader, evaluate::Constants, evaluate_raw_with,
};
use primase_anvil::components::{AnvilMaterial, AnvilShaders, MaterialBinding, MaterialValue};
use primase_anvil::passes::BAKE_ANVIL_MATERIAL;
use primase_anvil::read::placements::{
    Gender, InstancedLevels, Placed, Placer, RiderPartChoice, RiderSpec, Variant,
};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{Entity, Name, Texture, World};
use primase_foundations::{sampling::Address, sampling::Levels, sampling::Transfer, sampling::footprint_lod};
use primase_testdata::{Game, install};

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

/// A rider's head with the hair `part` in the colour `colour` (a name of the hair colour table), placed with its
/// Materials and shaders read (the hair's skin weights name joints the head's addon skeleton gives the rig).
fn rider(archive: &mut Archive, part: &str, colour: &str) -> World {
    let list = colours(archive, HAIR_COLOURS).expect("hair colours");
    let colour = list.iter().find(|c| c.name == colour).expect("a hair colour");
    let spec = RiderSpec {
        name: "rider".to_owned(),
        skeletons: ["MAD_CHR_Skel_U", "MAD_CHR_SkelAddon_U", "MAD_CHR_SkelAddon_M"]
            .iter()
            .map(|n| id_of(archive, SKELETON, n))
            .collect(),
        gender: Gender::Male,
        parts: vec![
            RiderPartChoice {
                table: id_of(archive, BUILD_TABLE, "MAD_CHR_HEAD_000_U"),
                variant: Variant::Stated,
            },
            RiderPartChoice {
                table: id_of(archive, BUILD_TABLE, part),
                variant: Variant::Row {
                    table: colour.table,
                    condition: colour.condition,
                },
            },
        ],
    };
    let mut w = World::new();
    let mut placed = Placed::default();
    let rider = Placer::new(archive, &mut w, SourceFrame::WORLD, InstancedLevels::Selector)
        .with_materials()
        .with_shaders()
        .place_rider(&spec, &mut placed);
    assert!(
        rider.parts[1].refused.is_empty() && rider.parts[1].variant_refused.is_none(),
        "{:?} {:?}",
        rider.parts[1].refused,
        rider.parts[1].variant_refused
    );
    w
}

fn options(size: u32) -> Options {
    [
        ("size", size.to_string().as_str()),
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

/// The Material named `name` of the world, and what it holds.
fn material(w: &World, name: &str) -> (Entity, AnvilMaterial) {
    w.iter::<AnvilMaterial>()
        .find(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == name))
        .map(|(e, m)| (e, m.clone()))
        .unwrap_or_else(|| panic!("no Material {name}"))
}

/// A Material's scalar at the `cb3` row `row` (its property, else its template's default).
fn scalar(m: &AnvilMaterial, row: u32) -> f32 {
    let (key, binding) = m
        .bindings
        .iter()
        .find(|(_, b)| *b == MaterialBinding::Scalar { vec4: row })
        .unwrap_or_else(|| panic!("no scalar at row {row}"));
    match m
        .values
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .or_else(|| m.defaults.iter().find(|(b, _)| b == binding).map(|(_, v)| v))
    {
        Some(MaterialValue::Float(x)) => *x,
        other => panic!("row {row}: {other:?}"),
    }
}

/// A Material's vector at the `cb3` row `row`.
fn vector(m: &AnvilMaterial, row: u32) -> [f32; 3] {
    let (key, binding) = m
        .bindings
        .iter()
        .find(|(_, b)| *b == MaterialBinding::Vector { vec4: row })
        .unwrap_or_else(|| panic!("no vector at row {row}"));
    match m
        .values
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .or_else(|| m.defaults.iter().find(|(b, _)| b == binding).map(|(_, v)| v))
    {
        Some(MaterialValue::Float4(v)) => [v[0], v[1], v[2]],
        other => panic!("row {row}: {other:?}"),
    }
}

fn srgb8(linear: f32) -> f32 {
    (primase_foundations::color::linear_to_srgb(f64::from(linear.clamp(0.0, 1.0))) * 255.0).round() as f32
}

fn sat(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// One baked Material held against the program's arithmetic, over a grid of its texels. Returns the mean of the baked
/// colour's green (to tell the colours apart) and the numbers of texels compared, and off in colour, roughness, normal and
/// occlusion.
fn compare(w: &World, entity: Entity, m: &AnvilMaterial, size: u32) -> (f64, [u64; 5]) {
    let pbr = w.get::<PbrMaterial>(entity).expect("baked Material");
    let texture = |t: Option<&primase_core::pbr::TextureRef>| -> Vec<u8> {
        w.get::<Texture>(t.expect("a baked map").texture)
            .expect("texture")
            .levels[0]
            .clone()
    };
    let base = texture(pbr.base_color_texture.as_ref());
    let rough = texture(pbr.metallic_roughness_texture.as_ref());
    let normal = texture(pbr.normal_texture.as_ref());
    let occlusion = texture(pbr.occlusion_texture.as_ref());
    let bound = |slot: u32| -> Levels {
        let e = m
            .textures
            .iter()
            .find(|t| t.shader_slot == slot)
            .and_then(|t| t.texture)
            .unwrap_or_else(|| panic!("no texture at slot {slot}"));
        let t = w.get::<Texture>(e).expect("texture");
        assert_eq!((t.width, t.height), (size, size), "the maps are the bake's size");
        primase_image::levels(t).expect("levels")
    };
    let (mask, diffuse, normals) = (bound(0), bound(1), bound(2));
    // t76 is read as sRGB (its TextureMap's field 25), t75 and t77 as stored
    let lin = |b: u8| primase_foundations::color::srgb8_to_linear(b) as f32;
    let (k15, k16, k17, k21) = (scalar(m, 15), scalar(m, 16), scalar(m, 17), scalar(m, 21));
    let [c40, c41, c42, c43] = [40, 41, 42, 43].map(|r| vector(m, r));
    // the first zone only (the vertex colour's red 0), as the Material's second and third zone are off
    assert_eq!((scalar(m, 24), scalar(m, 25)), (0.0, 0.0));
    let (mut compared, mut colour_off, mut rough_off, mut normal_off, mut occlusion_off) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut green = 0.0f64;
    let step = (size / 64).max(1) as usize;
    for y in (0..size as usize).step_by(step) {
        for x in (0..size as usize).step_by(step) {
            let at = (y * size as usize + x) * 4;
            let [t75x, _, t75z, _] = mask
                .texel(0, x as i64, y as i64, [Address::Wrap; 2])
                .map(|c| f32::from(c) / 255.0);
            let t76 = diffuse.texel(0, x as i64, y as i64, [Address::Wrap; 2]).map(lin);
            let [t77x, t77y, ..] = normals
                .texel(0, x as i64, y as i64, [Address::Wrap; 2])
                .map(|c| f32::from(c) / 255.0);
            let (g, t76x, t76z) = (t76[1], t76[0], t76[2]);
            // colour: the first zone's two colours blended by the strand coordinate, tinted by the ID maps
            let f = sat(k15 * 100.0 * g.powf(k16 + 1.0));
            let expected: [f32; 3] = std::array::from_fn(|c| {
                let zone = c40[c] + f * (c41[c] - c40[c]);
                let tint = 0.5 * c43[c] * (1.0 + t76z) + t75z * c43[c] + t76x * c42[c];
                srgb8(zone * tint)
            });
            compared += 1;
            if base[at..at + 3]
                .iter()
                .zip(expected)
                .any(|(got, e)| (f32::from(*got) - e).abs() > 1.5)
            {
                colour_off += 1;
            }
            green += f64::from(base[at + 1]);
            // roughness: the first zone's `k17 (1 + k21 (t75.x - 1))`
            let expected_rough = (k17 * (1.0 + k21 * (t75x - 1.0))).clamp(0.0, 1.0) * 255.0;
            if (f32::from(rough[at + 1]) - expected_rough).abs() > 1.5 {
                rough_off += 1;
            }
            // the normal: Normal's red and green, z rebuilt, unit length
            let (nx, ny) = (t77x * 2.0 - 1.0, t77y * 2.0 - 1.0);
            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let want = [nx / len, ny / len, nz / len].map(|c| (c * 0.5 + 0.5) * 255.0);
            // the 9-bit octahedral store: a few units of 255
            if normal[at..at + 3]
                .iter()
                .zip(want)
                .any(|(got, e)| (f32::from(*got) - e).abs() > 3.0)
            {
                normal_off += 1;
            }
            // the occlusion: Diffuse's blue (the vertex occlusion is 1)
            let expected_occlusion = t76z.clamp(0.0, 1.0) * 255.0;
            if (f32::from(occlusion[at]) - expected_occlusion).abs() > 1.5 {
                occlusion_off += 1;
            }
        }
    }
    (
        green / compared as f64,
        [compared, colour_off, rough_off, normal_off, occlusion_off],
    )
}

/// A texture the executor samples trilinear and wrapped at a quad's footprint.
#[derive(Debug)]
struct Atlas(Levels);

impl iridia_exec::Image for Atlas {
    fn sample(&self, uv: [f32; 2], lod: Lod) -> Result<[f32; 4], String> {
        let (w, h) = (self.0.width, self.0.height);
        let grad = |g: [[f32; 2]; 2]| g.map(|d| d.map(f64::from));
        let lod = match lod {
            Lod::Gradients(g) => footprint_lod(w, h, grad(g), 0.0),
            Lod::GradientsBias(g, b) => footprint_lod(w, h, grad(g), f64::from(b.clamp(-16.0, 15.99))),
            Lod::Level(l) => f64::from(l),
            Lod::Implicit | Lod::Bias(_) => return Err("an implicit level without a quad".into()),
        };
        let v = self
            .0
            .trilinear(
                [f64::from(uv[0]), f64::from(uv[1])],
                lod,
                [Address::Wrap; 2],
                Transfer::Stored,
            )
            .map_err(|e| e.to_string())?;
        Ok(v.map(|c| c as f32))
    }
}

/// The texels the template's alpha-tested G-buffer permutation (`0x404000`) keeps over the Material's texture square: that
/// program run as the game's own, its Mask1 at t75, the Material's alpha reference at `cb2[18].y`, the dither's constants
/// 0.
fn kept_by_the_alpha_tested_program(w: &World, m: &AnvilMaterial, size: u32) -> Vec<bool> {
    let shaders = m
        .shaders
        .and_then(|s| w.get::<AnvilShaders>(s))
        .expect("the template's shaders");
    let program = shaders.pixel(0x40_4000).expect("the alpha-tested permutation");
    let shader = ProgramShader::new(iridia_dxbc::lift(program).expect("lift")).expect("shader");
    let buffers: Vec<(u32, u32)> = PixelShader::constant_buffers(&shader)
        .iter()
        .map(|&(slot, n)| (slot, n.saturating_mul(4)))
        .collect();
    let mut constants = Constants::new(&Layout {
        buffers,
        fields: BTreeMap::new(),
    });
    let reference = f32::from(m.flags.alpha_test_value) / 255.0;
    constants
        .set_words(2, 18 * 4 + 1, &[reference.to_bits()])
        .expect("cb2");
    let mask1 = m
        .textures
        .iter()
        .find(|t| t.shader_slot == 0)
        .and_then(|t| t.texture)
        .expect("Mask1");
    let image = primase_image::levels(w.get::<Texture>(mask1).expect("texture")).expect("levels");
    let mut textures = BTreeMap::new();
    textures.insert(75, iridia_exec::View::Image(std::sync::Arc::new(Atlas(image))));
    for slot in [76, 77] {
        textures.insert(slot, iridia_exec::View::Constant([1.0; 4]));
    }
    let textures = iridia_exec::Views::refusing(textures);
    let constant = |v: [f32; 4]| Input::Constant(v);
    let inputs = BTreeMap::from([
        (("SV_Position".to_owned(), 0), constant([0.0, 0.0, 0.0, 1.0])),
        (("TEXCOORD".to_owned(), 0), Input::GridUv),
        (("TEXCOORD".to_owned(), 1), constant([0.0; 4])),
        (("TEXCOORD".to_owned(), 2), constant([1.0, 0.0, 0.0, 0.0])),
        (("TEXCOORD".to_owned(), 3), constant([0.0, 0.0, 1.0, 1.0])),
        (("TEXCOORD".to_owned(), 4), constant([0.0, 0.0, 0.0, 1.0])),
        (("COLOR".to_owned(), 0), constant([0.0, 1.0, 1.0, 1.0])),
    ]);
    let raw = evaluate_raw_with(
        primase_core::parallel::threads(w),
        &shader,
        &constants,
        &textures,
        &inputs,
        Grid {
            size,
            period: [1.0; 2],
        },
        &[(0, 15)],
        Invocation::Quads,
    )
    .expect("the alpha-tested permutation runs");
    raw.discarded.iter().map(|discarded| !discarded).collect()
}

/// The Materials of a hair in two colours, baked at their maps' size: the colour, roughness, normal and occlusion are the
/// program's arithmetic of the maps and rows, the blond is the lighter, the cost of what the bake leaves out is counted.
#[test]
fn a_hair_material_bakes_to_the_programs_arithmetic() {
    let Some(mut archive) = open() else {
        return;
    };
    let mut means: BTreeMap<&str, f64> = BTreeMap::new();
    for name in ["MAD_CHR_HairCut_V001_Blond", "MAD_CHR_HairCut_V002_Chatin"] {
        let mut w = rider(&mut archive, "MAD_CHR_Hair_005_U_Full", name);
        let (entity, m) = material(&w, name);
        assert_eq!(m.template_name.as_deref(), Some("MAD_CHR_Hair_Anisotropic"));
        let size = {
            let e = m
                .textures
                .iter()
                .find(|t| t.shader_slot == 1)
                .and_then(|t| t.texture)
                .expect("the Diffuse");
            w.get::<Texture>(e).expect("texture").width
        };
        let pass = BAKE_ANVIL_MATERIAL.instance(&options(size)).expect("options");
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).expect("bake");
        assert_eq!(
            log.counts.get("MAD_CHR_Hair_Anisotropic Materials baked"),
            Some(&1),
            "{name}: {:?} {:?}",
            log.counts,
            log.omitted
        );
        // the generic Material the mesh names (TwoSided with FlipNormalOnBackFace) is not baked, and said so
        assert!(
            log.omitted
                .keys()
                .any(|k| k.contains("MAD_CHR_Hair_Anisotropic Materials not baked")),
            "{:?}",
            log.omitted
        );
        // the specular occlusion, translucency and strand direction have no place: counted
        assert!(
            log.omitted
                .keys()
                .any(|k| k.contains("MAD_CHR_Hair_Anisotropic Materials' specular occlusion")),
            "{:?}",
            log.omitted
        );
        let (green, [compared, colour_off, rough_off, normal_off, occlusion_off]) =
            compare(&w, entity, &m, size);
        println!(
            "{name}: {size} px, {compared} texels compared, off: {colour_off} colour, {rough_off} roughness, {normal_off} normal, {occlusion_off} occlusion; mean baked green {green:.1}"
        );
        assert!(compared > 3000);
        assert_eq!(
            (colour_off, rough_off, normal_off, occlusion_off),
            (0, 0, 0, 0),
            "{name}"
        );
        // the cut-out: a mask at the Material's AlphaTestValue, the base colour's alpha Mask1's, and the texels it keeps
        // the ones the game's own alpha-tested program keeps
        let pbr = w.get::<PbrMaterial>(entity).expect("baked");
        assert_eq!(
            pbr.alpha_mode,
            AlphaMode::Mask {
                cutoff: f32::from(m.flags.alpha_test_value) / 255.0
            }
        );
        assert_eq!(
            log.counts
                .get("MAD_CHR_Hair_Anisotropic Materials baked with their alpha cut-out"),
            Some(&1)
        );
        let base = &w
            .get::<Texture>(pbr.base_color_texture.as_ref().expect("base").texture)
            .expect("texture")
            .levels[0];
        let kept = kept_by_the_alpha_tested_program(&w, &m, size);
        let n = size as usize;
        let cut = m.flags.alpha_test_value;
        let (mut kept_n, mut disagree) = (0u64, 0u64);
        for p in 0..n * n {
            let ours = base[p * 4 + 3] >= cut;
            kept_n += u64::from(kept[p]);
            disagree += u64::from(ours != kept[p]);
        }
        println!(
            "{name}: the alpha-tested program keeps {kept_n} of {} texels; the baked alpha disagrees at {disagree}",
            n * n
        );
        assert!(kept_n > (n * n) as u64 / 10 && kept_n < (n * n) as u64 * 9 / 10);
        assert_eq!(disagree, 0, "{name}");
        means.insert(name, green);
    }
    // blond is lighter than chestnut
    assert!(
        means["MAD_CHR_HairCut_V001_Blond"] > means["MAD_CHR_HairCut_V002_Chatin"] + 10.0,
        "{means:?}"
    );
}

/// A colour whose third hair zone is on (`Use Z3`, rows 24 and 25 are the switches) is baked as its first zone, and
/// counted as so approximated.
#[test]
fn a_hair_material_with_a_second_zone_is_counted() {
    let Some(mut archive) = open() else {
        return;
    };
    let name = "MAD_CHR_HairCut_V007_BlueBlond";
    let mut w = rider(&mut archive, "MAD_CHR_Hair_005_U_Full", name);
    let (entity, m) = material(&w, name);
    assert_eq!(scalar(&m, 25), 1.0, "its third zone is on");
    let pass = BAKE_ANVIL_MATERIAL.instance(&options(128)).expect("options");
    let mut log = PassLog::default();
    pass.run(&mut w, &mut log).expect("bake");
    assert!(w.get::<PbrMaterial>(entity).is_some(), "{:?}", log.omitted);
    assert!(
        log.approximations
            .keys()
            .any(|k| k
                .contains("MAD_CHR_Hair_Anisotropic Materials with the hair zones the mesh's vertex colour")),
        "{:?}",
        log.approximations
    );
}
