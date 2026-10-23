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

mod resource_arrays {
    use super::*;
    use primase_shader::exec::{ImageLoad, ImageQuery, QueryKind, Runner};

    /// The textures of `resource_arrays.hlsl` answer with their element: a sample is (texture, sampler, 0.25, 1), a
    /// load 100 + the texture's element, a size 2 << element wide and 1 << element high; the single texture is 32 by 64.
    struct Elements {
        /// The texture array's resource id.
        array: ResourceId,
    }
    impl Backend for Elements {
        fn sample(&mut self, r: ImageSample<Element>) -> Result<[f32; 4], String> {
            assert_eq!(r.texture.id, self.array);
            Ok([r.texture.index as f32, r.sampler.index as f32, 0.25, 1.0])
        }
        fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
            assert_eq!((r.texture.id, r.texel, r.mip), (self.array, [1, 2, 0], Some(0)));
            Ok([f(100.0 + r.texture.index as f32); 4])
        }
        fn query(&mut self, r: ImageQuery<Element>) -> Result<[u32; 4], String> {
            assert_eq!(r.kind, QueryKind::Size);
            Ok(if r.texture.id == self.array {
                [2 << r.texture.index, 1 << r.texture.index, 1, 0]
            } else {
                [32, 64, 1, 0]
            })
        }
    }

    fn run(variant: &str, which: [u32; 2]) -> Result<Output, String> {
        let (program, exe) = lifted(&fixture("resource_arrays", variant));
        program.validate().unwrap();
        let find = |register: u32, class: primase_shader::D3dClass| {
            program
                .interface
                .resources
                .iter()
                .find(|r| {
                    r.provenance
                        .d3d
                        .is_some_and(|d| d.register == register && d.class == class)
                })
                .unwrap_or_else(|| panic!("{variant}: no {class:?} {register}"))
        };
        // the arrays are arrays: four textures, and samplers the program does not size
        let tex = find(0, primase_shader::D3dClass::ShaderResource);
        assert_eq!(tex.count, 4, "{variant}");
        let smp = find(0, primase_shader::D3dClass::Sampler);
        assert!(smp.is_unbounded(), "{variant}");
        assert_eq!(
            find(8, primase_shader::D3dClass::ShaderResource).count,
            1,
            "{variant}"
        );
        let mut resources = Resources::default();
        // Select { uint2 which; float2 uv; }
        let cb = find(0, primase_shader::D3dClass::ConstantBuffer).id;
        resources
            .constants
            .insert(cb, vec![[which[0], which[1], f(0.25), f(0.75)]]);
        let bound = exe.bind_resources(&resources).unwrap();
        let mut backend = Elements { array: tex.id };
        let mut runner = Runner::<1>::new(&exe, &bound, &mut backend, None);
        let [out] = runner.run([&[[f(0.5), f(0.5), 0, f(1.0)]]])?;
        Ok(out)
    }

    /// which = (1, 3): Target0 samples texture 1 with sampler 3; Target1 samples texture 2 with sampler 1 and adds a
    /// load of texture 2 (100 + 2); Target2 is texture 1's size (4 by 2) and the single texture's (32 by 64).
    #[test]
    fn a_computed_index_picks_the_texture_and_the_sampler() {
        for v in BOTH {
            let o = run(v, [1, 3]).unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(o.values[0], [1.0, 3.0, 0.25, 1.0].map(f32::to_bits), "{v}");
            assert_eq!(
                o.values[1],
                [104.0, 103.0, 102.25, 103.0].map(f32::to_bits),
                "{v}"
            );
            assert_eq!(o.values[2], [4.0, 2.0, 32.0, 64.0].map(f32::to_bits), "{v}");
        }
    }

    /// Another index picks other elements; one past the array's four textures stops the run naming the resource.
    #[test]
    fn another_index_picks_other_elements_and_one_past_the_array_is_an_error() {
        for v in BOTH {
            let o = run(v, [0, 7]).unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(o.values[0], [0.0, 7.0, 0.25, 1.0].map(f32::to_bits), "{v}");
            assert_eq!(
                o.values[1],
                [103.0, 102.0, 101.25, 102.0].map(f32::to_bits),
                "{v}"
            );
            assert_eq!(o.values[2], [2.0, 1.0, 32.0, 64.0].map(f32::to_bits), "{v}");
            let error = run(v, [4, 0]).unwrap_err();
            assert!(error.contains("element 4 of an array of 4"), "{v}: {error}");
        }
    }
}

mod pixel_ops {
    use super::*;
    use primase_shader::exec::{ImageLoad, ImageQuery, QueryKind, Runner};

    /// A load answers its texel as floats; a query of the sample count answers 4.
    struct Texels;
    impl Backend for Texels {
        fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
            Err("no sampling in this fixture".into())
        }
        fn load(&mut self, r: ImageLoad<Element>) -> Result<[u32; 4], String> {
            assert_eq!(r.mip, Some(0));
            Ok([f(r.texel[0] as f32), f(r.texel[1] as f32), 0, f(1.0)])
        }
        fn query(&mut self, r: ImageQuery<Element>) -> Result<[u32; 4], String> {
            assert_eq!(r.kind, QueryKind::Samples);
            Ok([4, 0, 0, 0])
        }
    }

    fn run(variant: &str, sample: u32, h: f32, packed: u32) -> Output {
        let (program, exe) = lifted(&fixture("pixel_ops", variant));
        program.validate().unwrap();
        // the sample index is an input the signature does not list (the position is not read, so it is not kept)
        let names: Vec<_> = program
            .interface
            .inputs
            .iter()
            .map(|i| i.label.name.as_str())
            .collect();
        assert_eq!(names, ["SV_SampleIndex"], "{variant}");
        let cb = program
            .interface
            .resources
            .iter()
            .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
            .unwrap()
            .id;
        let mut resources = Resources::default();
        // Values { float h; uint packed; }
        resources.constants.insert(cb, vec![[f(h), packed, 0, 0]]);
        let bound = exe.bind_resources(&resources).unwrap();
        let mut backend = Texels;
        let mut runner = Runner::<1>::new(&exe, &bound, &mut backend, None);
        let [out] = runner
            .run([&[[sample, 0, 0, 0]]])
            .unwrap_or_else(|e| panic!("{variant}: {e}"));
        out
    }

    /// The offset load reads texel (5, 6) moved by (-2, 3): (3, 9). The four-sample positions are D3D's standard
    /// pattern in sixteenths; f16tof32(0x4000) is 2 and f32tof16(1.5) is 0x3e00 = 15872.
    #[test]
    fn the_sample_index_its_position_the_offset_load_and_the_half_conversions() {
        let positions = [(-2.0, -6.0), (6.0, -2.0), (-6.0, 2.0), (2.0, 6.0)];
        for v in BOTH {
            for (sample, (x, y)) in positions.iter().enumerate() {
                let o = run(v, sample as u32, 1.5, 0x4000);
                assert_eq!(o.values[0], [f(3.0), f(9.0), 0, f(1.0)], "{v} sample {sample}");
                assert_eq!(
                    o.values[1],
                    [x / 16.0, y / 16.0, 2.0, 15872.0].map(f32::to_bits),
                    "{v} sample {sample}"
                );
            }
        }
    }
}

