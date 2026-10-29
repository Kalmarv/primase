//! The material view the binding rules read ([`primase_fromsoft::materials::MaterialView`]): an MTD's and a material
//! binary's parameters in one vocabulary, and the program names they select.
use fromsoft_formats::matbin::{self, Param, Sampler, Value as B};
use fromsoft_formats::mtd::{MaterialDefinition, Parameter, Texture, Value as M};
use primase_fromsoft::materials::{DefinitionRef, MaterialView, ParameterValue};
use std::collections::BTreeMap;

fn param(name: &str, value: B) -> Param {
    Param {
        name: name.into(),
        key: 0,
        value,
    }
}

/// An Elden Ring material binary of shader `M[AMSN_V]` with the parameters given and one sampler.
fn binary(params: Vec<Param>) -> matbin::Material {
    matbin::Material {
        shader: r"N:\GR\data\INTERROOT_win64\material\shader\M[AMSN_V].spx".into(),
        source: "M[AMSN_V].matxml".into(),
        key: 0,
        params,
        samplers: vec![Sampler {
            kind: "M_AMSN_V_snp_Texture2D_1_AlbedoMap_0".into(),
            path: String::new(),
            key: 0,
            unknown: [0.; 2],
        }],
        unreferenced_tail: Vec::new(),
    }
}

fn mtd(parameters: Vec<(&str, M)>) -> MaterialDefinition {
    MaterialDefinition {
        shader: "M[ARSN].spx".into(),
        description: String::new(),
        parameters: parameters
            .into_iter()
            .map(|(name, value)| Parameter {
                name: name.into(),
                value,
            })
            .collect(),
        textures: vec![Texture {
            kind: "g_DiffuseTexture".into(),
            uv_number: 1,
            shader_data_index: 0,
            path: String::new(),
            extra: Vec::new(),
            version: 5,
        }],
        block_lengths: Vec::new(),
        padding: Vec::new(),
    }
}

#[test]
fn a_material_binarys_values_read_in_the_shared_vocabulary() {
    let m = binary(vec![
        param("g_AlphaRef", B::Int2([128, 0])),
        param("g_BlendMode", B::Int(2)),
        param("g_GX_bDisableDepthWrite", B::Bool(true)),
        // a float3 colour stored with two more floats: the value is its three
        param("g_DiffuseMapColor", B::Float3([1., 0.5, 0.25], Some([1., 1.]))),
        param("group_1_CommonUV-UVParam", B::Float2([5., 5.])),
        param(
            "short",
            B::Truncated {
                code: 11,
                bytes: vec![0; 4],
            },
        ),
    ]);
    let d = DefinitionRef::Matbin(&m);
    assert_eq!(d.shader_name(), "M[AMSN_V]");
    assert_eq!(d.parameter("g_AlphaRef"), Some(ParameterValue::Int(vec![128, 0])));
    assert_eq!(
        d.parameter("g_DiffuseMapColor"),
        Some(ParameterValue::Float(vec![1., 0.5, 0.25]))
    );
    assert_eq!(
        d.parameter("group_1_CommonUV-UVParam"),
        Some(ParameterValue::Float(vec![5., 5.]))
    );
    assert_eq!(
        d.parameter("g_GX_bDisableDepthWrite"),
        Some(ParameterValue::Bool(true))
    );
    // a value stored shorter than its type is not one, and an absent parameter is absent
    assert_eq!(d.parameter("short"), None);
    assert_eq!(d.parameter("missing"), None);
    assert!(d.declares("M_AMSN_V_snp_Texture2D_1_AlbedoMap_0"));
    assert!(!d.declares("g_DiffuseTexture"));
}

#[test]
fn a_material_binary_is_drawn_forward_where_it_blends_or_states_forward_rendering() {
    let bindings = BTreeMap::new();
    let view = |m: &matbin::Material| -> (Option<i32>, bool, String) {
        let v = MaterialView {
            mtd: "m",
            definition: DefinitionRef::Matbin(m),
            bindings: &bindings,
        };
        // whether the placements drawing it receive shadows does not matter to a material binary
        assert_eq!(v.pass_program(true), v.pass_program(false));
        (v.blend_mode(), v.is_forward(), v.pass_program(false))
    };
    // blend mode 2 stated: the shadowed forward program (every forward colour draw of the recorded Elden Ring frames is)
    let blended = binary(vec![
        param("g_BlendMode", B::Int(2)),
        param("g_AlphaRef", B::Int2([0, 0])),
    ]);
    assert_eq!(view(&blended), (Some(2), true, "M[AMSN_V]_FwdSdw.ppo".into()));
    // opaque, stating forward rendering (the skysphere `M[Sky9]`, the cloud shell `M[Clouds9]`): forward
    let stated = binary(vec![
        param("g_BlendMode", B::Int(0)),
        param("g_GX_ForwardRendering", B::Bool(true)),
    ]);
    assert_eq!(view(&stated), (Some(0), true, "M[AMSN_V]_FwdSdw.ppo".into()));
    // the plain forward program, which a bundle without the shadowed one is drawn with, and the vertex program's pass
    let v = MaterialView {
        mtd: "m",
        definition: DefinitionRef::Matbin(&stated),
        bindings: &bindings,
    };
    assert_eq!(
        (v.plain_pass_program(), v.pass()),
        ("M[AMSN_V]_Fwd.ppo".into(), "Fwd")
    );
    // forward rendering stated false, and an additive blend (4) alone, which no recorded frame draws: the G-buffer's
    let opaque = binary(vec![
        param("g_BlendMode", B::Int(0)),
        param("g_GX_ForwardRendering", B::Bool(false)),
    ]);
    assert_eq!(view(&opaque), (Some(0), false, "M[AMSN_V]_Gbuf.ppo".into()));
    let added = binary(vec![param("g_BlendMode", B::Int(4))]);
    assert_eq!(view(&added), (Some(4), false, "M[AMSN_V]_Gbuf.ppo".into()));
    // unstated is not 0; an alpha reference selects the alpha-tested permutation; a switch names the program
    let tested = binary(vec![
        param("g_AlphaRef", B::Int2([64, 0])),
        param("GXFT_SATStaticSwitch", B::Int(3)),
    ]);
    assert_eq!(view(&tested), (None, false, "M[AMSN_V]_3_Gbuf_[A].ppo".into()));
}

