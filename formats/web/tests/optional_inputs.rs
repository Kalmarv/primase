//! A part may hold nothing a preparation pass works on: an empty part, and one with only a light, go through the glTF
//! and web preparation passes as the pass manager runs them; each finds nothing to do and says so in its counts.
#![cfg(feature = "ktx2")]
#![allow(clippy::unwrap_used)]

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Light, LightKind, Transform, World};

const PIPELINE: &str = "fill-missing-texcoords, prepare-gltf-materials{base-color=0.8/0.8/0.8/1,metallic=0,roughness=0.85}, \
    prepare-web-textures{color=512,normal=512,mask=256,color-bytes=360448,mask-bytes=98304}";

fn run(w: &mut World) -> RunReport {
    let mut registry = PassRegistry::new();
    primase_gltf::register(&mut registry);
    primase_web::prepare::register(&mut registry);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager
        .run(w, &manager.registry.parse(PIPELINE).unwrap(), &mut report)
        .unwrap();
    report
}

#[test]
fn parts_without_meshes_materials_or_textures_are_prepared() {
    let mut light = World::new();
    let e = light.spawn();
    light.insert(
        e,
        Light {
            kind: LightKind::Point,
            color: [1.; 3],
            intensity: 1.,
            range: None,
            cast_shadows: None,
            radius: None,
        },
    );
    light.insert(e, Transform::IDENTITY);
    for mut w in [World::new(), light] {
        let report = run(&mut w);
        let count = |pass: &str, what: &str| {
            report
                .steps
                .iter()
                .find(|s| s.pass.starts_with(pass))
                .unwrap()
                .log
                .counts[what]
        };
        assert_eq!(count("fill-missing-texcoords", "meshes looked at"), 0);
        assert_eq!(
            count(
                "prepare-gltf-materials",
                "materials given the program's PBR factors"
            ),
            0
        );
        assert_eq!(count("prepare-web-textures", "textures prepared"), 0);
    }
}