mod compute {
    use super::*;
    use primase_shader::D3dClass;
    use std::collections::BTreeMap;

    /// `compute_basic.hlsl` dispatched as (2, 2, 1) groups of 8 by 4 by 2 with scale 0xff. Per group g = gx + 2 gy
    /// and invocation index idx = local.x + 8 local.y + 32 local.z: the group's counter is the sum of the 64
    /// indices (2016); words[0] counts the 256 invocations; words[1 + idx + 64 g] = tile[63 - idx] + 2016, where
    /// tile[i] = id.x * 100 + id.y * 10 + id.z of the group's invocation i; the raw word idx + 64 g is
    /// tile[idx] ^ 0xff; the typed element idx + 64 g is (local.x, local.y, local.z, idx).
    #[test]
    fn a_dispatch_of_groups_with_shared_memory_barriers_and_atomics() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("compute_basic", v));
            assert_eq!(exe.workgroup_size(), Some([8, 4, 2]), "{v}");
            let find = |class: D3dClass, register: u32| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| {
                        r.provenance
                            .d3d
                            .is_some_and(|d| d.class == class && d.register == register)
                    })
                    .unwrap_or_else(|| panic!("{v}: no {class:?} {register}"))
                    .id
            };
            let (raw, words, typed) = (
                find(D3dClass::UnorderedAccess, 0),
                find(D3dClass::UnorderedAccess, 1),
                find(D3dClass::UnorderedAccess, 2),
            );
            let mut buffers = BTreeMap::from([
                (raw, vec![0u32; 256]),
                (words, vec![0u32; 257]),
                (typed, vec![0u32; 1024]),
            ]);
            let mut resources = Resources::default();
            resources
                .constants
                .insert(find(D3dClass::ConstantBuffer, 0), vec![[0xff, 0, 0, 0]]);
            let bound = exe.bind_resources(&resources).unwrap();
            let done = exe
                .dispatch(&bound, &mut NoBackend, &mut buffers, [2, 2, 1], 32)
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(done.groups, 4, "{v}");
            assert_eq!(buffers[&words][0], 256, "{v}");
            for g in 0..4u32 {
                let (gx, gy) = (g % 2, g / 2);
                let tile = |i: u32| {
                    let (lx, ly, lz) = (i % 8, (i / 8) % 4, i / 32);
                    (gx * 8 + lx) * 100 + (gy * 4 + ly) * 10 + lz
                };
                for idx in 0..64u32 {
                    let at = (idx + 64 * g) as usize;
                    assert_eq!(
                        buffers[&words][1 + at],
                        tile(63 - idx) + 2016,
                        "{v} words {g} {idx}"
                    );
                    assert_eq!(buffers[&raw][at], tile(idx) ^ 0xff, "{v} raw {g} {idx}");
                    let local = [idx % 8, (idx / 8) % 4, idx / 32, idx].map(|x| f(x as f32));
                    assert_eq!(buffers[&typed][at * 4..at * 4 + 4], local, "{v} typed {g} {idx}");
                }
            }
        }
    }
}

mod waves {
    use super::*;
    use primase_shader::D3dClass;
    use std::collections::BTreeMap;

    /// `waves.hlsl`: 64 invocations in one group, 13 result words each, on waves of 32 lanes (invocation i is lane
    /// i % 32 of wave i / 32). The values are the arithmetic of the source.
    #[test]
    fn wave_operations_over_waves_of_thirty_two_lanes() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("waves", v));
            assert_eq!(exe.workgroup_size(), Some([64, 1, 1]), "{v}");
            let results = program
                .interface
                .resources
                .iter()
                .find(|r| {
                    r.provenance
                        .d3d
                        .is_some_and(|d| d.class == D3dClass::UnorderedAccess && d.register == 0)
                })
                .unwrap()
                .id;
            let mut buffers = BTreeMap::from([(results, vec![0u32; 64 * 13])]);
            let resources = Resources::default();
            let bound = exe.bind_resources(&resources).unwrap();
            exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            let words = &buffers[&results];
            for i in 0..64u32 {
                let (lane, wave) = (i % 32, i / 32);
                let at = (i * 13) as usize;
                let got = &words[at..at + 13];
                let want = [
                    lane,
                    if wave == 0 { 496 } else { 1520 },
                    wave * 32,
                    wave * 32 + 31,
                    0xffff_ffff,
                    u32::from(wave == 0),
                    u32::from(wave == 1),
                    0x5555_5555,
                    lane * lane.saturating_sub(1) / 2 + 32 * wave * lane,
                    wave * 32,
                    wave * 32 + 5,
                    i ^ 1,
                    u32::from(lane == 0),
                ];
                assert_eq!(got, want, "{v} invocation {i}");
            }
        }
    }
}

mod stage_functions {
    use super::*;
    use primase_shader::exec::Runner;
    use primase_shader::{D3dClass, stage_functions};
    use std::collections::BTreeMap;

