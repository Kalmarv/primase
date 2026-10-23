#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The lifted fixtures run on the executor with outputs worked out by hand from the HLSL (tests/fixtures/*.hlsl), for
//! both the default and the -Od build of each (different instruction streams of one source). The evaluator cannot run
//! them (loops, switches, memory); the values below are the arithmetic of the source, not of the lifter.
use primase_dxil::lift;
use primase_shader::exec::{Backend, Element, Executable, ImageSample, Output, Resources};
use primase_shader::{Program, ResourceId, ResourceKind};
use std::path::Path;

fn f(x: f32) -> u32 {
    x.to_bits()
}

struct NoBackend;
impl Backend for NoBackend {
    fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
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

const BOTH: [&str; 3] = ["", ".od", ".66"];

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
        // the clip distance array member is returned as its elements, after the others: position.y
        assert_eq!(o.values[3], [f(2.0), 0, 0, 0], "{v}");
    }
}

mod textures {
    use super::*;
    use primase_shader::exec::{ImageLoad, ImageQuery, QueryKind, Runner};
    use std::collections::BTreeSet;

    /// Answers every request with a fixed value and records what was asked, by texture register.
    struct Recording {
        registers: Vec<u32>,
        requests: BTreeSet<String>,
    }
    impl Recording {
        fn register(&self, id: ResourceId) -> u32 {
            self.registers[id.0 as usize]
        }
    }
    impl Backend for Recording {
        fn sample(&mut self, r: ImageSample<Element>) -> Result<[f32; 4], String> {
            self.requests.insert(format!(
                "t{} {:?} coord={:?} layer={:?} lod={:?} offset={:?} gather={:?} ref={:?} depth={}",
                self.register(r.texture.id),
                r.dim,
                r.coord,
                r.layer,
                r.lod,
                r.offset,
                r.gather,
                r.depth_ref,
                r.depth
            ));
            Ok(if r.depth_ref.is_some() {
                [0.5, 0.0, 0.0, 0.0]
            } else if r.gather.is_some() {
                [0.1, 0.2, 0.3, 0.4]
            } else {
                [0.25, 0.5, 0.75, 1.0]
            })
        }
        fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
            self.requests.insert(format!(
                "load t{} {:?} texel={:?} mip={:?} storage={}",
                self.register(r.texture.id),
                r.dim,
                r.texel,
                r.mip,
                r.storage
            ));
            Ok(match self.register(r.texture.id) {
                5 => [10, 20, 30, 40],
                _ => [0.5f32.to_bits(); 4],
            })
        }
        fn query(&mut self, r: ImageQuery<Element>) -> Result<[u32; 4], String> {
            self.requests.insert(format!(
                "query t{} {:?} level={:?}",
                self.register(r.texture.id),
                r.kind,
                r.level
            ));
            Ok(match r.kind {
                QueryKind::Size => [256, 128, 1, 0],
                QueryKind::Levels => [9, 0, 0, 0],
                _ => [1, 0, 0, 0],
            })
        }
    }

    #[test]
    fn texture_operations() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("textures", v));
            let mut resources = Resources::default();
            // Values { float4 a; int4 n; }
            let cb = program
                .interface
                .resources
                .iter()
                .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
                .unwrap()
                .id;
            resources.constants.insert(
                cb,
                vec![[0.5f32, 1.5, 0.25, 0.75].map(f32::to_bits), [3u32, 4, 5, 6]],
            );
            let bound = exe.bind_resources(&resources).unwrap();
            let mut backend = Recording {
                registers: program
                    .interface
                    .resources
                    .iter()
                    .map(|r| r.provenance.d3d.unwrap().register)
                    .collect(),
                requests: BTreeSet::new(),
            };
            // a quad of four equal lanes: every screen derivative is zero. The argument is uvw = (0.25, 0.5, 0.75).
            let uvw = [[f(0.25), f(0.5), f(0.75), 0]];
            let mut runner = Runner::<4>::new(&exe, &bound, &mut backend, None);
            let out = runner
                .run([&uvw, &uvw, &uvw, &uvw])
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            let c = [0.25f32, 0.5, 0.75, 1.0];
            let three = c.map(|x| x + x + x);
            // x = Sample + SampleBias + SampleLevel, then += Load (0.5) + Sample(offset) + ddx + ddy
            let load_and_sample = c.map(|x| 0.5 + x + 0.0 + 0.0);
            let x: Vec<u32> = (0..4).map(|i| f(three[i] + load_and_sample[i])).collect();
            assert_eq!(out[0].values[0], [x[0], x[1], x[2], x[3]], "{v}");
            assert_eq!(out[0].values[1], three.map(f32::to_bits), "{v}");
            // z = SampleGrad + Gather, z.x += SampleCmp + SampleCmpLevelZero (0.5 each)
            let gather = [0.1f32, 0.2, 0.3, 0.4];
            let mut z: Vec<f32> = (0..4).map(|i| c[i] + gather[i]).collect();
            z[0] += 0.5 + 0.5;
            assert_eq!(
                out[0].values[2],
                [z[0], z[1], z[2], z[3]].map(f32::to_bits),
                "{v}"
            );
            // k = Load of the uint texture (10, 20, 30, 40) + (width 256, height 128, levels 9, 0)
            assert_eq!(out[0].values[3], [266, 148, 39, 40], "{v}");
            // what was asked: coordinates, levels, offsets, layers, references, gathers
            let asked = backend.requests;
            let has = |needle: &str| {
                assert!(
                    asked.iter().any(|r| r.contains(needle)),
                    "{v}: nothing asks {needle:?} in {asked:#?}"
                );
            };
            // in a quad an implicit level is the quad's own footprint (zero here), with the bias the program states
            let zero = "Gradients([[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]])";
            has(&format!(
                "t0 D2 coord=[0.25, 0.5, 0.0] layer=None lod={zero} offset=[0, 0, 0] gather=None ref=None"
            ));
            has(&format!(
                "t0 D2 coord=[0.25, 0.5, 0.0] layer=None lod={zero} offset=[1, -1, 0] gather=None ref=None"
            ));
            has("lod=GradientsBias([[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]], 0.5)");
            has("t0 D2 coord=[0.25, 0.5, 0.0] layer=None lod=Level(1.5)");
            has(&format!(
                "t1 D2 coord=[0.25, 0.5, 0.0] layer=Some(2.0) lod={zero}"
            ));
            has("t2 D3 coord=[0.25, 0.5, 0.75] layer=None lod=Level(1.0)");
            has("t3 Cube coord=[0.25, 0.5, 0.75] layer=None lod=Level(0.0)");
            has("t0 D2 coord=[0.25, 0.5, 0.0] layer=None lod=Level(0.0) offset=[0, 0, 0] gather=Some(0)");
            has(&format!(
                "t4 D2 coord=[0.25, 0.5, 0.0] layer=None lod={zero} offset=[0, 0, 0] gather=None ref=Some(0.25) depth=true"
            ));
            has(
                "t4 D2 coord=[0.25, 0.5, 0.0] layer=None lod=Level(0.0) offset=[0, 0, 0] gather=None ref=Some(0.75) depth=true",
            );
            has("load t5 D2 texel=[3, 4, 0] mip=Some(0)");
            has("load t0 D2 texel=[5, 6, 0] mip=Some(0)");
            has("query t0 Size level=Some(0)");
        }
    }
}

