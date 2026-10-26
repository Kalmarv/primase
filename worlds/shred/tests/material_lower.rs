//! Hand-worked values and graph boundaries; source names alone do not prove shader semantics.
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{ColorSpace, Entity, Name, NormalConvention, TexFormat, TexRole, Texture, World};
use primase_shred::components::{
    EncodedTexture, MaterialKey, MaterialParameter, MaterialSampler, MaterialTextures, SamplerNameSource,
    SamplerTexture, ShredMaterial, SourceTexture, TextureCopy, TextureResidency,
};

fn run(w: &mut World, normals: &str) -> Result<RunReport, String> {
    run_options(w, normals, "named")
}

fn run_options(w: &mut World, normals: &str, alpha: &str) -> Result<RunReport, String> {
    let mut registry = PassRegistry::new();
    primase_shred::register(&mut registry);
    let pipeline = registry
        .parse(&format!(
            "lower-shred-materials{{normals={normals},alpha={alpha}}}"
        ))
        .map_err(|e| e.to_string())?;
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(w, &pipeline, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn material(w: &mut World, family: &str, names: &[&str], parameters: &[(&str, [f32; 4])]) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        ShredMaterial {
            key: MaterialKey {
                bank: "environments".into(),
                id: 4,
            },
            unknown_header: [0; 4],
            vertex_shader: "fixture-vs".into(),
            fragment_shader: format!("shaders\\pixel\\environment\\{family}"),
            parameters: parameters
                .iter()
                .map(|&(name, value)| MaterialParameter {
                    hash: primase_hash::sdbm(name.as_bytes()),
                    name: Some(name.into()),
                    source_encoding: 0x100ad,
                    value,
                })
                .collect(),
            samplers: names
                .iter()
                .enumerate()
                .map(|(i, &name)| MaterialSampler {
                    slot: i as u32,
                    hash: primase_hash::sdbm(name.as_bytes()),
                    name: Some(name.into()),
                    name_source: Some(SamplerNameSource::ExecutableStringSdbm),
                    texture: format!("{name}.source"),
                    unknown_state: [0; 5],
                })
                .collect(),
        },
    );
    w.insert(
        e,
        MaterialTextures(
            names
                .iter()
                .enumerate()
                .map(|(i, _)| SamplerTexture {
                    slot: i as u32,
                    texture: None,
                })
                .collect(),
        ),
    );
    e
}

fn pixels() -> Texture {
    Texture {
        width: 2,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: vec![vec![9, 128, 7, 128, 11, 0, 17, 255], vec![19, 255, 23, 128]],
        color_space: ColorSpace::Linear,
        role: TexRole::Other,
        normal: None,
    }
}

fn source(w: &mut World, texture: Option<Texture>) -> (Entity, Entity, Entity) {
    let full = w.spawn();
    if let Some(texture) = texture {
        w.insert(full, texture);
    }
    let tail = w.spawn();
    w.insert(
        tail,
        Texture {
            width: 1,
            height: 1,
            levels: vec![vec![1, 2, 3, 4]],
            ..pixels()
        },
    );
    let e = w.spawn();
    let mut header = vec![0; 84];
    header[62] = 2;
    w.insert(
        e,
        SourceTexture {
            path: "fixture/shared".into(),
            preferred: Some(1),
            copies: vec![
                TextureCopy {
                    archive: "prefetch".into(),
                    section: None,
                    header: header.clone(),
                    residency: TextureResidency::Prefetch,
                    first_mip: 1,
                    cube_tail: false,
                    faces: vec![tail],
                },
                TextureCopy {
                    archive: "streamed".into(),
                    section: None,
                    header,
                    residency: TextureResidency::Streamed,
                    first_mip: 0,
                    cube_tail: false,
                    faces: vec![full],
                },
            ],
        },
    );
    (e, full, tail)
}

fn bind(w: &mut World, material: Entity, slot: usize, source: Entity) {
    w.get_mut::<MaterialTextures>(material)
        .expect("fixture material texture graph")
        .0[slot]
        .texture = Some(source);
}

