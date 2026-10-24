#![allow(clippy::unwrap_used)]
//! The tessellation executor against Microsoft's WARP rasterizer (Direct3D 11): a one-point patch goes through a vertex
//! program, a hull program whose patch-constant function reads its tessellation factors from a constant buffer, the
//! fixed-function tessellator, a domain program that writes each point's domain location, and a pass-through geometry
//! program whose stream output WARP's buffer is read back from. The executor runs the same hull and domain programs
//! (lifted from the same DXBC) and the primitives it makes are compared with the ones WARP wrote: every vertex of every
//! point, line or triangle, in order, bit by bit.
//!
//! This checks the tessellator ([`primase_shader_exec::tessellator`]: which points, in which order, joined how, for
//! every domain, partitioning and output topology and a spread of factors), the hull and domain stages around it
//! (patch constants and control points reaching the domain program) and the factor conventions of the programs (which
//! constant is which edge, which is the line density).
//!
//! Needs the WARP oracle program with the `d3d11ts` case form (the configured `Tool::Warp`), `tools.fxc` and the
//! scratch folder. Every program is compiled here from the HLSL below, so the cases are the whole of the input.
use primase_dxbc::lift;
use primase_shader::TessTopology;
use primase_shader_exec::Constants;
use primase_shader_exec::tessellation::TessellationShader;
use primase_shader_exec::{Sample, Sampler};
use primase_testdata::{Tool, own_scratch, tool};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const HLSL: &str = r#"
// the vertex and geometry programs do not depend on the patch; their compilation still parses the rest
#ifndef DOMAIN
#define DOMAIN "tri"
#endif
#ifndef PARTITION
#define PARTITION "integer"
#endif
#ifndef OUTPUT
#define OUTPUT "triangle_cw"
#endif
#ifndef CPS
#define CPS 1
#endif
#ifndef EDGES
#define EDGES 3
#endif
#ifndef INSIDES
#define INSIDES 1
#endif
#ifndef LOCATION
#define LOCATION float3
#endif
#ifndef LOC_EXPR
#define LOC_EXPR loc.x, loc.y, loc.z, 1
#endif
cbuffer Factors : register(b0) {
    float4 edges;
    float4 inside;
};
struct CP { float4 p : POSITION; };
struct PC {
    float e[EDGES] : SV_TessFactor;
#if INSIDES > 0
    float i[INSIDES] : SV_InsideTessFactor;
#endif
    float4 extra : EXTRA;
};
struct DO {
    float4 loc : LOC;
    float4 mix : MIX;
};

CP vs(uint id : SV_VertexID) {
    CP o;
    o.p = float4(id + 1, 2 * id + 1, 3, 4);
    return o;
}

PC pc(InputPatch<CP, CPS> patch, uint id : SV_PrimitiveID) {
    PC o;
    o.extra = patch[0].p + patch[CPS - 1].p + float4(id, 0, 0, 0);
    for (uint k = 0; k < EDGES; k++) o.e[k] = edges[k];
#if INSIDES > 0
    for (uint j = 0; j < INSIDES; j++) o.i[j] = inside[j];
#endif
    return o;
}

[domain(DOMAIN)]
[partitioning(PARTITION)]
[outputtopology(OUTPUT)]
[outputcontrolpoints(CPS)]
[patchconstantfunc("pc")]
[maxtessfactor(64.0)]
CP hs(InputPatch<CP, CPS> patch, uint i : SV_OutputControlPointID) {
    CP o = patch[i];
    o.p += float4(i, 0.5 * i, 0.25, 0);
    return o;
}

[domain(DOMAIN)]
DO ds(PC pc, LOCATION loc : SV_DomainLocation, const OutputPatch<CP, CPS> patch, uint id : SV_PrimitiveID) {
    DO o;
    o.loc = float4(LOC_EXPR);
    o.mix = float4(loc.x * patch[0].p.x + loc.y * patch[CPS - 1].p.y + pc.extra.z,
                   patch[CPS / 2].p.w * loc.y + pc.extra.x, patch[0].p.z + pc.extra.w + id, loc.x * pc.extra.y);
    return o;
}

[maxvertexcount(3)]
void gs_triangle(triangle DO v[3], inout TriangleStream<DO> s) {
    for (uint i = 0; i < 3; i++) s.Append(v[i]);
}
[maxvertexcount(2)]
void gs_line(line DO v[2], inout LineStream<DO> s) {
    for (uint i = 0; i < 2; i++) s.Append(v[i]);
}
[maxvertexcount(1)]
void gs_point(point DO v[1], inout PointStream<DO> s) {
    s.Append(v[0]);
}
"#;

