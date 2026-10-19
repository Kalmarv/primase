//! Dev-only local real-data configuration, with environment overrides.
//! Add this crate only in dev-dependencies. It has no dependency on core.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod inputs;
pub use inputs::{Kind, Status, VARIABLES, Variable, declared};

/// Refuse a name the registry ([`VARIABLES`]) does not declare: a typo must not skip quietly.
#[track_caller]
fn checked(name: &str) -> &'static Variable {
    declared(name).unwrap_or_else(|| {
        panic!("{name} is not declared in toolkit/testdata/src/inputs.rs: declare it there (and in CONTRIBUTING.md)")
    })
}

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
#[track_caller]
pub fn try_real_data_value(name: &str) -> Result<Option<OsString>, DataConfigError> {
    checked(name);
    Ok(lookup(name)?.value)
}

/// Resolve a raw setting for a gate, failing the gate on a broken configuration.
/// Unlike real_data, this accepts flags, filters and output paths that do not yet exist.
#[track_caller]
pub fn real_data_value(name: &str) -> Option<OsString> {
    try_real_data_value(name).unwrap_or_else(|error| panic!("{error}"))
}

/// Resolve an existing real-data path. A name that is not set prints a SKIP line naming the test and returns None,
/// unless `PRIMASE_REQUIRE_REAL_DATA` (environment or the PRIMASE_DATA file) is "1", or "gate" and the variable is one
/// the landing gate must have ([`Variable::gate`]), in which case a missing name fails the gate. A name that is set but whose path does not exist always fails
/// the gate: a configured input that is gone is never a skip. A named but unreadable or invalid
/// configuration also fails. See CONTRIBUTING.md for the names and the optional local TOML file.
#[track_caller]
pub fn real_data(name: &str) -> Option<PathBuf> {
    let variable = checked(name);
    let found = lookup(name).unwrap_or_else(|error| panic!("{error}"));
    let require = lookup(REQUIRE)
        .unwrap_or_else(|error| panic!("{error}"))
        .value
        .is_some_and(|value| value == "1" || (value == "gate" && variable.gate));
    existing_path(name, found, require).unwrap_or_else(|message| panic!("{message}"))
}

/// The bytes of a real-data input file, for a unit test inside a library crate, which has no file access of its own:
/// the file `real_data` resolved, or one its input names.
pub fn read_input(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

const REQUIRE: &str = "PRIMASE_REQUIRE_REAL_DATA";

/// A configured input that yields nothing is a failure, not a pass: call it after enumerating what `name` gave
/// (`what`: "scene folders", "drawables"), with how many were found.
#[track_caller]
pub fn expect_items(name: &str, what: &str, n: usize) {
    let variable = checked(name);
    if n == 0 {
        panic!(
            "{name} is set but yielded 0 {what} (expected: {})",
            variable.about
        );
    }
}

fn existing_path(name: &str, found: Lookup, require: bool) -> Result<Option<PathBuf>, String> {
    match found.value.map(PathBuf::from) {
        Some(path) if path.exists() => Ok(Some(path)),
        Some(path) => Err(format!("{name}={} is set but does not exist", path.display())),
        None if require => Err(format!("{name} is not set and {REQUIRE} requires it")),
        None => {
            // one line per skip, naming the test (the harness names its thread after it), for the gate's summary
            let test = std::thread::current().name().unwrap_or("?").to_owned();
            match found.file {
                Some(file) => eprintln!(
                    "primase-testdata: SKIP {test} ({name} unset, not in {})",
                    file.display()
                ),
                None => eprintln!("primase-testdata: SKIP {test} ({name} unset)"),
            }
            Ok(None)
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
        assert_eq!(existing_path("PRIMASE_OTHER", missing, false), Ok(None));
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
            for require in [false, true] {
                assert_eq!(
                    existing_path(
                        "PRIMASE_TEST",
                        Lookup {
                            value: Some(directory.clone().into_os_string()),
                            file: file.clone()
                        },
                        require
                    ),
                    Ok(Some(directory.clone()))
                );
                // An existing regular file cannot have a child path.
                let missing = std::env::current_exe().unwrap().join("nonexistent-child");
                let error = existing_path(
                    "PRIMASE_TEST",
                    Lookup {
                        value: Some(missing.into_os_string()),
                        file: file.clone(),
                    },
                    require,
                )
                .unwrap_err();
                assert!(error.contains("PRIMASE_TEST") && error.contains("does not exist"));
            }
        }
    }
    #[test]
    fn an_unset_name_skips_unless_real_data_is_required() {
        let unset = || Lookup {
            value: None,
            file: None,
        };
        assert_eq!(existing_path("PRIMASE_TEST", unset(), false), Ok(None));
        let error = existing_path("PRIMASE_TEST", unset(), true).unwrap_err();
        assert!(error.contains("PRIMASE_TEST") && error.contains("PRIMASE_REQUIRE_REAL_DATA"));
    }
    #[test]
    fn undeclared_names_and_empty_inputs_fail() {
        let undeclared = std::panic::catch_unwind(|| real_data_value(concat!("PRIMASE", "_NOT_DECLARED")));
        assert!(undeclared.is_err());
        let empty = std::panic::catch_unwind(|| expect_items("PRIMASE_SCENES_GTA", "scene folders", 0));
        assert!(empty.is_err());
        expect_items("PRIMASE_SCENES_GTA", "scene folders", 1);
        // every variable has something to say and a unique name
        let mut names: Vec<&str> = VARIABLES.iter().map(|v| v.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), VARIABLES.len());
        assert!(
            VARIABLES
                .iter()
                .all(|v| v.name.starts_with("PRIMASE_") && !v.about.is_empty())
        );
        assert!(VARIABLES.iter().all(|v| !v.gate || v.kind == Kind::Input));
    }
    #[test]
    fn every_input_states_its_source_and_retired_ones_leave_the_gate() {
        for v in VARIABLES {
            if v.kind == Kind::Input {
                assert!(!v.source.is_empty(), "{}: say where its data comes from", v.name);
            } else {
                assert!(
                    v.source.is_empty() && v.status == Status::Live,
                    "{}: only inputs have a source and a status",
                    v.name
                );
            }
            assert!(
                !(v.status == Status::Retired && v.gate),
                "{}: a retired obligation is not a landing-gate input",
                v.name
            );
        }
    }
}
