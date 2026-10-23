#![allow(clippy::unwrap_used)]
//! The geometry fixture (`geometry.hlsl`: a triangle in, a strip of its corners with x scaled, a restart, a strip of its
//! corners moved by the primitive and the instance; two instances) drawn on the CPU: the primitives a topology makes,
//! the instances of each, the strips they emit. The values are the arithmetic of the HLSL.
use primase_dxil::lift;
use primase_shader::exec::geometry::{GeometryShader, Strip, Topology, Vertex};
use primase_shader::{GsInput, GsOutput, Sample, Sampler};
use std::collections::BTreeMap;
use std::path::Path;

struct NoSampler;
impl Sampler for NoSampler {
    fn sample(&mut self, _: Sample) -> Result<[f32; 4], String> {
        Err("no sampling in this fixture".into())
    }
}

fn shader(name: &str) -> GeometryShader {
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap();
    GeometryShader::new(lift(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"))).unwrap()
}

/// The constant buffer `scale` (register 0): a row of the scale.
fn constants(scale: f32) -> BTreeMap<(u32, u32), Vec<[u32; 4]>> {
    BTreeMap::from([((0, 0), vec![[scale.to_bits(), 0, 0, 0]])])
}

/// A vertex of position (x, y, 0, 1) and texture coordinates (u, 0).
fn vertex(x: f32, y: f32, u: f32) -> Vertex {
    vec![
        [x, y, 0.0, 1.0].map(f32::to_bits),
        [u, 0.0, 0.0, 0.0].map(f32::to_bits),
    ]
}

/// A vertex's (x, y, u).
fn xyu(v: &Vertex) -> [f32; 3] {
    let at = |register: usize, lane: usize| f32::from_bits(v[register][lane]);
    [at(0, 0), at(0, 1), at(1, 0)]
}

/// A strip's vertices as (x, y, u), the position's z and w checked.
fn values(strip: &Strip) -> Vec<[f32; 3]> {
    strip
        .vertices
        .iter()
        .map(|v| {
            assert_eq!(f32::from_bits(v[0][2]), 0.0);
            assert_eq!(f32::from_bits(v[0][3]), 1.0);
            xyu(v)
        })
        .collect()
}

#[test]
fn the_program_is_described_by_its_facts_and_names() {
    for name in ["geometry.dxil", "geometry.od.dxil"] {
        let gs = shader(name);
        let facts = gs.facts();
        assert_eq!(
            (facts.input, facts.output, facts.max_vertices, facts.instances),
            (GsInput::Triangle, GsOutput::TriangleStrip, 6, 2),
            "{name}"
        );
        assert_eq!(
            gs.inputs().collect::<Vec<_>>(),
            ["SV_Position0", "TEXCOORD0"],
            "{name}"
        );
        assert_eq!(
            gs.outputs().collect::<Vec<_>>(),
            ["SV_Position0", "TEXCOORD0"],
            "{name}"
        );
    }
}

/// Two triangles of a list, scale 2: each runs as instance 0 and instance 1, in that order, and each invocation emits
/// the strip of its corners with x doubled, then the strip moved by the primitive in x and the instance in y (and in
/// u).
#[test]
fn every_primitive_runs_every_instance_and_its_strips_are_cut_where_the_program_cuts() {
    for name in ["geometry.dxil", "geometry.od.dxil"] {
        let gs = shader(name);
        let corners = [
            vertex(1.0, 2.0, 0.5),
            vertex(3.0, 4.0, 1.5),
            vertex(5.0, 6.0, 2.5),
            vertex(7.0, 8.0, 3.5),
            vertex(9.0, 10.0, 4.5),
            vertex(11.0, 12.0, 5.5),
        ];
        let strips = gs
            .draw(&constants(2.0), &mut NoSampler, &corners, Topology::TriangleList)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(strips.len(), 8, "{name}");
        let mut at = strips.iter();
        for primitive in 0..2u32 {
            let first = primitive as usize * 3;
            for instance in 0..2u32 {
                let (p, i) = (primitive as f32, instance as f32);
                let one = at.next().unwrap();
                let two = at.next().unwrap();
                assert_eq!(
                    ((one.primitive, one.instance), (two.primitive, two.instance)),
                    ((primitive, instance), (primitive, instance)),
                    "{name}"
                );
                let scaled: Vec<[f32; 3]> = (0..3)
                    .map(|k| {
                        let [x, y, u] = xyu(&corners[first + k]);
                        [x * 2.0, y, u]
                    })
                    .collect();
                assert_eq!(
                    values(one),
                    scaled,
                    "{name} primitive {primitive} instance {instance}"
                );
                let moved: Vec<[f32; 3]> = (0..3)
                    .map(|k| {
                        let [x, y, u] = xyu(&corners[first + k]);
                        [x + p, y + i, u + i]
                    })
                    .collect();
                assert_eq!(
                    values(two),
                    moved,
                    "{name} primitive {primitive} instance {instance}"
                );
            }
        }
    }
}

/// A strip of four vertices gives the program the triangles (0, 1, 2) and (1, 3, 2): the second's corners are read in
/// that order.
#[test]
fn a_triangle_strip_is_read_as_the_assembler_makes_it() {
    let gs = shader("geometry.dxil");
    let strip = [
        vertex(1.0, 0.0, 0.0),
        vertex(2.0, 0.0, 0.0),
        vertex(3.0, 0.0, 0.0),
        vertex(4.0, 0.0, 0.0),
    ];
    let strips = gs
        .draw(&constants(1.0), &mut NoSampler, &strip, Topology::TriangleStrip)
        .unwrap();
    // primitive 1, instance 0, its first strip
    let second = strips
        .iter()
        .find(|s| (s.primitive, s.instance) == (1, 0))
        .unwrap();
    let xs: Vec<f32> = values(second).iter().map(|v| v[0]).collect();
    assert_eq!(xs, [2.0, 4.0, 3.0]);
}

#[test]
fn a_draw_of_another_kind_of_primitive_is_refused() {
    let gs = shader("geometry.dxil");
    let vertices = [vertex(0.0, 0.0, 0.0), vertex(1.0, 0.0, 0.0)];
    let e = gs
        .draw(&constants(1.0), &mut NoSampler, &vertices, Topology::LineList)
        .unwrap_err();
    assert!(e.contains("reads Triangle"), "{e}");
    // a primitive given the wrong number of vertices
    let e = gs
        .run(&constants(1.0), &mut NoSampler, &[&vertices[0]], 0, 0)
        .unwrap_err();
    assert!(e.contains("which has 3"), "{e}");
}