/// Sekiro's sky dome and far mountains are opaque (`g_BlendMode` 0) and state `g_GX_ForwardRendering`: the recorded
/// frames draw them forward, so their pixel program is the forward one, with the switch and alpha test as before; the
/// plain `Fwd` (the sky draws them, in a job without shadows), the shadowed `FwdSdw` where placements receiving shadows
/// draw it.
#[test]
fn an_opaque_mtd_stating_forward_rendering_is_drawn_forward() {
    let bindings = BTreeMap::new();
    let program = |d: &MaterialDefinition| -> (bool, String) {
        let v = MaterialView {
            mtd: "m",
            definition: DefinitionRef::Mtd(d),
            bindings: &bindings,
        };
        (v.is_forward(), v.pass_program(false))
    };
    let dome = mtd(vec![
        ("g_BlendMode", M::Int(vec![0])),
        ("g_AlphaRef", M::Int(vec![0, 0])),
        ("GXFT_SATStaticSwitch", M::Int(vec![1])),
        ("g_GX_ForwardRendering", M::Bool(true)),
    ]);
    assert_eq!(program(&dome), (true, "M[ARSN]_1_Fwd.ppo".into()));
    let receiving = MaterialView {
        mtd: "m",
        definition: DefinitionRef::Mtd(&dome),
        bindings: &bindings,
    };
    assert_eq!(receiving.pass_program(true), "M[ARSN]_1_FwdSdw.ppo");
    let mountains = mtd(vec![
        ("g_BlendMode", M::Int(vec![0])),
        ("g_AlphaRef", M::Int(vec![64, 0])),
        ("g_GX_ForwardRendering", M::Bool(true)),
    ]);
    assert_eq!(program(&mountains), (true, "M[ARSN]_Fwd_[A].ppo".into()));
    // the same opaque MTD without it keeps the G-buffer program
    let wall = mtd(vec![("g_BlendMode", M::Int(vec![0]))]);
    assert_eq!(program(&wall), (false, "M[ARSN]_Gbuf.ppo".into()));
    let v = MaterialView {
        mtd: "m",
        definition: DefinitionRef::Mtd(&wall),
        bindings: &bindings,
    };
    assert_eq!(v.pass_program(true), "M[ARSN]_Gbuf.ppo");
}

#[test]
fn an_mtds_values_and_programs_are_as_before() {
    let bindings = BTreeMap::new();
    let d = mtd(vec![
        ("g_BlendMode", M::Int(vec![2])),
        ("g_AlphaRef", M::Int(vec![128, 0])),
        ("GXFT_SATStaticSwitch", M::Int(vec![7])),
    ]);
    let v = MaterialView {
        mtd: "m",
        definition: DefinitionRef::Mtd(&d),
        bindings: &bindings,
    };
    assert_eq!(v.blend_mode(), Some(2));
    assert!(v.is_forward());
    assert_eq!(v.pass_program(false), "M[ARSN]_7_Fwd_[A].ppo");
    assert_eq!(v.pass_program(true), "M[ARSN]_7_FwdSdw_[A].ppo");
    assert_eq!(v.gbuffer_program(), "M[ARSN]_7_Gbuf_[A].ppo");
    assert_eq!(v.program_name().unwrap(), "M[ARSN]_7_Gbuf_[A].ppo");
    assert!(v.definition.declares("g_DiffuseTexture"));
    // without a switch the layered program name is refused
    let plain = mtd(Vec::new());
    let v = MaterialView {
        mtd: "m",
        definition: DefinitionRef::Mtd(&plain),
        bindings: &bindings,
    };
    assert!(v.program_name().is_err());
    assert_eq!((v.blend_mode(), v.alpha_reference()), (None, 0));
}