    /// Run function `name` of the lifted program `base` once, its constant buffer 0 `constants`, giving each argument
    /// the registers `give` says for its name (an array argument one register per element); the result members by
    /// name.
    fn call(
        base: &str,
        variant: &str,
        name: &str,
        constants: &[[u32; 4]],
        give: &dyn Fn(&str) -> Option<Vec<[u32; 4]>>,
    ) -> BTreeMap<String, [u32; 4]> {
        let bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(format!("tessellation.{base}{variant}.dxil")),
        )
        .unwrap();
        let program = lift(&bytes).unwrap_or_else(|e| panic!("{base}{variant}: {e}"));
        let sigs = stage_functions(&program.interface).unwrap().unwrap();
        let sig = sigs.iter().find(|s| s.name == name).unwrap();
        let exe = Executable::function(&program, name).unwrap_or_else(|e| panic!("{base}{variant}: {e}"));
        let mut registers = Vec::new();
        for argument in &sig.arguments {
            let given = give(&argument.name).unwrap_or_else(|| {
                panic!(
                    "{base}{variant}: no value for argument {} of {name}",
                    argument.name
                )
            });
            registers.extend(given);
        }
        let mut resources = Resources::default();
        if let Some(cb) = program.interface.resources.iter().find(|r| {
            r.provenance
                .d3d
                .is_some_and(|d| d.class == D3dClass::ConstantBuffer && d.register == 0)
        }) {
            resources.constants.insert(cb.id, constants.to_vec());
        }
        let bound = exe.bind_resources(&resources).unwrap();
        let mut backend = NoBackend;
        let mut runner = Runner::<1>::new(&exe, &bound, &mut backend, None);
        let [out] = runner
            .run([&registers])
            .unwrap_or_else(|e| panic!("{base}{variant} {name}: {e}"));
        assert_eq!(out.values.len(), sig.result.len(), "{base}{variant} {name}");
        sig.result
            .iter()
            .zip(out.values)
            .map(|(item, v)| (item.name.clone(), v))
            .collect()
    }

    fn floats(x: [f32; 4]) -> [u32; 4] {
        x.map(f32::to_bits)
    }
    fn identity_and_scale(scale: f32) -> Vec<[u32; 4]> {
        vec![
            floats([1.0, 0.0, 0.0, 0.0]),
            floats([0.0, 1.0, 0.0, 0.0]),
            floats([0.0, 0.0, 1.0, 0.0]),
            floats([0.0, 0.0, 0.0, 1.0]),
            floats([scale, 0.0, 0.0, 0.0]),
        ]
    }
    /// The control points of the patch the tests use.
    const POSITIONS: [[f32; 4]; 3] = [[1.0, 2.0, 3.0, 0.0], [4.0, 5.0, 6.0, 0.0], [7.0, 8.0, 9.0, 0.0]];
    const COLOURS: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]];
    fn points(of: &[[f32; 4]; 3]) -> Vec<[u32; 4]> {
        of.map(floats).to_vec()
    }
    fn lanes(v: [u32; 4], n: usize) -> Vec<f32> {
        v[..n].iter().map(|b| f32::from_bits(*b)).collect()
    }

    /// The vertex program scales the position (cbuffer scale 2) and passes the colour on.
    #[test]
    fn a_vertex_program_without_a_position() {
        for v in ["", ".od"] {
            let out = call("vs", v, "main", &identity_and_scale(2.0), &|n| match n {
                "POSITION0" => Some(vec![floats([1.0, 2.0, 3.0, 0.0])]),
                "COLOR0" => Some(vec![floats([0.25, 0.5, 0.75, 1.0])]),
                _ => None,
            });
            assert_eq!(lanes(out["CTRL_POSITION0"], 3), [2.0, 4.0, 6.0], "{v}");
            assert_eq!(lanes(out["CTRL_COLOR0"], 4), [0.25, 0.5, 0.75, 1.0], "{v}");
        }
    }

    /// The control-point phase of the hull program gives point i its own position moved by i along z.
    #[test]
    fn the_hull_programs_control_point_phase_runs_per_output_point() {
        for v in ["", ".od"] {
            for i in 0..3u32 {
                let out = call("hs", v, "main", &[], &|n| match n {
                    "CTRL_POSITION0" => Some(points(&POSITIONS)),
                    "CTRL_COLOR0" => Some(points(&COLOURS)),
                    "SV_OutputControlPointID" => Some(vec![[i, 0, 0, 0]]),
                    _ => None,
                });
                let mut want = POSITIONS[i as usize];
                want[2] += i as f32;
                assert_eq!(lanes(out["CTRL_POSITION0"], 3), want[..3], "{v} point {i}");
                assert_eq!(lanes(out["CTRL_COLOR0"], 4), COLOURS[i as usize], "{v} point {i}");
            }
        }
    }

    /// The patch-constant phase: edges 2, 3 and 4 + the primitive id, inside 5, the centre of the three points.
    #[test]
    fn the_hull_programs_patch_constant_phase() {
        for v in ["", ".od"] {
            let out = call("hs", v, "patch_constant", &[], &|n| match n {
                "CTRL_POSITION0" | "out_CTRL_POSITION0" => Some(points(&POSITIONS)),
                "CTRL_COLOR0" | "out_CTRL_COLOR0" => Some(points(&COLOURS)),
                "SV_PrimitiveID" => Some(vec![[7, 0, 0, 0]]),
                _ => None,
            });
            let edge = |k: usize| f32::from_bits(out[&format!("SV_TessFactor{k}")][0]);
            assert_eq!([edge(0), edge(1), edge(2)], [2.0, 3.0, 11.0], "{v}");
            assert_eq!(f32::from_bits(out["SV_InsideTessFactor0"][0]), 5.0, "{v}");
            assert_eq!(lanes(out["CENTRE0"], 3), [4.0, 5.0, 6.0], "{v}");
        }
    }

    /// The domain program places a vertex at (0.5, 0.25, 0.25) of the patch: the weighted points plus the centre
    /// halved, a quarter of the first edge factor and an eighth of the inside factor; under an identity matrix.
    #[test]
    fn the_domain_program_places_a_vertex() {
        for v in ["", ".od"] {
            let out = call("ds", v, "main", &identity_and_scale(1.0), &|n| match n {
                "SV_TessFactor0" => Some(vec![floats([2.0, 0.0, 0.0, 0.0])]),
                "SV_TessFactor1" => Some(vec![floats([3.0, 0.0, 0.0, 0.0])]),
                "SV_TessFactor2" => Some(vec![floats([4.0, 0.0, 0.0, 0.0])]),
                "SV_InsideTessFactor0" => Some(vec![floats([5.0, 0.0, 0.0, 0.0])]),
                "CENTRE0" => Some(vec![floats([4.0, 5.0, 6.0, 0.0])]),
                "CTRL_POSITION0" => Some(points(&POSITIONS)),
                "CTRL_COLOR0" => Some(points(&COLOURS)),
                "SV_DomainLocation" => Some(vec![floats([0.5, 0.25, 0.25, 0.0])]),
                "SV_PrimitiveID" => Some(vec![[0, 0, 0, 0]]),
                _ => None,
            });
            // x: 0.5 + 1 + 1.75 + 2 + 0.5 + 0.625; y: 1 + 1.25 + 2 + 2.5 + 0.5 + 0.625; z: 1.5 + 1.5 + 2.25 + 3 + 1.125
            assert_eq!(lanes(out["SV_Position0"], 4), [6.375, 7.875, 9.375, 1.0], "{v}");
            assert_eq!(lanes(out["COLOR0"], 4), [0.5, 0.25, 0.25, 1.0], "{v}");
        }
    }
}

