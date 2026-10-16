#![allow(clippy::unwrap_used)]
use primase_dxbc::{Reflection, Type};
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
    let mut b = vec![0; 176];
    for (at, v) in [
        (0, 1),
        (4, 60),
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
    b[172..].copy_from_slice(b"B\0v\0");
    b
}
#[test]
fn checked_reflection_ranges_and_cycles() {
    let valid = rdef();
    let r = Reflection::parse(&fixture(&valid)).unwrap();
    assert_eq!(
        (r.buffers[0].name.as_str(), r.buffers[0].variables[0].ty.columns),
        ("B", 4)
    );
    let mut bad = valid.clone();
    set(&mut bad, 92, 17);
    assert!(
        Reflection::parse(&fixture(&bad))
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
        Reflection::parse(&fixture(&bad))
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
    let mut bad = valid.clone();
    set(&mut bad, 100, 10000);
    assert!(Reflection::parse(&fixture(&bad)).is_err());
    let mut bad = valid.clone();
    set(&mut bad, 40, 31);
    assert!(Reflection::parse(&fixture(&bad)).is_err());
    for n in 0..valid.len() {
        assert!(Reflection::parse(&fixture(&valid[..n])).is_err());
    }
}

fn leaves<'a>(name: &str, offset: u32, ty: &'a Type, out: &mut BTreeMap<String, (u32, &'a Type)>) {
    if ty.members.is_empty() {
        assert!(out.insert(name.to_owned(), (offset, ty)).is_none());
    }
    for m in &ty.members {
        leaves(&m.name, offset + m.offset, &m.ty, out);
    }
}

#[test]
fn reflected_bindings_match_independent_disassembly() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = primase_core::gate::real_data_value("PRIMASE_DXBC_SHADERS") else {
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
        let r = Reflection::parse(&std::fs::read(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        for (group, kind) in [("textures", 2), ("buffers", 0)] {
            for (name, value) in j[group].as_object().unwrap() {
                let resource = r
                    .resources
                    .iter()
                    .find(|b| b.kind == kind && b.name == *name)
                    .unwrap();
                assert_eq!(resource.slot as u64, value.as_u64().unwrap());
                bindings += 1;
            }
        }
        for (name, expected) in j["inputs"].as_object().unwrap() {
            assert!(
                r.inputs
                    .iter()
                    .any(|v| format!("{}{}", v.name.to_uppercase(), v.index) == *name
                        && v.register as u64 == expected.as_u64().unwrap()),
                "{name}"
            );
            inputs += 1;
        }
        for (buffer, expected) in j["layout"].as_object().unwrap() {
            let mut actual = BTreeMap::new();
            for v in &r.buffers.iter().find(|b| b.name == *buffer).unwrap().variables {
                leaves(&v.name, v.offset, &v.ty, &mut actual);
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
                    ty.elements.max(1) as u64,
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
