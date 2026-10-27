#![allow(clippy::unwrap_used)]
//! The executor's ray tracing pipeline against Microsoft's WARP (DXR 1.0 ray tracing pipelines): the six programs of
//! `ray_tracing.hlsl` form a state object (a triangle hit group with any-hit and closest-hit programs, a procedural one
//! with intersection, any-hit and closest-hit programs), the raygen program traces one ray per image pixel through the
//! same scene on both, and the images are compared texel by texel (to a relative 1e-4: the arithmetic of the programs is
//! the same, the traversal and its rounding are each implementation's own).
//!
//! Needs the WARP oracle program with the `d3d12rtp` case form (the configured `Tool::Warp`) and the scratch folder.
use primase_dxil::lift;
use iridia::{HitGroup, HitGroupKind};
use iridia_exec::{
    Geometry, Instance, InstanceFlags, Pipeline, PipelineResources, Scene, ShaderTable,
};
use iridia_exec::{Sample, Sampler, Store};
use primase_testdata::{Tool, own_scratch, tool};
use std::collections::BTreeMap;
use std::path::PathBuf;

const PROGRAMS: [&str; 6] = [
    "raygen",
    "miss",
    "closest_hit",
    "any_hit",
    "intersection",
    "callable",
];

fn fixture(export: &str, variant: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("ray_tracing.{export}{variant}.dxil"))
}

fn pipeline(variant: &str) -> Pipeline {
    let programs = PROGRAMS
        .map(|n| lift(&std::fs::read(fixture(n, variant)).unwrap()).unwrap())
        .to_vec();
    let group = |name: &str, kind, intersection: Option<&str>| HitGroup {
        name: name.to_owned(),
        kind,
        any_hit: Some("any_hit".to_owned()),
        closest_hit: Some("closest_hit".to_owned()),
        intersection: intersection.map(str::to_owned),
    };
    Pipeline::new(
        programs,
        ShaderTable {
            miss: vec!["miss".to_owned()],
            hit_groups: vec!["triangles".to_owned(), "procedural".to_owned()],
            callable: vec!["callable".to_owned()],
        },
    )
    .unwrap()
    // the fixtures' libraries state no pipeline config; the oracle's state object declares 2
    .with_max_recursion(2)
    .with_hit_groups([
        group("triangles", HitGroupKind::Triangles, None),
        group(
            "procedural",
            HitGroupKind::ProceduralPrimitives,
            Some("intersection"),
        ),
    ])
}

