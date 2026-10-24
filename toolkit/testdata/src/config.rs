//! The real-data configuration: the TOML file `PRIMASE_DATA` names, read into one `Config`.
//!
//! ```toml
//! [installs]
//! sekiro = 'D:/placeholder/Sekiro'
//! [keys]
//! gta = 'D:/placeholder/gta-keys'
//! [tools]
//! warp = 'D:/placeholder/warp.exe'
//! [oracles]
//! root = 'D:/placeholder/oracles'   # <root>/<set>/<game>/<file>
//! [scratch]
//! root = 'D:/placeholder/scratch'
//! [run]
//! require = 'gate'                  # or 'all': an unset input fails instead of skipping
//! gate = 'smoke'                    # or 'full': how much the heavy real-data tests cover (smoke by default)
//! seed = '20384'                     # the smoke sample's seed (the day's by default), to rerun one exactly
//! threads = '1'                     # the threads a heavy real-data test's work may use (one by default)
//! ```
//!
//! Every section and key is optional; an unknown section or key is an error (a typo must not skip quietly), and so is
//! the older flat map of `PRIMASE_*` names to strings.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::registry::{DEBUG, INSTALLS, KEYS, TOOLS};

/// What the configuration file says.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Config {
    /// The file it was read from; `None` when `PRIMASE_DATA` is unset.
    pub file: Option<PathBuf>,
    /// The structured sections, by full key (`installs.sekiro`, `tools.warp`, `oracles.root`, `scratch.root`).
    pub roots: BTreeMap<String, PathBuf>,
    /// `[run] require`: `gate` or `all`.
    pub require: Option<String>,
    /// `[run] gate`: `smoke` or `full`.
    pub gate: Option<String>,
    /// `[run] seed`: a smoke sample's seed.
    pub seed: Option<u64>,
    /// `[run] threads`: the threads a heavy real-data test's work may use.
    pub threads: Option<std::num::NonZeroUsize>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read PRIMASE_DATA file {}: {source}", path.display())]
    Read { path: PathBuf, source: std::io::Error },
    #[error("PRIMASE_DATA file {}: {detail}", path.display())]
    Invalid { path: PathBuf, detail: String },
}

/// The sections of the structured format and the keys each may hold (`*`: any declared key of that section).
fn section_keys(section: &str) -> Option<Vec<&'static str>> {
    let of = |entries: &'static [crate::registry::Entry], prefix: &str| -> Vec<&'static str> {
        entries
            .iter()
            .filter_map(|e| e.key.strip_prefix(prefix))
            .collect()
    };
    match section {
        "installs" => Some(of(INSTALLS, "installs.")),
        "keys" => Some(of(KEYS, "keys.")),
        "tools" => Some(of(TOOLS, "tools.")),
        "oracles" => Some(vec!["root"]),
        "scratch" => Some(vec!["root"]),
        "run" => Some(vec!["require", "gate", "seed", "threads"]),
        _ => None,
    }
}

impl Config {
    /// The configuration `PRIMASE_DATA` (read through `environment`) names, read with `read`; empty when unset.
    pub(crate) fn load(
        environment: &impl Fn(&str) -> Option<OsString>,
        read: impl Fn(&Path) -> std::io::Result<String>,
    ) -> Result<Config, ConfigError> {
        let mut config = Config::default();
        if let Some(path) = environment("PRIMASE_DATA").map(PathBuf::from) {
            let text = read(&path).map_err(|source| ConfigError::Read {
                path: path.clone(),
                source,
            })?;
            config.parse(&path, &text)?;
            config.file = Some(path);
        }
        Ok(config)
    }

    fn parse(&mut self, path: &Path, text: &str) -> Result<(), ConfigError> {
        let invalid = |detail: String| ConfigError::Invalid {
            path: path.to_owned(),
            detail,
        };
        let table: toml::Table = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        if !table.is_empty() && table.values().all(|v| v.is_str()) {
            return Err(invalid(
                "the older flat map of PRIMASE_* names: write the structured sections".into(),
            ));
        }
        for (section, body) in &table {
            let Some(keys) = section_keys(section) else {
                return Err(invalid(format!("unknown section [{section}]")));
            };
            let body = body
                .as_table()
                .ok_or_else(|| invalid(format!("[{section}] is not a table")))?;
            for (key, value) in body {
                if !keys.contains(&key.as_str()) {
                    return Err(invalid(format!("unknown key {key} in [{section}]")));
                }
                let value = value
                    .as_str()
                    .ok_or_else(|| invalid(format!("{section}.{key} is not a string")))?;
                if section == "run" {
                    match key.as_str() {
                        "require" if matches!(value, "gate" | "all") => self.require = Some(value.to_owned()),
                        "require" => {
                            return Err(invalid(format!("run.require = '{value}': 'gate' or 'all'")));
                        }
                        "gate" if matches!(value, "smoke" | "full") => self.gate = Some(value.to_owned()),
                        "gate" => return Err(invalid(format!("run.gate = '{value}': 'smoke' or 'full'"))),
                        "seed" => {
                            self.seed = Some(
                                value
                                    .parse()
                                    .map_err(|_| invalid(format!("run.seed = '{value}': a whole number")))?,
                            )
                        }
                        _ => {
                            self.threads = Some(value.parse().map_err(|_| {
                                invalid(format!("run.threads = '{value}': a whole number, at least 1"))
                            })?)
                        }
                    }
                } else {
                    self.roots
                        .insert(format!("{section}.{key}"), PathBuf::from(value));
                }
            }
        }
        Ok(())
    }
}

/// Check `text` as a configuration file (the example file's check): its sections and keys known, its values strings,
/// `[run] require` one of `gate` and `all`, `[run] gate` one of `smoke` and `full`, `[run] seed` a whole number, `[run]
/// threads` a whole number from 1. The older flat map is refused.
pub fn validate(text: &str) -> Result<(), ConfigError> {
    Config::default().parse(Path::new("(text)"), text)
}

/// The value `PRIMASE_DEBUG` (read through `environment`) gives `key` (`key=value;key=value`).
pub(crate) fn debug(environment: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<String> {
    assert!(
        DEBUG.iter().any(|(k, _)| *k == key),
        "debug key {key} is not declared in toolkit/testdata/src/registry.rs"
    );
    let all = environment("PRIMASE_DEBUG")?;
    all.to_string_lossy().split(';').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_owned())
    })
}