struct NoSampler;
impl Sampler for NoSampler {
    fn sample(&mut self, _: Sample) -> Result<[f32; 4], String> {
        Err("no sampling in this fixture".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Domain {
    Tri,
    Quad,
    Isoline,
}
impl Domain {
    fn name(self) -> &'static str {
        match self {
            Domain::Tri => "tri",
            Domain::Quad => "quad",
            Domain::Isoline => "isoline",
        }
    }
    /// The edge factors, the inside factors and the domain location's type and the domain program's result.
    fn defines(self) -> [(&'static str, &'static str); 4] {
        match self {
            Domain::Tri => [
                ("EDGES", "3"),
                ("INSIDES", "1"),
                ("LOCATION", "float3"),
                ("LOC_EXPR", "loc.x, loc.y, loc.z, 1"),
            ],
            Domain::Quad => [
                ("EDGES", "4"),
                ("INSIDES", "2"),
                ("LOCATION", "float2"),
                ("LOC_EXPR", "loc.x, loc.y, 0, 1"),
            ],
            Domain::Isoline => [
                ("EDGES", "2"),
                ("INSIDES", "0"),
                ("LOCATION", "float2"),
                ("LOC_EXPR", "loc.x, loc.y, 0, 1"),
            ],
        }
    }
}

const PARTITIONS: [&str; 4] = ["integer", "pow2", "fractional_odd", "fractional_even"];

fn fxc_compile(
    fxc: &Path,
    dir: &Path,
    name: &str,
    profile: &str,
    entry: &str,
    defines: &[(&str, &str)],
) -> PathBuf {
    let source = dir.join("tess.hlsl");
    let out = dir.join(format!("{name}.dxbc"));
    let mut command = std::process::Command::new(fxc);
    command
        .args(["/nologo", "/T", profile, "/E", entry, "/Fo"])
        .arg(&out);
    for (k, v) in defines {
        command.arg(format!("/D{k}={v}"));
    }
    let run = command.arg(&source).output().unwrap();
    assert!(
        run.status.success(),
        "fxc {name}: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    out
}

/// Tessellation factors as the constant buffer holds them, and what to run them as.
struct Case {
    edges: [f32; 4],
    inside: [f32; 2],
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
    fn float(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.next() as f32 / (1u64 << 31) as f32)
    }
}

fn cases(domain: Domain) -> Vec<Case> {
    let mut out = Vec::new();
    let mut add = |edges: [f32; 4], inside: [f32; 2]| out.push(Case { edges, inside });
    let fixed: &[([f32; 4], [f32; 2])] = &[
        ([1.0; 4], [1.0; 2]),
        ([2.0; 4], [2.0; 2]),
        ([3.0; 4], [3.0; 2]),
        ([5.0, 3.0, 2.0, 4.0], [4.0, 6.0]),
        ([7.3, 3.7, 12.5, 9.1], [8.8, 5.2]),
        ([8.0, 8.0, 8.0, 8.0], [3.0, 3.0]),
        ([16.0, 4.0, 9.0, 63.0], [2.0, 17.0]),
        ([2.5; 4], [2.5; 2]),
        ([1.2; 4], [1.2; 2]),
        ([33.0, 17.6, 5.0, 64.0], [64.0, 9.0]),
        ([1.0, 1.0, 1.0, 1.0], [5.0, 1.0]),
        ([0.0, 2.0, 2.0, 2.0], [2.0, 2.0]),
        ([65.0, 70.0, 100.0, 200.0], [90.0, 66.0]),
        ([4.0, 4.0, 4.0, 4.0], [1.0, 8.0]),
        ([6.0, 10.0, 6.0, 10.0], [7.0, 3.0]),
        ([63.0, 64.0, 1.0, 1.0], [64.0, 63.0]),
    ];
    for &(edges, inside) in fixed {
        add(edges, inside);
    }
    let mut rng = Lcg(0x7e55_e11a_71c0_ffee);
    for _ in 0..6 {
        let mut f = || rng.float(1.0, 64.0);
        add([f(), f(), f(), f()], [f(), f()]);
    }
    // only the factors the domain reads matter
    let _ = domain;
    out
}

fn hex(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

/// A vertex of the stream output: (x, y, z, w).
type Vertex = [f32; 4];

#[test]
fn tessellated_patches_are_the_ones_warp_makes() {
    let (Some(fxc), Some(warp)) = (tool(Tool::Fxc), tool(Tool::Warp)) else {
        eprintln!("skipped: needs tools.fxc and tools.warp");
        return;
    };
    let own = own_scratch("warp");
    let dir = own.join("tess");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tess.hlsl"), HLSL).unwrap();
    let vs = fxc_compile(&fxc, &dir, "vs", "vs_5_0", "vs", &[]);

    let mut failures = Vec::new();
    let (mut cases_run, mut vertices_compared) = (0usize, 0usize);
    for domain in [Domain::Tri, Domain::Quad, Domain::Isoline] {
        let d = domain.defines();
        let mut defines = vec![("DOMAIN", format!("\"{}\"", domain.name()))];
        defines.extend(d.iter().map(|(k, v)| (*k, (*v).to_owned())));
        let borrowed = |extra: &[(&'static str, String)]| -> Vec<(&str, String)> {
            let mut all: Vec<(&str, String)> = defines.iter().map(|(k, v)| (*k, v.clone())).collect();
            all.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
            all
        };
        let compile = |name: &str, profile: &str, entry: &str, extra: &[(&'static str, String)]| {
            let all = borrowed(extra);
            let refs: Vec<(&str, &str)> = all.iter().map(|(k, v)| (*k, v.as_str())).collect();
            fxc_compile(&fxc, &dir, name, profile, entry, &refs)
        };
        let outputs: &[&str] = match domain {
            Domain::Isoline => &["line", "point"],
            // (hull programs may only name a line output for isolines)
            _ => &["triangle_cw", "triangle_ccw", "point"],
        };
        // every partitioning and output with one control point, a few with more (the hull program's control point phase
        // runs for each, the domain program reads them all)
        let mut combos: Vec<(&str, &str, u32)> = Vec::new();
        for partition in PARTITIONS {
            for &output in outputs {
                combos.push((partition, output, 1));
            }
        }
        combos.push(("integer", outputs[0], 3));
        combos.push(("fractional_odd", outputs[outputs.len() - 1], 3));
        combos.push(("fractional_even", outputs[0], 4));
        for (partition, output, control_points) in combos {
            {
                let tag = format!("{}.{partition}.{output}.cp{control_points}", domain.name());
                let hs = compile(
                    &format!("hs.{tag}"),
                    "hs_5_0",
                    "hs",
                    &[
                        ("PARTITION", format!("\"{partition}\"")),
                        ("OUTPUT", format!("\"{output}\"")),
                        ("CPS", control_points.to_string()),
                    ],
                );
                let ds = compile(
                    &format!("ds.{}.cp{control_points}", domain.name()),
                    "ds_5_0",
                    "ds",
                    &[("CPS", control_points.to_string())],
                );
                let (gs_entry, per_primitive) = match output {
                    "line" => ("gs_line", 2),
                    "point" => ("gs_point", 1),
                    _ => ("gs_triangle", 3),
                };
                let gs = compile(&format!("gs.{}", gs_entry), "gs_5_0", gs_entry, &[]);

                // the executor
                let shader = TessellationShader::new(
                    lift(&std::fs::read(&hs).unwrap()).unwrap_or_else(|e| panic!("{tag}: hull: {e}")),
                    lift(&std::fs::read(&ds).unwrap()).unwrap_or_else(|e| panic!("{tag}: domain: {e}")),
                )
                .unwrap_or_else(|e| panic!("{tag}: {e}"));
                // the vertex program's output for vertex k: (k + 1, 2k + 1, 3, 4)
                let control_points_given: Vec<Vec<[u32; 4]>> = (0..control_points)
                    .map(|k| vec![[k as f32 + 1.0, 2.0 * k as f32 + 1.0, 3.0, 4.0].map(f32::to_bits)])
                    .collect();
                let names: Vec<&str> = shader.outputs().collect();
                let register = |name: &str| names.iter().position(|n| *n == name).unwrap();
                let (loc, mix) = (register("LOC0"), register("MIX0"));

                // the cases
                let list = cases(domain);
                let mut text = String::new();
                for (k, c) in list.iter().enumerate() {
                    text.push_str(&format!(
                        "case {tag}.{k}\napi d3d11ts\nvs {}\nhs {}\nds {}\ngs {}\nsopv {per_primitive}\ncps {control_points}\ncb 0 2\nrow {} {} {} {}\nrow {} {} 0 0\nend\n",
                        vs.display(),
                        hs.display(),
                        ds.display(),
                        gs.display(),
                        hex(c.edges[0]),
                        hex(c.edges[1]),
                        hex(c.edges[2]),
                        hex(c.edges[3]),
                        hex(c.inside[0]),
                        hex(c.inside[1]),
                    ));
                }
                let cases_path = dir.join(format!("{tag}.cases.txt"));
                let results_path = dir.join(format!("{tag}.results.jsonl"));
                std::fs::write(&cases_path, text).unwrap();
                let _oracle = primase_testdata::warp_exclusive();
                let run = std::process::Command::new(&warp)
                    .arg(&cases_path)
                    .arg(&results_path)
                    .output()
                    .unwrap();
                assert!(
                    run.status.success(),
                    "{tag}: warp: {}",
                    String::from_utf8_lossy(&run.stderr)
                );
                let results = std::fs::read_to_string(&results_path).unwrap();
                assert_eq!(results.lines().count(), list.len(), "{tag}");

                for (k, (c, line)) in list.iter().zip(results.lines()).enumerate() {
                    cases_run += 1;
                    let v: serde_json::Value = serde_json::from_str(line).unwrap();
                    if let Some(e) = v["error"].as_str() {
                        failures.push(format!("{tag}.{k}: WARP: {e}"));
                        continue;
                    }
                    let words: Vec<u32> = v["words"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|w| u32::from_str_radix(w.as_str().unwrap(), 16).unwrap())
                        .collect();
                    let warp_vertices: Vec<(Vertex, Vertex)> = words
                        .chunks(8)
                        .map(|w| {
                            (
                                [w[0], w[1], w[2], w[3]].map(f32::from_bits),
                                [w[4], w[5], w[6], w[7]].map(f32::from_bits),
                            )
                        })
                        .collect();

                    let mut constants: Constants = BTreeMap::new();
                    constants.insert(
                        (0, 0),
                        vec![
                            c.edges.map(f32::to_bits),
                            [c.inside[0].to_bits(), c.inside[1].to_bits(), 0, 0],
                        ],
                    );
                    let patch = shader
                        .tessellate(
                            &constants,
                            &BTreeMap::new(),
                            &mut NoSampler,
                            &control_points_given,
                            0,
                        )
                        .unwrap_or_else(|e| panic!("{tag}.{k}: {e}"));
                    let expected_topology = match output {
                        "line" => TessTopology::Line,
                        "point" => TessTopology::Point,
                        "triangle_cw" => TessTopology::TriangleCw,
                        _ => TessTopology::TriangleCcw,
                    };
                    assert_eq!(patch.topology, expected_topology, "{tag}.{k}");
                    let ours: Vec<(Vertex, Vertex)> = patch
                        .indices
                        .iter()
                        .map(|&i| {
                            let v = &patch.vertices[i as usize];
                            (v[loc].map(f32::from_bits), v[mix].map(f32::from_bits))
                        })
                        .collect();
                    vertices_compared += ours.len();
                    // the domain location exactly; the arithmetic on the control points and patch constants to a
                    // relative 1e-5 (a multiply and an add may or may not be fused)
                    let same = |a: &(Vertex, Vertex), b: &(Vertex, Vertex)| {
                        a.0 == b.0
                            && a.1
                                .iter()
                                .zip(&b.1)
                                .all(|(x, y)| (x - y).abs() <= 1e-5 * x.abs().max(y.abs()).max(1.0))
                    };
                    if ours.len() != warp_vertices.len()
                        || !ours.iter().zip(&warp_vertices).all(|(a, b)| same(a, b))
                    {
                        let first = ours.iter().zip(&warp_vertices).position(|(a, b)| !same(a, b));
                        failures.push(format!(
                            "{tag}.{k}: factors {:?} {:?}: {} vertices here, {} from WARP; first difference at {:?}: {:?} against {:?}",
                            c.edges,
                            c.inside,
                            ours.len(),
                            warp_vertices.len(),
                            first,
                            first.map(|i| ours[i]),
                            first.map(|i| warp_vertices[i]),
                        ));
                    }
                }
            }
        }
    }
    eprintln!(
        "{cases_run} cases, {vertices_compared} vertices compared, {} cases differ",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {cases_run} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
