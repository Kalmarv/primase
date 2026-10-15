//! The .NET and SharpDX behaviours the writer reproduces, against a fixture recorded from the reference runtime
//! (.NET 8, SharpDX 4.2.0) on synthetic inputs: number text, `Math.Round`, `MathF.Round`, string escaping, the
//! indented layout, both vector types and the math library calls.

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance};
use primase_reskate::dotnet::{self, JsonWriter};
use primase_reskate::vec::{Num, Sdx};

const FIXTURE: &str = include_str!("fixtures/dotnet.tsv");

fn f32_of(h: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(h, 16).unwrap())
}
fn f64_of(h: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(h, 16).unwrap())
}
fn hex32(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}
fn hex64(v: f64) -> String {
    format!("{:016x}", v.to_bits())
}

#[test]
fn matches_the_runtime() {
    let mut gates: std::collections::BTreeMap<&str, GateReport> = std::collections::BTreeMap::new();
    for line in FIXTURE.lines() {
        let mut parts = line.splitn(3, '\t');
        let (kind, input, want) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let got: String = match kind {
            "jf32" => dotnet::f32_text(f32_of(input)),
            "jf64" => dotnet::f64_text(f64_of(input)),
            "round4" => hex64(dotnet::round(f64_of(input), 4)),
            "mathf_round" => hex32(f32_of(input).round_ties_even()),
            "jstr" => {
                let bytes: Vec<u8> = (0..input.len() / 2)
                    .map(|i| u8::from_str_radix(&input[i * 2..i * 2 + 2], 16).unwrap())
                    .collect();
                let mut s = String::new();
                dotnet::escape(std::str::from_utf8(&bytes).unwrap(), &mut s);
                s
            }
            "sdx" | "num" => {
                let v: Vec<f32> = input.split(' ').map(f32_of).collect();
                let out: Vec<f32> = if kind == "sdx" {
                    let (a, b, t) = (Sdx::new(v[0], v[1], v[2]), Sdx::new(v[3], v[4], v[5]), v[6]);
                    let n = a.normalize();
                    let d = if t != 0.0 { a / t } else { a };
                    let c = a.cross(b);
                    vec![
                        a.length(),
                        a.length_squared(),
                        n.x,
                        n.y,
                        n.z,
                        d.x,
                        d.y,
                        d.z,
                        a.dot(b),
                        c.x,
                        c.y,
                        c.z,
                    ]
                } else {
                    let (a, b, t) = (Num::new(v[0], v[1], v[2]), Num::new(v[3], v[4], v[5]), v[6]);
                    let n = a.normalize();
                    let l = Num::lerp(a, b, t);
                    let c = a.cross(b);
                    vec![
                        a.length(),
                        a.length_squared(),
                        n.x,
                        n.y,
                        n.z,
                        l.x,
                        l.y,
                        l.z,
                        a.dot(b),
                        c.x,
                        c.y,
                        c.z,
                    ]
                };
                out.iter().map(|x| hex32(*x)).collect::<Vec<_>>().join(" ")
            }
            "libm" => {
                let v: Vec<f32> = input.split(' ').map(f32_of).collect();
                let (x, y) = (v[0], v[1]);
                let xd = f64::from(x);
                format!(
                    "{} {} {} {} {} {}",
                    hex32(x.abs().powf(y)),
                    hex32(x.cos()),
                    hex32(x.sin()),
                    hex64(xd.cos()),
                    hex64(xd.sin()),
                    hex64(xd.abs().sqrt())
                )
            }
            "indented" => {
                let mut w = JsonWriter::new(true);
                w.start_object();
                w.int("format", 1);
                w.string("name", "x");
                w.start_object_named("empty_obj");
                w.end_object();
                w.start_array_named("empty_arr");
                w.end_array();
                w.start_array_named("nums");
                w.f32_value(1.5);
                w.f64_value(-0.0);
                w.end_array();
                w.start_array_named("objs");
                w.start_object();
                w.bool("b", true);
                w.start_array_named("inner");
                w.string_value("s");
                w.end_array();
                w.end_object();
                w.end_array();
                w.end_object();
                w.finish().bytes().map(|b| format!("{b:02X}")).collect()
            }
            other => panic!("unknown kind {other}"),
        };
        let g = gates
            .entry(kind)
            .or_insert_with(|| GateReport::new(format!(".NET {kind}"), Tolerance::Exact));
        if got == want {
            g.record(input, 0.0);
        } else {
            g.fail(input, format!("got {got}, want {want}"));
        }
    }
    for g in gates.values() {
        println!("{g}");
    }
    for g in gates.values() {
        g.assert_passed();
    }
}