mod resources {
    use super::*;
    use primase_shader::exec::{ImageLoad, ImageStore, Runner};

    #[derive(Default)]
    struct Views {
        loads: Vec<String>,
        stores: Vec<ImageStore<ResourceId>>,
    }
    impl Backend for Views {
        fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
            Err("no sampling".into())
        }
        fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
            self.loads
                .push(format!("{:?} storage={} texel={:?}", r.dim, r.storage, r.texel));
            Ok([0.5f32.to_bits(); 4])
        }
        fn store_image(&mut self, r: ImageStore<ResourceId>) -> Result<(), String> {
            self.stores.push(r);
            Ok(())
        }
    }

    #[test]
    fn buffer_views_and_an_unordered_access_texture() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("resources", v));
            let by_register = |register: u32, class: primase_shader::D3dClass| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| {
                        r.provenance
                            .d3d
                            .is_some_and(|d| d.register == register && d.class == class)
                    })
                    .unwrap_or_else(|| panic!("{v}: no {class:?} {register}"))
                    .id
            };
            let mut resources = Resources::default();
            // Values { int4 n; }: n = (2, 1, 3, 4)
            resources.constants.insert(
                by_register(0, primase_shader::D3dClass::ConstantBuffer),
                vec![[2u32, 1, 3, 4]],
            );
            // t0 StructuredBuffer<float4>: element e lane l is 10e + l; t2 Buffer<float4>: 100 + 10e + l; t1 raw: 1000 + word
            let sb: Vec<u32> = (0..16).map(|w| f((w / 4 * 10 + w % 4) as f32)).collect();
            let typed: Vec<u32> = (0..16).map(|w| f((100 + w / 4 * 10 + w % 4) as f32)).collect();
            let raw: Vec<u32> = (0..16).map(|w| 1000 + w).collect();
            resources
                .buffers
                .insert(by_register(0, primase_shader::D3dClass::ShaderResource), sb);
            resources
                .buffers
                .insert(by_register(1, primase_shader::D3dClass::ShaderResource), raw);
            resources
                .buffers
                .insert(by_register(2, primase_shader::D3dClass::ShaderResource), typed);
            let bound = exe.bind_resources(&resources).unwrap();
            let mut backend = Views::default();
            let mut runner = Runner::<1>::new(&exe, &bound, &mut backend, None);
            // the argument is SV_Position = (3.75, 4.25, 0.5, 1)
            let [out] = runner
                .run([&[[f(3.75), f(4.25), f(0.5), f(1.0)]]])
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            // x = sb[2] + typed[1] + the view's texel at n.zw (0.5)
            assert_eq!(out.values[0], [f(130.5), f(132.5), f(134.5), f(136.5)], "{v}");
            // k = raw.Load4(n.x * 16 = 32): words 8 to 11
            assert_eq!(out.values[1], [1008, 1009, 1010, 1011], "{v}");
            assert_eq!(backend.loads, ["D2 storage=true texel=[3, 4, 0]"], "{v}");
            // image[uint2(position.xy)] = x: texel (3, 4)
            let [store] = backend.stores.as_slice() else {
                panic!("{v}: {:?}", backend.stores)
            };
            assert_eq!(store.texel, [3, 4, 0], "{v}");
            assert_eq!(store.value, [f(130.5), f(132.5), f(134.5), f(136.5)], "{v}");
        }
    }
}
