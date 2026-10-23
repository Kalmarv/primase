#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The lifted fixtures run on the executor with outputs worked out by hand from the HLSL (tests/fixtures/*.hlsl), for
//! both the default and the -Od build of each (different instruction streams of one source). The evaluator cannot run
//! them (loops, switches, memory); the values below are the arithmetic of the source, not of the lifter.
use primase_dxil::lift;
use primase_shader::exec::{Backend, Executable, ImageSample, Output, Resources};
use primase_shader::{Program, ResourceId, ResourceKind};
use std::path::Path;

fn f(x: f32) -> u32 {
    x.to_bits()
}

struct NoBackend;
impl Backend for NoBackend {
    fn sample(&mut self, _: ImageSample<ResourceId>) -> Result<[f32; 4], String> {
        Err("no sampling in this fixture".into())
    }
}

fn lifted(name: &str) -> (Program, Executable) {
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap();
    let program = lift(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let exe = Executable::new(&program).unwrap_or_else(|e| panic!("{name}: {e}"));
    (program, exe)
}

/// Run `name` on one invocation: its entry arguments in order (one register each) and its constant buffers by register.
fn run(name: &str, args: &[[u32; 4]], constants: &[(u32, Vec<[u32; 4]>)]) -> Output {
    let (program, exe) = lifted(name);
    let mut resources = Resources::default();
    for (slot, rows) in constants {
        let id = program
            .interface
            .resources
            .iter()
            .find(|r| {
                matches!(r.kind, ResourceKind::ConstantBuffer { .. })
                    && r.provenance.d3d.is_some_and(|d| d.register == *slot)
            })
            .unwrap_or_else(|| panic!("{name}: no cb{slot}"))
            .id;
        resources.constants.insert(id, rows.clone());
    }
    let bound = exe.bind_resources(&resources).unwrap();
    let mut backend = NoBackend;
    let mut runner = primase_shader::exec::Runner::<1>::new(&exe, &bound, &mut backend, None);
    let [out] = runner.run([args]).unwrap_or_else(|e| panic!("{name}: {e}"));
    out
}

const BOTH: [&str; 2] = ["", ".od"];

fn fixture(base: &str, variant: &str) -> String {
    format!("{base}{variant}.dxil")
}

/// `flow.hlsl`: short-circuit tests with a shared else, an early return, a discard, nested ifs. Constants
/// `Values { float4 a; int4 n; }`, input t = TEXCOORD0.
fn flow(variant: &str, a: [f32; 4], n: [i32; 4], t: [f32; 2]) -> Output {
    run(
        &fixture("flow", variant),
        &[[f(t[0]), f(t[1]), 0, 0]],
        &[(0, vec![a.map(f32::to_bits), n.map(|x| x as u32)])],
    )
}

#[test]
fn branches_that_are_not_diamonds() {
    for v in BOTH {
        let a = [1.0, 4.0, 2.0, 3.0];
        // t.x = 2 > a.x = 1 and t.y = 0.25 < a.y = 4: o.x.x = a.z * t.x = 4. n.x = 5 > 3: k.x = 5. n.z != 7, t.x >= -1,
        // n.w = 2 > 0 and t.y = 0.25 <= 0.5: k.y = 13. n.x = 5 > n.y = 1: o.x.z = a.x = 1.
        let o = flow(v, a, [5, 1, 0, 2], [2.0, 0.25]);
        assert!(!o.discarded);
        assert_eq!(o.values[0], [f(4.0), f(0.0), f(1.0), f(1.0)], "{v}");
        assert_eq!(o.values[1], [5, 13, 0, 0], "{v}");
        // t.x = 0.5 is not > a.x: the else, o.x.x = a.w - t.y = 3 - 5 = -2. n = (1, 3, 7, -1): neither n.x > 3 nor n.y < 2:
        // k.x = 9. n.z = 7: o.x.y = 1 and the early return, k.y stays 0, o.x.z stays 0.
        let o = flow(v, a, [1, 3, 7, -1], [0.5, 5.0]);
        assert_eq!(o.values[0], [f(-2.0), f(1.0), f(0.0), f(1.0)], "{v}");
        assert_eq!(o.values[1], [9, 0, 0, 0], "{v}");
        // t.x = -2 < -1 discards
        assert!(flow(v, a, [5, 1, 0, 2], [-2.0, 1.0]).discarded, "{v}");
        // n = (0, 1, 0, 0): n.y = 1 < 2 gives k.x = 5; n.w = 0 is not > 0: k.y = 17; n.x = 0 is not > n.y = 1:
        // o.x.z = a.y = 4; t = (2, 0.25) as the first case: o.x.x = 4
        let o = flow(v, a, [0, 1, 0, 0], [2.0, 0.25]);
        assert_eq!(o.values[0], [f(4.0), f(0.0), f(4.0), f(1.0)], "{v}");
        assert_eq!(o.values[1], [5, 17, 0, 0], "{v}");
        // the same with t.y = 0.75 > 0.5 under n.w > 0: k.y = 11
        let o = flow(v, a, [5, 1, 0, 2], [2.0, 0.75]);
        assert_eq!(o.values[1], [5, 11, 0, 0], "{v}");
    }
}

fn loops(variant: &str, a: [f32; 4], n: [i32; 4], t: [f32; 2]) -> Output {
    run(
        &fixture("loops", variant),
        &[[f(t[0]), f(t[1]), 0, 0]],
        &[(0, vec![a.map(f32::to_bits), n.map(|x| x as u32)])],
    )
}

#[test]
fn loops_with_continue_break_nesting_and_two_exits() {
    for v in BOTH {
        // a = (2, 10, _, 100), n = (4, 2, 2, 5), t = (0.5, 1).
        // first loop (i < 4, skip i = 2, stop when sum > 100): i = 0: sum 0.5, count 1; i = 1: 0.5 + 2 + 0.5 = 3, count 2;
        // i = 3: 3 + 6 + 0.5 = 9.5, count 3.
        // inner loops (j < 2, m < 4, inner += 4j + m, leave the inner when inner > 5, the outer when inner > 1000):
        // j = 0: inner 0, 1, 3, 6 (> 5, leave); j = 1: inner = 6 + 4 = 10 (> 5, leave). inner = 10.
        // while (v < 10 and guard < 16), v = 1.5v + 0.25: 1 -> 1.75 -> 2.875 -> 4.5625 -> 7.09375 -> 10.890625, guard 5.
        let o = loops(v, [2.0, 10.0, 0.0, 100.0], [4, 2, 2, 5], [0.5, 1.0]);
        assert_eq!(o.values[0], [f(9.5), f(10.890625), f(0.0), f(1.0)], "{v}");
        assert_eq!(o.values[1], [3, 10, 5, 0], "{v}");
        // n.x = 6 and a.w = 5: i = 3 leaves sum 9.5 and i = 4 breaks (9.5 > 5) with count 3; n.z = 0: no inner turns;
        // t.y = 20 is not < a.y: guard 0, v = 20
        let o = loops(v, [2.0, 10.0, 0.0, 5.0], [6, 2, 0, 5], [0.5, 20.0]);
        assert_eq!(o.values[0], [f(9.5), f(20.0), f(0.0), f(1.0)], "{v}");
        assert_eq!(o.values[1], [3, 0, 0, 0], "{v}");
        // n.x = 0: no turns at all
        let o = loops(v, [2.0, 10.0, 0.0, 5.0], [0, 2, 1, 1000], [0.5, 20.0]);
        assert_eq!(o.values[0][0], f(0.0), "{v}");
        assert_eq!(o.values[1][0], 0, "{v}");
        // the outer loop's own exit: inner passes 1000 at j = 1 (n.w = 5000: the inner never breaks, inner = 0 + .. + 15)
        // j = 0: 0 + 1 + 2 + 3 = 6, j = 1: 6 + 4 + 5 + 6 + 7 = 28, j = 2: 28 + 8 + 9 + 10 + 11 = 66, j = 3: 66 + 12 + 13 + 14 + 15 = 120
        let o = loops(v, [2.0, 10.0, 0.0, 100.0], [0, 0, 4, 5000], [0.5, 20.0]);
        assert_eq!(o.values[1][1], 120, "{v}");
    }
}

fn switch(variant: &str, a: [f32; 4], n: [i32; 4], t: [f32; 2]) -> Output {
    run(
        &fixture("switch", variant),
        &[[f(t[0]), f(t[1]), 0, 0]],
        &[(0, vec![a.map(f32::to_bits), n.map(|x| x as u32)])],
    )
}

#[test]
fn a_switch_and_a_switch_in_a_loop() {
    for v in BOTH {
        let a = [1.5, 2.5, 3.5, 0.0];
        let t = [0.25, 2.0];
        // case 1 and 2 share a body: a.y + t.x = 2.75. The loop over i < 6 with i & 3 selecting: 1 (+5), 10 (continue,
        // no +5), 100 (+5), 1000 (+5), 1 (+5), 10 (continue): 6 + 10 + 105 + 1005 + 6 + 10 = 1142
        let o = switch(v, a, [2, 6, 0, 0], t);
        assert_eq!(o.values[0], [f(2.75), f(0.0), f(0.0), f(1.0)], "{v}");
        assert_eq!(o.values[1], [1142, 0, 0, 0], "{v}");
        // case 0: a.x; case 1: a.y + t.x; case 5: a.z * t.y = 7; the default: -1
        assert_eq!(switch(v, a, [0, 0, 0, 0], t).values[0][0], f(1.5), "{v}");
        assert_eq!(switch(v, a, [1, 0, 0, 0], t).values[0][0], f(2.75), "{v}");
        assert_eq!(switch(v, a, [5, 0, 0, 0], t).values[0][0], f(7.0), "{v}");
        assert_eq!(switch(v, a, [9, 0, 0, 0], t).values[0][0], f(-1.0), "{v}");
        assert_eq!(switch(v, a, [3, 0, 0, 0], t).values[0][0], f(-1.0), "{v}");
    }
}

#[test]
fn local_arrays_and_constant_tables() {
    for v in BOTH {
        // Values { float4 a; int4 n; float4 table[6]; }: rows 0 and 1, then the table's six rows
        let a = [1.0f32, 2.0, 3.0, 4.0];
        let table: Vec<[u32; 4]> = (0..6)
            .map(|r| [f(10.0 + r as f32), f(20.0 + r as f32), 0, 0])
            .collect();
        let mut rows = vec![a.map(f32::to_bits), [1u32, 2, 3, 4]];
        rows.extend(table);
        let o = run(&fixture("arrays", v), &[[f(0.0), f(0.0), 0, 0]], &[(0, rows)]);
        // local[(i + 1) & 3] = 2, 3, 4, 1 against weights 0.1, 0.2, 0.4, 0.2 (f32, accumulated in order)
        let w = [0.1f32, 0.2, 0.4, 0.2];
        let mut s = 0.0f32;
        for (i, l) in [2.0f32, 3.0, 4.0, 1.0].iter().enumerate() {
            s += l * w[i];
        }
        // weights[n.y & 3] = weights[2]; table[n.z = 3].x = 13; table[(n.w + 1) % 6 = 5].y = 25
        assert_eq!(o.values[0], [f(s), f(0.4), f(13.0), f(25.0)], "{v}");
    }
}

#[test]
fn bit_operations_divisions_and_conversions() {
    for v in BOTH {
        // Values { float4 f; uint4 u; int4 i; }
        let f4 = [3.0f32, 4.0, 1.5, 0.0];
        let u4 = [176u32, 8, 0x1_0000, 1];
        let i4 = [-7i32, 2, -16, 2];
        let o = run(
            &fixture("bits", v),
            &[],
            &[(0, vec![f4.map(f32::to_bits), u4, i4.map(|x| x as u32)])],
        );
        // countbits(176 = 0b10110000) = 3; firstbitlow(8) = 3; firstbithigh(0x10000) = 16; reversebits(1) = 0x80000000
        assert_eq!(o.values[0], [3, 3, 16, 0x8000_0000], "{v}");
        // 176 / 8 = 22, 176 % 8 = 0; -7 / 2 = -3 and -7 % 2 = -1 truncate toward zero
        assert_eq!(o.values[1], [22, 0, (-3i32) as u32, (-1i32) as u32], "{v}");
        // 3 / 4, min, max, saturate(1.5) = 1
        assert_eq!(o.values[2], [f(0.75), f(3.0), f(4.0), f(1.0)], "{v}");
        // 176 * 8, 0x10000 * 1, (int)3.0 = 3 then += (-16 >> 2) + (0x10000 << 1) + (176 * 8 + 0x10000), (uint)4.0
        let z = 3u32
            .wrapping_add((-4i32) as u32)
            .wrapping_add(0x2_0000)
            .wrapping_add(176 * 8 + 0x1_0000);
        assert_eq!(o.values[3], [1408, 0x1_0000, z, 4], "{v}");
    }
}

#[test]
fn a_vertex_program() {
    for v in BOTH {
        // Values { float4x4 world_view_projection; float4 tint; }: HLSL stores a matrix by columns, so register j holds
        // the weights of output j: out_j = dot(float4(position, 1), register j)
        let rows = vec![
            [f(1.0), f(0.0), f(2.0), f(0.5)],
            [f(0.0), f(1.0), f(0.0), f(1.0)],
            [f(0.0), f(0.0), f(1.0), f(0.0)],
            [f(0.0), f(0.0), f(0.0), f(1.0)],
            [f(0.5), f(1.0), f(2.0), f(4.0)],
        ];
        // arguments: POSITION, TEXCOORD0, SV_VertexID, SV_InstanceID
        let args = [
            [f(1.0), f(2.0), f(3.0), 0],
            [f(0.5), f(1.5), 0, 0],
            [6, 0, 0, 0],
            [4, 0, 0, 0],
        ];
        let o = run(&fixture("vertex", v), &args, &[(0, rows)]);
        // x = 1 * 1 + 3 * 2 + 0.5 = 7.5, y = 2 + 1 = 3, z = 3, w = 1
        assert_eq!(o.values[0], [f(7.5), f(3.0), f(3.0), f(1.0)], "{v}");
        // uv * (6 & 3 = 2)
        assert_eq!(o.values[1], [f(1.0), f(3.0), 0, 0], "{v}");
        // tint * (4 + 1)
        assert_eq!(o.values[2], [f(2.5), f(5.0), f(10.0), f(20.0)], "{v}");
        // (the executor returns the clip distance array member as zeros: asserted when it carries arrays)
    }
}
