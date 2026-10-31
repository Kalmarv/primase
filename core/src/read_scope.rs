//! Caller-stated reader scopes: declarations reuse pass options, never introduce reader defaults.
//!
//! A costly read may load only stated data families, levels of detail or forms. The reader retains
//! its unloaded alternatives as metadata and reports the scope. Selection and lowering remain
//! registered passes; this declaration supplies no value for an absent caller choice.
use serde::Serialize;

use crate::convert::{Notice, NoticeKind};
use crate::pass::{OptionSpec, Options};
use crate::{JsonError, Resource, World};

/// The named scope options a reader accepts. Examples document values; they never fill a choice.
#[derive(Clone, Copy, Debug)]
pub struct ReadScopeSpec {
    /// The reader's stable name, independent of an application's source aliases.
    pub name: &'static str,
    /// Required choices, one-of groups, or optional values with a stated meaning for absence.
    pub options: &'static [OptionSpec],
}

impl ReadScopeSpec {
    /// Check caller choices using the same declarations and rules as registered pass options.
    pub fn check(&self, choices: &Options) -> Result<(), String> {
        crate::pass_options::validate_options(self.name, self.options);
        crate::pass_options::check_options(self.name, self.options, choices)
    }

    /// Retain the caller's exact choices. Missing required values are refused, not inferred.
    pub fn resolve(&self, choices: Options) -> Result<ReadScope, String> {
        self.check(&choices)?;
        Ok(ReadScope {
            reader: self.name.into(),
            choices,
            unloaded: Vec::new(),
        })
    }

    /// Record a source alternative that was not loaded under this scope, without inventing payload.
    /// `source` is its source identity (a member, prototype or named family), not a local machine path.
    pub fn record_unloaded(
        &self,
        scope: &mut ReadScope,
        option: &str,
        value: &str,
        source: impl Into<String>,
    ) -> Result<(), String> {
        if scope.reader != self.name {
            return Err("unloaded alternative belongs to another reader scope".into());
        }
        let declared = self
            .options
            .iter()
            .find(|item| item.key() == option)
            .ok_or_else(|| format!("unloaded alternative names undeclared scope option {option}"))?;
        if declared.values().is_some_and(|values| !values.contains(&value)) {
            return Err(format!("unloaded alternative has unclassified {option}={value}"));
        }
        let alternative = UnloadedAlternative {
            option: option.into(),
            value: value.into(),
            source: source.into(),
        };
        if alternative.source.trim().is_empty() || value.trim().is_empty() {
            return Err("unloaded alternative needs a value and source identity".into());
        }
        if !scope.unloaded.contains(&alternative) {
            scope.unloaded.push(alternative);
        }
        Ok(())
    }
}

/// A combination of scope choices a reader's crate offers by name: hand-picked and labelled so, with what is known of
/// why. Nothing applies it: a caller names the preset it wants, and the choices are then the caller's, stated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ReadPreset {
    /// The preset's name, stable within its reader.
    pub name: &'static str,
    /// The reader scope ([`ReadScopeSpec::name`]) whose options it states.
    pub reader: &'static str,
    /// Option and value, each an option the scope declares and a value it accepts.
    pub choices: &'static [(&'static str, &'static str)],
    /// What the choices are and the evidence for them, saying where they are observed and where only picked.
    pub note: &'static str,
}

impl ReadPreset {
    /// The choices as options, checked against `spec`'s declarations.
    pub fn options(&self, spec: &ReadScopeSpec) -> Result<Options, String> {
        if spec.name != self.reader {
            return Err(format!(
                "preset {} belongs to the {} scope, not {}",
                self.name, self.reader, spec.name
            ));
        }
        let options: Options = self
            .choices
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        for key in options.keys() {
            if !spec.options.iter().any(|item| item.key() == key) {
                return Err(format!("preset {} states undeclared option {key}", self.name));
            }
        }
        Ok(options)
    }
}

