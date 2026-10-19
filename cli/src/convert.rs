//! `convert`: one reader, the passes, one writer ([`primase_core::convert`]). Readers and writers are registered once,
//! here; this module parses the command line, builds them and the pipelines, runs the conversion and prints its report.
use std::collections::BTreeMap;
use std::path::Path;

use primase_core::convert::{ConvertReport, PartSize, PartSource, Reader, Writer, convert_parts};
use primase_core::pass::{PassManager, PassRegistry};

pub type Result<T> = primase_core::convert::Result<T>;

pub const USAGE: &str = "usage: primase convert --from <reader> [reader options] --select \"<pipeline>\" --passes \"<pipeline>\" --to <writer> \
<out> [writer options]
--conservation off|report|enforce: count what each pass leaves out against what it declares (off unless given;
report: differences as diagnostic notices; enforce: a difference fails the pass).
--select and --passes are the conversion's choices, written out (no reader or writer supplies them): for a GTA region
the map state and hour (resolve-state{assign=gta.map:online;gta.hour:12}), the region rules (gta-select-region{...}),
lights, collision and water, then each part's GTA passes.
readers:
  gta   --gta <install> --keys <key folder> --region <legion|vespucci|x0,y0,x1,y1> [--map online|story] [--reach M]
writers:
  web      [--ktx2 [--cache <folder> [--cache-max-gib N] [--cache-verify]] [--encoding-memory-mib N]]
           [--budget-mib N | --no-budget] [--plain]
  reskate  [--name N] (after --passes \"<the part passes>, bake-normal-scale, raise-skate{name=N}, lower-layered-reskate,
           cap-textures{max=256,normals=box}, prepare-reskate-textures, snap-reskate-markers,
           prepare-reskate-overlays{step=0.004}, prepare-reskate-blend-order{grid=200},
           plan-reskate-blend-budget{objects=pool}\", and a pause map if wanted:
           prepare-reskate-pause-map{from=geometry,height=2160,extent=content}; a spawn from place-spawn at --select)";

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
    primase_reskate::register(&mut r);
    primase_unity::register(&mut r);
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
    let conservation = match take("--conservation")?.as_deref() {
        None | Some("off") => primase_core::pass::Conservation::Off,
        Some("report") => primase_core::pass::Conservation::Report,
        Some("enforce") => primase_core::pass::Conservation::Enforce,
        Some(v) => return Err(format!("--conservation {v}: off, report or enforce").into()),
    };
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
    let manager = PassManager::new(registry).with_conservation(conservation);
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
    /// Pipelines a GTA conversion may be given (examples of the passes in their order: a selection by day, at night
    /// and with the hour kept, each part's GTA passes, and the ReSkate writer's documented passes after them) pass the
    /// checks made from the passes' declarations (Pipeline::check), apart and in the order convert runs them, with no
    /// game data.
    #[test]
    fn gta_pipelines_pass_the_declaration_checks() {
        let registry = super::registry();
        let rect = "x0=0,y0=0,x1=100,y1=100";
        let leave = "foliage=leave-out,river=leave-out";
        let mut pipelines = Vec::new();
        for (assign, lights, night) in [
            ("assign=gta.map:online;gta.hour:12", "", "false"),
            ("assign=gta.map:story;gta.hour:23", "hour=23,", "true"),
            ("assign=gta.map:online,keep=gta.hour", "state=true,", "state"),
        ] {
            for interiors in ["", " gta-leave-out-interiors,"] {
                let select = format!(
                    "resolve-state{{{assign}}}, gta-select-region{{{rect},min_size=0.5}},{interiors} \
                     lower-gta-lights{{{lights}{rect}}}, gta-select-collision, lower-gta-collision{{{rect},{leave}}}, \
                     lower-gta-surfaces, gta-select-water{{{rect},clip=kept}}"
                );
                let passes = format!(
                    "select-lod{{form=finest}}, lower-gta-embedded-collision{{{leave}}}, lower-gta-surfaces, \
                     gta-pose-fragments, \
                     lower-gta-pbr{{night={night}}}, gta-winding, gta-tint, gta-terrain-layers, gta-vertex-colours, \
                     lower-gta-water, normalize-normals"
                );
                let reskate = format!(
                    "{passes}, bake-normal-scale, raise-skate{{name=n}}, lower-layered-reskate, \
                     cap-textures{{max=256,normals=box}}, prepare-reskate-textures, budget-reskate-textures{{max=64}}, snap-reskate-markers, \
                     prepare-reskate-pause-map{{from=geometry,height=2160,extent=content}}, \
                     prepare-reskate-overlays{{step=0.004}}, prepare-reskate-blend-order{{grid=200}}, \
                     plan-reskate-blend-budget{{objects=pool}}"
                );
                pipelines.push(format!("{select}, {passes}"));
                pipelines.extend([select, passes, reskate]);
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

    /// Every registered pass describes its settings in its options: made as registered, written as pipeline text,
    /// parsed back, it is the same pass with the same options (so a typed pass and its text agree).
    #[test]
    fn every_registered_pass_round_trips_through_its_text() {
        let registry = super::registry();
        let names: Vec<&str> = registry.names().collect();
        assert!(!names.is_empty());
        for name in names {
            let made = registry
                .make_registered(name)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let text = primase_core::pass::pass_text(made.as_ref());
            let parsed = registry
                .parse(&text)
                .unwrap_or_else(|e| panic!("{name}: its own text {text} does not parse: {e}"));
            assert_eq!(parsed.passes.len(), 1, "{text}");
            let back = &parsed.passes[0];
            assert_eq!(back.name(), made.name(), "{text}");
            assert_eq!(
                back.options(),
                made.options(),
                "{name}: {text} parses to other options"
            );
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
