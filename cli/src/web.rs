//! `--to web`: web tiles for the viewer ([`primase_web::convert::TileWriter`]). Writes `<out>` (compressed,
//! instanced) and, with `--plain`, `<out>/plain` (the plain comparison set, for gates).
use std::path::Path;

use primase_core::convert::Writer;
use primase_web::Options;
use primase_web::convert::TileWriter;

use crate::convert::Result;

/// `[--ktx2] [--budget-mib N | --no-budget] [--plain]`
pub fn writer(out: &Path, args: &[String]) -> Result<Box<dyn Writer>> {
    let (mut options, mut plain) = (Options::default(), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("web: {a} needs a value"));
        match a.as_str() {
            "--ktx2" => {
                #[cfg(feature = "ktx2")]
                {
                    options.ktx2 = true;
                }
                #[cfg(not(feature = "ktx2"))]
                return Err("web: --ktx2 needs primase built with the ktx2 feature".into());
            }
            "--budget-mib" => {
                let mib: u64 = value()?.parse().map_err(|_| "web: --budget-mib is a number")?;
                options.tile_byte_budget = Some(mib * 1024 * 1024);
            }
            "--no-budget" => options.tile_byte_budget = None,
            "--plain" => plain = true,
            _ => return Err(format!("web: unknown option {a}").into()),
        }
    }
    let mut sets = vec![(options.clone(), out.to_path_buf())];
    if plain {
        let plain = Options {
            compress: false,
            gpu_instancing: false,
            #[cfg(feature = "ktx2")]
            ktx2: false,
            ..options
        };
        sets.push((plain, out.join("plain")));
    }
    Ok(Box::new(TileWriter::new(sets)))
}