/// Metadata for a source candidate the caller's scope did not load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnloadedAlternative {
    /// The declared scope dimension (for example a form or data family).
    pub option: String,
    /// The candidate's value in that dimension.
    pub value: String,
    /// The source identity retained without loading its payload.
    pub source: String,
}

/// A reader's effective scope and the alternatives it left unloaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReadScope {
    /// The stable reader name.
    pub reader: String,
    /// Exactly the caller's checked choices; absent optional values remain absent.
    pub choices: Options,
    /// Source candidates not loaded under that scope, retained as metadata.
    pub unloaded: Vec<UnloadedAlternative>,
}

impl ReadScope {
    /// Retain this scope in the world without replacing scopes from other readers or earlier reads.
    pub fn record_in(&self, world: &mut World) {
        let mut scopes = world.resource::<ReadScopes>().cloned().unwrap_or_default();
        if !scopes.0.contains(self) {
            scopes.0.push(self.clone());
        }
        world.insert_resource(scopes);
    }

    /// The scope as a generic selection notice; diagnostics never select content for the reader.
    pub fn notice(&self) -> Notice {
        let choices = self
            .choices
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut notice = Notice::new(
            &self.reader,
            NoticeKind::Selection,
            format!(
                "caller-stated read scope {{{choices}}}; {} alternatives retained unloaded",
                self.unloaded.len()
            ),
            1,
        );
        for alternative in &self.unloaded {
            notice = notice.example(&alternative.source);
        }
        notice
    }
}

/// Read scopes retained in the world as portable metadata, including unloaded source identities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReadScopes(pub Vec<ReadScope>);

impl Resource for ReadScopes {
    const NAME: &'static str = "core.read_scopes";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _world: &World) -> Result<(), String> {
        for scope in &self.0 {
            if scope.reader.trim().is_empty() {
                return Err("read scope needs a reader name".into());
            }
            for alternative in &scope.unloaded {
                if alternative.option.trim().is_empty()
                    || alternative.value.trim().is_empty()
                    || alternative.source.trim().is_empty()
                {
                    return Err(
                        "unloaded alternative needs a scope option, value and source identity".into(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::Choice;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Payload {
        List,
        Load,
    }
    impl Choice for Payload {
        const VALUES: &'static [(&'static str, Self)] = &[("list", Self::List), ("load", Self::Load)];
    }
    const SPEC: ReadScopeSpec = ReadScopeSpec {
        name: "test-reader",
        options: &[
            OptionSpec::choice::<Payload>("payload", "Which payloads the caller asks to load"),
            OptionSpec::optional("forms", "all", "Named forms", "No form restriction"),
        ],
    };

    #[test]
    fn declarations_never_fill_required_or_optional_choices() {
        assert!(SPEC.resolve(Options::new()).is_err());
        assert!(
            SPEC.resolve(Options::from([("payload".into(), "guess".into())]))
                .is_err()
        );
        let scope = SPEC
            .resolve(Options::from([("payload".into(), "list".into())]))
            .unwrap();
        assert_eq!(scope.choices.len(), 1);
        assert!(!scope.choices.contains_key("forms"));
    }

    #[test]
    fn unloaded_source_identities_and_scope_survive_portable_reporting() {
        let mut scope = SPEC
            .resolve(Options::from([("payload".into(), "list".into())]))
            .unwrap();
        SPEC.record_unloaded(&mut scope, "payload", "load", "prototype-a")
            .unwrap();
        assert!(
            SPEC.record_unloaded(&mut scope, "other", "load", "prototype-b")
                .is_err()
        );
        assert_eq!(scope.notice().kind, NoticeKind::Selection);
        assert_eq!(scope.notice().examples, ["prototype-a"]);
        let resource = ReadScopes(vec![scope]);
        assert!(resource.verify(&World::new()).is_ok());
        let json = resource.to_json().unwrap().unwrap();
        assert_eq!(json[0]["choices"]["payload"], "list");
        assert_eq!(json[0]["unloaded"][0]["source"], "prototype-a");
    }
}
