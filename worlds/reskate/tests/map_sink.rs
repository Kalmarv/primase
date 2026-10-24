//! The map writer as a sink of parts (`MapSink`): a part's payloads (its meshes' and textures' files) are on disk once
//! the part is taken, the index files (`map.json`, `stats.json`) only once the sink finishes; `write_map` is the sink
//! given the whole map and writes the same folder. A second part of a map taken as whole, and a finish with no part, are
//! refused. As a conversion's writer, the sink reports what it left out and what it wrote approximately as notices.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::convert::{NoticeKind, Writer};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Instance, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology,
    Transform, World,
};
use primase_reskate::components::{
    MarkerKind, Other, SkateMarker, SkateScene, Snap, SnapMode, TextureBudget,
};
use primase_reskate::passes::{LightCalibration, RaiseOptions};
use primase_reskate::{EncodingOptions, MapSink};

/// A raised map: a textured slab placed twice, and a spawn.
fn map() -> World {
    map_of(vec![(
        "stone".to_owned(),
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![200; 16]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    )])
}

/// A raised map: a slab of each texture (named after it), placed twice, and a spawn.
fn map_of(slabs: Vec<(String, Texture)>) -> World {
    let mut w = World::new();
    for (k, (name, texture)) in slabs.into_iter().enumerate() {
        let tex = w.spawn();
        w.insert(tex, Name(format!("{name}_d")));
        w.insert(tex, texture);
        let material = w.spawn();
        w.insert(material, Name(name.clone()));
        w.insert(
            material,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(tex)),
                ..PbrMaterial::default()
            },
        );
        let proto = w.spawn();
        w.insert(
            proto,
            Name(if k == 0 {
                "slab".to_owned()
            } else {
                format!("slab{k}")
            }),
        );
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, -4.0]]),
                    ),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
                    ),
                ]),
                indices: None,
                material: Some(material),
            }]),
        );
        for x in [0.0, 10.0] {
            let p = w.spawn();
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3] = [x, 20.0 * k as f32, 0.0];
            w.insert(p, t);
        }
    }
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "sink",
            LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
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
    w
}

fn encoding(tag: &str) -> EncodingOptions {
    EncodingOptions {
        out_root: std::env::temp_dir()
            .join(format!("primase-reskate-sink-{}", std::process::id()))
            .join(tag),
        name: Some("sink".into()),
        ..EncodingOptions::default()
    }
}

/// Every file under `dir`, by its path relative to it, with its bytes.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(
                    p.strip_prefix(dir).unwrap().to_owned(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    out
}

#[test]
fn a_part_writes_its_payloads_and_finish_the_index() {
    let w = map();
    let e = encoding("sink");
    let mut sink = MapSink::new(Path::new(""), &e);
    sink.part(&w).unwrap();
    let out = e.out_root.join("sink");
    let after_part = files(&out);
    assert!(
        after_part.keys().any(|p| p.starts_with("meshes")),
        "{:?}",
        after_part.keys()
    );
    assert!(
        after_part.keys().any(|p| p.starts_with("textures")),
        "{:?}",
        after_part.keys()
    );
    assert!(!out.join("map.json").exists() && !out.join("stats.json").exists());
    // a second part of a map without its layout summary is refused: the first was the whole map
    let err = sink.part(&w).unwrap_err().to_string();
    assert!(err.contains("a second part of a map taken as whole"), "{err}");
    let r = sink.finish().unwrap();
    assert_eq!(r.out_dir, out);
    assert!(out.join("map.json").exists() && out.join("stats.json").exists());
    let by_sink = files(&out);
    // write_map is the sink given the one part: the same folder, byte for byte
    let e2 = encoding("write_map");
    let r2 = primase_reskate::write_map(&w, Path::new(""), &e2).unwrap();
    assert_eq!(files(&r2.out_dir), by_sink);
    for e in [e, e2] {
        std::fs::remove_dir_all(e.out_root).ok();
    }
}

#[test]
fn a_finish_with_no_part_is_refused() {
    let e = encoding("empty");
    let err = MapSink::new(Path::new(""), &e).finish().unwrap_err().to_string();
    assert!(err.contains("given no part"), "{err}");
}