#[test]
fn preferred_copy_becomes_separate_role_views_without_mutating_source_or_fade_constant() {
    let mut w = World::new();
    let m = material(
        &mut w,
        "env_base_specular",
        &["Color_Map", "Spec_Map", "Effect_Map"],
        &[
            ("ColorConstant", [0.1, 0.2, 0.3, 0.0]),
            ("ColorBias", [0.5, 0.0, 0.0, 0.0]),
        ],
    );
    let (s, full, tail) = source(&mut w, Some(pixels()));
    for slot in 0..3 {
        bind(&mut w, m, slot, s);
    }
    let before = w.get::<Texture>(full).unwrap().clone();
    let graph = w.get::<SourceTexture>(s).unwrap().clone();
    let report = run(&mut w, "omit").unwrap();
    let p = w.get::<PbrMaterial>(m).unwrap();
    assert_eq!(p.base_color, [1.0; 4]); // ColorConstant is a demonstrated fade target, not a tint.
    assert_eq!((p.metallic, p.roughness), (0.0, 1.0));
    assert!(p.emissive_texture.is_none());
    let base = p.base_color_texture.unwrap().texture;
    let spec = p.specular.unwrap().color_texture.unwrap().texture;
    assert_ne!(base, full);
    assert_ne!(base, tail);
    assert_ne!(base, spec);
    for e in [base, spec] {
        let t = w.get::<Texture>(e).unwrap();
        assert_eq!(t.levels, before.levels);
        assert_eq!((t.color_space, t.role), (ColorSpace::Srgb, TexRole::Color));
    }
    assert_eq!(w.get::<Texture>(full), Some(&before));
    assert_eq!(w.get::<SourceTexture>(s), Some(&graph));
    assert!(w.get::<Name>(base).is_some());
    let text = format!("{report:?}");
    for reason in [
        "Effect_Map",
        "ColorConstant",
        "ColorBias",
        "UV0/repeat",
        "opaque alpha",
        "single-sided",
    ] {
        assert!(text.contains(reason), "{reason}: {text}");
    }
    let count = w.iter::<Texture>().count();
    run(&mut w, "omit").unwrap();
    assert_eq!(w.iter::<Texture>().count(), count); // A repeated pass leaves prepared PBR authoritative.
}

#[test]
fn all_mips_of_chosen_normal_encoding_are_converted_and_source_is_reachable() {
    for (mode, x, y, z) in [
        ("ag-up", 128, 128, 255),
        ("ag-down", 128, 127, 255),
        ("rgb-down", 9, 127, 7),
        ("rgb-up", 9, 128, 7),
    ] {
        let mut w = World::new();
        let m = material(&mut w, "fixture", &["Normal_Map"], &[]);
        let (s, full, tail) = source(&mut w, Some(pixels()));
        bind(&mut w, m, 0, s);
        run(&mut w, mode).unwrap();
        let e = w.get::<PbrMaterial>(m).unwrap().normal_texture.unwrap().texture;
        let normal = w.get::<Texture>(e).unwrap();
        assert_eq!(normal.levels.len(), 2);
        assert_eq!(&normal.levels[0][..3], &[x, y, z]);
        assert_eq!(
            (normal.color_space, normal.role, normal.normal),
            (ColorSpace::Linear, TexRole::Normal, Some(NormalConvention::GLTF))
        );
        if mode.starts_with("ag") {
            assert_eq!(normal.levels[1][2], 128);
        }
        assert_eq!(w.get::<Texture>(full), Some(&pixels()));
        let closure = w.closure([m]);
        assert!(closure.contains(&full) && closure.contains(&tail) && closure.contains(&e));
        assert!(w.verify_components().is_empty());
    }
}

#[test]
fn names_do_not_make_water_normal_pixels_a_colour_or_effects_emission() {
    let mut w = World::new();
    let m = material(
        &mut w,
        "env_water",
        &["Color_Map", "Normal_Map", "Effect_Map"],
        &[("baseWaterColor", [0.1, 0.2, 0.3, 0.0])],
    );
    let (s, _, _) = source(&mut w, Some(pixels()));
    for i in 0..3 {
        bind(&mut w, m, i, s);
    }
    run(&mut w, "omit").unwrap();
    let p = w.get::<PbrMaterial>(m).unwrap();
    assert_eq!(p.base_color, [0.1, 0.2, 0.3, 1.0]);
    assert!(p.base_color_texture.is_none() && p.normal_texture.is_none() && p.emissive_texture.is_none());
}

