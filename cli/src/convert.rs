//! `convert`: one reader, the passes, one writer ([`primase_core::convert`]). Readers and writers are registered once,
//! here; this module parses the command line, builds them and the pipelines, runs the conversion and prints its report.
use std::collections::BTreeMap;
use std::path::Path;

use primase_core::convert::{ConvertReport, PartSize, PartSource, Reader, Writer, convert_parts};
use primase_core::pass::{PassManager, PassRegistry};

pub type Result<T> = primase_core::convert::Result<T>;

pub const USAGE: &str = "usage: primase convert --from <reader> [reader options] --select \"<pipeline>\" --passes \"<pipeline>\" --to <writer> \
<out> [writer options]
--select and --passes are the conversion's choices (no reader supplies them). For a GTA region the gta crate documents
one recipe: primase_gta::pipeline::select_passes and DEFAULT_PASSES.
readers:
  gta   --gta <install> --keys <key folder> --region <legion|vespucci|x0,y0,x1,y1> [--map online|story] [--hour H | --keep-hour]
        [--min-size M] [--max-priority N] [--interiors none|all]
writers:
  web      [--ktx2 [--cache <folder> [--cache-max-gib N] [--cache-verify]] [--encoding-memory-mib N]]
           [--budget-mib N | --no-budget] [--plain]
  reskate  [--name N] [--blend-objects N] (after --passes \"<reader's>, bake-normal-scale, raise-skate{name=N}, lower-layered-reskate\"; a spawn from place-spawn at --select)";

/// Every pass by name: core's, the toolkit's and each world crate's.
pub fn registry() -> PassRegistry {
    let mut r = PassRegistry::new();
    r.register("select-lod", primase_core::select_lod);
    r.register("drop-optional", primase_core::drop_optional);
    r.register("place-spawn", primase_core::place_spawn);
    r.register("resolve-state", primase_core::state::resolve_state);
    // cap-textures, bake-layer-channels, normalize-normals and bake-normal-scale
    primase_image::register(&mut r);
    primase_meshops::register(&mut r);
    primase_gta::register(&mut r);
    primase_skate_scene::register(&mut r);
    primase_reskate::register(&mut r);
    r
}

type ReaderFactory = fn(&[String]) -> Result<Box<dyn Reader>>;
/// A destination: how it divides the selection into parts, and its writer.
pub type Destination = (Box<dyn PartSource>, Box<dyn Writer>);
type WriterFactory = fn(&Path, &[String]) -> Result<Destination>;

/// The readers, by `--from` name.
pub const READERS: &[(&str, ReaderFactory)] = &[("gta", crate::gta::reader)];
/// The writers, by `--to` name.
pub const WRITERS: &[(&str, WriterFactory)] =
    &[("web", crate::web::writer), ("reskate", crate::reskate::writer)];

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
    for n in r.notices() {
        println!("notice: {n}");
    }
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
    let (mut parts, writer) = find(WRITERS, writer_name, "writer")?(out, &args[to + 3..])?;
    let registry = registry();
    // the selection and the passes are the conversion's choices: no reader supplies them
    let (Some(select_text), Some(passes_text)) = (select_text, passes_text) else {
        return Err(format!("--select and --passes are required; {USAGE}").into());
    };
    let select = registry.parse(&select_text)?;
    let passes = registry.parse(&passes_text)?;
    println!("select: {select}");
    println!("passes: {passes}");
    let manager = PassManager::new(registry);
    let report = match convert_parts(
        reader.as_mut(),
        parts.as_mut(),
        writer,
        &manager,
        &select,
        &passes,
    ) {
        Ok(report) => report,
        Err(e) => {
            // what was done before the failure, then the failure
            if let Some(failed) = e.downcast_ref::<primase_core::convert::ConvertError>() {
                print(&failed.partial);
            }
            return Err(e);
        }
    };
    print(&report);
    Ok(())
}

#[cfg(test)]
mod tests {
    use primase_gta::pipeline::{Map, Selection, default_passes, select_passes};

    /// Every pipeline a conversion composes from this crate's readers and writers passes the checks made from the
    /// passes' declarations (Pipeline::check), with no game data: the reader's selection and its default passes for
    /// the selection's variants, apart and in the order convert runs them, and the ReSkate writer's documented passes
    /// after them.
    #[test]
    fn every_default_pipeline_passes_the_declaration_checks() {
        let registry = super::registry();
        let mut pipelines = Vec::new();
        for map in [Map::Story, Map::Online] {
            for hour in [None, Some(12), Some(23)] {
                for interiors in [true, false] {
                    let mut s = Selection::new([0.0, 0.0, 100.0, 100.0], map);
                    s.hour = hour;
                    s.interiors = interiors;
                    let (select, passes) = (select_passes(&s), default_passes(&s));
                    let reskate =
                        format!("{passes}, bake-normal-scale, raise-skate{{name=n}}, lower-layered-reskate");
                    pipelines.push(format!("{select}, {passes}"));
                    pipelines.extend([select, passes, reskate]);
                }
            }
        }
        for text in pipelines {
            let pipeline = registry
                .parse(&text)
                .unwrap_or_else(|e| panic!("{text}: does not parse: {e}"));
            if let Err(e) = pipeline.check() {
                panic!("{text}: {e}");
            }
        }
    }

    /// Every library pass a conversion may name or plan is registered (bake-nonrigid lowers non-rigid transforms for
    /// writers that take rigid ones).
    #[test]
    fn the_registry_knows_bake_nonrigid() {
        let pass = super::registry()
            .make("bake-nonrigid", &primase_core::pass::Options::new())
            .expect("primase_meshops::register is wired into registry()");
        assert_eq!(pass.name(), "bake-nonrigid");
    }
}