mod half_ops {
    use super::*;

    /// 1 + 2^-10, a half, and 0.333251953125, the half nearest a third, as f32 bits.
    const ONE_AND_A_BIT: u32 = 0x3f80_2000;
    const THIRD_AS_HALF: u32 = 0x3eaa_a000;

    /// `half_ops.hlsl` with a = 1 + 2^-10 and b = 1: x + y = 2.0009765625 is half-way between the halves 2 and
    /// 2.001953125 and rounds to the even one; x * y is exact; 1 - 2 in 16 bits wraps to 65535; 1/3 as a half is
    /// 0.333251953125.
    #[test]
    fn halves_round_to_nearest_even_and_sixteen_bit_integers_wrap() {
        for v in ["", ".od"] {
            let o = run(
                &fixture("half_ops", v),
                &[[f(0.5), f(0.5), f(0.5), f(1.0)]],
                &[(0, vec![[ONE_AND_A_BIT, f(1.0), 0, 0]])],
            );
            assert_eq!(
                o.values[0],
                [f(2.0), ONE_AND_A_BIT, f(65535.0), THIRD_AS_HALF],
                "{v}"
            );
        }
    }
}

mod image_atomics {
    use super::*;
    use primase_shader::D3dClass;
    use primase_shader::exec::{ImageAtomic, ImageAtomicOp};
    use std::collections::BTreeMap;

    /// Two textures that apply the atomic operations they are asked for: the words of each by texel, absent ones 0
    /// except where the test presets them.
    struct Textures {
        words: BTreeMap<(ResourceId, [u32; 3]), u32>,
        requests: usize,
    }
    impl Backend for Textures {
        fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
            Err("no sampling in this fixture".into())
        }
        fn atomic_image(&mut self, r: ImageAtomic<ResourceId>) -> Result<(), String> {
            self.requests += 1;
            let word = self.words.entry((r.view, r.texel)).or_insert(0);
            *word = match (r.op, r.signed) {
                (ImageAtomicOp::Add, _) => word.wrapping_add(r.value),
                (ImageAtomicOp::Sub, _) => word.wrapping_sub(r.value),
                (ImageAtomicOp::And, _) => *word & r.value,
                (ImageAtomicOp::Or, _) => *word | r.value,
                (ImageAtomicOp::Xor, _) => *word ^ r.value,
                (ImageAtomicOp::Min, true) => (*word as i32).min(r.value as i32) as u32,
                (ImageAtomicOp::Max, true) => (*word as i32).max(r.value as i32) as u32,
                (ImageAtomicOp::Min, false) => (*word).min(r.value),
                (ImageAtomicOp::Max, false) => (*word).max(r.value),
                (ImageAtomicOp::Exchange, _) => r.value,
            };
            Ok(())
        }
    }

    /// `image_atomics.hlsl`: thread (x, y) of an 8 by 8 group, at = (x % 4, y % 4), adds x + 1 to counts[at], maxes
    /// x * 10 + y into counts[at + (0, 4)], mins it into counts[at + (4, 0)] (preset to all ones) and signed-maxes
    /// x - 4 into signed_counts[at]. Per cell, the two threads of each coordinate pair that map to it give:
    /// the sum 4 cx + 12, the maximum (cx + 4) * 10 + cy + 4, the minimum cx * 10 + cy and the signed maximum cx.
    #[test]
    fn atomics_on_textures_reach_the_backend_in_every_build() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("image_atomics", v));
            let uav = |register: u32| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| {
                        r.provenance
                            .d3d
                            .is_some_and(|d| d.class == D3dClass::UnorderedAccess && d.register == register)
                    })
                    .unwrap_or_else(|| panic!("{v}: no u{register}"))
                    .id
            };
            let (counts, signed) = (uav(0), uav(1));
            let mut backend = Textures {
                words: BTreeMap::new(),
                requests: 0,
            };
            for cx in 0..4 {
                for cy in 0..4 {
                    backend.words.insert((counts, [cx + 4, cy, 0]), u32::MAX);
                    backend.words.insert((signed, [cx, cy, 0]), i32::MIN as u32);
                }
            }
            let resources = Resources::default();
            let bound = exe.bind_resources(&resources).unwrap();
            exe.dispatch(&bound, &mut backend, &mut BTreeMap::new(), [1, 1, 1], 32)
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(backend.requests, 64 * 4, "{v}");
            for cx in 0..4u32 {
                for cy in 0..4u32 {
                    let at = |view, x, y| backend.words[&(view, [x, y, 0])];
                    assert_eq!(at(counts, cx, cy), 4 * cx + 12, "{v} add {cx} {cy}");
                    assert_eq!(
                        at(counts, cx, cy + 4),
                        (cx + 4) * 10 + cy + 4,
                        "{v} max {cx} {cy}"
                    );
                    assert_eq!(at(counts, cx + 4, cy), cx * 10 + cy, "{v} min {cx} {cy}");
                    assert_eq!(at(signed, cx, cy), cx, "{v} signed max {cx} {cy}");
                }
            }
        }
    }
}

