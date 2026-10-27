#![allow(clippy::unwrap_used)]
//! The draw stage ([`iridia_exec::draw`]) against Microsoft's WARP rasterizer (Direct3D 12): each case draws
//! the same triangles through the same vertex and pixel programs (tests/fixtures/draw.hlsl, lifted from their DXIL)
//! into 16 x 16 targets on both, and every pixel is compared: which pixels each draws (exactly), what it writes (to a
//! stated tolerance: the interpolation's arithmetic is each implementation's own) and the depth.
//!
//! Needs the WARP oracle program with the `d3d12draw` case form (the configured `Tool::Warp`; the form is in
//! `oracles/warp-draw`) and the scratch folder. Without them the test does nothing.
use iridia_exec::draw::{Compare, Cull, DepthState, DrawShader, Rasterizer, Surface, Viewport};
use iridia_exec::{PixelShader, ProgramShader, Sample};
use primase_testdata::{Tool, own_scratch, tool};
use std::path::Path;

const SIZE: u32 = 16;
/// What every target is cleared to: no pixel program here writes it.
const CLEAR: f32 = -7.0;
/// The interpolated values' tolerance: relative to the larger magnitude, with a floor for values near 0.
const TOLERANCE: f32 = 2e-5;

/// One vertex: position (x, y, z, w in clip space), colour, uv, id (the four inputs of `vs_main`, 14 floats).
type Vertex = [f32; 14];

fn v(pos: [f32; 4], col: [f32; 4], uv: [f32; 2], id: f32) -> Vertex {
    [
        pos[0],
        pos[1],
        pos[2],
        pos[3],
        col[0],
        col[1],
        col[2],
        col[3],
        uv[0],
        uv[1],
        id,
        id + 0.5,
        0.0,
        1.0,
    ]
}

/// A clip-space position at pixel coordinates (x right, y down, over the 16 x 16 targets), depth z, w.
fn at(x: f32, y: f32, z: f32, w: f32) -> [f32; 4] {
    let s = SIZE as f32;
    [(x / s * 2.0 - 1.0) * w, (1.0 - y / s * 2.0) * w, z * w, w]
}

struct DrawCase {
    name: &'static str,
    ps: &'static str,
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    cull: Cull,
    ccw: bool,
    depth: Option<(Compare, bool, f32)>,
    targets: Vec<u32>,
}

fn compare_name(c: Compare) -> &'static str {
    match c {
        Compare::Never => "never",
        Compare::Less => "less",
        Compare::Equal => "equal",
        Compare::LessEqual => "lessequal",
        Compare::Greater => "greater",
        Compare::NotEqual => "notequal",
        Compare::GreaterEqual => "greaterequal",
        Compare::Always => "always",
    }
}

