#![allow(dead_code)]
//! What the executor's gates share: deterministic test vectors for a lifted pixel program (input registers, constant
//! buffers), a quad's lanes, and a game's shader corpus.
pub mod differential;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_testdata::{Game, OracleSet, Query, expect_found, resolve};

use primase_shader::{ConstantType, Majorness, PixelShader, ResourceKind, ScalarType, SystemValue};

pub fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

pub fn mix(x: u64) -> u32 {
    mix64(x) as u32
}

pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// The word classes of a constant buffer: 1 bool, 2 integer, 3 float.
pub fn classify(ty: &ConstantType, base: u32, out: &mut BTreeMap<u32, u32>) {
    let class = |s: ScalarType| match s {
        ScalarType::Bool => 1,
        ScalarType::Sint | ScalarType::Uint => 2,
        _ => 3,
    };
    match ty {
        ConstantType::Scalar(s) => {
            out.insert(base / 4, class(*s));
        }
        ConstantType::Vector(v) => {
            for i in 0..u32::from(v.width) {
                out.insert(base / 4 + i, class(v.scalar));
            }
        }
        ConstantType::Matrix {
            scalar,
            rows,
            columns,
            major,
            stride,
        } => {
            let (lines, lanes) = match major {
                Majorness::Row => (*rows, *columns),
                Majorness::Column => (*columns, *rows),
            };
            for line in 0..u32::from(lines) {
                for lane in 0..u32::from(lanes) {
                    out.insert((base + line * stride) / 4 + lane, class(*scalar));
                }
            }
        }
        ConstantType::Array {
            element,
            count,
            stride,
        } => {
            for i in 0..*count {
                classify(element, base + i * stride, out);
            }
        }
        ConstantType::Struct { members } => {
            for m in members {
                classify(&m.ty, base + m.offset, out);
            }
        }
    }
}

/// One deterministic test vector: input registers, constant buffers.
pub struct Case {
    pub inputs: Vec<[u32; 4]>,
    pub buffers: Vec<(u32, Vec<[u32; 4]>)>,
}

pub fn case(shader: &dyn PixelShader, other: &dyn PixelShader, c: u32) -> Case {
    let interface = shader.interface();
    let mut inputs = vec![[0u32; 4]; shader.input_count().max(1)];
    let front = if c >= 2 {
        !c.is_multiple_of(3)
    } else {
        c.is_multiple_of(2)
    };
    for v in &interface.inputs {
        let Some(sig) = v.provenance.d3d else { continue };
        let Some(register) = inputs.get_mut(sig.register as usize) else {
            continue;
        };
        for (j, lane) in (0..4usize).filter(|b| sig.mask >> b & 1 != 0).enumerate() {
            let h = mix(u64::from(c) << 40 ^ u64::from(sig.register) << 8 ^ lane as u64 ^ 0x5151);
            register[lane] = match (v.system, v.value.scalar) {
                (Some(SystemValue::FragCoord), _) => [0.5f32, 0.5, 0.5, 1.0][lane].to_bits(),
                (Some(SystemValue::FrontFacing), _) => {
                    if front {
                        u32::MAX
                    } else {
                        0
                    }
                }
                (_, ScalarType::Float) => (unit(h) * if j == 3 { 1.0 } else { 2.0 } - 0.25).to_bits(),
                (_, ScalarType::Uint) => h % 4,
                (_, ScalarType::Sint) => (h % 5).wrapping_sub(2),
                _ => 0,
            };
        }
    }
    let mut buffers = Vec::new();
    // the buffers of both implementations, each as long as the longer of the two says: a lifted program's array is
    // the larger of the declared rows and the reflection's
    let mut sizes: BTreeMap<u32, u32> = BTreeMap::new();
    for &(slot, rows) in shader.constant_buffers().iter().chain(other.constant_buffers()) {
        let e = sizes.entry(slot).or_default();
        *e = (*e).max(rows);
    }
    for (slot, rows) in sizes {
        let mut classes = BTreeMap::new();
        for r in &interface.resources {
            if r.provenance.d3d.is_some_and(|d| d.register == slot)
                && let ResourceKind::ConstantBuffer { layout } = &r.kind
            {
                for k in &layout.constants {
                    classify(&k.ty, k.offset, &mut classes);
                }
            }
        }
        let mut v = vec![[0u32; 4]; rows as usize];
        for (r, row) in v.iter_mut().enumerate() {
            for (k, w) in row.iter_mut().enumerate() {
                let word = (r * 4 + k) as u32;
                let h = mix(u64::from(c) << 44 ^ u64::from(slot) << 32 ^ u64::from(word) ^ 0x77);
                *w = match classes.get(&word).copied().unwrap_or(3) {
                    1 => h & 1,
                    2 => h % 3,
                    _ => (unit(h) * 2.0 - 0.25).to_bits(),
                };
            }
        }
        buffers.push((slot, v));
    }
    Case { inputs, buffers }
}

