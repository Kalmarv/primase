#![allow(clippy::unwrap_used)]
use primase_shader::{ConstantType, ResourceKind, ScalarType, Value};
use std::collections::BTreeMap;

fn set(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn fixture(rdef: &[u8]) -> Vec<u8> {
    let signature = [0u32.to_le_bytes(), 8u32.to_le_bytes()].concat();
    let mut bytes = vec![0; 44];
    bytes[..4].copy_from_slice(b"DXBC");
    set(&mut bytes, 20, 1);
    set(&mut bytes, 28, 3);
    for (i, (kind, data)) in [
        (b"RDEF", rdef),
        (b"ISGN", signature.as_slice()),
        (b"OSGN", signature.as_slice()),
    ]
    .into_iter()
    .enumerate()
    {
        let at = bytes.len();
        set(&mut bytes, 32 + 4 * i, at as u32);
        bytes.extend(kind);
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
    }
    let len = bytes.len();
    set(&mut bytes, 24, len as u32);
    bytes
}
fn rdef() -> Vec<u8> {
    let mut b = vec![0; 208];
    for (at, v) in [
        (0, 1),
        (4, 60),
        // one resource record, at 176: the constant buffer B at b0
        (8, 1),
        (12, 176),
        (176, 172),
        (200, 1),
        (16, 0xffff0500),
        (28, u32::from_le_bytes(*b"RD11")),
        (32, 60),
        (36, 24),
        (40, 32),
        (44, 40),
        (48, 36),
        (52, 12),
        (60, 172),
        (64, 1),
        (68, 84),
        (72, 16),
        (84, 174),
        (92, 16),
        (100, 124),
        (124, 1 | 3 << 16),
        (128, 1 | 4 << 16),
    ] {
        set(&mut b, at, v);
    }
    b[172..176].copy_from_slice(b"B\0v\0");
    b
}
#[test]
fn checked_reflection_ranges_and_cycles() {
    let valid = rdef();
    let r = primase_dxbc::reflection(&fixture(&valid)).unwrap();
    let ResourceKind::ConstantBuffer { layout } = &r.resources[0].kind else {
        panic!("a constant buffer")
    };
    assert_eq!(
        (
            r.resources[0].provenance.name.as_str(),
            r.resources[0].provenance.d3d.unwrap().register,
            layout.constants[0].ty.clone()
        ),
        (
            "B",
            0,
            ConstantType::Vector(Value {
                scalar: ScalarType::Float,
                width: 4
            })
        )
    );
    // what the mapping does not know is refused by name: a tbuffer, a resource type with a counter, flags and counts
    for (at, value, named) in [
        (180, 1, "D3D_SIT_TBUFFER"),
        (180, 9, "D3D_SIT_UAV_APPEND_STRUCTURED"),
        (180, 99, "an unknown resource type"),
        (204, 0x40, "flags 0x40"),
        (200, 0, "bind count 0"),
    ] {
        let mut bad = valid.clone();
        set(&mut bad, at, value);
        let message = primase_dxbc::reflection(&fixture(&bad)).unwrap_err().to_string();
        assert!(message.contains(named), "{named}: {message}");
    }
    let mut bad = valid.clone();
    set(&mut bad, 80, 1);
    let message = primase_dxbc::reflection(&fixture(&bad)).unwrap_err().to_string();
    assert!(message.contains("tbuffer"), "{message}");
    let mut bad = valid.clone();
    set(&mut bad, 92, 17);
    assert!(
        primase_dxbc::reflection(&fixture(&bad))
            .unwrap_err()
            .to_string()
            .contains("exceeds constant buffer")
    );
    let mut bad = valid.clone();
    set(&mut bad, 124, 5);
    set(&mut bad, 132, 1 << 16);
    set(&mut bad, 136, 160);
    set(&mut bad, 160, 174);
    set(&mut bad, 164, 124);
    assert!(
        primase_dxbc::reflection(&fixture(&bad))
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
    let mut bad = valid.clone();
    set(&mut bad, 100, 10000);
    assert!(primase_dxbc::reflection(&fixture(&bad)).is_err());
    let mut bad = valid.clone();
    set(&mut bad, 40, 31);
    assert!(primase_dxbc::reflection(&fixture(&bad)).is_err());
    for n in 0..valid.len() {
        assert!(primase_dxbc::reflection(&fixture(&valid[..n])).is_err());
    }
}

fn leaves<'a>(
    name: &str,
    offset: u32,
    ty: &'a ConstantType,
    out: &mut BTreeMap<String, (u32, &'a ConstantType)>,
) {
    if ty.members().is_empty() {
        assert!(out.insert(name.to_owned(), (offset, ty)).is_none());
    }
    for m in ty.members() {
        leaves(&m.name, offset + m.offset, &m.ty, out);
    }
}

#[test]
fn reflected_bindings_match_independent_disassembly() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = primase_testdata::real_data_value("PRIMASE_DXBC_SHADERS") else {
        eprintln!("SKIPPED: PRIMASE_DXBC_SHADERS not set");
        return Ok(());
    };
    let (mut shaders, mut fields, mut bindings, mut inputs) = (0, 0, 0, 0);
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_none_or(|x| x != "ppo") {
            continue;
        }
        let oracle = path.with_extension("ppo.reflection.json");
        if !oracle.exists() {
            continue;
        }
        let j: serde_json::Value = serde_json::from_slice(&std::fs::read(oracle)?)?;
        let r = primase_dxbc::reflection(&std::fs::read(&path)?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        for (group, textures) in [("textures", true), ("buffers", false)] {
            for (name, value) in j[group].as_object().unwrap() {
                let resource = r
                    .resources
                    .iter()
                    .find(|b| {
                        matches!(b.kind, ResourceKind::Texture { .. }) == textures
                            && matches!(
                                b.kind,
                                ResourceKind::Texture { .. } | ResourceKind::ConstantBuffer { .. }
                            )
                            && b.provenance.name == *name
                    })
                    .unwrap();
                assert_eq!(
                    resource.provenance.d3d.unwrap().register as u64,
                    value.as_u64().unwrap()
                );
                bindings += 1;
            }
        }
        for (name, expected) in j["inputs"].as_object().unwrap() {
            assert!(
                r.inputs.iter().any(
                    |v| format!("{}{}", v.label.name.to_uppercase(), v.label.index) == *name
                        && v.provenance.d3d.unwrap().register as u64 == expected.as_u64().unwrap()
                ),
                "{name}"
            );
            inputs += 1;
        }
        for (buffer, expected) in j["layout"].as_object().unwrap() {
            let mut actual = BTreeMap::new();
            let resource = r.resources.iter().find(|b| b.provenance.name == *buffer).unwrap();
            let ResourceKind::ConstantBuffer { layout } = &resource.kind else {
                panic!("{buffer} is not a constant buffer")
            };
            for c in &layout.constants {
                leaves(&c.name, c.offset, &c.ty, &mut actual);
            }
            for (name, expected) in expected.as_object().unwrap() {
                let (offset, ty) = actual
                    .get(name)
                    .unwrap_or_else(|| panic!("{} {buffer}.{name}", path.display()));
                assert_eq!(
                    *offset as u64,
                    expected["offset"].as_u64().unwrap(),
                    "{buffer}.{name}"
                );
                assert_eq!(
                    match ty {
                        ConstantType::Array { count, .. } => *count as u64,
                        _ => 1,
                    },
                    expected["count"].as_u64().unwrap(),
                    "{buffer}.{name} count"
                );
                fields += 1;
            }
        }
        shaders += 1;
    }
    assert!(shaders > 0, "no reflection fixtures");
    println!(
        "reflection exact: {shaders} shaders, {bindings} resource bindings, {inputs} input registers, {fields} member offsets/counts"
    );
    Ok(())
}
