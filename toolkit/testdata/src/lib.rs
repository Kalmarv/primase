//! Dev-only local real-data configuration, with environment overrides.
//! Add this crate only in dev-dependencies. It has no dependency on core.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DataConfigError {
    #[error("resolving {name}: cannot read PRIMASE_DATA file {}: {source}", path.display())]
    Read {
        name: String,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("resolving {name}: PRIMASE_DATA file {} must be a TOML string map: {source}", path.display())]
    Parse {
        name: String,
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
}

struct Lookup {
    value: Option<OsString>,
    file: Option<PathBuf>,
}

fn resolve(
    name: &str,
    environment: impl Fn(&str) -> Option<OsString>,
    read: impl Fn(&Path) -> std::io::Result<String>,
) -> Result<Lookup, DataConfigError> {
    // Even an empty environment value wins. Do not open a file that is not needed.
    if let Some(value) = environment(name) {
        return Ok(Lookup {
            value: Some(value),
            file: None,
        });
    }
    let Some(path) = environment("PRIMASE_DATA").map(PathBuf::from) else {
        return Ok(Lookup {
            value: None,
            file: None,
        });
    };
    let text = read(&path).map_err(|source| DataConfigError::Read {
        name: name.into(),
        path: path.clone(),
        source,
    })?;
    let values: BTreeMap<String, String> =
        toml::from_str(&text).map_err(|source| DataConfigError::Parse {
            name: name.into(),
            path: path.clone(),
            source: Box::new(source),
        })?;
    Ok(Lookup {
        value: values.get(name).map(OsString::from),
        file: Some(path),
    })
}

fn lookup(name: &str) -> Result<Lookup, DataConfigError> {
    resolve(
        name,
        |name| std::env::var_os(name),
        |path| std::fs::read_to_string(path),
    )
}

/// Resolve a raw path or value: environment first, then the explicitly named PRIMASE_DATA
/// TOML file, then None. Every file value must be a string; partial files are normal.
/// No defaults, expansion, or relative-path rewriting is performed. Environment values
/// retain their native OS encoding. The file is read afresh when it is needed.
pub fn try_real_data_value(name: &str) -> Result<Option<OsString>, DataConfigError> {
    Ok(lookup(name)?.value)
}

/// Resolve a raw setting for a gate, failing the gate on a broken configuration.
/// Unlike real_data, this accepts flags, filters and output paths that do not yet exist.
#[track_caller]
pub fn real_data_value(name: &str) -> Option<OsString> {
    try_real_data_value(name).unwrap_or_else(|error| panic!("{error}"))
}

/// Resolve an existing real-data path. Missing names and nonexistent data paths print
/// SKIPPED and return None. A named but unreadable or invalid configuration fails the gate.
/// See CONTRIBUTING.md for the names and the optional local TOML file.
#[track_caller]
pub fn real_data(name: &str) -> Option<PathBuf> {
    let found = lookup(name).unwrap_or_else(|error| panic!("{error}"));
    existing_path(name, found)
}

fn existing_path(name: &str, found: Lookup) -> Option<PathBuf> {
    match found.value.map(PathBuf::from) {
        Some(path) if path.exists() => Some(path),
        Some(path) => {
            eprintln!("SKIPPED: {name}={} does not exist", path.display());
            None
        }
        None => {
            match found.file {
                Some(file) => eprintln!("SKIPPED: {name} not set and not in {}", file.display()),
                None => eprintln!("SKIPPED: {name} not set"),
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn from_file(text: &str, name: &str) -> Result<Lookup, DataConfigError> {
        resolve(
            name,
            |key| (key == "PRIMASE_DATA").then(|| "local-data.toml".into()),
            |_| Ok(text.into()),
        )
    }
    #[test]
    fn environment_wins_without_reading_even_a_bad_file() {
        for value in ["override", ""] {
            let found = resolve(
                "PRIMASE_TEST",
                |name| Some(if name == "PRIMASE_TEST" { value } else { "bad.toml" }.into()),
                |_| panic!("must not read overridden config"),
            )
            .unwrap();
            assert_eq!(found.value, Some(value.into()));
        }
    }
    #[test]
    fn file_values_are_literal_strings_and_missing_names_are_optional() {
        let text = "PRIMASE_TEST = 'placeholder/data'\nPRIMASE_FLAG = '1'\nPRIMASE_FILTER = ''\n";
        assert_eq!(
            from_file(text, "PRIMASE_TEST").unwrap().value,
            Some("placeholder/data".into())
        );
        assert_eq!(from_file(text, "PRIMASE_FLAG").unwrap().value, Some("1".into()));
        assert_eq!(from_file(text, "PRIMASE_FILTER").unwrap().value, Some("".into()));
        let missing = from_file(text, "PRIMASE_OTHER").unwrap();
        assert!(missing.value.is_none());
        assert_eq!(missing.file, Some(PathBuf::from("local-data.toml")));
        assert!(existing_path("PRIMASE_OTHER", missing).is_none());
        let absent = resolve("PRIMASE_TEST", |_| None, |_| panic!("no implicit config")).unwrap();
        assert!(absent.value.is_none());
        assert!(absent.file.is_none());
    }
    #[test]
    fn malformed_and_non_string_maps_fail_with_name_and_file_context() {
        for text in [
            "not valid = [",
            "PRIMASE_TEST = 42",
            "OTHER = false",
            "[nested]\nvalue='x'",
            "PRIMASE_TEST='a'\nPRIMASE_TEST='b'",
        ] {
            let error = from_file(text, "PRIMASE_TEST").err().expect("invalid map");
            assert!(matches!(error, DataConfigError::Parse { .. }));
            let message = error.to_string();
            assert!(message.contains("PRIMASE_TEST"));
            assert!(message.contains("local-data.toml"));
        }
    }
    #[test]
    fn unreadable_files_fail_with_context() {
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidData,
        ] {
            let error = resolve(
                "PRIMASE_TEST",
                |key| (key == "PRIMASE_DATA").then(|| "unreadable.toml".into()),
                |_| Err(std::io::Error::from(kind)),
            )
            .err()
            .expect("read error");
            assert!(matches!(&error, DataConfigError::Read { source, .. } if source.kind() == kind));
            assert!(error.to_string().contains("PRIMASE_TEST"));
            assert!(error.to_string().contains("unreadable.toml"));
        }
    }
    #[test]
    fn path_existence_policy_is_independent_of_value_source() {
        let directory = std::env::temp_dir();
        for file in [None, Some(PathBuf::from("local-data.toml"))] {
            assert_eq!(
                existing_path(
                    "PRIMASE_TEST",
                    Lookup {
                        value: Some(directory.clone().into_os_string()),
                        file: file.clone()
                    }
                ),
                Some(directory.clone())
            );
            // An existing regular file cannot have a child path.
            let missing = std::env::current_exe().unwrap().join("nonexistent-child");
            assert!(
                existing_path(
                    "PRIMASE_TEST",
                    Lookup {
                        value: Some(missing.into_os_string()),
                        file
                    }
                )
                .is_none()
            );
        }
    }
}
