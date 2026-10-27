#![allow(clippy::unwrap_used)]
//! The executor's ray queries against Microsoft's WARP rasterizer (Direct3D 12, DXR 1.1 inline ray tracing), the
//! oracle that is not ours: the same scene (triangles, instances with transforms, masks and flags) and the same rays
//! go through `ray_query_grid.hlsl` on WARP and on the executor, and every committed hit is compared: the integer
//! fields (status, instance index and id, hit-group contribution, geometry, primitive, facing) exactly, the distance
//! and the barycentrics to a relative 1e-4 (the arithmetic of a hardware or software traversal is its own).
//!
//! Needs the WARP oracle program with the `d3d12rt` case form (the configured `Tool::Warp`) and the scratch folder. The
//! scenes and rays are made from fixed seeds.
use iridia::D3dClass;
use iridia_exec::{
    Backend, Element, Executable, Geometry, ImageSample, Instance, InstanceFlags, Resources, Scene,
};
use primase_dxil::lift;
use primase_testdata::{Tool, own_scratch, tool};
use std::collections::BTreeMap;
use std::path::Path;

struct NoBackend;
impl Backend for NoBackend {
    fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
        Err("no sampling in this fixture".into())
    }
}

/// A small deterministic generator.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    /// A float in [lo, hi).
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

/// A scene from a seed: bottom levels of several geometries of random triangles (some opaque), and instances with a
/// rotation about z, a scale and a translation, masks, ids and flags.
/// Eight spheres (centre, radius) the procedural primitives stand for.
fn spheres() -> Vec<[f32; 4]> {
    let mut r = Lcg(31337);
    (0..8)
        .map(|_| {
            [
                r.float(-2.5, 2.5),
                r.float(-2.5, 2.5),
                r.float(-2.5, 2.5),
                r.float(0.5, 1.5),
            ]
        })
        .collect()
}

fn scene(seed: u64, with_instance_flags: bool, procedural: bool) -> Scene {
    let mut r = Lcg(seed);
    let mut bottoms = Vec::new();
    for _ in 0..2 {
        let mut geometries = Vec::new();
        for _ in 0..(1 + r.below(3)) {
            let n = 6 + r.below(10);
            let mut positions = Vec::new();
            let centre = [r.float(-2.0, 2.0), r.float(-2.0, 2.0), r.float(-2.0, 2.0)];
            for _ in 0..n * 3 {
                positions.push([
                    centre[0] + r.float(-2.5, 2.5),
                    centre[1] + r.float(-2.5, 2.5),
                    centre[2] + r.float(-2.5, 2.5),
                ]);
            }
            geometries.push(Geometry {
                positions,
                boxes: Vec::new(),
                indices: None,
                opaque: r.below(2) == 0,
            });
        }
        if procedural {
            // four boxes, each bounding sphere k with room to spare, so that some rays enter a box and miss the sphere
            let boxes = spheres()
                .iter()
                .take(4)
                .map(|s| {
                    let slack = 1.3 * s[3];
                    [
                        [s[0] - slack, s[1] - slack, s[2] - slack],
                        [s[0] + slack, s[1] + slack, s[2] + slack],
                    ]
                })
                .collect();
            geometries.push(Geometry {
                positions: Vec::new(),
                boxes,
                indices: None,
                opaque: r.below(2) == 0,
            });
        }
        bottoms.push(geometries);
    }
    let mut instances = Vec::new();
    for k in 0..3 {
        let angle = r.float(0.0, std::f32::consts::TAU);
        let scale = r.float(0.6, 1.6);
        let (s, c) = angle.sin_cos();
        let t = [r.float(-3.0, 3.0), r.float(-3.0, 3.0), r.float(-3.0, 3.0)];
        let transform = [
            [c * scale, -s * scale, 0.0, t[0]],
            [s * scale, c * scale, 0.0, t[1]],
            [0.0, 0.0, scale, t[2]],
        ];
        let flags = if with_instance_flags {
            let f = r.below(8);
            InstanceFlags {
                cull_disable: f & 1 != 0,
                front_counter_clockwise: f & 2 != 0,
                force_opaque: f & 4 != 0,
                force_non_opaque: f == 7,
            }
        } else {
            InstanceFlags::default()
        };
        instances.push(Instance {
            bottom: (r.below(2)) as usize,
            transform,
            id: 100 + 7 * k,
            mask: [0xff, 0x01, 0x02][r.below(3) as usize],
            contribution: 10 * k + r.below(5),
            flags,
        });
    }
    Scene { bottoms, instances }
}

