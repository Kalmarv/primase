#![allow(clippy::unwrap_used)]
//! The program executor against Microsoft's WARP rasterizer (Direct3D 11, Shader Model 5), the oracle that is not ours:
//! each lifted pixel program of a corpus is rendered by WARP on one pixel, with the same input registers, constant
//! buffers and uniform textures (of the dimension the program declares: 1D, 2D, 3D, cube, arrays, cube arrays), and
//! run by [`ProgramShader`]; every lane both write is compared.
//!
//! Needs `tools.warp` (the oracle program) and `tools.fxc` (Microsoft's `fxc`, for the oracle's vertex shader), and a
//! corpus folder (the corpus oracle set's `sekiro/shaders`, `riders/shaders`, `sledding/shaders`); it works in
//! `warp` under the scratch folder. By default only the programs with a texture that is not a plain 2D one run (1D, 3D, cube
//! and arrayed shapes: the dimension plumbing); the debug key `shader.warp_all=1` runs every program the
//! oracle can bind.
//!
//! Textures are uniform and every sampler point-filters with clamp, so a sample cannot depend on its coordinate: the
//! gate checks the arithmetic around it, the dimension plumbing and the declared shapes, not the sampler. Direct3D
//! leaves `exp`, `log`, `sin`, `cos`, `rsqrt` and `rcp` within a few units in the last place: lanes are classed by how
//! far apart they are, and only a lane further apart than 1e-3 relative fails.
mod common;
use common::case;
use iridia::{
    InterpMode, InterpSampling, Program, ResourceKind, SampleType, ScalarType, StorageTarget, SystemValue,
    TextureDim,
};
use iridia_exec::{Dim, ImageLoad, ImageQuery, ImageSample, ProgramShader, QueryKind};
use iridia_exec::{PixelShader, Sampler};
use primase_testdata::{Game, Tool, debug, tool};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn hex(words: &[u32]) -> String {
    words
        .iter()
        .map(|w| format!("{w:08x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The oracle's name for a texture's shape, and its layer count.
fn oracle_dimension(dim: TextureDim, arrayed: bool) -> (&'static str, u32) {
    match (dim, arrayed) {
        (TextureDim::D1, false) => ("1d", 1),
        (TextureDim::D1, true) => ("1darray", 2),
        (TextureDim::D2, false) => ("2d", 1),
        (TextureDim::D2, true) => ("2darray", 2),
        (TextureDim::D3, _) => ("3d", 1),
        (TextureDim::Cube, false) => ("cube", 6),
        (TextureDim::Cube, true) => ("cubearray", 12),
    }
}

/// Why the oracle cannot bind a program, or `None` when it can.
fn unbindable(program: &Program) -> Option<String> {
    for r in &program.interface.resources {
        match &r.kind {
            ResourceKind::Buffer(_) | ResourceKind::Storage { .. } => {
                return Some(
                    match &r.kind {
                        ResourceKind::Storage {
                            target: StorageTarget::Image { .. },
                            ..
                        } => "an unordered-access image",
                        _ => "a buffer view",
                    }
                    .into(),
                );
            }
            ResourceKind::Texture {
                multisampled, sample, ..
            } if *multisampled || *sample != SampleType::Float => {
                return Some("a multisampled, depth or integer texture".into());
            }
            ResourceKind::Sampler { comparison: true } => return Some("a comparison sampler".into()),
            _ => {}
        }
    }
    // a loop whose bound is a constant of unknown type could run away on any value the test makes up
    let mut loops = false;
    fn has_loop(block: &naga::Block, found: &mut bool) {
        for st in block.iter() {
            match st {
                naga::Statement::Loop { .. } => *found = true,
                naga::Statement::Block(b) => has_loop(b, found),
                naga::Statement::If { accept, reject, .. } => {
                    has_loop(accept, found);
                    has_loop(reject, found);
                }
                naga::Statement::Switch { cases, .. } => cases.iter().for_each(|c| has_loop(&c.body, found)),
                _ => {}
            }
        }
    }
    has_loop(&program.module.entry_points[0].function.body, &mut loops);
    if loops
        && program.interface.resources.iter().any(
            |r| matches!(&r.kind, ResourceKind::ConstantBuffer { layout } if layout.constants.is_empty()),
        )
    {
        return Some("a loop and constants of unknown type".into());
    }
    if program.interface.inputs.iter().any(|v| {
        v.provenance.d3d.is_none()
            && !matches!(
                v.system,
                Some(SystemValue::PrimitiveIndex | SystemValue::Coverage)
            )
    }) {
        return Some("an input without a register".into());
    }
    None
}

/// The interpolation modifiers of an input in HLSL.
fn modifiers(v: &iridia::Varying) -> String {
    match (v.value.scalar, v.interp) {
        (ScalarType::Float, Some(i)) => {
            let mut m = String::new();
            match i.mode {
                InterpMode::Flat => m += "nointerpolation ",
                InterpMode::Linear => m += "noperspective ",
                InterpMode::Perspective => {}
            }
            match i.sampling {
                InterpSampling::Centroid => m += "centroid ",
                InterpSampling::Sample => m += "sample ",
                InterpSampling::Center => {}
            }
            m
        }
        (ScalarType::Float, None) => String::new(),
        _ => "nointerpolation ".into(),
    }
}

/// The vertex shader that feeds a program's non-system inputs from `cbuffer Case : register(b13) { uint4 r[32]; }`.
///
/// The pixel stage reads the vertex stage's outputs lane for lane, so the vertex shader must pack them where the pixel
/// shader's signature has them: members are declared register by register, in lane order, with a padding member for
/// every lane and register nothing uses (the compiler packs a later scalar into an earlier register's free lanes
/// otherwise). Output register 0 is the position, so a signature that starts at register 0 is shifted up by one.
/// `None` for a signature this cannot reproduce (an element on lanes that are not consecutive).
fn vertex_hlsl(program: &Program) -> Option<String> {
    let mut inputs: Vec<_> = program
        .interface
        .inputs
        .iter()
        .filter(|v| v.system.is_none())
        .collect();
    inputs.sort_by_key(|v| v.provenance.d3d.map(|s| (s.register, s.mask)));
    let shift = u32::from(
        inputs
            .iter()
            .any(|v| v.provenance.d3d.is_some_and(|s| s.register == 0)),
    );
    let last = inputs
        .iter()
        .filter_map(|v| v.provenance.d3d)
        .map(|s| s.register + shift)
        .max()
        .unwrap_or(0);
    let mut members = String::new();
    let mut body = String::new();
    for register in 1..=last {
        let mut cursor = 0u32;
        // elements that share a register share their interpolation (the compiler packs by it), so the register has one
        // style: the one of the first element the program reads, and an element it does not read (whose interpolation
        // its signature does not state) takes it too
        let in_register =
            |v: &iridia::Varying| v.provenance.d3d.is_some_and(|s| s.register + shift == register);
        let style = inputs
            .iter()
            .copied()
            .filter(|v| in_register(v))
            .find(|v| v.used)
            .or_else(|| inputs.iter().copied().find(|v| in_register(v)))
            .map(modifiers)
            .unwrap_or_default();
        for (n, v) in inputs.iter().enumerate() {
            let sig = v.provenance.d3d?;
            if sig.register + shift != register {
                continue;
            }
            let (first, width) = (sig.mask.trailing_zeros(), sig.mask.count_ones());
            if u32::from(sig.mask) != ((1u32 << width) - 1) << first {
                return None;
            }
            if first > cursor {
                members += &format!(
                    "  {style}float{} pad{register}_{cursor} : PADDING{register}{cursor};
",
                    first - cursor
                );
            }
            let (ty, conv) = match v.value.scalar {
                ScalarType::Uint => ("uint", ""),
                ScalarType::Sint => ("int", "asint"),
                _ => ("float", "asfloat"),
            };
            let name = if width == 1 {
                ty.to_string()
            } else {
                format!("{ty}{width}")
            };
            let swizzle: String = (first..first + width)
                .map(|b| ["x", "y", "z", "w"][b as usize])
                .collect();
            let own = if v.used || ty != "float" {
                modifiers(v)
            } else {
                style.clone()
            };
            members += &format!(
                "  {own}{name} f{n} : {}{};
",
                v.label.name, v.label.index
            );
            body += &format!(
                "  o.f{n} = {conv}(r[{}].{swizzle});
",
                sig.register
            );
            cursor = first + width;
        }
        if cursor < 4 {
            members += &format!(
                "  {style}float{} pad{register}_{cursor} : PADDING{register}{cursor};
",
                4 - cursor
            );
        }
    }
    Some(format!(
        "cbuffer Case : register(b13) {{ uint4 r[32]; }};
struct VSOut {{
  float4 pos : SV_Position;
{members}}};
VSOut main(uint id : SV_VertexID) {{
  VSOut o;
  o.pos = float4(id == 1 ? 3 : -1, id == 2 ? -3 : 1, 0.5, 1);
{body}  return o;
}}
"
    ))
}

/// Answers every request with the slot's uniform colour, as the oracle's textures do; a load outside the texture, or at
/// another mip level than the only one, reads zeros.
struct Uniform {
    colours: BTreeMap<u32, [f32; 4]>,
    /// Texture slot -> (dimension, layers)
    shapes: BTreeMap<u32, (TextureDim, u32)>,
    samples: usize,
}
impl Sampler for Uniform {
    fn sample(&mut self, r: iridia_exec::Sample) -> Result<[f32; 4], String> {
        self.sample_image(ImageSample {
            texture: r.texture,
            sampler: r.sampler,
            dim: Dim::D2,
            coord: [r.uv[0], r.uv[1], 0.0],
            layer: r.layer,
            lod: iridia_exec::ImageLod::Implicit,
            offset: [0; 3],
            gather: None,
            depth_ref: None,
            depth: false,
        })
    }
    fn sample_image(&mut self, r: ImageSample<u32>) -> Result<[f32; 4], String> {
        self.samples += 1;
        // a gather returns the component of each of four texels: of a uniform texture, that component four times
        let c = self
            .colours
            .get(&r.texture)
            .ok_or_else(|| format!("no texture t{}", r.texture))?;
        Ok(match r.gather {
            Some(k) => [c[usize::from(k)]; 4],
            None => *c,
        })
    }
    fn load(&mut self, r: iridia_exec::Load) -> Result<[u32; 4], String> {
        // a plain load: x, y and an array layer or 3D slice in the third lane
        let (dim, layers) = self.shapes[&r.texture];
        let (layer, z) = match dim {
            TextureDim::D3 => (None, r.texel[2]),
            _ if layers > 1 => (Some(r.texel[2]), 0),
            _ => (None, 0),
        };
        self.load_image(ImageLoad {
            texture: r.texture,
            dim: Dim::D2,
            storage: false,
            texel: [r.texel[0], r.texel[1], z],
            layer,
            sample: None,
            mip: Some(r.mip),
        })
    }
    fn load_image(&mut self, r: ImageLoad<u32>) -> Result<[u32; 4], String> {
        let c = self
            .colours
            .get(&r.texture)
            .ok_or_else(|| format!("no texture t{}", r.texture))?;
        let (dim, layers) = self.shapes[&r.texture];
        let comps = match dim {
            TextureDim::D1 => 1,
            TextureDim::D2 => 2,
            TextureDim::D3 | TextureDim::Cube => 3,
        };
        let inside = r.mip.unwrap_or(0) == 0
            && r.texel[..comps].iter().all(|t| (0..4).contains(t))
            && r.layer.is_none_or(|l| (0..layers as i32).contains(&l));
        Ok(if inside { c.map(f32::to_bits) } else { [0; 4] })
    }
    fn query_image(&mut self, r: ImageQuery<u32>) -> Result<[u32; 4], String> {
        let (_, layers) = self.shapes[&r.texture];
        Ok(match r.kind {
            QueryKind::Size => [4, 4, 4, 0],
            QueryKind::Levels => [1, 0, 0, 0],
            QueryKind::Layers => [layers, 0, 0, 0],
            QueryKind::Samples => [1, 0, 0, 0],
        })
    }
}

/// How far apart two lanes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Exact,
    BothNan,
    Ulps4,
    Relative1e5,
    Relative1e3,
    Beyond,
}
const CLASSES: [&str; 6] = [
    "exact",
    "both NaN",
    "<= 4 ulp",
    "<= 1e-5",
    "<= 1e-3",
    "BEYOND 1e-3",
];

fn classify(float: bool, a: u32, b: u32) -> Class {
    if a == b {
        return Class::Exact;
    }
    if !float {
        return Class::Beyond;
    }
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    if x.is_nan() && y.is_nan() {
        return Class::BothNan;
    }
    if x == y {
        return Class::Exact; // +0 and -0
    }
    if x.is_nan() || y.is_nan() {
        return Class::Beyond;
    }
    // a difference of a few units in the last place
    let key = |w: u32| if w >> 31 != 0 { !w } else { w | 0x8000_0000 };
    if key(a).abs_diff(key(b)) <= 4 {
        return Class::Ulps4;
    }
    let (x, y) = (f64::from(x), f64::from(y));
    let relative = (x - y).abs() / x.abs().max(y.abs());
    if relative <= 1e-5 {
        Class::Relative1e5
    } else if relative <= 1e-3 {
        Class::Relative1e3
    } else {
        Class::Beyond
    }
}

#[derive(Default)]
struct Tally {
    programs: usize,
    lifted: usize,
    selected: usize,
    compared: usize,
    discarded_runs: usize,
    unbindable: BTreeMap<String, usize>,
    errors: BTreeMap<String, usize>,
    lanes: BTreeMap<Class, usize>,
    worst: Vec<String>,
    by_program: BTreeMap<String, Class>,
    /// Compared programs by the shape of each of their textures (a program counts once per shape).
    shapes: BTreeMap<&'static str, usize>,
}

fn run_program(
    name: &str,
    bytes: &[u8],
    cases: u32,
    all: bool,
    tools: &(PathBuf, PathBuf, primase_testdata::OwnScratch),
    t: &mut Tally,
) {
    let (fxc, warp, scratch) = tools;
    t.programs += 1;
    let Ok(program) = iridia_dxbc::lift(bytes) else {
        return;
    };
    if program.stage != iridia::Stage::Pixel {
        return;
    }
    t.lifted += 1;
    let exotic = program.interface.resources.iter().any(|r| {
        matches!(
            r.kind,
            ResourceKind::Texture { dim, arrayed, .. } if dim != TextureDim::D2 || arrayed
        )
    });
    if !exotic && !all {
        return;
    }
    if let Some(why) = unbindable(&program) {
        *t.unbindable.entry(why).or_default() += 1;
        return;
    }
    t.selected += 1;
    let Some(vertex) = vertex_hlsl(&program) else {
        *t.unbindable
            .entry("inputs on lanes the vertex shader cannot pack alike".into())
            .or_default() += 1;
        return;
    };
    let Ok(shader) = ProgramShader::new(program) else {
        *t.errors.entry("ProgramShader::new".into()).or_default() += 1;
        return;
    };
    let tag = Path::new(name)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let hlsl_path = scratch.join(format!("{tag}.vs.hlsl"));
    let vs_path = scratch.join(format!("{tag}.vs.cso"));
    let ps_path = scratch.join(format!("{tag}.ps.dxbc"));
    std::fs::write(&hlsl_path, vertex).unwrap();
    std::fs::write(&ps_path, bytes).unwrap();
    let compiled = std::process::Command::new(fxc)
        .args(["/nologo", "/T", "vs_5_0", "/E", "main", "/Fo"])
        .arg(&vs_path)
        .arg(&hlsl_path)
        .output()
        .unwrap();
    if !compiled.status.success() {
        let first = String::from_utf8_lossy(&compiled.stderr)
            .lines()
            .next()
            .unwrap_or("")
            .to_owned();
        *t.errors.entry(format!("fxc: {first}")).or_default() += 1;
        return;
    }
    let interface = shader.interface().clone();
    let targets: Vec<(u32, u32, u8, ScalarType)> = interface
        .outputs
        .iter()
        .filter_map(|v| match (v.system, v.provenance.d3d) {
            (Some(SystemValue::Target(n)), Some(sig)) => Some((n, sig.register, sig.mask, v.value.scalar)),
            _ => None,
        })
        .collect();
    let textures: Vec<(u32, TextureDim, bool)> = interface
        .resources
        .iter()
        .filter_map(|r| match (&r.kind, r.provenance.d3d) {
            (ResourceKind::Texture { dim, arrayed, .. }, Some(b)) => Some((b.register, *dim, *arrayed)),
            _ => None,
        })
        .collect();
    let mut text = String::new();
    let mut ours = Vec::new();
    for c in 0..cases {
        let case = case(&shader, c);
        let front = if c >= 2 {
            !c.is_multiple_of(3)
        } else {
            c.is_multiple_of(2)
        };
        text += &format!(
            "case c{c}\napi d3d11\nps {}\nvs {}\nfront {}\n",
            ps_path.display(),
            vs_path.display(),
            u8::from(front)
        );
        for (r, w) in case.inputs.iter().enumerate() {
            text += &format!("input {r} {}\n", hex(w));
        }
        for (slot, rows) in &case.buffers {
            text += &format!("cb {slot} {}\n", rows.len());
            for row in rows {
                text += &format!("row {}\n", hex(row));
            }
        }
        let mut colours = BTreeMap::new();
        let mut shapes = BTreeMap::new();
        for &(slot, dim, arrayed) in &textures {
            let v = |k: u32| {
                if std::env::var("EXP_TEX").is_ok() {
                    0.5
                } else {
                    ((slot * 3 + k + c) % 8 + 1) as f32 / 8.0
                }
            };
            let colour = [v(0), v(1), v(2), v(3)];
            let (dimension, layers) = oracle_dimension(dim, arrayed);
            text += &format!("texture {slot} {} {dimension}\n", hex(&colour.map(f32::to_bits)));
            colours.insert(slot, colour);
            shapes.insert(slot, (dim, layers));
        }
        for &(n, _, _, scalar) in &targets {
            text += &format!(
                "target {n} {}\n",
                match scalar {
                    ScalarType::Uint => "uint",
                    ScalarType::Sint => "sint",
                    _ => "float",
                }
            );
        }
        text += "end\n";
        let mut sampler = Uniform {
            colours,
            shapes,
            samples: 0,
        };
        let zero = vec![[[0u32; 4]; 2]; case.inputs.len()];
        let refs = common::refs(&case, &shader);
        ours.push(shader.execute_with_derivatives(&case.inputs, &zero, &refs, &mut sampler));
    }
    let cases_path = scratch.join(format!("{tag}.cases.txt"));
    let results_path = scratch.join(format!("{tag}.warp.jsonl"));
    std::fs::write(&cases_path, text).unwrap();
    let _oracle = primase_testdata::warp_exclusive();
    let run = std::process::Command::new(warp)
        .arg(&cases_path)
        .arg(&results_path)
        .output()
        .unwrap();
    if !run.status.success() {
        *t.errors
            .entry(format!(
                "warp: {}",
                String::from_utf8_lossy(&run.stderr).lines().next().unwrap_or("")
            ))
            .or_default() += 1;
        return;
    }
    let mut worst = Class::Exact;
    let results = std::fs::read_to_string(&results_path).unwrap();
    for (c, line) in results.lines().enumerate() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(e) = v["error"].as_str() {
            *t.errors.entry(format!("WARP: {e}")).or_default() += 1;
            continue;
        }
        let execution = match &ours[c] {
            Ok(e) => e,
            // a scalar run cannot derive a computed value (WARP's one pixel has no neighbours either): not a comparison
            Err(e) if e.detail.contains("derivative of a computed value") => {
                *t.unbindable
                    .entry("a derivative of a computed value".into())
                    .or_default() += 1;
                continue;
            }
            Err(e) => {
                *t.errors
                    .entry(format!(
                        "executor: {}",
                        e.detail.chars().take(80).collect::<String>()
                    ))
                    .or_default() += 1;
                continue;
            }
        };
        if execution.discarded {
            t.discarded_runs += 1;
            continue;
        }
        t.compared += 1;
        for (k, &(_, register, mask, scalar)) in targets.iter().enumerate() {
            for lane in (0..4usize).filter(|l| mask >> l & 1 != 0) {
                let warp_bits =
                    u32::from_str_radix(v["targets"][k][lane].as_str().unwrap_or(""), 16).unwrap();
                let our_bits = execution.outputs[register as usize][lane];
                let class = classify(scalar == ScalarType::Float, our_bits, warp_bits);
                *t.lanes.entry(class).or_default() += 1;
                worst = worst.max(class);
                if class == Class::Beyond && t.worst.len() < 30 {
                    t.worst.push(format!(
                        "{name} case {c} t{k}.{lane}: executor {our_bits:#010x} ({}) WARP {warp_bits:#010x} ({})",
                        f32::from_bits(our_bits),
                        f32::from_bits(warp_bits)
                    ));
                }
            }
        }
    }
    t.by_program.insert(name.to_owned(), worst);
    let mut seen = std::collections::BTreeSet::new();
    for &(_, dim, arrayed) in &textures {
        if seen.insert(oracle_dimension(dim, arrayed).0) {
            *t.shapes.entry(oracle_dimension(dim, arrayed).0).or_default() += 1;
        }
    }
}

fn report(title: &str, t: &Tally) {
    println!(
        "{title}: {} programs, {} lifted, {} selected, {} compared runs ({} discarded runs skipped)",
        t.programs, t.lifted, t.selected, t.compared, t.discarded_runs
    );
    let total: usize = t.lanes.values().sum();
    for (c, n) in &t.lanes {
        println!("  {:12} {n:7} lanes of {total}", CLASSES[*c as usize]);
    }
    let mut programs: BTreeMap<Class, usize> = BTreeMap::new();
    for c in t.by_program.values() {
        *programs.entry(*c).or_default() += 1;
    }
    for (c, n) in &programs {
        println!("  programs whose worst lane is {:12} {n}", CLASSES[*c as usize]);
    }
    for (shape, n) in &t.shapes {
        println!("  compared programs with a {shape} texture: {n}");
    }
    for (why, n) in &t.unbindable {
        println!("  unbindable {n:5}  {why}");
    }
    for (why, n) in &t.errors {
        println!("  ERROR {n:5}  {why}");
    }
    for w in &t.worst {
        println!("  BEYOND {w}");
    }
}

fn corpus(game: Game) {
    let (Some(fxc), Some(warp), Some((name, paths))) =
        (tool(Tool::Fxc), tool(Tool::Warp), common::corpus(game))
    else {
        return;
    };
    let scratch = primase_testdata::own_scratch("warp");
    let all = debug("shader.warp_all").is_some_and(|v| v == "1");
    let tools = (fxc, warp, scratch);
    let mut t = Tally::default();
    for path in paths.iter().take(common::diff_limit()) {
        let bytes = std::fs::read(path).unwrap();
        run_program(&path.display().to_string(), &bytes, 3, all, &tools, &mut t);
    }
    report(&name, &t);
    // Direct3D leaves exp2, log2, sin, cos and rsqrt to the implementation within a few ulp, and a program that raises
    // a value to a large power through them turns that into more than 1e-3; only the textures that are not plain 2D are
    // asserted, the everything-bindable run (the debug key `shader.warp_all=1`) reports
    assert!(
        all || t.worst.is_empty(),
        "{} lanes are further than 1e-3 from WARP",
        t.worst.len()
    );
    assert!(t.errors.is_empty(), "{} programs errored", t.errors.len());
}

#[test]
fn sekiro_programs_agree_with_warp() {
    corpus(Game::Sekiro);
}
#[test]
fn riders_republic_programs_agree_with_warp() {
    corpus(Game::Riders);
}
#[test]
fn unity_programs_agree_with_warp() {
    corpus(Game::Sledding);
}