mod buffer_arrays {
    use super::*;
    use primase_shader::D3dClass;
    use std::collections::BTreeMap;

    /// `buffer_arrays.hlsl`: one group of 4 with which = 1. Thread i writes bufs[1][i] = n1 * 100 + i, where n1 is the
    /// word count of bufs[1] (6 here); adds i + 1 to bufs[2][0] (10 in all); and copies raws[1][i] to bufs[2][4 + i].
    #[test]
    fn arrays_of_buffers_are_read_written_measured_and_added_to() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("buffer_arrays", v));
            let find = |class: D3dClass, register: u32| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| {
                        r.provenance
                            .d3d
                            .is_some_and(|d| d.class == class && d.register == register)
                    })
                    .unwrap_or_else(|| panic!("{v}: no {class:?} {register}"))
                    .id
            };
            let (bufs, raws, select) = (
                find(D3dClass::UnorderedAccess, 0),
                find(D3dClass::ShaderResource, 0),
                find(D3dClass::ConstantBuffer, 0),
            );
            let mut elements = BTreeMap::from([
                ((bufs, 0), vec![0u32; 4]),
                ((bufs, 1), vec![0u32; 6]),
                ((bufs, 2), vec![0u32; 8]),
                ((raws, 0), vec![10, 20, 30, 40]),
                ((raws, 1), vec![11, 21, 31, 41]),
            ]);
            let mut resources = Resources::default();
            resources.constants.insert(select, vec![[1, 0, 0, 0]]);
            let bound = exe.bind_resources(&resources).unwrap();
            exe.dispatch_elements(
                &bound,
                &mut NoBackend,
                &mut BTreeMap::new(),
                &mut elements,
                [1, 1, 1],
                32,
            )
            .unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(elements[&(bufs, 0)], [0; 4], "{v}");
            assert_eq!(elements[&(bufs, 1)], [600, 601, 602, 603, 0, 0], "{v}");
            assert_eq!(elements[&(bufs, 2)], [10, 0, 0, 0, 11, 21, 31, 41], "{v}");
        }
    }
}

mod image_arrays {
    use super::*;
    use primase_shader::D3dClass;
    use primase_shader::exec::ImageStore;
    use std::collections::BTreeMap;

    /// Keeps the stores by view and element, and by texel.
    struct Views {
        stores: BTreeMap<(ResourceId, u32, [u32; 3]), [u32; 4]>,
    }
    impl Backend for Views {
        fn sample(&mut self, _: ImageSample<Element>) -> Result<[f32; 4], String> {
            Err("no sampling in this fixture".into())
        }
        fn store_image(&mut self, r: ImageStore<ResourceId>) -> Result<(), String> {
            assert_eq!(r.lanes, 4);
            self.stores.insert((r.view, r.index, r.texel), r.value);
            Ok(())
        }
    }

    /// `image_arrays.hlsl`: thread (x, y) of an 8 by 8 group, group index g = x + 8 y, writes (x, y, g, 1) to texel
    /// (x, y) of imgs[g & 3].
    #[test]
    fn stores_reach_the_element_of_an_array_of_images_the_program_computes() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("image_arrays", v));
            let imgs = program
                .interface
                .resources
                .iter()
                .find(|r| {
                    r.provenance
                        .d3d
                        .is_some_and(|d| d.class == D3dClass::UnorderedAccess && d.register == 0)
                })
                .unwrap_or_else(|| panic!("{v}: no u0"))
                .id;
            let mut backend = Views {
                stores: BTreeMap::new(),
            };
            let resources = Resources::default();
            let bound = exe.bind_resources(&resources).unwrap();
            exe.dispatch(&bound, &mut backend, &mut BTreeMap::new(), [1, 1, 1], 32)
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(backend.stores.len(), 64, "{v}");
            for y in 0..8u32 {
                for x in 0..8u32 {
                    let g = x + 8 * y;
                    let want = [x as f32, y as f32, g as f32, 1.0].map(f32::to_bits);
                    assert_eq!(backend.stores[&(imgs, g & 3, [x, y, 0])], want, "{v} ({x}, {y})");
                }
            }
        }
    }
}

mod wide_ints {
    use super::*;
    use primase_shader::D3dClass;
    use std::collections::BTreeMap;

    /// The 32 words `wide_ints.hlsl` writes for a, b and n: thirteen 64-bit results as low then high word (a + b, a - b,
    /// a * b, a & b, a | b, a ^ b, a << n, a >> n logical and arithmetic, a / b and a % b unsigned and signed), then
    /// a < b, a < b signed, a == b, countbits(a), firstbitlow(a) and firstbithigh(a) (the bit's position from the least
    /// significant end, all ones for none): the arithmetic of the source, worked out on the machine's own 64-bit integers.
    fn expected(a: u64, b: u64, n: u32) -> Vec<u32> {
        let (sa, sb) = (a as i64, b as i64);
        let wide = [
            a.wrapping_add(b),
            a.wrapping_sub(b),
            a.wrapping_mul(b),
            a & b,
            a | b,
            a ^ b,
            a << n,
            a >> n,
            (sa >> n) as u64,
            a / b,
            a % b,
            sa.wrapping_div(sb) as u64,
            sa.wrapping_rem(sb) as u64,
        ];
        let mut words = Vec::new();
        for w in wide {
            words.push(w as u32);
            words.push((w >> 32) as u32);
        }
        words.push(u32::from(a < b));
        words.push(u32::from(sa < sb));
        words.push(u32::from(a == b));
        words.push(a.count_ones());
        words.push(if a == 0 { u32::MAX } else { a.trailing_zeros() });
        words.push(if a == 0 { u32::MAX } else { 63 - a.leading_zeros() });
        words
    }