/// 64 rays: origin, direction, flags, mask. With `outside`, every origin is far from the scene (further than any
/// instance of it reaches), so that no ray starts inside a procedural primitive's box: WARP offers no candidate for a box
/// the ray starts in (its entry before the ray's t min), which DXR does not say, so the gate leaves such rays out.
fn rays(seed: u64, outside: bool) -> Vec<([f32; 3], [f32; 3], u32, u32)> {
    let mut r = Lcg(seed);
    let flag_sets = [
        0u32,
        0x01,
        0x02,
        0x10,
        0x20,
        0x40,
        0x80,
        0x100,
        0x10 | 0x80,
        0x20 | 0x40,
        0x01 | 0x20,
    ];
    (0..64)
        .map(|_| {
            let mut origin = [r.float(-5.0, 5.0), r.float(-5.0, 5.0), r.float(-5.0, 5.0)];
            if outside {
                // push the origin out along itself to a distance of 16 or more
                let length = origin.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-3);
                origin = origin.map(|x| x / length * 16.0);
            }
            let target = [r.float(-3.0, 3.0), r.float(-3.0, 3.0), r.float(-3.0, 3.0)];
            let direction = [
                target[0] - origin[0],
                target[1] - origin[1],
                target[2] - origin[2],
            ];
            (
                origin,
                direction,
                flag_sets[r.below(flag_sets.len() as u32) as usize],
                [0xff, 0xff, 0x01, 0x02, 0x03][r.below(5) as usize],
            )
        })
        .collect()
}

#[test]
fn ray_queries_agree_with_warp() {
    gate("ray_query_grid", false);
}

#[test]
fn procedural_primitives_agree_with_warp() {
    gate("ray_query_procedural", true);
}

