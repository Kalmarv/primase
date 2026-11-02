//! What the shader-database tests share: the database's distinct programs, the work on each of them spread over the
//! threads the configuration lets a heavy test use, and the independent reader of the constant lanes a program's emitted
//! WGSL takes.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::shader::Shader;
use primase_core::{Entity, World};
use primase_foundations::parallel::{self, Threads};
use primase_gta::components::{GtaEffect, GtaEffectProgram, GtaShaderTable};
use rage_engine_formats::sgd2_table::ProgramTable;

/// A program of the shader database, where it first appears: the effect, the effect's entry for it, its shader and its
/// constant table.
pub struct Program<'w> {
    pub effect: &'w GtaEffect,
    pub program: &'w GtaEffectProgram,
    pub entity: Entity,
    pub shader: &'w Shader,
    pub table: Option<&'w ProgramTable>,
}

/// Every program of the shader database once (programs are shared between effects), in the effects' order.
pub fn programs(world: &World) -> Vec<Program<'_>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (_, effect) in world.iter::<GtaEffect>() {
        for program in &effect.programs {
            let Some(entity) = program.shader else {
                continue;
            };
            if !seen.insert(entity) {
                continue;
            }
            let Some(shader) = world.get::<Shader>(entity) else {
                continue;
            };
            out.push(Program {
                effect,
                program,
                entity,
                shader,
                table: world.get::<GtaShaderTable>(entity).map(|t| &t.0),
            });
        }
    }
    out
}

/// `f` of every item, on the threads the configuration lets a heavy test use (`[run] threads`); the results in item
/// order, so whatever is folded from them is the same on any thread count.
pub fn each<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    parallel::map(Threads(crate::inputs::threads()), items, |_, item| f(item)).unwrap()
}

/// The lanes of rows a program reads, from its WGSL (the emitter writes only what the program uses; the lifted IR also
/// holds an access per lane it never uses): (space, register, row, lane) directly taken, rows used otherwise (a whole
/// vector), and buffers indexed by a value that is not a literal.
pub type Reads = (
    BTreeSet<(u32, u32, u32, u32)>,
    BTreeSet<(u32, u32, u32)>,
    BTreeSet<(u32, u32)>,
);

/// [`Reads`] of a lifted program, read off the WGSL it emits.
pub fn wgsl_reads(program: &iridia::Program) -> Result<Reads, String> {
    let text = iridia::emit::wgsl(program).map_err(|e| format!("{e:?}"))?;
    // buffer variable name -> (space, register)
    let mut buffers: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if let Some(b) = l
            .trim()
            .strip_prefix("@group(0) @binding(")
            .and_then(|r| r.strip_suffix(')'))
            && let Ok(b) = b.trim().parse::<u32>()
            && let Some(v) = lines
                .get(i + 1)
                .and_then(|n| n.trim().strip_prefix("var<uniform> "))
            && let Some((name, _)) = v.split_once(':')
        {
            buffers.insert(name.to_owned(), (b >> 16, b & 0xffff));
        }
    }
    let (mut lanes, mut whole, mut dynamic) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    // let _eK = cbN_[ROW]; -> K -> (space, reg, row)
    let mut loaded: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    for l in &lines {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("let ")
            && let Some((var, rhs)) = rest.split_once(" = ")
            && let Some((buf, idx)) = rhs.trim_end_matches(';').split_once('[')
            && let Some(&(sp, rg)) = buffers.get(buf)
            && let Some(idx) = idx.strip_suffix(']')
        {
            match idx.parse::<u32>() {
                Ok(row) => {
                    loaded.insert(var.to_owned(), (sp, rg, row));
                }
                Err(_) => {
                    dynamic.insert((sp, rg));
                }
            }
        }
    }
    for (var, &(sp, rg, row)) in &loaded {
        let mut any_lane = false;
        let mut other = false;
        for l in &lines {
            let mut rest: &str = l;
            let declared = l.trim().starts_with(&format!("let {var} = "));
            if declared {
                continue;
            }
            while let Some(at) = rest.find(var.as_str()) {
                let after = &rest[at + var.len()..];
                let before_ok = rest[..at]
                    .chars()
                    .last()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                if before_ok {
                    match after.strip_prefix('.').and_then(|a| a.chars().next()) {
                        Some(c @ ('x' | 'y' | 'z' | 'w')) => {
                            lanes.insert((sp, rg, row, "xyzw".find(c).unwrap() as u32));
                            any_lane = true;
                        }
                        _ if after
                            .chars()
                            .next()
                            .is_none_or(|c| !(c.is_alphanumeric() || c == '_')) =>
                        {
                            other = true
                        }
                        _ => {}
                    }
                }
                rest = after;
            }
        }
        if other || !any_lane {
            whole.insert((sp, rg, row));
        }
    }
    Ok((lanes, whole, dynamic))
}