/// The sink as a conversion's writer: the parts through `write`, and at `finish` the map's report lines, what it left
/// out as unsupported notices and what it wrote approximately as approximation notices, each with its count; the
/// folder is the one `write_map` writes.
#[test]
fn as_a_conversions_writer_the_sink_reports_its_losses_as_notices() {
    let mut w = map();
    // left out: the scene's texture budget (a pass applies it); approximated: a marker whose snap is unresolved
    let scene = w.resource_mut::<SkateScene>().unwrap();
    scene.texture_budget = Some(TextureBudget {
        max: Some(512.0),
        ..TextureBudget::default()
    });
    let m = w.spawn();
    w.insert(
        m,
        SkateMarker {
            kind: MarkerKind::Travel,
            position: Some(([5.0, -1.0], Some(0.0))),
            yaw: None,
            m: None,
            blueprint: None,
            snap: Some(Snap::Mode(SnapMode::Ground)),
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    let e = encoding("writer");
    let mut writer: Box<dyn Writer> = Box::new(MapSink::new(Path::new(""), &e));
    writer.write(&w).unwrap();
    let finished = writer.finish().unwrap();
    let line = |label: &str| {
        finished
            .report
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("no {label} line: {:?}", finished.report))
    };
    assert!(line("reskate").contains("tiles="), "{}", line("reskate"));
    assert!(
        line("reskate parts").starts_with("parts=1 "),
        "{}",
        line("reskate parts")
    );
    let notices: Vec<(NoticeKind, &str, u64)> = finished
        .notices
        .iter()
        .map(|n| (n.kind, n.reason.as_str(), n.count))
        .collect();
    assert_eq!(
        notices,
        [
            (
                NoticeKind::Unsupported,
                "skate.scene texture_budget (budget-reskate-textures applies a budget): not written",
                1
            ),
            (
                NoticeKind::Approximation,
                "markers whose snap is unresolved, written where they stand (snap-reskate-markers resolves it)",
                1
            ),
        ]
    );
    assert!(finished.notices.iter().all(|n| n.stage == "reskate"));
    // the same folder as write_map's
    let e2 = encoding("writer_map");
    let r = primase_reskate::write_map(&w, Path::new(""), &e2).unwrap();
    assert_eq!(files(&r.out_dir), files(&e.out_root.join("sink")));
    for e in [e, e2] {
        std::fs::remove_dir_all(e.out_root).ok();
    }
}

/// The texture files encoded on several threads ([`MapSink::with_threads`]): the folder `write_map` writes on one,
/// byte for byte, for a map of nine textures (sizes not powers of two, some with alpha).
#[test]
fn the_files_on_any_thread_count_are_the_serial_ones() {
    let noise = |n: usize, seed: u32| -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 24) as u8
            })
            .collect()
    };
    let slabs = || {
        (0..9u32)
            .map(|k| {
                let (w, h) = (16 + 5 * k, 12 + 3 * k);
                let mut px = noise((w * h * 4) as usize, 1 + k);
                if k % 3 != 0 {
                    px.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
                }
                (
                    format!("stone{k}"),
                    Texture {
                        width: w,
                        height: h,
                        format: TexFormat::Rgba8Unorm,
                        levels: vec![px],
                        color_space: ColorSpace::Srgb,
                        role: TexRole::Color,
                        normal: None,
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let e = encoding("threads_one");
    let r = primase_reskate::write_map(&map_of(slabs()), Path::new(""), &e).unwrap();
    let serial = files(&r.out_dir);
    assert!(
        serial
            .keys()
            .filter(|p| p.extension().is_some_and(|x| x == "png"))
            .count()
            >= 9
    );
    for n in [2, 8] {
        let e2 = encoding(&format!("threads_{n}"));
        let mut sink =
            MapSink::new(Path::new(""), &e2).with_threads(primase_core::parallel::Threads::new(n).unwrap());
        sink.part(&map_of(slabs())).unwrap();
        let r2 = sink.finish().unwrap();
        assert_eq!(files(&r2.out_dir), serial, "{n} threads");
        std::fs::remove_dir_all(e2.out_root).ok();
    }
    std::fs::remove_dir_all(e.out_root).ok();
}