fn gate(fixture_name: &str, procedural: bool) {
    let Some(warp) = tool(Tool::Warp) else {
        return;
    };
    let scratch = own_scratch("warp");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{fixture_name}.dxil"));
    let bytes = std::fs::read(&fixture).unwrap();
    let program = lift(&bytes).unwrap();
    let exe = Executable::new(&program).unwrap();
    let find = |class: Option<D3dClass>| {
        program
            .interface
            .resources
            .iter()
            .find(|r| match class {
                Some(c) => r.provenance.d3d.is_some_and(|d| d.class == c && d.register == 0),
                None => matches!(r.kind, iridia::ResourceKind::AccelerationStructure),
            })
            .unwrap()
            .id
    };
    let (structure, results, constants) = (
        find(None),
        find(Some(D3dClass::UnorderedAccess)),
        find(Some(D3dClass::ConstantBuffer)),
    );

    let mut text = String::new();
    let mut ours = BTreeMap::new();
    let mut cases = Vec::new();
    for (seed, instance_flags) in (1u64..=12).map(|seed| (seed, seed > 3)) {
        let name = format!("{fixture_name}{seed}");
        let scene = scene(seed, instance_flags, procedural);
        let rays = rays(seed * 7919, procedural);
        let mut rows: Vec<[u32; 4]> = Vec::new();
        rows.extend(
            rays.iter()
                .map(|t| [t.0[0], t.0[1], t.0[2], 0.0].map(f32::to_bits)),
        );
        rows.extend(
            rays.iter()
                .map(|t| [t.1[0], t.1[1], t.1[2], 0.0].map(f32::to_bits)),
        );
        rows.extend(rays.iter().map(|t| [t.2, t.3, 0, 0]));
        if procedural {
            rows.extend(spheres().iter().map(|s| s.map(f32::to_bits)));
        }

        text.push_str(&format!(
            "case {name}\napi d3d12rt\ncs {}\nresults 640\ngroups 1 1 1\ncb 0 {}\n",
            fixture.display(),
            rows.len()
        ));
        for row in &rows {
            text.push_str(&format!(
                "row {:08x} {:08x} {:08x} {:08x}\n",
                row[0], row[1], row[2], row[3]
            ));
        }
        for (b, geometries) in scene.bottoms.iter().enumerate() {
            for g in geometries {
                if !g.boxes.is_empty() {
                    text.push_str(&format!(
                        "procedural {b} {} {}
",
                        u8::from(g.opaque),
                        g.boxes.len()
                    ));
                    for corners in &g.boxes {
                        text.push_str(&format!(
                            "box {}
",
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
            let flags = u32::from(i.flags.cull_disable)
                | u32::from(i.flags.front_counter_clockwise) << 1
                | u32::from(i.flags.force_opaque) << 2
                | u32::from(i.flags.force_non_opaque) << 3;
            text.push_str(&format!(
                "instance {} {} {} {} {flags} {}\n",
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

        let mut resources = Resources::default();
        resources.constants.insert(constants, rows);
        resources.scenes.insert(structure, scene);
        let bound = exe.bind_resources(&resources).unwrap();
        let mut buffers = BTreeMap::from([(results, vec![0u32; 640])]);
        exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        ours.insert(name.clone(), buffers.remove(&results).unwrap());
        cases.push((name, rays));
    }
    let cases_file = scratch.join(format!("{fixture_name}.cases.txt"));
    let out_file = scratch.join(format!("{fixture_name}.warp.jsonl"));
    std::fs::write(&cases_file, text).unwrap();
    let _oracle = primase_testdata::warp_exclusive();
    let run = std::process::Command::new(&warp)
        .arg(&cases_file)
        .arg(&out_file)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "warp: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let (mut hits, mut misses, mut mismatches) = (0, 0, Vec::new());
    for line in std::fs::read_to_string(&out_file).unwrap().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let name = v["case"].as_str().unwrap().to_owned();
        if let Some(e) = v["error"].as_str() {
            mismatches.push(format!("{name}: WARP error {e}"));
            continue;
        }
        let theirs: Vec<u32> = v["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| u32::from_str_radix(w.as_str().unwrap(), 16).unwrap())
            .collect();
        let mine = &ours[&name];
        let rays = &cases.iter().find(|c| c.0 == name).unwrap().1;
        for i in 0..64 {
            let (a, b) = (&mine[10 * i..10 * i + 10], &theirs[10 * i..10 * i + 10]);
            if a[0] != b[0] {
                let (o, d, f, m) = &rays[i];
                mismatches.push(format!(
                    "{name} ray {i} (origin {o:?}, direction {d:?}, flags {f:#x}, mask {m:#x}): status executor {}, WARP {}",
                    a[0], b[0]
                ));
                continue;
            }
            if a[0] == 0 {
                misses += 1;
                continue;
            }
            hits += 1;
            let close = |x: u32, y: u32| {
                let (x, y) = (f32::from_bits(x), f32::from_bits(y));
                (x - y).abs() <= 1e-4 * x.abs().max(y.abs()).max(1e-2)
            };
            // a triangle also has barycentrics and a facing; a procedural hit has neither (the words are 0)
            let same = [2, 3, 4, 5, 6, 9].iter().all(|&k| a[k] == b[k])
                && close(a[1], b[1])
                && (a[0] != 1 || (close(a[7], b[7]) && close(a[8], b[8])));
            if !same {
                let (o, d, f, m) = &rays[i];
                mismatches.push(format!(
                    "{name} ray {i} (origin {o:?}, direction {d:?}, flags {f:#x}, mask {m:#x}):\n    executor {a:08x?}\n    WARP     {b:08x?}"
                ));
            }
        }
    }
    println!(
        "{fixture_name}: {} rays: {hits} hits and {misses} misses agree with WARP, {} differ",
        12 * 64,
        mismatches.len()
    );
    for m in &mismatches {
        println!("MISMATCH {m}");
    }
    assert!(hits > 0 && mismatches.is_empty());
}