    /// Each case runs in every build: values with bits in both halves, carries and borrows across the halves, shifts
    /// below, at and above 32, and negative operands for the signed results.
    #[test]
    fn sixty_four_bit_integers_agree_with_the_machines() {
        let cases: [(u64, u64, u32); 6] = [
            (0x0000_0003_8000_0001, 0xffff_ffff_ffff_fff9, 33),
            (0x8000_0000_0000_0005, 1000, 3),
            (0xffff_ffff_0000_0000, 0x0000_0001_ffff_ffff, 32),
            (0x1234_5678_9abc_def0, 0x0fed_cba9_8765_4321, 1),
            (u64::MAX - 1, 3, 63),
            (7, 0x1_0000_0000, 0),
        ];
        for v in BOTH {
            let (program, exe) = lifted(&fixture("wide_ints", v));
            let find = |class: D3dClass| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| {
                        r.provenance
                            .d3d
                            .is_some_and(|d| d.class == class && d.register == 0)
                    })
                    .unwrap_or_else(|| panic!("{v}: no {class:?} 0"))
                    .id
            };
            let (results, constants) = (find(D3dClass::UnorderedAccess), find(D3dClass::ConstantBuffer));
            for (a, b, n) in cases {
                let mut resources = Resources::default();
                resources.constants.insert(
                    constants,
                    vec![
                        [a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32],
                        [n, 0, 0, 0],
                    ],
                );
                let bound = exe.bind_resources(&resources).unwrap();
                let mut buffers = BTreeMap::from([(results, vec![0u32; 32])]);
                exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
                    .unwrap_or_else(|e| panic!("{v}: {e}"));
                assert_eq!(
                    buffers[&results],
                    expected(a, b, n),
                    "{v}: a {a:#x} b {b:#x} n {n}"
                );
            }
        }
    }
}

mod lane_2d {
    use super::*;
    use primase_shader::D3dClass;
    use std::collections::BTreeMap;

    /// `lane_2d.hlsl`: 64 invocations in an 8 by 8 group, on waves of 32 lanes (invocation i, the group's flattened
    /// index, is lane i % 32 of wave i / 32). Each writes its lane, the lanes active with it (32) and the flattened index
    /// of its wave's first lane (32 times the wave). WebGPU has no lane number for a group laid out in two dimensions,
    /// so the lifted program makes it as the rank among the active invocations when it starts.
    #[test]
    fn a_wave_lane_in_a_group_of_two_dimensions() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("lane_2d", v));
            assert_eq!(exe.workgroup_size(), Some([8, 8, 1]), "{v}");
            assert!(
                program
                    .notes
                    .iter()
                    .any(|n| n.contains("more than one dimension")),
                "{v}: {:?}",
                program.notes
            );
            let results = program
                .interface
                .resources
                .iter()
                .find(|r| {
                    r.provenance
                        .d3d
                        .is_some_and(|d| d.class == D3dClass::UnorderedAccess && d.register == 0)
                })
                .unwrap()
                .id;
            let mut buffers = BTreeMap::from([(results, vec![0u32; 64 * 3])]);
            let resources = Resources::default();
            let bound = exe.bind_resources(&resources).unwrap();
            exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            for i in 0..64u32 {
                let at = (i * 3) as usize;
                assert_eq!(
                    buffers[&results][at..at + 3],
                    [i % 32, 32, (i / 32) * 32],
                    "{v} invocation {i}"
                );
            }
        }
    }
}

mod ray_queries {
    use super::*;
    use primase_shader::D3dClass;
    use primase_shader::exec::{Geometry, Instance, InstanceFlags, Scene};
    use std::collections::BTreeMap;

    /// Two bottom-level geometries in one bottom level, the triangle (0,0), (4,0), (0,4) at z = 5 (opaque, geometry 0)
    /// and at z = 2 (not opaque, geometry 1); instance 0 places it as it is (id 7, contribution 3, mask 0xff) and
    /// instance 1 moves it 10 along x (id 9, contribution 5, mask 0x02).
    fn scene() -> Scene {
        let triangle = |z: f32, opaque: bool| Geometry {
            positions: vec![[0.0, 0.0, z], [4.0, 0.0, z], [0.0, 4.0, z]],
            boxes: Vec::new(),
            indices: None,
            opaque,
        };
        let moved = [[1.0, 0.0, 0.0, 10.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let placed = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        Scene {
            bottoms: vec![vec![triangle(5.0, true), triangle(2.0, false)]],
            instances: vec![
                Instance {
                    bottom: 0,
                    transform: placed,
                    id: 7,
                    mask: 0xff,
                    contribution: 3,
                    flags: InstanceFlags::default(),
                },
                Instance {
                    bottom: 0,
                    transform: moved,
                    id: 9,
                    mask: 0x02,
                    contribution: 5,
                    flags: InstanceFlags::default(),
                },
            ],
        }
    }

    /// `ray_query_hits.hlsl` on that scene. Per thread: (origin, direction, flags, mask) and the committed hit.
    #[test]
    fn rays_find_the_closest_committed_hit_of_the_scene() {
        for v in ["", ".od"] {
            let (program, exe) = lifted(&fixture("ray_query_hits", v));
            let find = |class: Option<D3dClass>| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| match class {
                        Some(c) => r.provenance.d3d.is_some_and(|d| d.class == c && d.register == 0),
                        None => matches!(r.kind, ResourceKind::AccelerationStructure),
                    })
                    .unwrap_or_else(|| panic!("{v}: resource missing"))
                    .id
            };
            let (structure, results, rays) = (
                find(None),
                find(Some(D3dClass::UnorderedAccess)),
                find(Some(D3dClass::ConstantBuffer)),
            );
            // thread: origin, direction, flags, mask
            let threads: [([f32; 3], [f32; 3], u32, u32); 5] = [
                ([1.0, 1.0, -1.0], [0.0, 0.0, 1.0], 0, 0xff),
                ([11.0, 1.0, -1.0], [0.0, 0.0, 1.0], 0, 0xff),
                ([11.0, 1.0, -1.0], [0.0, 0.0, 1.0], 0, 0x01),
                ([1.0, 1.0, -1.0], [0.0, 0.0, 1.0], 0x80, 0xff),
                ([1.0, 1.0, 8.0], [0.0, 0.0, -1.0], 0, 0xff),
            ];
            let mut rows: Vec<[u32; 4]> = Vec::new();
            rows.extend(
                threads
                    .iter()
                    .map(|t| [t.0[0], t.0[1], t.0[2], 0.0].map(f32::to_bits)),
            );
            rows.extend(
                threads
                    .iter()
                    .map(|t| [t.1[0], t.1[1], t.1[2], 0.0].map(f32::to_bits)),
            );
            rows.extend(threads.iter().map(|t| [t.2, t.3, 0, 0]));
            let mut resources = Resources::default();
            resources.constants.insert(rays, rows);
            resources.scenes.insert(structure, scene());
            let bound = exe.bind_resources(&resources).unwrap();
            let mut buffers = BTreeMap::from([(results, vec![0u32; 50])]);
            exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            let words = &buffers[&results];
            let hit = |t: usize| {
                let w = &words[10 * t..10 * t + 10];
                (
                    w[0],
                    f32::from_bits(w[1]),
                    w[2],
                    w[3],
                    w[4],
                    w[5],
                    w[6],
                    f32::from_bits(w[7]),
                    f32::from_bits(w[8]),
                )
            };
            // (status, t, instance index, id, contribution, geometry, primitive, barycentrics)
            assert_eq!(hit(0), (1, 3.0, 0, 7, 3, 1, 0, 0.25, 0.25), "{v} thread 0");
            assert_eq!(hit(1), (1, 3.0, 1, 9, 5, 1, 0, 0.25, 0.25), "{v} thread 1");
            assert_eq!(
                words[20], 0,
                "{v} thread 2 sees no instance: the mask excludes instance 1"
            );
            assert_eq!(
                hit(3),
                (1, 6.0, 0, 7, 3, 0, 0, 0.25, 0.25),
                "{v} thread 3 culls the non-opaque triangle"
            );
            assert_eq!(
                hit(4),
                (1, 3.0, 0, 7, 3, 0, 0, 0.25, 0.25),
                "{v} thread 4 from behind"
            );
        }
    }
}

