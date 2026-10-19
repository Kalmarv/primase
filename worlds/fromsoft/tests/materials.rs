use primase_fromsoft::{
    flver, materials,
    mtd::{MaterialDefinition, Parameter, Texture, Value},
};
fn definition() -> MaterialDefinition {
    MaterialDefinition {
        shader: "M[Multiple].sps".into(),
        description: String::new(),
        block_lengths: vec![],
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
        },
        flver::Texture {
            kind: "Texture2D_3_NormalMap".into(),
            path: "custom_n.dds".into(),
            scale: [1.; 2],
        },
        flver::Texture {
            kind: "Texture2D_5_AlbedoMap".into(),
            path: "moss_a.dds".into(),
            scale: [4.; 2],
        },
    ];
    let m = materials::resolve(&src, Some(&definition()), |_| true);
    assert_eq!(m.albedo.as_deref(), Some("custom_a"));
    assert_eq!(m.normal.as_deref(), Some("custom_n"));
    assert_eq!(m.uv_scale, [2., 3.]);
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
    });
    let m = materials::resolve(&src, Some(&definition()), |n| n == "available_a");
    assert_eq!(m.albedo.as_deref(), Some("available_a"));
    assert!(m.bindings.contains_key("Texture2D_8_AlbedoMap"));
}
#[test]
fn foliage_cutoff_and_writer_fallback() {
    let mut d = definition();
    d.shader = "M[Translucency]_e.sps".into();
    d.parameters.push(Parameter {
        name: "g_AlphaRef".into(),
        value: Value::Int(vec![128, 0]),
    });
    let m = materials::resolve(&source(), Some(&d), |_| false);
    assert_eq!(m.class, "leaves");
    assert_eq!(m.cutoff, 128. / 255.);
    assert_eq!(m.surface, "default");
    let output = m.skate_material("synthetic".into(), |_, _| None);
    assert_eq!(output.base_color, Some([128, 128, 128, 255]));
    assert_eq!(output.cutoff, Some(0.5019608));
    assert_eq!((output.cutoff.unwrap() as f32).to_bits(), m.cutoff.to_bits());
    assert_eq!(output.class, primase_reskate::dialect::MaterialClass::Leaves);
    assert_eq!(output.domain, Some(primase_reskate::dialect::Domain::Surface));
    assert!(m.layered.is_none());
}

#[test]
fn graph_qualified_slot_names_select_the_main_layer() {
    let mut d = definition();
    d.textures[0].kind = "M_Multiple__snp_Texture2D_8_AlbedoMap_0".into();
    let m = materials::resolve(&source(), Some(&d), |_| true);
    assert_eq!(m.albedo.as_deref(), Some("fixed_wood_a"));
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
    });
    let m = materials::resolve(&s, Some(&d), |_| true);
    assert_eq!(m.albedo.as_deref(), Some("fixed_wood_a"));
    assert_eq!(m.bindings.len(), 2);
}