/// Lane `k` of a quad: every float input advances by (0.01, 0.02) per step in x and y.
pub fn quad_lane(shader: &dyn PixelShader, c: &Case, k: usize) -> Vec<[u32; 4]> {
    let off = (k & 1) as f32 * 0.01 + (k >> 1) as f32 * 0.02;
    let mut inputs = c.inputs.clone();
    for v in &shader.interface().inputs {
        let Some(sig) = v.provenance.d3d else { continue };
        if v.system.is_some() || v.value.scalar != ScalarType::Float {
            continue;
        }
        let Some(register) = inputs.get_mut(sig.register as usize) else {
            continue;
        };
        for lane in (0..4usize).filter(|b| sig.mask >> b & 1 != 0) {
            register[lane] = (f32::from_bits(register[lane]) + off).to_bits();
        }
    }
    inputs
}

/// The buffers as `shader` takes them: each as long as the shader says (the evaluator wants its declared rows exactly,
/// a lifted program's array may be longer), the contents the same word for word.
pub fn refs<'a>(c: &'a Case, shader: &dyn PixelShader) -> Vec<(u32, &'a [[u32; 4]])> {
    shader
        .constant_buffers()
        .iter()
        .filter_map(|&(slot, rows)| {
            let (_, v) = c.buffers.iter().find(|(s, _)| *s == slot)?;
            Some((slot, &v[..(rows as usize).min(v.len())]))
        })
        .collect()
}

/// A game's shader corpus (`oracles.corpus.<game>/shaders`: a folder, searched recursively, of containers as the game
/// ships them): its key and its containers in path order; `None` when it is not configured. A configured corpus without
/// a container fails.
pub fn corpus(game: Game) -> Option<(String, Vec<PathBuf>)> {
    let query = Query::Oracle(OracleSet::Corpus, game, "shaders");
    let root = resolve(query)?;
    let mut paths = Vec::new();
    containers(&root, &mut paths);
    expect_found(query, "DXBC containers", paths.len());
    Some((query.key(), paths))
}

/// How many programs of a corpus a run takes (the debug key `shader.diff_limit`, default all).
pub fn diff_limit() -> usize {
    primase_testdata::debug("shader.diff_limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX)
}

pub fn containers(root: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            containers(&path, out);
        } else if std::fs::read(&path).is_ok_and(|b| b.starts_with(b"DXBC")) {
            out.push(path);
        }
    }
}

/// A refusal's reason with its numbers as `N` (offsets and counts differ program to program; the reason does not).
pub fn reason(refusal: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in refusal.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('N');
            }
            digits = true;
        } else {
            out.push(c);
            digits = false;
        }
    }
    out
}

/// Hold a corpus's refusals (by [`reason`]) to the pinned ones: a reason not pinned, or more programs than its pin,
/// fails; fewer is printed so the pin is lowered.
pub fn hold_to_pins(refused: &BTreeMap<String, usize>, pinned: &[(&str, usize)]) {
    let mut over = Vec::new();
    for (why, &n) in refused {
        match pinned.iter().find(|(p, _)| p == why) {
            Some(&(_, pin)) if n < pin => println!("  {why}: {n}, below its pin of {pin}: lower the pin"),
            Some(&(_, pin)) if n == pin => {}
            Some(&(_, pin)) => over.push(format!("{why}: {n}, pinned at {pin}")),
            None => over.push(format!("{why}: {n}, not pinned")),
        }
    }
    for (why, pin) in pinned {
        if !refused.contains_key(*why) {
            println!("  {why}: 0, below its pin of {pin}: lower the pin");
        }
    }
    assert!(
        over.is_empty(),
        "refusals beyond the pins (a regression):\n{}",
        over.join("\n")
    );
}