fn cases() -> Vec<DrawCase> {
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];
    // a triangle whose vertices have different w: perspective-correct and linear interpolation differ
    let perspective = vec![
        v(at(1.0, 14.0, 0.5, 1.0), red, [0.0, 0.0], 1.0),
        v(at(8.0, 1.5, 0.5, 3.0), green, [0.5, 1.0], 2.0),
        v(at(15.0, 13.0, 0.25, 0.5), blue, [1.0, 0.0], 3.0),
    ];
    // a square of two triangles sharing a diagonal, edges on pixel centres and between them, and a triangle with a
    // horizontal top edge on a row of centres: the top-left rule decides every shared and on-centre pixel
    let edges = vec![
        v(at(2.0, 2.0, 0.5, 1.0), red, [0.0, 0.0], 1.0),
        v(at(10.0, 2.0, 0.5, 1.0), green, [1.0, 0.0], 1.0),
        v(at(2.0, 10.0, 0.5, 1.0), blue, [0.0, 1.0], 1.0),
        v(at(10.0, 10.0, 0.5, 1.0), red, [1.0, 1.0], 2.0),
        v(at(10.5, 3.5, 0.5, 1.0), green, [0.0, 0.0], 3.0),
        v(at(15.5, 3.5, 0.5, 1.0), blue, [1.0, 0.0], 3.0),
        v(at(13.0, 13.5, 0.5, 1.0), red, [0.5, 1.0], 3.0),
    ];
    // two clockwise triangles and one counter-clockwise, at three depths
    let layered = vec![
        v(at(1.0, 1.0, 0.7, 1.0), red, [0.0, 0.0], 1.0),
        v(at(15.0, 1.0, 0.7, 1.0), red, [1.0, 0.0], 1.0),
        v(at(1.0, 15.0, 0.7, 1.0), red, [0.0, 1.0], 1.0),
        v(at(3.0, 3.0, 0.3, 2.0), green, [0.0, 0.0], 2.0),
        v(at(15.0, 8.0, 0.3, 2.0), green, [1.0, 0.0], 2.0),
        v(at(5.0, 15.0, 0.3, 2.0), green, [0.0, 1.0], 2.0),
        v(at(0.0, 8.0, 0.9, 1.0), blue, [0.0, 0.0], 3.0),
        v(at(16.0, 16.0, 0.9, 1.0), blue, [1.0, 0.0], 3.0),
        v(at(16.0, 0.0, 0.9, 1.0), blue, [0.0, 1.0], 3.0),
    ];
    // a triangle with one vertex behind the near plane (z < 0): clipped to a quadrilateral
    let near = vec![
        v(at(2.0, 14.0, 0.5, 1.0), red, [0.0, 0.0], 1.0),
        v([0.2, 0.9, -0.5, 1.0], green, [0.5, 1.0], 1.0),
        v(at(14.0, 12.0, 0.5, 1.0), blue, [1.0, 0.0], 1.0),
    ];
    // a triangle with a vertex near the camera plane (w = 1e-7): cut by the w plane, it is about 8e9 pixels right of
    // the targets, so x and y are clipped to the guard band before the positions are snapped
    let near_w = vec![
        v(at(2.25, 8.0, 0.5, 1.0), red, [0.0, 0.0], 1.0),
        v(at(8.25, 14.0, 0.5, 1.0), green, [1.0, 0.0], 1.0),
        v([1e3, 2e2, 0.0, 1e-7], blue, [0.5, 1.0], 1.0),
    ];
    // a vertical left edge and a horizontal top edge exactly halfway between two 16.8 fixed-point steps (x * 256 =
    // 1152.5, y * 256 = 896.5; the values are dyadic, so the clip coordinates are exact in f32): rounded half away from
    // zero the edge is 1/256 past the centres of column 4 (row 3) and leaves them out; rounded to even it passes
    // through them, and the top-left rule draws them. WARP draws them: the snap takes ties to even (3.2.4.1's
    // reference conversion; it allows 0.6 ULP)
    let tie = 4.5 + 1.0 / 512.0;
    let tie_x = vec![
        v(at(tie, 2.25, 0.5, 1.0), red, [0.0, 0.0], 1.0),
        v(at(12.25, 8.25, 0.5, 1.0), green, [1.0, 0.0], 1.0),
        v(at(tie, 13.75, 0.5, 1.0), blue, [0.0, 1.0], 1.0),
    ];
    let tie_y = vec![
        v(at(2.25, tie - 1.0, 0.5, 1.0), red, [0.0, 0.0], 1.0),
        v(at(13.75, tie - 1.0, 0.5, 1.0), green, [1.0, 0.0], 1.0),
        v(at(8.25, 13.25, 0.5, 1.0), blue, [0.0, 1.0], 1.0),
    ];
    let one = |name, ps, vertices: &Vec<Vertex>, targets: Vec<u32>| DrawCase {
        name,
        ps,
        indices: (0..vertices.len() as u32).collect(),
        vertices: vertices.clone(),
        cull: Cull::None,
        ccw: false,
        depth: None,
        targets,
    };
    vec![
        one("perspective", "colour", &perspective, vec![0, 1]),
        DrawCase {
            indices: vec![0, 1, 2, 2, 1, 3, 4, 5, 6],
            ..one("edges", "colour", &edges, vec![0, 1])
        },
        DrawCase {
            depth: Some((Compare::LessEqual, true, 1.0)),
            ..one("depth", "colour", &layered, vec![0, 1])
        },
        DrawCase {
            cull: Cull::Back,
            ..one("cull", "position", &layered, vec![0, 1])
        },
        DrawCase {
            ccw: true,
            ..one("facing", "position", &layered, vec![0, 1])
        },
        one("discard", "discard", &perspective, vec![0]),
        one("near", "colour", &near, vec![0, 1]),
        one("near_w", "colour", &near_w, vec![0, 1]),
        one("tie_x", "colour", &tie_x, vec![0, 1]),
        one("tie_y", "colour", &tie_y, vec![0, 1]),
        one("derivatives", "derivatives", &perspective, vec![0, 1]),
        one("texture", "texture", &perspective, vec![0]),
    ]
}

