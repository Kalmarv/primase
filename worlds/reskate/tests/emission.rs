//! Emission into Studio's: `raise-skate` with `emission` raises a material's emissive factor, texture and strength to
//! a `skate.material`'s emission, the map writer writes them as Studio's `emission_*` keys, and without the option the
//! emission is left out, counted. A synthetic scene, made here; the numbers are worked from the stated formula
//! (strength = emissive strength x nits per strength x pi x the lights' calibration).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    Collision, ColorSpace, Instance, Mesh, Name, Primitive, Semantic, Stream, Surfaces, TexFormat, TexRole,
    Texture, Topology, Transform, World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::SkateMaterial;
use primase_reskate::passes::{
    LightCalibration, RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, SnapReskateMarkers, Unclassified,
};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-emission-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// A quad drawn with a lamp material (an sRGB emissive texture, factor (1, 0.5, 0.25), strength 20) and one with a
/// plain material, placed once over ground collision, with a spawn on it.
fn world() -> World {
    let mut w = World::new();
    let texture = w.spawn();
    w.insert(texture, Name("lamp_emissive".into()));
    w.insert(
        texture,
        Texture {
            width: 4,
            height: 4,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![200; 64]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let lamp = w.spawn();
    w.insert(lamp, Name("lamp".into()));
    w.insert(
        lamp,
        PbrMaterial {
            emissive: [1.0, 0.5, 0.25],
            emissive_strength: 20.0,
            emissive_texture: Some(TextureRef::new(texture)),
            ..PbrMaterial::default()
        },
    );
    let plain = w.spawn();
    w.insert(plain, Name("plain".into()));
    w.insert(plain, PbrMaterial::default());
    let prim = |m, x: f32| Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![
                    [x, 0.0, 0.0],
                    [x + 1.0, 0.0, 0.0],
                    [x, 1.0, 0.0],
                    [x + 1.0, 1.0, 0.0],
                ]),
            ),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
            ),
        ]),
        indices: Some(vec![0, 1, 2, 2, 1, 3]),
        material: Some(m),
    };
    let proto = w.spawn();
    w.insert(proto, Name("sign".into()));
    w.insert(proto, Mesh::new(vec![prim(lamp, 0.0), prim(plain, 2.0)]));
    let at = w.spawn();
    w.insert(
        at,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [0.0, 0.0, -5.0];
    w.insert(at, t);
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [-50.0, 0.0, 50.0],
            [50.0, 0.0, 50.0],
            [-50.0, 0.0, -50.0],
            [50.0, 0.0, -50.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    let spawn = w.spawn();
    w.insert(
        spawn,
        primase_core::Marker {
            kind: "spawn".into(),
            ground: Some([0.0, 0.0]),
            yaw: None,
        },
    );
    w
}

fn options() -> RaiseOptions {
    RaiseOptions::new("emits", LightCalibration::Standard, Unclassified::LeaveOut).unwrap()
}

/// With `emission=558.5` the lamp's `skate.material` emits its texture, its factor and 20 x 558.5 x pi x the
/// standard calibration; the plain material emits nothing; `map.json` has the lamp material's four `emission_*` keys
/// (its texture file, sRGB, the colour and the strength) and none on the plain one.
#[test]
fn emission_is_raised_and_written_as_studios() {
    let mut w = world();
    let mut log = PassLog::default();
    RaiseSkate::new(options().with_emission(558.5).unwrap())
        .run(&mut w, &mut log)
        .unwrap();
    let expected = (20.0 * 558.5 * std::f64::consts::PI * SKATE_LIGHT_SCALE) as f32;
    let emitting: Vec<&SkateMaterial> = w
        .iter::<SkateMaterial>()
        .filter_map(|(_, m)| m.emission.as_ref().map(|_| m))
        .collect();
    assert_eq!(emitting.len(), 1);
    let em = emitting[0].emission.as_ref().unwrap();
    assert_eq!(em.color, [1.0, 0.5, 0.25]);
    assert!(
        (em.strength - expected).abs() <= 1e-6 * expected,
        "{} vs {expected}",
        em.strength
    );
    assert!(em.texture.is_some());
    assert!(
        log.approximations
            .keys()
            .any(|k| k.starts_with("emission as Studio's: strength = emissive strength x 558.5 cd/m2")),
        "{:?}",
        log.approximations
    );
    SnapReskateMarkers::for_default_encoding()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let out_root = tmp("out");
    primase_reskate::write_map(
        &w,
        &tmp("scene"),
        &EncodingOptions {
            out_root: out_root.clone(),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out_root.join("emits/map.json")).unwrap()).unwrap();
    let materials = map["materials"].as_object().unwrap();
    let with: Vec<&serde_json::Value> = materials
        .values()
        .filter(|m| m.get("emission_strength").is_some())
        .collect();
    assert_eq!(with.len(), 1, "{materials:?}");
    let m = with[0];
    let file = m["emission_texture"].as_str().unwrap();
    assert!(file.starts_with("textures/"), "{file}");
    assert!(out_root.join("emits").join(file).exists(), "{file} not written");
    assert_eq!(m["emission_srgb"], serde_json::json!(true));
    assert_eq!(m["emission_color"], serde_json::json!([1.0, 0.5, 0.25]));
    let s = m["emission_strength"].as_f64().unwrap() as f32;
    assert!((s - expected).abs() <= 1e-6 * expected, "{s} vs {expected}");
    let plain = materials
        .values()
        .filter(|m| m.get("emission_strength").is_none())
        .count();
    assert!(plain >= 1, "the plain material emits nothing");
    assert!(
        materials
            .values()
            .all(|m| m.get("emission_texture").is_none() || m.get("emission_strength").is_some()),
        "an emission texture without its strength"
    );
}

/// Without `emission` nothing is raised: the lamp's emission is left out, counted; an emission texture on other
/// texture coordinates than the base texture's is left out too, by its reason; the option round-trips.
#[test]
fn emission_is_left_out_without_its_calibration_or_on_other_coordinates() {
    let mut w = world();
    let mut log = PassLog::default();
    RaiseSkate::new(options()).run(&mut w, &mut log).unwrap();
    assert!(w.iter::<SkateMaterial>().all(|(_, m)| m.emission.is_none()));
    assert_eq!(
        log.omitted["emission left out (raise-skate's emission not stated)"],
        1
    );
    let mut w = world();
    let lamp = w
        .iter::<PbrMaterial>()
        .map(|(e, _)| e)
        .find(|&e| w.get::<Name>(e).is_some_and(|n| n.0 == "lamp"))
        .unwrap();
    w.get_mut::<PbrMaterial>(lamp)
        .unwrap()
        .emissive_texture
        .as_mut()
        .unwrap()
        .texcoord = 1;
    let mut log = PassLog::default();
    RaiseSkate::new(options().with_emission(558.5).unwrap())
        .run(&mut w, &mut log)
        .unwrap();
    assert!(w.iter::<SkateMaterial>().all(|(_, m)| m.emission.is_none()));
    assert_eq!(
        log.omitted["emission left out: its texture on other texture coordinates than the base texture's"],
        1
    );
    let o = options().with_emission(558.5).unwrap().options();
    assert_eq!(o["emission"], "558.5");
    assert!(options().with_emission(0.0).is_err());
    assert!(options().with_emission(f64::NAN).is_err());
}
