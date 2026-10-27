//! Elden Ring's material binaries against an independent reader: SoulsFormatsNEXT ee1dd61's MATBIN reading of every
//! material binder the install holds (`matbin.jsonl` of `oracles.soulsformats.elden/format`, one record per member in member
//! order): the shader and source paths and key, every parameter's name, key, type and value (floats as bit patterns)
//! and every sampler's type, path, key and two trailing floats.
use fromsoft_formats::{
    Binder, Game,
    dcx::Decoder,
    matbin::{Material, Value},
};
use primase_testdata::{OracleSet, Query, Tool, expect_found, oodle, oracle};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
/// A binder's members: name and decoded bytes.
type Members = Vec<(Option<String>, Vec<u8>)>;
mod common;

/// A value as the reference writes it: its type code, its words as text, and whether it stores two floats the
/// reference drops (a float3 stored with five).
fn value_text(v: &Value) -> (u32, Vec<String>, usize) {
    let f = |x: &[f32]| {
        x.iter()
            .map(|f| format!("{:08x}", f.to_bits()))
            .collect::<Vec<_>>()
    };
    match v {
        Value::Bool(b) => (0, vec![u8::from(*b).to_string()], 0),
        Value::Int(i) => (4, vec![i.to_string()], 0),
        Value::Int2(a) => (5, a.iter().map(i32::to_string).collect(), 0),
        Value::Float(x) => (8, f(&[*x]), 0),
        Value::Float2(a) => (9, f(a), 0),
        // The reference keeps the first three floats of this type and drops the two stored after them.
        Value::Float3(a, extra) => (10, f(a), usize::from(extra.is_some())),
        Value::Float4(a) => (11, f(a), 0),
        Value::Float5(a) => (12, f(a), 0),
        // Compared as the stored prefix: the reference reads the declared size, past the next item's start.
        Value::Truncated { code, bytes } => (
            *code,
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| format!("{:08x}", u32::from_le_bytes(*w)))
                .collect(),
            0,
        ),
    }
}

#[test]
fn material_binaries_match_an_independent_reading() -> Result<()> {
    let (Some(archive), Some(oracle), Some(dll)) = (
        common::archive(Game::EldenRing)?,
        oracle(OracleSet::SoulsFormats, primase_testdata::Game::Elden, "format"),
        oodle(Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut binders: BTreeMap<String, Members> = BTreeMap::new();
    let mut position: BTreeMap<String, usize> = BTreeMap::new();
    let (mut materials, mut params, mut samplers, mut extra_floats) = (0usize, 0usize, 0usize, 0usize);
    let (mut tails, mut tail_bytes, mut truncated) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for line in std::fs::read_to_string(oracle.join("matbin.jsonl"))?.lines() {
        let record: serde_json::Value = serde_json::from_str(line)?;
        let binder = record["binder"].as_str().ok_or("binder")?.to_owned();
        if !binders.contains_key(&binder) {
            let b = Binder::read(&archive.read(&binder, &decoder)?, &decoder)?;
            binders.insert(
                binder.clone(),
                b.files.into_iter().map(|f| (f.name, f.bytes)).collect(),
            );
        }
        let index = position.entry(binder.clone()).or_default();
        let (name, bytes) = &binders[&binder][*index];
        *index += 1;
        let label = name.clone().unwrap_or_default();
        if name.as_deref() != record["name"].as_str() {
            failures.push(format!("{label}: member order"));
            continue;
        }
        let m = Material::read(bytes).map_err(|e| format!("{label}: {e}"))?;
        materials += 1;
        if !m.unreferenced_tail.is_empty() {
            tails += 1;
            tail_bytes += m.unreferenced_tail.len();
        }
        if Some(m.shader.as_str()) != record["shader"].as_str()
            || Some(m.source.as_str()) != record["source"].as_str()
            || Some(u64::from(m.key)) != record["key"].as_u64()
        {
            failures.push(format!("{label}: header"));
        }
        let want = record["params"].as_array().ok_or("params")?;
        if want.len() != m.params.len() {
            failures.push(format!("{label}: parameter count"));
            continue;
        }
        for (p, w) in m.params.iter().zip(want) {
            params += 1;
            let (code, values, extra) = value_text(&p.value);
            extra_floats += extra;
            truncated += usize::from(matches!(p.value, Value::Truncated { .. }));
            let want_values: Vec<&str> = w["value"]
                .as_array()
                .ok_or("value")?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            if Some(p.name.as_str()) != w["name"].as_str()
                || Some(u64::from(p.key)) != w["key"].as_u64()
                || Some(u64::from(code)) != w["type"].as_u64()
                || !(values == want_values
                    || matches!(p.value, Value::Truncated { .. })
                        && values.len() <= want_values.len()
                        && values.iter().zip(&want_values).all(|(a, b)| a == b))
            {
                failures.push(format!("{label}: parameter {}", p.name));
            }
        }
        let want = record["samplers"].as_array().ok_or("samplers")?;
        if want.len() != m.samplers.len() {
            failures.push(format!("{label}: sampler count"));
            continue;
        }
        for (s, w) in m.samplers.iter().zip(want) {
            samplers += 1;
            let unknown: Vec<String> = s.unknown.iter().map(|f| format!("{:08x}", f.to_bits())).collect();
            let want_unknown: Vec<&str> = w["unknown"]
                .as_array()
                .ok_or("unknown")?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            if Some(s.kind.as_str()) != w["type"].as_str()
                || Some(s.path.as_str()) != w["path"].as_str()
                || Some(u64::from(s.key)) != w["key"].as_u64()
                || unknown != want_unknown
            {
                failures.push(format!("{label}: sampler {}", s.kind));
            }
        }
    }
    for (binder, files) in &binders {
        if position.get(binder) != Some(&files.len()) {
            failures.push(format!(
                "{binder}: the reference lists {:?} of {} members",
                position.get(binder),
                files.len()
            ));
        }
    }
    println!(
        "materials {materials}, parameters {params}, samplers {samplers}, unreferenced tails {tails} ({tail_bytes} \
         bytes), values stored shorter than their type {truncated}, float3 values with two more stored floats (kept; \
         the reference drops them) {extra_floats}, mismatches {}",
        failures.len()
    );
    for f in failures.iter().take(40) {
        println!("MISMATCH {f}");
    }
    expect_found(
        Query::Oracle(OracleSet::SoulsFormats, primase_testdata::Game::Elden, "format"),
        "material binary records in matbin.jsonl",
        materials,
    );
    assert!(failures.is_empty());
    Ok(())
}