#[test]
fn named_alpha_uses_its_scalar_not_the_unused_colour_lane_and_reports_state_uncertainty() {
    let mut w = World::new();
    let m = material(
        &mut w,
        "env_tree_leafmesh",
        &[],
        &[("Alpha_test_ref", [0.25, 0.0, 0.0, 0.0])],
    );
    let report = run(&mut w, "omit").unwrap();
    let p = w.get::<PbrMaterial>(m).unwrap();
    assert_eq!(p.alpha_mode, AlphaMode::Mask { cutoff: 0.25 });
    assert_eq!(p.base_color[3], 1.0);
    assert!(format!("{report:?}").contains("state enable/comparison unproved"));
}

#[test]
fn missing_unsupported_and_not_loaded_are_distinct_and_ambiguous_data_refuses() {
    let mut w = World::new();
    let m = material(&mut w, "fixture", &["Color_Map"], &[]);
    let report = run(&mut w, "omit").unwrap();
    assert!(format!("{report:?}").contains("source path unavailable"));
    assert!(w.get::<PbrMaterial>(m).unwrap().base_color_texture.is_none());

    let mut w = World::new();
    let m = material(&mut w, "fixture", &["Color_Map"], &[]);
    let (s, full, _) = source(&mut w, None);
    bind(&mut w, m, 0, s);
    assert!(run(&mut w, "omit").unwrap_err().contains("pixels not loaded"));
    assert!(w.get::<PbrMaterial>(m).is_none());
    w.insert(
        full,
        EncodedTexture {
            bytes: vec![3, 4],
            reason: "volume".into(),
        },
    );
    assert!(format!("{:?}", run(&mut w, "omit").unwrap()).contains("encoded volume"));

    let mut w = World::new();
    material(
        &mut w,
        "env_tree_leafmesh",
        &[],
        &[("Alpha_test_ref", [0.5; 4]), ("Alpha_test_ref", [0.2; 4])],
    );
    assert!(
        run(&mut w, "omit")
            .unwrap_err()
            .contains("repeated Alpha_test_ref")
    );
    let mut registry = PassRegistry::new();
    primase_shred::register(&mut registry);
    assert!(registry.parse("lower-shred-materials").is_err());
    assert!(registry.parse("lower-shred-materials{normals=guessed}").is_err());
}

#[test]
fn threshold_presence_does_not_enable_cutout_outside_the_requested_environment_heuristic() {
    for family in ["effect_params", "char_matcap", "actor", "fixture"] {
        let mut w = World::new();
        let m = material(&mut w, family, &[], &[("Alpha_test_ref", [0.25, 0.0, 0.0, 0.0])]);
        let report = run(&mut w, "omit").unwrap();
        assert_eq!(w.get::<PbrMaterial>(m).unwrap().alpha_mode, AlphaMode::Opaque);
        assert!(format!("{report:?}").contains("parameter Alpha_test_ref omitted"));
    }
    let mut w = World::new();
    let leaf = material(
        &mut w,
        "env_tree_leafmesh",
        &[],
        &[("Alpha_test_ref", [0.25, 0.0, 0.0, 0.0])],
    );
    let glass = material(&mut w, "env_glass", &[], &[("GlassColor", [0.1, 0.2, 0.3, 0.4])]);
    run_options(&mut w, "omit", "opaque").unwrap();
    for e in [leaf, glass] {
        let pbr = w.get::<PbrMaterial>(e).unwrap();
        assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);
        assert_eq!(pbr.base_color[3], 1.0);
    }
}

#[test]
fn shared_source_and_role_reuse_one_view_across_materials() {
    let mut w = World::new();
    let a = material(&mut w, "fixture", &["Color_Map"], &[]);
    let b = material(&mut w, "fixture", &["Color_Map"], &[]);
    let (s, _, _) = source(&mut w, Some(pixels()));
    bind(&mut w, a, 0, s);
    bind(&mut w, b, 0, s);
    run(&mut w, "omit").unwrap();
    assert_eq!(
        w.get::<PbrMaterial>(a).unwrap().base_color_texture,
        w.get::<PbrMaterial>(b).unwrap().base_color_texture
    );
    assert_eq!(w.iter::<Texture>().count(), 3);
}
