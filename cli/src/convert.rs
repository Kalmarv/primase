//! `convert`: one reader, the passes, one writer ([`primase_core::convert`]). Readers and writers are registered once,
//! here; this module parses the command line, builds them and the pipelines, runs the conversion and prints its report.
use std::collections::BTreeMap;
use std::path::Path;

use primase_core::convert::{ConvertReport, PartSize, Reader, Writer, convert};
use primase_core::pass::{PassManager, PassRegistry};

pub type Result<T> = primase_core::convert::Result<T>;

pub const USAGE: &str = "usage: primase convert --from <reader> [reader options] [--select \"<pipeline>\"] [--passes \"<pipeline>\"] --to <writer> \
<out> [writer options]
readers:
  gta   --gta <install> --keys <key folder> --region <legion|vespucci|x0,y0,x1,y1> [--map online|story] [--hour H]
        [--min-size M] [--max-priority N] [--interiors none|all]
writers:
  web   [--ktx2] [--budget-mib N | --no-budget] [--plain]";

/// Every pass by name: core's, the toolkit's and each world crate's.
pub fn registry() -> PassRegistry {
    let mut r = PassRegistry::new();
    r.register("select-lod", primase_core::select_lod);
    r.register("drop-optional", primase_core::drop_optional);
    r.register("resolve-state", primase_core::state::resolve_state);
    r.register("normalize-normals", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(primase_image::NormalizeNormals)),
    });
    primase_gta::register(&mut r);
    r
}

type ReaderFactory = fn(&[String]) -> Result<Box<dyn Reader>>;
type WriterFactory = fn(&Path, &[String]) -> Result<Box<dyn Writer>>;

/// The readers, by `--from` name.
pub const READERS: &[(&str, ReaderFactory)] = &[("gta", crate::gta::reader)];
/// The writers, by `--to` name.
pub const WRITERS: &[(&str, WriterFactory)] = &[("web", crate::web::writer)];

/// The entry of `table` named `name`.
fn find<T: Copy>(table: &[(&str, T)], name: &str, kind: &str) -> Result<T> {
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
        .ok_or_else(|| format!("no {kind} {name}; {USAGE}").into())
}

/// The conversion's report, as the log shows it.
fn print(r: &ConvertReport) {
    for (k, v) in &r.read {
        println!("{k}: {v}");
    }
    for step in &r.select.steps {
        println!("select: {}: {:?}", step.pass, step.log.counts);
    }
    for (k, v) in &r.loads {
        println!("{k}: {v}");
    }
    println!("parts loaded: {}", r.parts);
    let mib = |b: u64| b as f64 / 1_048_576.0;
    let show = |what: &str, p: &PartSize, names: &BTreeMap<String, u64>| {
        let by_name: Vec<String> = names.iter().map(|(n, b)| format!("{n} {:.1}", mib(*b))).collect();
        println!(
            "largest part {what}: {:.1} MiB ({} roots; {})",
            mib(names.values().sum()),
            p.roots,
            by_name.join(", ")
        );
    };
    if let Some(p) = r.part_sizes.iter().max_by_key(|p| p.loaded_total()) {
        show("as loaded", p, &p.loaded);
    }
    if let Some(p) = r.part_sizes.iter().max_by_key(|p| p.written_total()) {
        show("at the writer", p, &p.written);
    }
    println!("passes over all parts: {:?}", r.counts);
    for (k, v) in &r.written {
        println!("{k}: {v}");
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let at = |flag: &str| args.iter().position(|a| a == flag);
    let (Some(from), Some(to)) = (at("--from"), at("--to")) else {
        return Err(USAGE.into());
    };
    if from != 0 || to < from + 2 || to + 2 > args.len() {
        return Err(USAGE.into());
    }
    let reader_name = &args[from + 1];
    let mut reader_args: Vec<String> = args[from + 2..to].to_vec();
    let mut take = |flag: &str| -> Result<Option<String>> {
        let Some(p) = reader_args.iter().position(|a| a == flag) else {
            return Ok(None);
        };
        let text = reader_args.get(p + 1).ok_or(USAGE)?.clone();
        reader_args.drain(p..p + 2);
        Ok(Some(text))
    };
    let passes_text = take("--passes")?;
    let select_text = take("--select")?;
    let writer_name = &args[to + 1];
    let out = Path::new(args.get(to + 2).ok_or(USAGE)?);
    let mut reader = find(READERS, reader_name, "reader")?(&reader_args)?;
    let writer = find(WRITERS, writer_name, "writer")?(out, &args[to + 3..])?;
    let registry = registry();
    let select = registry.parse(&select_text.unwrap_or_else(|| reader.default_select()))?;
    let passes = registry.parse(&passes_text.unwrap_or_else(|| reader.default_passes()))?;
    println!("select: {select}");
    println!("passes: {passes}");
    let manager = PassManager::new(registry);
    let report = convert(reader.as_mut(), writer, &manager, &select, &passes)?;
    print(&report);
    Ok(())
}
