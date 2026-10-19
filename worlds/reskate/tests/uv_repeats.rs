//! `prepare-reskate-uv-repeats`: a repeating decal's triangles over more texture repeats than Studio draws are left out
//! (`decals=omit`) or kept (`keep`); a repeating blended surface with such a triangle is drawn alpha tested through a
//! cutout twin of its material (`blended=cutout`) or kept blended (`keep`); triangles within the limit change nothing.
//! The writer writes what the pass prepared: the twin as an alpha-masked material, the kept blend as blended.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::ledger::Measure;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture,
    Topology, Transform, World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{AlphaMode, MarkerKind, MaterialClass, Other, SkateMarker, SkateMaterial};
use primase_reskate::passes::{PrepareReskateUvRepeats, RaiseOptions, RepeatBlended, RepeatDecals};

/// A raised world: one prototype placed once, two triangles (the first's texture coordinates spanning 20 x 20 repeats
/// when `over`, else within one), drawn with a textured material of `class` (alpha blended when `blend`), and a spawn.
/// The prototype's entity.
fn world(class: MaterialClass, blend: bool, over: bool) -> (World, Entity) {
    let mut w = World::new();
    let tex = w.spawn();
    w.insert(tex, Name("stone_d".into()));
    w.insert(
        tex,
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![200; 16]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(
        material,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(tex)),
            alpha_mode: if blend { PbrAlpha::Blend } else { PbrAlpha::Opaque },
            ..PbrMaterial::default()
        },
    );
    let far = if over { 20.0 } else { 0.5 };
    let proto = w.spawn();
    w.insert(proto, Name("slab".into()));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![
                        [0.0, 0.0, 0.0],
                        [4.0, 0.0, 0.0],
                        [0.0, 0.0, -4.0],
                        [5.0, 0.0, 0.0],
                        [6.0, 0.0, 0.0],
                        [5.0, 0.0, -1.0],
                    ]),
                ),
                (
                    Semantic::TexCoord(0),
                    Stream::F32x2(vec![
                        [0.0, 0.0],
                        [far, 0.0],
                        [0.0, far],
                        [0.1, 0.1],
                        [0.2, 0.1],
                        [0.1, 0.2],
                    ]),
                ),
            ]),
            indices: None,
            material: Some(material),
        }]),
    );
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, Transform::IDENTITY);
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("repeats", primase_reskate::passes::LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    w.get_mut::<SkateMaterial>(material).unwrap().class = class;
    let spawn = w.spawn();
    w.insert(
        spawn,
        SkateMarker {
            kind: MarkerKind::Spawn,
            position: Some(([1.0, 1.0], Some(0.0))),
            yaw: None,
            m: None,
            blueprint: None,
            snap: None,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    (w, proto)
}

fn run(w: &mut World, decals: RepeatDecals, blended: RepeatBlended) -> PassLog {
    let mut log = PassLog::default();
    PrepareReskateUvRepeats::new(decals, blended)
        .run(w, &mut log)
        .unwrap();
    log
}

fn triangles(w: &World, proto: Entity) -> usize {
    let p = &w.get::<Mesh>(proto).unwrap().primitives()[0];
    p.indices.as_ref().map_or(p.vertex_count(), Vec::len) / 3
}

fn material(w: &World, proto: Entity) -> SkateMaterial {
    let m = w.get::<Mesh>(proto).unwrap().primitives()[0].material.unwrap();
    w.get::<SkateMaterial>(m).unwrap().clone()
}

/// The map's material alpha modes ("mask", "blend", ...), written from `w`.
fn written_alphas(w: &World, tag: &str) -> Vec<String> {
    let out_root = std::env::temp_dir()
        .join(format!("primase-reskate-uv-repeats-{}", std::process::id()))
        .join(tag);
    let r = primase_reskate::write_map(
        w,
        Path::new(""),
        &EncodingOptions {
            out_root,
            name: Some("repeats".into()),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(r.out_dir.join("map.json")).unwrap()).unwrap();
    map["materials"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|m| m["alpha"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn a_repeating_decal_over_the_limit_is_omitted_or_kept() {
    let (mut w, proto) = world(MaterialClass::Decal, false, true);
    let log = run(&mut w, RepeatDecals::Omit, RepeatBlended::Keep);
    assert_eq!(triangles(&w, proto), 1);
    assert_eq!(log.declared(Measure::Triangles), 1);
    let (mut w, proto) = world(MaterialClass::Decal, false, true);
    let log = run(&mut w, RepeatDecals::Keep, RepeatBlended::Keep);
    assert_eq!(triangles(&w, proto), 2);
    assert_eq!(log.declared(Measure::Triangles), 0);
}

#[test]
fn a_repeating_blend_over_the_limit_is_drawn_alpha_tested_or_kept() {
    let (mut w, proto) = world(MaterialClass::Alpha, true, true);
    let original = material(&w, proto);
    assert_eq!(original.alpha, Some(AlphaMode::Blend), "{original:?}");
    let log = run(&mut w, RepeatDecals::Omit, RepeatBlended::Cutout);
    let twin = material(&w, proto);
    assert_eq!(
        (twin.class, twin.alpha),
        (MaterialClass::Cutout, Some(AlphaMode::Mask))
    );
    assert_ne!(twin.key, original.key);
    assert_eq!(triangles(&w, proto), 2, "the blend keeps its triangles");
    assert_eq!(log.approximations.values().sum::<u64>(), 1);
    assert_eq!(log.counts["cutout materials made"], 1);
    // the writer writes the twin alpha tested
    assert!(written_alphas(&w, "cutout").contains(&"mask".to_owned()));
    // kept: the blend stays, written blended
    let (mut w, proto) = world(MaterialClass::Alpha, true, true);
    let log = run(&mut w, RepeatDecals::Omit, RepeatBlended::Keep);
    assert_eq!(material(&w, proto), original);
    assert!(log.approximations.is_empty());
    let alphas = written_alphas(&w, "kept");
    assert!(
        alphas.contains(&"blend".to_owned()) && !alphas.contains(&"mask".to_owned()),
        "{alphas:?}"
    );
}

#[test]
fn triangles_within_the_limit_change_nothing() {
    for (class, blend) in [(MaterialClass::Decal, false), (MaterialClass::Alpha, true)] {
        let (mut w, proto) = world(class, blend, false);
        let before = (triangles(&w, proto), material(&w, proto));
        let log = run(&mut w, RepeatDecals::Omit, RepeatBlended::Cutout);
        assert_eq!((triangles(&w, proto), material(&w, proto)), before);
        assert!(log.drops.is_empty() && log.approximations.is_empty());
    }
}