mod uav_counter {
    use super::*;
    use primase_shader::D3dClass;
    use std::collections::BTreeMap;

    /// `uav_counter.hlsl`: 8 threads. Thread i appends i * 10 to `appended` (u0), consumes one value of `consumed` (u1) into
    /// results[i], takes the old count of `counted` (u2) into results[8 + i] and the new count of `drained` (u3) after a
    /// decrement into results[16 + i]. The counters are resources of their own (`counter_of` names the view): started at
    /// 0, 8, 5 and 20, they end at 8, 0, 13 and 12, and the values each thread got are the distinct ones in between
    /// whatever order the threads' atomic operations took.
    #[test]
    fn append_consume_increment_and_decrement_move_the_counters() {
        for v in BOTH {
            let (program, exe) = lifted(&fixture("uav_counter", v));
            let view = |register: u32| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| {
                        r.provenance
                            .d3d
                            .is_some_and(|d| d.class == D3dClass::UnorderedAccess && d.register == register)
                    })
                    .unwrap_or_else(|| panic!("{v}: no u{register}"))
                    .id
            };
            let counter = |of: ResourceId| {
                program
                    .interface
                    .resources
                    .iter()
                    .find(|r| r.provenance.counter_of == Some(of))
                    .unwrap_or_else(|| panic!("{v}: no counter of {of:?}"))
                    .id
            };
            let (appended, consumed, counted, drained, results) =
                (view(0), view(1), view(2), view(3), view(4));
            let counters = [appended, consumed, counted, drained].map(counter);
            let consumed_values: Vec<u32> = (0..8).map(|k| 100 + k).collect();
            let mut buffers = BTreeMap::from([
                (appended, vec![0u32; 8]),
                (consumed, consumed_values.clone()),
                (counted, vec![0u32; 1]),
                (drained, vec![0u32; 1]),
                (results, vec![0u32; 24]),
                (counters[0], vec![0]),
                (counters[1], vec![8]),
                (counters[2], vec![5]),
                (counters[3], vec![20]),
            ]);
            let resources = Resources::default();
            let bound = exe.bind_resources(&resources).unwrap();
            exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
                .unwrap_or_else(|e| panic!("{v}: {e}"));
            let sorted = |words: &[u32]| {
                let mut w = words.to_vec();
                w.sort_unstable();
                w
            };
            assert_eq!(
                sorted(&buffers[&appended]),
                (0..8).map(|k| k * 10).collect::<Vec<_>>(),
                "{v} appended"
            );
            assert_eq!(sorted(&buffers[&results][0..8]), consumed_values, "{v} consumed");
            assert_eq!(
                sorted(&buffers[&results][8..16]),
                (5..13).collect::<Vec<_>>(),
                "{v} incremented"
            );
            assert_eq!(
                sorted(&buffers[&results][16..24]),
                (12..20).collect::<Vec<_>>(),
                "{v} decremented"
            );
            assert_eq!(counters.map(|c| buffers[&c][0]), [8, 0, 13, 12], "{v} counters");
        }
    }
}

mod procedural_primitives {
    use super::*;
    use primase_shader::D3dClass;
    use primase_shader::exec::{Geometry, Instance, InstanceFlags, Scene};
    use std::collections::BTreeMap;