#[derive(Default)]
struct Image {
    texels: BTreeMap<(u32, u32), [f32; 4]>,
}
impl Sampler for Image {
    fn sample(&mut self, _: Sample) -> Result<[f32; 4], String> {
        Err("no sampling".into())
    }
    fn store(&mut self, s: Store) -> Result<(), String> {
        self.texels
            .insert((s.address[0], s.address[1]), s.value.map(f32::from_bits));
        Ok(())
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn float(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.next() as f32 / (1u64 << 31) as f32)
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

fn bits(x: f32) -> String {
    format!("{:08x}", x.to_bits())
}

const IDENTITY: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

fn triangles(r: &mut Lcg, n: u32, opaque: bool) -> Geometry {
    let mut positions = Vec::new();
    for _ in 0..n {
        let c = [r.float(-0.2, 1.2), r.float(-0.2, 1.2), r.float(1.0, 6.0)];
        for _ in 0..3 {
            positions.push([
                c[0] + r.float(-0.6, 0.6),
                c[1] + r.float(-0.6, 0.6),
                c[2] + r.float(-0.3, 0.3),
            ]);
        }
    }
    Geometry {
        positions,
        boxes: Vec::new(),
        indices: None,
        opaque,
    }
}

/// A scene from a seed: an opaque triangle geometry, a non-opaque one and a procedural one, each its own bottom level
/// (a hit group is of one kind), with several instances (a hit group index is the instance's contribution plus the
/// geometry's index, the geometries being the only one of their bottom level): contribution 0 for the triangle levels,
/// 1 for the procedural.
fn scene(seed: u64) -> Scene {
    let mut r = Lcg(seed);
    let bottoms = vec![
        vec![triangles(&mut r, 6, true)],
        vec![triangles(&mut r, 6, false)],
        vec![Geometry {
            positions: Vec::new(),
            boxes: (0..3)
                .map(|_| {
                    let c = [r.float(0.0, 1.0), r.float(0.0, 1.0), r.float(2.0, 5.0)];
                    [
                        [c[0] - 0.5, c[1] - 0.5, c[2] - 0.5],
                        [c[0] + 0.5, c[1] + 0.5, c[2] + 0.5],
                    ]
                })
                .collect(),
            indices: None,
            opaque: r.below(2) == 0,
        }],
    ];
    let mut instances = Vec::new();
    for k in 0..4u32 {
        let bottom = r.below(3) as usize;
        let angle = r.float(0.0, std::f32::consts::TAU);
        let scale = r.float(0.7, 1.4);
        let (s, c) = angle.sin_cos();
        instances.push(Instance {
            bottom,
            transform: [
                [c * scale, -s * scale, 0.0, r.float(-0.5, 0.5)],
                [s * scale, c * scale, 0.0, r.float(-0.5, 0.5)],
                [0.0, 0.0, scale, r.float(-0.5, 0.5)],
            ],
            id: 100 + 3 * k,
            mask: 0xff,
            contribution: u32::from(bottom == 2),
            flags: InstanceFlags::default(),
        });
    }
    Scene { bottoms, instances }
}

fn case_text(name: &str, variant: &str, dims: [u32; 3], scene: &Scene) -> String {
    let mut text = format!("case {name}\napi d3d12rtp\n");
    for n in PROGRAMS {
        text.push_str(&format!("lib {}\n", fixture(n, variant).display()));
    }
    text.push_str(
        "raygen raygen\nmiss miss\ncallable callable\n\
         hitgroup triangles triangles closest_hit any_hit -\n\
         hitgroup procedural procedural closest_hit any_hit intersection\n\
         payload 20\nattributes 8\nrecursion 2\n",
    );
    text.push_str(&format!(
        "dims {} {} {}\nimage {} {}\n",
        dims[0], dims[1], dims[2], dims[0], dims[1]
    ));
    for (b, geometries) in scene.bottoms.iter().enumerate() {
        for g in geometries {
            if !g.boxes.is_empty() {
                text.push_str(&format!(
                    "procedural {b} {} {}\n",
                    u8::from(g.opaque),
                    g.boxes.len()
                ));
                for corners in &g.boxes {
                    text.push_str(&format!(
                        "box {}\n",
                        corners
                            .iter()
                            .flatten()
                            .map(|x| bits(*x))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
                continue;
            }
            text.push_str(&format!(
                "geometry {b} {} {}\n",
                u8::from(g.opaque),
                g.positions.len() / 3
            ));
            for tri in g.positions.chunks(3) {
                text.push_str(&format!(
                    "tri {}\n",
                    tri.iter()
                        .flatten()
                        .map(|x| bits(*x))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
        }
    }
    for i in &scene.instances {
        text.push_str(&format!(
            "instance {} {} {} {} 0 {}\n",
            i.bottom,
            i.id,
            i.mask,
            i.contribution,
            i.transform
                .iter()
                .flatten()
                .map(|x| bits(*x))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    text.push_str("end\n");
    text
}

/// The scenes: three of a random seed, and the hand-worked ones of `pipeline.rs` (an opaque triangle seen from behind,
/// and a procedural hit at the end of the range).
fn scenes() -> Vec<(String, [u32; 3], Scene)> {
    let mut all: Vec<(String, [u32; 3], Scene)> = (1..=8u64)
        .map(|seed| (format!("random{seed}"), [8, 8, 1], scene(seed)))
        .collect();
    all.push((
        "opaque_triangle".to_owned(),
        [2, 2, 1],
        Scene {
            bottoms: vec![vec![Geometry {
                positions: vec![[0.0, 0.0, 5.0], [1.25, 0.0, 5.0], [0.0, 1.25, 5.0]],
                boxes: Vec::new(),
                indices: None,
                opaque: true,
            }]],
            instances: vec![Instance {
                bottom: 0,
                transform: IDENTITY,
                id: 7,
                mask: 0xff,
                contribution: 0,
                flags: InstanceFlags::default(),
            }],
        },
    ));
    all.push((
        "procedural_at_the_end_of_the_range".to_owned(),
        [1, 1, 1],
        Scene {
            bottoms: vec![vec![Geometry {
                positions: Vec::new(),
                boxes: vec![[[-1.0, -1.0, 2.0], [1.0, 1.0, 3.0]]],
                indices: None,
                opaque: false,
            }]],
            instances: vec![Instance {
                bottom: 0,
                transform: [[1.0, 0.0, 0.0, 0.5], [0.0, 1.0, 0.0, 0.5], [0.0, 0.0, 1.0, 0.0]],
                id: 7,
                mask: 0xff,
                contribution: 1,
                flags: InstanceFlags::default(),
            }],
        },
    ));
    all
}

#[test]
fn pipelines_agree_with_warp() {
    let Some(warp) = tool(Tool::Warp) else {
        return;
    };
    let scratch = own_scratch("warp");
    let (mut compared, mut hits, mut mismatches) = (0, 0, Vec::new());
    for variant in ["", ".od"] {
        let pipeline = pipeline(variant);
        let mut text = String::new();
        let mut ours = BTreeMap::new();
        for (name, dims, scene) in scenes() {
            let name = format!("{name}{}", variant.replace('.', "_"));
            text.push_str(&case_text(&name, variant, dims, &scene));
            let resources = PipelineResources {
                constants: BTreeMap::new(),
                scene,
            };
            let mut image = Image::default();
            pipeline
                .dispatch_rays("raygen", dims, &resources, &mut image)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            ours.insert(name, (dims, image));
        }
        let cases = scratch.join(format!("pipeline{}.cases.txt", variant.replace('.', "_")));
        let out = scratch.join(format!("pipeline{}.warp.jsonl", variant.replace('.', "_")));
        std::fs::write(&cases, text).unwrap();
        let _oracle = primase_testdata::warp_exclusive();
        let run = std::process::Command::new(&warp)
            .arg(&cases)
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "warp: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        for line in std::fs::read_to_string(&out).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let name = v["case"].as_str().unwrap().to_owned();
            if let Some(e) = v["error"].as_str() {
                mismatches.push(format!("{name}: WARP error {e}"));
                continue;
            }
            let theirs: Vec<f32> = v["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| f32::from_bits(u32::from_str_radix(w.as_str().unwrap(), 16).unwrap()))
                .collect();
            let (dims, image) = &ours[&name];
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let mine = image.texels[&(x, y)];
                    let their = &theirs[((y * dims[0] + x) * 4) as usize..][..4];
                    compared += 1;
                    if mine[3] >= 0.0 {
                        hits += 1;
                    }
                    let same = (0..4).all(|k| {
                        (mine[k] - their[k]).abs() <= 1e-4 * mine[k].abs().max(their[k].abs()).max(1.0)
                    });
                    if !same {
                        mismatches.push(format!(
                            "{name} pixel ({x}, {y}): executor {mine:?}, WARP {their:?}"
                        ));
                    }
                }
            }
        }
    }
    println!(
        "{compared} pixels ({hits} hit) compared with WARP, {} differ",
        mismatches.len()
    );
    for m in &mismatches {
        println!("MISMATCH {m}");
    }
    assert!(hits > 0 && mismatches.is_empty());
}
