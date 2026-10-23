use primase_fromsoft::materials::{Material, stem};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn matches(reference: &Value, material: &Material) -> bool {
    if !reference["mtd"]
        .as_str()
        .expect("oracle MTD")
        .eq_ignore_ascii_case(&material.mtd)
    {
        return false;
    }
    let Some(layered) = &material.layered else {
        return false;
    };
    let mut expected: BTreeMap<_, _> = layered
        .definition
        .textures
        .iter()
        .filter(|t| !t.path.trim().is_empty())
        .map(|t| (t.kind.clone(), (stem(&t.path), [1.; 2])))
        .collect();
    for binding in reference["bindings"].as_array().expect("oracle bindings") {
        let path = binding["path"].as_str().expect("binding path");
        if !path.trim().is_empty() {
            expected.insert(
                binding["kind"].as_str().expect("binding kind").into(),
                (
                    stem(path),
                    serde_json::from_value::<[f32; 2]>(binding["scale"].clone()).expect("binding scale"),
                ),
            );
        }
    }
    let actual: BTreeMap<_, _> = material
        .bindings
        .iter()
        .map(|(kind, b)| (kind.clone(), (b.texture.clone(), b.scale)))
        .collect();
    actual == expected
}

#[test]
fn partial_bindings_do_not_identify_a_material() {
    use primase_fromsoft::{flver, materials, mtd};
    let source = flver::Material {
        name: "part".into(),
        mtd: "test.mtd".into(),
        gx: vec![],
        textures: vec![
            flver::Texture {
                kind: "Texture2D_8_AlbedoMap".into(),
                path: "colour.dds".into(),
                scale: [1.; 2],
            ..Default::default()
        },
            flver::Texture {
                kind: "Texture2D_3_NormalMap".into(),
                path: "normal.dds".into(),
                scale: [2.; 2],
            ..Default::default()
        },
        ],
    };
    let definition = mtd::MaterialDefinition {
        shader: "M[Multiple].fx".into(),
        description: String::new(),
        parameters: vec![mtd::Parameter {
            name: "GXFT_SATStaticSwitch".into(),
            value: mtd::Value::Int(vec![0]),
        }],
        textures: vec![],
        block_lengths: vec![],
    };
    let material = materials::resolve(&source, Some(&definition), |_| true);
    let mut reference = serde_json::json!({"mtd":"test", "bindings":[
        {"kind":"Texture2D_8_AlbedoMap","path":"colour.dds","scale":[1,1]}
    ]});
    assert!(!matches(&reference, &material));
    reference["bindings"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!(
            {"kind":"Texture2D_3_NormalMap","path":"normal.dds","scale":[2,2]}
        ));
    assert!(matches(&reference, &material));
}
