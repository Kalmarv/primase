use fromsoft_formats::{
    flver,
    mtd::{MaterialDefinition, Parameter, Texture, Value},
};
use primase_fromsoft::materials;
use primase_fromsoft::preview::{named_surface, preview};
fn definition() -> MaterialDefinition {
    MaterialDefinition {
        shader: "M[Multiple].sps".into(),
        description: String::new(),
        block_lengths: vec![],
        padding: vec![],
        parameters: vec![Parameter {
            name: "GXFT_SATStaticSwitch".into(),
            value: Value::Int(vec![0]),
        }],
        textures: vec![Texture {
            kind: "Texture2D_8_AlbedoMap".into(),
            uv_number: 1,
            shader_data_index: 8,
            path: "fixed_wood_a.dds".into(),
            extra: vec![],
        }],
    }
}
fn source() -> flver::Material {
    flver::Material {
        string_bytes: 0,
        index: 0,
        name: "synthetic".into(),
        mtd: "wood.mtd".into(),
        textures: vec![],
        gx: vec![],
    }
}
#[test]
fn raw_gx_payload_is_retained_in_material_dedup_identity() {
    let mut src = source();
    src.gx.push(flver::GxItem {
        id: *b"GXMD",
        version: 123,
        data: vec![1, 2, 3, 4],
    });
    let first = materials::resolve(&src, Some(&definition()), |_| true);
    assert_eq!(first.gx, src.gx);
    src.gx[0].data[3] = 5;
    let second = materials::resolve(&src, Some(&definition()), |_| true);
    // Area assembly keys its dedup map with this serialized source material.
    let identity = |m: &materials::Material| primase_core::json::to_value(m).unwrap().to_string();
    assert_ne!(identity(&first), identity(&second));
    assert_eq!(
        identity(&second),
        identity(&materials::resolve(&src, Some(&definition()), |_| true))
    );
}

#[test]
fn overrides_fixed_slots_and_preserves_all_layers() {
    let mut src = source();
    src.textures = vec![
        flver::Texture {
            kind: "Texture2D_8_AlbedoMap".into(),
            path: "custom_a.dds".into(),
            scale: [2., 3.],
            ..Default::default()
        },
        flver::Texture {
            kind: "Texture2D_3_NormalMap".into(),
            path: "custom_n.dds".into(),
            scale: [1.; 2],
            ..Default::default()
        },
        flver::Texture {
            kind: "Texture2D_5_AlbedoMap".into(),
            path: "moss_a.dds".into(),
            scale: [4.; 2],
            ..Default::default()
        },
    ];
    let m = materials::resolve(&src, Some(&definition()), |_| true);
    assert_eq!(preview(&m).albedo.as_deref(), Some("custom_a"));
    assert_eq!(preview(&m).normal.as_deref(), Some("custom_n"));
    assert_eq!(preview(&m).uv_scale, [2., 3.]);
    assert_eq!(m.layered.as_ref().unwrap().bindings.len(), 3);
    assert_eq!(
        m.layered.unwrap().bindings["Texture2D_5_AlbedoMap"].scale,
        [4.; 2]
    );
}
#[test]
fn missing_albedo_uses_available_layer() {
    let mut src = source();
    src.textures.push(flver::Texture {
        kind: "Texture2D_2_AlbedoMap".into(),
        path: "available_a.dds".into(),
        scale: [1.; 2],
        ..Default::default()
    });
    let m = materials::resolve(&src, Some(&definition()), |n| n == "available_a");
    assert_eq!(preview(&m).albedo.as_deref(), Some("available_a"));
    assert!(m.bindings.contains_key("Texture2D_8_AlbedoMap"));
}
#[test]
fn foliage_cutoff_and_missing_albedo() {
    let mut d = definition();
    d.shader = "M[Translucency]_e.sps".into();
    d.parameters.push(Parameter {
        name: "g_AlphaRef".into(),
        value: Value::Int(vec![128, 0]),
    });
    let m = materials::resolve(&source(), Some(&d), |_| false);
    assert_eq!(m.class, "leaves");
    assert_eq!(m.cutoff, 128. / 255.);
    assert_eq!(named_surface(&m).0, "default");
    // A target's class, domain and fallback colour are gated by its own tests.
    assert!(preview(&m).albedo.is_none());
    assert!(m.layered.is_none());
}

