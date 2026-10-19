//! `--to web`: web tiles ([`primase_web::convert::tiles`]: the tiles plan their own parts). Writes `<out>` (compressed,
//! instanced) and, with `--plain`, `<out>/plain` (the plain comparison set, for gates).
use std::path::Path;

use primase_web::Options;

use crate::convert::Result;

/// `[--tile-m N] [--ktx2 [--cache <folder> [--cache-max-gib N] [--cache-verify]] [--encoding-memory-mib N]]
/// [--budget-mib N | --no-budget] [--plain]`. This command's choices unless told otherwise: 200 m cells, files of at
/// most 96 MiB. KTX2 encodings are kept in the cache folder across runs, else in `PRIMASE_CACHE` when set; in memory,
/// at most `--encoding-memory-mib` (512 by default) between tiles. The writer writes textures at the sizes it is
/// given: `prepare-web-textures{...}` in `--passes` chooses them.
pub fn writer(out: &Path, args: &[String]) -> Result<crate::convert::Destination> {
    let (mut tile_m, mut plain) = (200., false);
    let mut options = Options::new(primase_web::TileSize::new(tile_m)?);
    options.tile_byte_budget = Some(96 << 20);
    #[cfg(feature = "ktx2")]
    let (mut cache_dir, mut cache_max_gib) = (None::<std::path::PathBuf>, None::<u64>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("web: {a} needs a value"));
        match a.as_str() {
            "--tile-m" => {
                tile_m = value()?
                    .parse()
                    .map_err(|_| "web: --tile-m is a number of metres")?;
                options.tile_size = primase_web::TileSize::new(tile_m)?;
            }
            "--ktx2" => {
                #[cfg(feature = "ktx2")]
                {
                    options.ktx2 = true;
                }
                #[cfg(not(feature = "ktx2"))]
                return Err("web: --ktx2 needs primase built with the ktx2 feature".into());
            }
            #[cfg(feature = "ktx2")]
            "--cache" => cache_dir = Some(value()?.into()),
            #[cfg(feature = "ktx2")]
            "--cache-max-gib" => {
                cache_max_gib = Some(value()?.parse().map_err(|_| "web: --cache-max-gib is a number")?)
            }
            #[cfg(feature = "ktx2")]
            "--cache-verify" => options.cache_verify = true,
            #[cfg(feature = "ktx2")]
            "--encoding-memory-mib" => {
                let mib: usize = value()?
                    .parse()
                    .map_err(|_| "web: --encoding-memory-mib is a number")?;
                options.encoding_memory = mib << 20;
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
    #[cfg(feature = "ktx2")]
    if options.ktx2 {
        // --cache, else PRIMASE_CACHE when set and not empty, else no disk cache
        let configured = cache_dir.or_else(|| {
            std::env::var_os("PRIMASE_CACHE")
                .filter(|v| !v.is_empty())
                .map(std::path::PathBuf::from)
        });
        if let Some(dir) = configured {
            let max = cache_max_gib.map_or(primase_cache::DEFAULT_MAX_BYTES, |g| g << 30);
            options.cache = Some(std::sync::Arc::new(primase_cache::Cache::open(&dir, max)?));
        }
    } else if cache_dir.is_some() || cache_max_gib.is_some() || options.cache_verify {
        return Err("web: --cache, --cache-max-gib and --cache-verify go with --ktx2".into());
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
    let (parts, writer) = primase_web::convert::tiles(sets);
    Ok((Box::new(parts), Box::new(writer)))
}
