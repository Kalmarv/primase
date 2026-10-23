#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The shader database in the world: a database written by hand around two fixture programs (a vertex and a pixel
//! program compiled from this repository's own HLSL) and a compute stand-in the world has no stage for.
use std::path::Path;

use primase_core::shader::{Shader, ShaderFormat, Stage};
use primase_core::{Entity, Name, World};
use primase_gta::components::{GtaEffect, GtaStage};
use primase_gta::rage::sgd2;
use primase_gta::read::shaders::add_shader_database;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../toolkit/dxil/tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn name(out: &mut Vec<u8>, s: &str) {
    out.push(u8::try_from(s.len() + 1).unwrap());
    out.extend(s.as_bytes());
    out.push(0);
}

/// The layout [`sgd2`] documents: eight groups of programs, then the effects.
fn database(groups: [Vec<(&str, Vec<u8>)>; 8], effects: &[(&str, [Vec<u32>; 6])]) -> Vec<u8> {
    let mut b = b"SGD2".to_vec();
    for group in &groups {
        b.extend(u32::try_from(group.len()).unwrap().to_le_bytes());
        for (n, container) in group {
            name(&mut b, n);
            b.push(0x32);
            b.extend(u32::try_from(container.len()).unwrap().to_le_bytes());
            b.extend(container);
            b.extend([0u8; 152]);
            b.extend(0u32.to_le_bytes());
        }
    }
    b.extend(u32::try_from(effects.len()).unwrap().to_le_bytes());
    for (n, lists) in effects {
        name(&mut b, n);
        b.extend(7u32.to_le_bytes());
        for list in lists {
            b.extend(u32::try_from(list.len()).unwrap().to_le_bytes());
            for i in list {
                b.extend(i.to_le_bytes());
            }
        }
        b.extend([0u8, 0, 1, 5, 0x40, 0x41]);
        b.extend(b"\0Param\0");
    }
    b
}

#[test]
fn programs_become_shader_entities_and_effects_name_them() {
    let vs = fixture("vertex.dxil");
    let ps = fixture("blend.dxil");
    // a compute stand-in: a container the world has no stage for (its bytes are never read)
    let mut cs = b"DXBC".to_vec();
    cs.extend([0u8; 20]);
    cs.extend(40u32.to_le_bytes());
    cs.extend([0u8; 12]);
    let groups = [
        vec![("VS_a_Wrapped", vs)],
        vec![("PS_a_Wrapped", ps.clone()), ("PS_b_Wrapped", ps)],
        vec![],
        vec![],
        vec![],
        vec![("CS_a_Wrapped", cs)],
        vec![],
        vec![],
    ];
    let effects = [
        ("default", [vec![0], vec![0, 1], vec![], vec![], vec![], vec![0]]),
        ("other", [vec![0], vec![1], vec![], vec![], vec![], vec![]]),
    ];
    let bytes = database(groups, &effects);
    let db = sgd2::parse(&bytes).unwrap();
    let mut w = World::new();
    let table = add_shader_database(&mut w, &db);
    assert!(table.unread.is_empty(), "{:?}", table.unread);
    assert_eq!(table.shaders, 3);
    assert_eq!(table.effects.len(), 2);

    let default = w.get::<GtaEffect>(table.effects[0]).unwrap().clone();
    assert_eq!(default.name, "default");
    let stages: Vec<_> = default
        .programs
        .iter()
        .map(|p| (p.stage, p.name.as_str()))
        .collect();
    assert_eq!(
        stages,
        [
            (GtaStage::Vertex, "VS_a_Wrapped"),
            (GtaStage::Pixel, "PS_a_Wrapped"),
            (GtaStage::Pixel, "PS_b_Wrapped"),
            (GtaStage::Compute, "CS_a_Wrapped"),
        ]
    );
    // the compute program is named without an entity; the others have theirs, with the container as stored
    assert_eq!(default.programs[3].shader, None);
    for p in &default.programs[..3] {
        let e = p.shader.unwrap();
        let shader = w.get::<Shader>(e).unwrap();
        assert_eq!(shader.format, ShaderFormat::Dxil);
        assert_eq!(w.get::<Name>(e).unwrap().0, p.name);
        assert!(shader.bytecode.starts_with(b"DXBC"));
        assert_eq!(
            shader.interface.stage,
            if p.stage == GtaStage::Vertex {
                Stage::Vertex
            } else {
                Stage::Pixel
            }
        );
    }
    // a program shared by two effects is one entity
    let other = w.get::<GtaEffect>(table.effects[1]).unwrap();
    assert_eq!(other.programs[0].shader, default.programs[0].shader);
    assert_eq!(other.programs[1].shader, default.programs[2].shader);
    let entities: std::collections::BTreeSet<Entity> = default
        .programs
        .iter()
        .chain(&other.programs)
        .filter_map(|p| p.shader)
        .collect();
    assert_eq!(entities.len(), 3);
}
