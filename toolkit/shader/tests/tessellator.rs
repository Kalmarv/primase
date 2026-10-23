#![allow(clippy::unwrap_used)]
//! The fixed-function tessellator against the output of Microsoft's reference tessellator (`CHWTessellator`, which it is a
//! port of): 480 cases over every domain, partitioning and output primitive, with factors from fractions, integers, the
//! limits, zero, negative, NaN and infinity. For each case the reference gave the number of points and of indices and a
//! digest of them (`tests/fixtures/tessellator_reference.txt`, made by the reference program of `dumps/tess-ref` in
//! primase-oracles); the port must agree on all three. The point positions are 16.16 fixed-point numbers, so the
//! digest is of the float bits: there is no tolerance.
use primase_shader::Partitioning;
use primase_shader::exec::tessellator::{
    Factors, OutputPrimitive, Tessellation, tessellate_isolines, tessellate_quad, tessellate_triangle,
};

fn fnv(mut hash: u64, word: u32) -> u64 {
    for byte in word.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn digest(t: &Tessellation) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for p in &t.points {
        hash = fnv(hash, p[0].to_bits());
        hash = fnv(hash, p[1].to_bits());
    }
    for &i in &t.indices {
        hash = fnv(hash, i);
    }
    hash
}

fn bits(word: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(word, 16).unwrap())
}

#[test]
fn the_tessellator_agrees_with_microsofts_reference_on_every_case() {
    let text = include_str!("fixtures/tessellator_reference.txt");
    let mut cases = 0;
    let mut culled = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let w: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(w.len(), 12, "{line}");
        let partitioning = match w[1] {
            "integer" => Partitioning::Integer,
            "pow2" => Partitioning::Pow2,
            "fractional_odd" => Partitioning::FractionalOdd,
            "fractional_even" => Partitioning::FractionalEven,
            other => panic!("{other}"),
        };
        let output = match w[2] {
            "point" => OutputPrimitive::Point,
            "line" => OutputPrimitive::Line,
            "triangle_cw" => OutputPrimitive::TriangleCw,
            "triangle_ccw" => OutputPrimitive::TriangleCcw,
            other => panic!("{other}"),
        };
        let f: Vec<f32> = w[3..9].iter().map(|h| bits(h)).collect();
        let factors = Factors {
            edges: [f[0], f[1], f[2], f[3]],
            inside: [f[4], f[5]],
        };
        let got = match w[0] {
            "tri" => tessellate_triangle(partitioning, output, factors),
            "quad" => tessellate_quad(partitioning, output, factors),
            "isoline" => tessellate_isolines(partitioning, output, f[0], f[1]),
            other => panic!("{other}"),
        };
        let (points, indices) = (w[9].parse::<usize>().unwrap(), w[10].parse::<usize>().unwrap());
        let want = u64::from_str_radix(w[11], 16).unwrap();
        assert_eq!(got.points.len(), points, "points: {line}");
        assert_eq!(got.indices.len(), indices, "indices: {line}");
        assert_eq!(digest(&got), want, "digest: {line}");
        cases += 1;
        culled += usize::from(points == 0);
    }
    assert_eq!(cases, 480);
    // the sample holds culled patches (a factor of 0, negative or NaN) as well as full ones
    assert!(culled > 0 && culled < cases);
}

/// The smallest patches, worked out by hand: with every factor 1 a triangle patch is its one triangle, a quad patch its
/// two, an isoline of density 1 and detail 2 one line of two segments.
#[test]
fn the_smallest_patches() {
    let one = Factors {
        edges: [1.0; 4],
        inside: [1.0; 2],
    };
    let t = tessellate_triangle(Partitioning::Integer, OutputPrimitive::TriangleCw, one);
    assert_eq!(t.points, [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0]]);
    assert_eq!(t.indices, [0, 1, 2]);
    let t = tessellate_triangle(Partitioning::Integer, OutputPrimitive::TriangleCcw, one);
    assert_eq!(t.indices, [0, 2, 1]);

    let q = tessellate_quad(Partitioning::Integer, OutputPrimitive::TriangleCw, one);
    assert_eq!(q.points, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    assert_eq!(q.indices, [0, 1, 3, 1, 2, 3]);

    let l = tessellate_isolines(Partitioning::Integer, OutputPrimitive::Line, 1.0, 2.0);
    assert_eq!(l.points, [[0.0, 0.0], [0.5, 0.0], [1.0, 0.0]]);
    assert_eq!(l.indices, [0, 1, 1, 2]);

    // a factor of 0 (or NaN) culls the patch
    let culled = Factors {
        edges: [0.0, 2.0, 2.0, 2.0],
        inside: [2.0; 2],
    };
    assert!(
        tessellate_triangle(Partitioning::Integer, OutputPrimitive::TriangleCw, culled)
            .points
            .is_empty()
    );
}