    /// `ray_query_procedural.hlsl` on one sphere (centre (0.5, 0, 1), radius 1, in the object space of an instance)
    /// bounded by a box, under the identity, a uniform scale of 2 about the origin, and a translation. The program
    /// intersects the ray with the sphere in the object space it makes from the instance's world-to-object matrix, so
    /// the distance it commits is the distance in the world: 5 - sqrt(0.75) + 1 = 5.134 under the identity (the ray runs
    /// along z from z = -5 at x = 0).
    #[test]
    fn a_procedural_primitive_is_intersected_by_the_program_in_the_instances_object_space() {
        let (program, exe) = lifted(&fixture("ray_query_procedural", ""));
        let find = |class: Option<D3dClass>| {
            program
                .interface
                .resources
                .iter()
                .find(|r| match class {
                    Some(c) => r.provenance.d3d.is_some_and(|d| d.class == c && d.register == 0),
                    None => matches!(r.kind, ResourceKind::AccelerationStructure),
                })
                .unwrap()
                .id
        };
        let (structure, results, rays) = (
            find(None),
            find(Some(D3dClass::UnorderedAccess)),
            find(Some(D3dClass::ConstantBuffer)),
        );
        let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let scaled = [[2.0, 0.0, 0.0, 0.0], [0.0, 2.0, 0.0, 0.0], [0.0, 0.0, 2.0, 0.0]];
        let moved = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 3.0]];
        for (name, transform, want) in [
            ("identity", identity, 5.0 + 1.0 - 0.75f32.sqrt()),
            // scaled by 2 about the origin: the sphere's centre (0.5, 0, 1) and radius become (1, 0, 2) and 2
            ("scaled", scaled, 5.0 + 2.0 - (4.0f32 - 1.0).sqrt()),
            ("moved", moved, 5.0 + 4.0 - 0.75f32.sqrt()),
        ] {
            let scene = Scene {
                bottoms: vec![vec![Geometry {
                    positions: Vec::new(),
                    boxes: vec![[[-0.5, -1.0, 0.0], [1.5, 1.0, 2.0]]],
                    indices: None,
                    opaque: false,
                }]],
                instances: vec![Instance {
                    bottom: 0,
                    transform,
                    id: 5,
                    mask: 0xff,
                    contribution: 2,
                    flags: InstanceFlags::default(),
                }],
            };
            let mut rows: Vec<[u32; 4]> = Vec::new();
            rows.extend((0..64).map(|_| [0.0f32, 0.0, -5.0, 0.0].map(f32::to_bits)));
            rows.extend((0..64).map(|_| [0.0f32, 0.0, 1.0, 0.0].map(f32::to_bits)));
            rows.extend((0..64).map(|_| [0, 0xff, 0, 0]));
            rows.extend((0..8).map(|_| [0.5f32, 0.0, 1.0, 1.0].map(f32::to_bits)));
            let mut resources = Resources::default();
            resources.constants.insert(rays, rows);
            resources.scenes.insert(structure, scene);
            let bound = exe.bind_resources(&resources).unwrap();
            let mut buffers = BTreeMap::from([(results, vec![0u32; 640])]);
            exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let w = &buffers[&results];
            assert_eq!(w[0], 2, "{name}: a procedural hit");
            let t = f32::from_bits(w[1]);
            assert!((t - want).abs() < 1e-4, "{name}: t {t}, want {want}");
            assert_eq!((w[3], w[4], w[5], w[6]), (5, 2, 0, 0), "{name}");
        }
    }
}

mod instance_matrices {
    use super::*;
    use primase_shader::D3dClass;
    use primase_shader::exec::{Geometry, Instance, InstanceFlags, Scene};
    use std::collections::BTreeMap;

    /// `ray_query_matrices.hlsl` on an instance that rotates a little, scales unevenly and translates: the matrices a
    /// program reads of a candidate's instance are the instance's transform (object to world) and its inverse (world to
    /// object), row by row, and the world ray is the ray traced. The expected inverse is the one WARP reports for the
    /// same instance.
    #[test]
    fn a_program_reads_the_transform_and_its_inverse_of_the_instance_it_is_offered() {
        let (program, exe) = lifted(&fixture("ray_query_matrices", ""));
        let find = |class: Option<D3dClass>| {
            program
                .interface
                .resources
                .iter()
                .find(|r| match class {
                    Some(c) => r.provenance.d3d.is_some_and(|d| d.class == c && d.register == 0),
                    None => matches!(r.kind, ResourceKind::AccelerationStructure),
                })
                .unwrap()
                .id
        };
        let (structure, results, rays) = (
            find(None),
            find(Some(D3dClass::UnorderedAccess)),
            find(Some(D3dClass::ConstantBuffer)),
        );
        let transform = [
            [1.242_584_3, -0.140_744_72, 0.0, 1.559_948_9],
            [0.140_744_72, 1.242_584_3, 0.0, -1.719_660_7],
            [0.0, 0.0, 1.250_529_9, 1.522_493_8],
        ];
        let scene = Scene {
            bottoms: vec![vec![Geometry {
                positions: Vec::new(),
                boxes: vec![[[-9.0; 3], [9.0; 3]]],
                indices: None,
                opaque: true,
            }]],
            instances: vec![Instance {
                bottom: 0,
                transform,
                id: 5,
                mask: 0xff,
                contribution: 2,
                flags: InstanceFlags::default(),
            }],
        };
        let mut resources = Resources::default();
        resources.constants.insert(
            rays,
            vec![
                [0.3f32, -0.2, -5.0, 0.0].map(f32::to_bits),
                [0.1f32, 0.05, 1.0, 0.0].map(f32::to_bits),
            ],
        );
        resources.scenes.insert(structure, scene);
        let bound = exe.bind_resources(&resources).unwrap();
        let mut buffers = BTreeMap::from([(results, vec![0u32; 64])]);
        exe.dispatch(&bound, &mut NoBackend, &mut buffers, [1, 1, 1], 32)
            .unwrap();
        let w: Vec<f32> = buffers[&results].iter().map(|b| f32::from_bits(*b)).collect();
        assert_eq!(buffers[&results][30], 1, "a candidate");
        let want_object_to_world: Vec<f32> = transform.iter().flatten().copied().collect();
        let want_world_to_object = [
            0.794_580_2,
            0.090_000_29,
            0.0,
            -1.084_734_4,
            -0.090_000_29,
            0.794_580_2,
            0.0,
            1.506_804_5,
            0.0,
            0.0,
            0.799_661_04,
            -1.217_478_9,
        ];
        for (k, (got, want)) in w[..12].iter().zip(&want_object_to_world).enumerate() {
            assert!((got - want).abs() < 1e-6, "object to world [{k}]: {got}, {want}");
        }
        for (k, (got, want)) in w[12..24].iter().zip(&want_world_to_object).enumerate() {
            assert!((got - want).abs() < 1e-5, "world to object [{k}]: {got}, {want}");
        }
        assert_eq!(w[24..30], [0.3, -0.2, -5.0, 0.1, 0.05, 1.0]);
    }
}