fn hex(x: f32) -> String {
    format!("{:08x}", x.to_bits())
}

fn case_text(c: &DrawCase, dir: &Path) -> String {
    let mut t = format!("case {}\napi d3d12draw\n", c.name);
    t += &format!("vs {}\n", dir.join("draw_vs.dxil").display());
    t += &format!("ps {}\n", dir.join(format!("draw_{}.dxil", c.ps)).display());
    t += &format!("size {SIZE} {SIZE}\n");
    t += &format!(
        "cull {}\nfront {}\n",
        match c.cull {
            Cull::None => "none",
            Cull::Front => "front",
            Cull::Back => "back",
        },
        u8::from(!c.ccw)
    );
    if let Some((compare, write, clear)) = c.depth {
        t += &format!(
            "depth {} {} {}\n",
            compare_name(compare),
            u8::from(write),
            hex(clear)
        );
    }
    t += "layout POSITION 0 f32x4 0\nlayout COLOR 0 f32x4 16\nlayout TEXCOORD 0 f32x2 32\nlayout TEXCOORD 1 f32x4 40\nstride 56\n";
    for v in &c.vertices {
        t += "vertex";
        for x in v {
            t += &format!(" {}", hex(*x));
        }
        t += "\n";
    }
    t += "index";
    for i in &c.indices {
        t += &format!(" {i:x}");
    }
    t += "\n";
    t += &format!(
        "vcb 0 2\nrow {} {} {} {}\nrow {} {} {} {}\n",
        hex(1.0),
        hex(1.0),
        hex(1.0),
        hex(1.0),
        hex(0.0),
        hex(0.0),
        hex(0.0),
        hex(0.0)
    );
    if c.ps == "texture" {
        t += &format!(
            "texture 0 {} {} {} {} ramp\n",
            hex(0.0),
            hex(0.0),
            hex(0.0),
            hex(0.0)
        );
    }
    for r in &c.targets {
        t += &format!(
            "target {r} float\nclear {r} {} {} {} {}\n",
            hex(CLEAR),
            hex(CLEAR),
            hex(CLEAR),
            hex(CLEAR)
        );
    }
    t + "end\n"
}

/// The oracle's ramp texture (4 x 4, texel (x, y) is (x, y, 0, 1)) through a point, clamp sampler.
fn ramp(s: Sample) -> Result<[f32; 4], String> {
    let texel = |c: f32| (c * 4.0).floor().clamp(0.0, 3.0);
    Ok([texel(s.uv[0]), texel(s.uv[1]), 0.0, 1.0])
}

fn ours(c: &DrawCase, dir: &Path) -> Surface {
    let lift = |name: &str| {
        ProgramShader::new(iridia_dxil::lift(&std::fs::read(dir.join(name)).unwrap()).unwrap()).unwrap()
    };
    let vs = lift("draw_vs.dxil");
    let ps = lift(&format!("draw_{}.dxil", c.ps));
    // each vertex's input registers, by the vertex program's signature: POSITION0, COLOR0, TEXCOORD0, TEXCOORD1
    let inputs: Vec<Vec<[u32; 4]>> = c
        .vertices
        .iter()
        .map(|vertex| {
            let mut regs = vec![[0u32; 4]; vs.input_count()];
            for input in &vs.interface().inputs {
                let (start, width) = match (input.label.name.to_ascii_uppercase().as_str(), input.label.index)
                {
                    ("POSITION", 0) => (0, 4),
                    ("COLOR", 0) => (4, 4),
                    ("TEXCOORD", 0) => (8, 2),
                    ("TEXCOORD", 1) => (10, 4),
                    _ => continue,
                };
                let sig = input.provenance.d3d.unwrap();
                let lanes: Vec<usize> = (0..4).filter(|l| sig.mask >> l & 1 != 0).collect();
                for (k, lane) in lanes.iter().take(width).enumerate() {
                    regs[sig.register as usize][*lane] = vertex[start + k].to_bits();
                }
            }
            regs
        })
        .collect();
    let draw = DrawShader::new(&vs, &ps).unwrap();
    let n = (SIZE * SIZE) as usize;
    let highest = c.targets.iter().max().map_or(0, |r| r + 1) as usize;
    let mut surface = Surface {
        width: SIZE,
        height: SIZE,
        targets: (0..highest)
            .map(|r| {
                c.targets
                    .contains(&(r as u32))
                    .then(|| vec![[CLEAR.to_bits(); 4]; n])
            })
            .collect(),
        depth: c.depth.map(|(_, _, clear)| vec![clear; n]),
    };
    let state = Rasterizer {
        viewport: Viewport {
            x: 0.0,
            y: 0.0,
            width: SIZE as f32,
            height: SIZE as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        },
        cull: c.cull,
        front_counter_clockwise: c.ccw,
        depth: c.depth.map(|(compare, write, _)| DepthState { compare, write }),
    };
    let vertex_cb = [[1f32.to_bits(); 4], [0; 4]];
    let mut sampler = ramp;
    draw.draw_indexed(
        &inputs,
        &c.indices,
        &[(0, &vertex_cb[..])],
        &[],
        &mut sampler,
        &state,
        &mut surface,
    )
    .unwrap();
    surface
}

