//! Cases rendered by Microsoft's WARP rasterizer (Direct3D 12) through the WARP oracle program and evaluated by
//! `primase-dxil` on the same inputs; every written output lane must agree bit for bit. Textures are uniform, so the
//! sampler state the oracle binds cannot change a sample. The oracle's vertex shader comes from
//! `Shader::passthrough_vertex_shader`, compiled with dxc. Shared by the fixture gate and the Elden Ring gate.
use primase_dxil::Shader;
use primase_shader::{PixelShader, Sample};
use std::collections::BTreeMap;
use std::path::Path;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
pub struct Case {
    pub name: String,
    pub front: bool,
    /// Input registers other than SV_Position (set by the rasterizer to (0.5, 0.5, 0.5, 1)).
    pub inputs: BTreeMap<u32, [u32; 4]>,
    pub buffers: BTreeMap<u32, Vec<[u32; 4]>>,
    pub textures: BTreeMap<u32, [f32; 4]>,
}

fn hex(words: &[u32]) -> String {
    words
        .iter()
        .map(|w| format!("{w:08x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a comparison found: written lanes compared, of them lanes that depend on an implementation-defined
/// approximation (compared within [`APPROXIMATE`] relative), the largest relative difference seen there, and the
/// mismatches.
#[derive(Debug, Default)]
pub struct Outcome {
    pub lanes: usize,
    pub approximate: usize,
    pub largest_relative: f64,
    /// Lanes where both are NaN: Direct3D does not specify NaN bit patterns, so any NaN agrees with any NaN.
    pub nan: usize,
    pub mismatches: Vec<String>,
}
/// Relative tolerance on lanes that depend on Exp, Log, Sin, Cos or Rsqrt, whose results Direct3D leaves to the
/// implementation within a few units in the last place.
pub const APPROXIMATE: f64 = 1e-5;

/// Render and evaluate `cases` of the shader in `container`.
pub fn compare(container: &Path, cases: &[Case], dxc: &Path, warp: &Path, scratch: &Path) -> Result<Outcome> {
    let shader = Shader::parse(&std::fs::read(container)?)?;
    let stem = container
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("shader")
        .replace(['[', ']'], "_");
    let hlsl = scratch.join(format!("{stem}.vs.hlsl"));
    let vs = scratch.join(format!("{stem}.vs.dxil"));
    std::fs::write(&hlsl, shader.passthrough_vertex_shader("b0, space1"))?;
    let out = std::process::Command::new(dxc)
        .args(["-T", "vs_6_0", "-E", "main", "-Fo"])
        .arg(&vs)
        .arg(&hlsl)
        .output()?;
    if !out.status.success() {
        return Err(format!("dxc: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    let position = shader
        .input_elements()
        .iter()
        .find(|e| e.system_value == 3 && e.placed)
        .map(|e| e.row);
    let front_register = shader
        .input_elements()
        .iter()
        .find(|e| e.system_value == 13 && e.placed)
        .map(|e| (e.row, e.column));
    let targets: Vec<(u32, &str)> = shader
        .output_elements()
        .iter()
        .filter(|e| e.placed)
        .map(|e| {
            (
                e.row,
                match e.component_type {
                    5 => "uint",
                    4 => "sint",
                    _ => "float",
                },
            )
        })
        .collect();
    let mut text = String::new();
    let mut ours = BTreeMap::new();
    for c in cases {
        text.push_str(&format!(
            "case {}\napi d3d12\nps {}\nvs {}\nfront {}\n",
            c.name,
            container.display(),
            vs.display(),
            u8::from(c.front)
        ));
        for (r, w) in &c.inputs {
            text.push_str(&format!("input {r} {}\n", hex(w)));
        }
        for (slot, rows) in &c.buffers {
            text.push_str(&format!("cb {slot} {}\n", rows.len()));
            for row in rows {
                text.push_str(&format!("row {}\n", hex(row)));
            }
        }
        for (slot, colour) in &c.textures {
            text.push_str(&format!("texture {slot} {}\n", hex(&colour.map(f32::to_bits))));
        }
        for (r, kind) in &targets {
            text.push_str(&format!("target {r} {kind}\n"));
        }
        text.push_str("end\n");
        // Our evaluation of the same case.
        let mut inputs = vec![[0u32; 4]; shader.input_count()];
        for (r, w) in &c.inputs {
            inputs[*r as usize] = *w;
        }
        if let Some(p) = position {
            inputs[p as usize] = [0.5f32, 0.5, 0.5, 1.0].map(f32::to_bits);
        }
        if let Some((r, col)) = front_register {
            inputs[r as usize][usize::from(col)] = if c.front { u32::MAX } else { 0 };
        }
        let refs: Vec<(u32, &[[u32; 4]])> = c.buffers.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let mut sampler = |q: Sample| {
            c.textures
                .get(&q.texture)
                .copied()
                .ok_or_else(|| format!("no texture at t{}", q.texture))
        };
        let marked = shader.execute_marked(&inputs, &refs, &mut sampler)?;
        ours.insert(c.name.clone(), marked);
    }
    let cases_file = scratch.join(format!("{stem}.cases.txt"));
    let results = scratch.join(format!("{stem}.warp.jsonl"));
    std::fs::write(&cases_file, text)?;
    let run = std::process::Command::new(warp)
        .arg(&cases_file)
        .arg(&results)
        .output()?;
    if !run.status.success() {
        return Err(format!("warp: {}", String::from_utf8_lossy(&run.stderr)).into());
    }
    let mut outcome = Outcome::default();
    for line in std::fs::read_to_string(&results)?.lines() {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let name = v["case"].as_str().unwrap_or("");
        if let Some(e) = v["error"].as_str() {
            outcome.mismatches.push(format!("{name}: WARP error {e}"));
            continue;
        }
        let (execution, inexact) = &ours[name];
        for (k, (reg, _)) in targets.iter().enumerate() {
            let want = &v["targets"][k];
            for lane in 0..4 {
                if execution.output_masks[*reg as usize] & (1 << lane) == 0 {
                    continue;
                }
                let warp_bits = u32::from_str_radix(want[lane].as_str().unwrap_or(""), 16)?;
                let our_bits = execution.outputs[*reg as usize][lane];
                outcome.lanes += 1;
                if f32::from_bits(our_bits).is_nan() && f32::from_bits(warp_bits).is_nan() {
                    outcome.nan += 1;
                    continue;
                }
                if inexact[*reg as usize] & (1 << lane) != 0 {
                    outcome.approximate += 1;
                    let (a, b) = (
                        f64::from(f32::from_bits(our_bits)),
                        f64::from(f32::from_bits(warp_bits)),
                    );
                    let relative = if a == b {
                        0.
                    } else {
                        (a - b).abs() / a.abs().max(b.abs())
                    };
                    outcome.largest_relative = outcome.largest_relative.max(relative);
                    if relative <= APPROXIMATE {
                        continue;
                    }
                }
                if warp_bits != our_bits {
                    outcome.mismatches.push(format!(
                        "{name} o{reg}.{lane}: ours {our_bits:08x} ({}), WARP {warp_bits:08x} ({})",
                        f32::from_bits(our_bits),
                        f32::from_bits(warp_bits)
                    ));
                }
            }
        }
    }
    Ok(outcome)
}
