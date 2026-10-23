//! The real-data configuration: the TOML file `PRIMASE_DATA` names, read into one [`Config`].
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
//! ```
//!
//! Every section and key is optional; an unknown section or key is an error (a typo must not skip quietly). For one
//! release the older flat map of `PRIMASE_*` names to strings is read too ([`crate::legacy`]): each of its names
//! becomes the file or folder it stood for, and says so once.
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
    /// What older `PRIMASE_*` names gave, by the query key they stand for ([`crate::legacy`]), with the old name.
    pub legacy: BTreeMap<String, (OsString, &'static str)>,
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
        "run" => Some(vec!["require"]),
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
        // older names set in the environment win over the file, as they did
        for (name, value) in crate::legacy::from_environment(environment) {
            config.legacy_name(name, value);
        }
        Ok(config)
    }

    fn parse(&mut self, path: &Path, text: &str) -> Result<(), ConfigError> {
        let invalid = |detail: String| ConfigError::Invalid {
            path: path.to_owned(),
            detail,
        };
        let table: toml::Table = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        let flat = table.values().all(|v| v.is_str());
        if flat && !table.is_empty() {
            // the older flat map: every name a declared PRIMASE_* name, every value a string
            for (name, value) in &table {
                let value = value.as_str().unwrap_or_default();
                if name == "PRIMASE_REQUIRE_REAL_DATA" {
                    self.require = Some(match value {
                        "1" => "all".to_owned(),
                        other => other.to_owned(),
                    });
                    continue;
                }
                let Some(declared) = crate::declared(name) else {
                    return Err(invalid(format!("{name} is not a declared name")));
                };
                self.legacy_name(declared.name, OsString::from(value));
            }
            return Ok(());
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
                    if !matches!(value, "gate" | "all") {
                        return Err(invalid(format!("run.require = '{value}': 'gate' or 'all'")));
                    }
                    self.require = Some(value.to_owned());
                } else {
                    self.roots
                        .insert(format!("{section}.{key}"), PathBuf::from(value));
                }
            }
        }
        Ok(())
    }

    /// An older name's value, filed under the query key it stands for.
    fn legacy_name(&mut self, name: &'static str, value: OsString) {
        if let Some(query) = crate::legacy::translate(name) {
            self.legacy.insert(query.key(), (value, name));
        }
    }
}

impl Config {
    /// The value `PRIMASE_DEBUG` (read through `environment`) gives `key` (`key=value;key=value`), else what an older
    /// switch's name gave it.
    pub(crate) fn debug(&self, environment: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<String> {
        assert!(
            DEBUG.iter().any(|(k, _)| *k == key),
            "debug key {key} is not declared in toolkit/testdata/src/registry.rs"
        );
        if let Some(all) = environment("PRIMASE_DEBUG") {
            for pair in all.to_string_lossy().split(';') {
                if let Some((k, v)) = pair.split_once('=')
                    && k.trim() == key
                {
                    return Some(v.trim().to_owned());
                }
            }
        }
        self.legacy
            .get(&format!("debug.{key}"))
            .map(|(v, _)| v.to_string_lossy().into_owned())
    }
}