/// Water binds no base colour (its colour is the program's) and a normal map of its own: that map is its preview's
/// normal, where any other material with no base colour keeps none.
#[test]
fn water_previews_its_own_normal_map_without_a_base_colour() {
    let mut d = definition();
    d.shader = "Tessellation_Water_a.sps".into();
    d.textures = vec![
        Texture {
            kind: "Texture2D_3_NormalMap".into(),
            uv_number: 1,
            shader_data_index: 3,
            path: "m99_water_n.dds".into(),
            extra: vec![],
        },
        Texture {
            kind: "Texture2D_0_MaskMap".into(),
            uv_number: 1,
            shader_data_index: 0,
            path: "m10_waterformmask_3m.dds".into(),
            extra: vec![],
        },
    ];
    let mut s = source();
    s.textures = vec![
        flver::Texture {
            kind: "Texture2D_3_NormalMap".into(),
            path: "m99_water_n.dds".into(),
            scale: [1.; 2],
            ..Default::default()
        },
        flver::Texture {
            kind: "Texture2D_0_MaskMap".into(),
            path: "m10_waterformmask_3m.dds".into(),
            scale: [1.; 2],
            ..Default::default()
        },
    ];
    let water = materials::resolve(&s, Some(&d), |_| true);
    assert_eq!(water.class, "water");
    let p = preview(&water);
    assert!(p.albedo.is_none());
    assert_eq!(p.normal.as_deref(), Some("m99_water_n"));
    // the same textures on a material that is not water: no base colour, no normal map
    d.shader = "M[Multiple].sps".into();
    let other = materials::resolve(&s, Some(&d), |_| true);
    assert_ne!(other.class, "water");
    assert!(preview(&other).normal.is_none());
}

#[test]
fn graph_qualified_slot_names_select_the_main_layer() {
    let mut d = definition();
    d.textures[0].kind = "M_Multiple__snp_Texture2D_8_AlbedoMap_0".into();
    let m = materials::resolve(&source(), Some(&d), |_| true);
    assert_eq!(preview(&m).albedo.as_deref(), Some("fixed_wood_a"));
}

#[test]
fn numeric_slot_ties_prefer_the_selected_mtd_declaration() {
    let mut d = definition();
    d.textures[0].kind = "Ground_Texture2D_8_AlbedoMap_0".into();
    let mut s = source();
    s.textures.push(flver::Texture {
        kind: "Foreign_Texture2D_8_AlbedoMap_0".into(),
        path: "aaa_foreign_a.dds".into(),
        scale: [1.; 2],
        ..Default::default()
    });
    let m = materials::resolve(&s, Some(&d), |_| true);
    assert_eq!(preview(&m).albedo.as_deref(), Some("fixed_wood_a"));
    assert_eq!(m.bindings.len(), 2);
}

/// A FLVER texture record's tiling types become the binding's addressing: the types the record states, `None`
/// where it states none, and the MTD-only bindings state none.
#[test]
fn the_stored_tiling_types_become_the_bindings_addressing() {
    use fromsoft_formats::flver::Tiling;
    use iridia_interface::AddressMode;
    let mut src = source();
    src.textures = vec![
        flver::Texture {
            kind: "Texture2D_8_AlbedoMap".into(),
            path: "a.dds".into(),
            tiling: [Tiling::Repeat, Tiling::MirrorRepeat],
            ..Default::default()
        },
        flver::Texture {
            kind: "Texture2D_3_NormalMap".into(),
            path: "n.dds".into(),
            tiling: [Tiling::Clamp, Tiling::None],
            ..Default::default()
        },
        flver::Texture {
            kind: "Texture2D_5_AlbedoMap".into(),
            path: "b.dds".into(),
            tiling: [Tiling::Border, Tiling::MirrorOnce],
            ..Default::default()
        },
    ];
    let m = materials::resolve(&src, Some(&definition()), |_| true);
    let address = |kind: &str| m.layered.as_ref().unwrap().bindings[kind].address;
    assert_eq!(
        address("Texture2D_8_AlbedoMap"),
        [Some(AddressMode::Repeat), Some(AddressMode::Mirror)]
    );
    assert_eq!(address("Texture2D_3_NormalMap"), [Some(AddressMode::Clamp), None]);
    assert_eq!(
        address("Texture2D_5_AlbedoMap"),
        [Some(AddressMode::Border), Some(AddressMode::MirrorOnce)]
    );
}

/// A stated repeat and an unstated addressing sample alike, so they are one material identity; an addressing that
/// samples differently is another.
#[test]
fn material_identity_compares_the_addressing_as_sampled() {
    use fromsoft_formats::flver::Tiling;
    let identity = |u, v| {
        let mut src = source();
        src.textures = vec![flver::Texture {
            kind: "Texture2D_8_AlbedoMap".into(),
            path: "a.dds".into(),
            tiling: [u, v],
            ..Default::default()
        }];
        materials::resolve(&src, Some(&definition()), |_| true)
            .identity()
            .unwrap()
    };
    assert_eq!(
        identity(Tiling::None, Tiling::None),
        identity(Tiling::Repeat, Tiling::Repeat)
    );
    assert_ne!(
        identity(Tiling::Repeat, Tiling::Repeat),
        identity(Tiling::Repeat, Tiling::MirrorRepeat)
    );
    assert_ne!(
        identity(Tiling::Repeat, Tiling::Repeat),
        identity(Tiling::Clamp, Tiling::Repeat)
    );
}