fn close(a: u32, b: u32) -> bool {
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    a == b || (x - y).abs() <= TOLERANCE * x.abs().max(y.abs()).max(1.0)
}

#[test]
fn draws_agree_with_warp() {
    let (Some(warp), Some(_)) = (tool(Tool::Warp), Some(())) else {
        return;
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let own = own_scratch("warp");
    let work = own.join("draw");
    std::fs::create_dir_all(&work).unwrap();
    let cases = cases();
    let text: String = cases.iter().map(|c| case_text(c, &dir)).collect();
    let (input, output) = (work.join("cases.txt"), work.join("results.jsonl"));
    std::fs::write(&input, text).unwrap();
    let _oracle = primase_testdata::warp_exclusive();
    let status = std::process::Command::new(&warp)
        .arg(&input)
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success(), "the oracle failed");
    let results: Vec<serde_json::Value> = std::fs::read_to_string(&output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for (c, r) in cases.iter().zip(&results) {
        assert_eq!(r["case"], c.name);
        if let Some(e) = r["error"].as_str() {
            failures.push(format!("{}: oracle error {e}", c.name));
            continue;
        }
        let words = |v: &serde_json::Value| -> Vec<u32> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|w| u32::from_str_radix(w.as_str().unwrap(), 16).unwrap())
                .collect()
        };
        let surface = ours(c, &dir);
        for (k, reg) in c.targets.iter().enumerate() {
            let warp_words = words(&r["targets"][k]);
            // a case must draw something, or agreement says nothing
            let drawn = warp_words
                .chunks(4)
                .filter(|t| t.iter().any(|w| *w != CLEAR.to_bits()))
                .count();
            println!("{} o{reg}: WARP drew {drawn} pixels", c.name);
            if drawn == 0 {
                failures.push(format!("{} o{reg}: WARP drew nothing", c.name));
            }
            let mine = surface.targets[*reg as usize].as_ref().unwrap();
            for p in 0..(SIZE * SIZE) as usize {
                let (x, y) = (p % SIZE as usize, p / SIZE as usize);
                let theirs = &warp_words[p * 4..p * 4 + 4];
                let covered_theirs = theirs.iter().any(|w| *w != CLEAR.to_bits());
                let covered_ours = mine[p].iter().any(|w| *w != CLEAR.to_bits());
                if covered_theirs != covered_ours {
                    failures.push(format!(
                        "{} o{reg} ({x}, {y}): drawn by {}",
                        c.name,
                        if covered_theirs {
                            "WARP only"
                        } else {
                            "the executor only"
                        }
                    ));
                    continue;
                }
                for lane in 0..4 {
                    compared += 1;
                    if !close(mine[p][lane], theirs[lane]) {
                        failures.push(format!(
                            "{} o{reg}.{lane} ({x}, {y}): executor {}, WARP {}",
                            c.name,
                            f32::from_bits(mine[p][lane]),
                            f32::from_bits(theirs[lane])
                        ));
                    }
                }
            }
        }
        if c.depth.is_some() {
            let warp_depth = words(&r["depth"]);
            let mine = surface.depth.as_ref().unwrap();
            for p in 0..(SIZE * SIZE) as usize {
                compared += 1;
                if !close(mine[p].to_bits(), warp_depth[p]) {
                    failures.push(format!(
                        "{} depth ({}, {}): executor {}, WARP {}",
                        c.name,
                        p % SIZE as usize,
                        p / SIZE as usize,
                        mine[p],
                        f32::from_bits(warp_depth[p])
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} values differ:\n{}",
        failures.len(),
        failures.iter().take(60).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(compared > 0);
}
