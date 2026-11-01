//! A program's stages as text: what one program ran, written for another to run as written. A `[name]` line opens a
//! stage and the stage's texts follow it, one per line (a pass's text, [`crate::pass::pass_text`], or another
//! statement's, [`crate::pass::text_form`]); a line starting with `#` is a comment and a blank line is nothing.
//!
//! ```text
//! # Stormveil Castle's slice, as the example states it
//! [selection]
//! select-fromsoft-area{environment=file:m10_00_0000,region=rect:-162:5:108:250}
//! [read]
//! read-fromsoft-area{area=m10_00_00_00,margin=30,...}
//! [lowering]
//! lower-fromsoft-render-graph
//! ```
//!
//! The file says which stage each text belongs to and in which order the texts run; what a stage means, and which
//! stages a program needs, is the program's.
//!
//! **Parameters.** Where a value is the user's to pick (which NPC row dresses a character, which clip binds), the file
//! writes a parameter in its place, `$name`, as an option's whole value: `select-fromsoft-dressing{npc=$npc,empty=show}`.
//! The program running the stages binds them to the user's picks ([`Stages::bind`]) before it parses them. A value the
//! file states is never a pick: a file that writes `npc=first-map` takes no `npc` pick, and one given is refused. A
//! parameter may state the value that stands when nothing is picked, after a `|`: `npc=$npc|first-map` is the user's
//! NPC row, else `first-map`. That fallback is a plain value, the program's choice, checked as the option's value is,
//! and what each parameter took (its pick or its fallback) is said ([`Binding`]).

use std::collections::{BTreeMap, BTreeSet};

use super::{Error, parse_pipeline, stated};

/// What one parameter of a bound text took: the user's pick, or the fallback the file states beside it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Binding {
    pub stage: String,
    /// The pass (or text form) whose option it is.
    pub text: String,
    pub option: String,
    pub parameter: String,
    pub value: String,
    /// Whether the value is the user's pick (else the file's fallback).
    pub picked: bool,
}

impl std::fmt::Display for Binding {
    /// `[selection] select-fromsoft-dressing npc=$npc: first-map (fallback, no pick)`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} {}=${}: {} ({})",
            self.stage,
            self.text,
            self.option,
            self.parameter,
            self.value,
            if self.picked {
                "picked"
            } else {
                "fallback, no pick"
            }
        )
    }
}

/// Stages bound to picks ([`Stages::bind`]), with what each parameter took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub stages: Stages,
    pub bindings: Vec<Binding>,
}

/// Named stages of texts, in the order they are stated; each name once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stages {
    stages: Vec<(String, Vec<String>)>,
}

/// Whether `name` can name a parameter: letters, digits, `-` and `_`.
fn is_parameter(name: &str) -> bool {
    is_stage_name(name)
}

/// Whether `name` can name a stage: letters, digits, `-` and `_`.
fn is_stage_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl Stages {
    /// No stages.
    pub fn new() -> Self {
        Stages::default()
    }

    /// `self` with stage `name` and its `texts` after the others; refused where the name is taken, cannot name a
    /// stage, or a text is empty, holds a line break or starts as a comment or a stage would.
    pub fn with(mut self, name: &str, texts: Vec<String>) -> Result<Self, Error> {
        if !is_stage_name(name) {
            return Err(Error::Parse(format!(
                "{name:?} cannot name a stage (letters, digits, - and _)"
            )));
        }
        if self.stage(name).is_some() {
            return Err(Error::Parse(format!("stage [{name}] twice")));
        }
        if let Some(text) = texts.iter().find(|text| {
            let line = text.trim();
            line.is_empty() || line.contains(['\n', '\r']) || line.starts_with(['#', '['])
        }) {
            return Err(Error::Parse(format!(
                "stage [{name}]: {text:?} is not a text of one line"
            )));
        }
        self.stages.push((
            name.to_owned(),
            texts.into_iter().map(|text| text.trim().to_owned()).collect(),
        ));
        Ok(self)
    }

    /// The stages `text` states ([`Stages::text`] writes it); refused, naming the line, where a text comes before
    /// any stage, a stage is named twice or a `[` line names none.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut stages = Stages::new();
        for (index, line) in text.lines().enumerate() {
            let at = |message: String| Error::Parse(format!("line {}: {message}", index + 1));
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[') {
                let name = name
                    .strip_suffix(']')
                    .ok_or_else(|| at(format!("{line:?} opens no stage: write [name]")))?
                    .trim();
                stages = stages.with(name, Vec::new()).map_err(|e| at(e.to_string()))?;
                continue;
            }
            match stages.stages.last_mut() {
                Some((_, texts)) => texts.push(line.to_owned()),
                None => {
                    return Err(at(format!(
                        "{line:?} comes before any stage: open one with [name]"
                    )));
                }
            }
        }
        Ok(stages)
    }

    /// The text [`Stages::parse`] reads: `comment`'s lines first, each after `# `, then each stage.
    pub fn text(&self, comment: &str) -> String {
        let mut out = String::new();
        for line in comment.lines() {
            out.push_str(format!("# {line}").trim_end());
            out.push('\n');
        }
        for (name, texts) in &self.stages {
            out.push_str(&format!("[{name}]\n"));
            for text in texts {
                out.push_str(text);
                out.push('\n');
            }
        }
        out
    }

    /// The stages with the parameters of stages `names` bound to `picks`: each option of their texts whose value is
    /// `$p` (or `$p|fallback`) takes `picks[p]`, else its fallback; the other stages kept as they are; with what each
    /// parameter took. Refused, naming them, where a stage of `names` is not stated, a parameter of those stages has
    /// no pick and no fallback, a fallback is empty or names a parameter, a pick is taken by none of their parameters
    /// (so a user never takes a pick as applied that was not), or a pick could not stand as a value (a brace or a
    /// line break).
    pub fn bind(&self, names: &[&str], picks: &BTreeMap<String, String>) -> Result<Bound, Error> {
        if let Some(name) = names.iter().find(|name| self.stage(name).is_none()) {
            return Err(Error::Parse(format!("no stage [{name}] to bind")));
        }
        if let Some((name, pick)) = picks
            .iter()
            .find(|(_, pick)| pick.contains(['{', '}', '\n', '\r']))
        {
            return Err(Error::Parse(format!(
                "the pick {name}={pick:?} cannot stand as a value (a brace or a line break)"
            )));
        }
        let mut used: BTreeSet<String> = BTreeSet::new();
        let mut unpicked: BTreeSet<String> = BTreeSet::new();
        let mut bindings = Vec::new();
        let mut bound = self.clone();
        for (stage, texts) in &mut bound.stages {
            if !names.contains(&stage.as_str()) {
                continue;
            }
            for text in texts.iter_mut() {
                let mut items =
                    parse_pipeline(text).map_err(|e| Error::Parse(format!("stage [{stage}]: {e}")))?;
                let mut parameters = 0;
                for (name, options) in &mut items {
                    for (option, value) in options.iter_mut() {
                        let Some(written) = value.strip_prefix('$').map(str::to_owned) else {
                            continue;
                        };
                        let (parameter, fallback) = match written.split_once('|') {
                            Some((parameter, fallback)) => {
                                (parameter.to_owned(), Some(fallback.trim().to_owned()))
                            }
                            None => (written.clone(), None),
                        };
                        if !is_parameter(&parameter) {
                            return Err(Error::Parse(format!(
                                "stage [{stage}]: ${parameter} cannot name a parameter (letters, digits, - and _)"
                            )));
                        }
                        if let Some(fallback) = &fallback
                            && (fallback.is_empty() || fallback.starts_with('$'))
                        {
                            return Err(Error::Parse(format!(
                                "stage [{stage}]: {name} {option}=${written}: a fallback is a plain value, not empty \
                                 and not a parameter"
                            )));
                        }
                        parameters += 1;
                        let took = match (picks.get(&parameter), fallback) {
                            (Some(pick), _) => Some((pick.clone(), true)),
                            (None, Some(fallback)) => Some((fallback, false)),
                            (None, None) => None,
                        };
                        match took {
                            Some((taken, picked)) => {
                                *value = taken.clone();
                                bindings.push(Binding {
                                    stage: stage.clone(),
                                    text: name.clone(),
                                    option: option.clone(),
                                    parameter: parameter.clone(),
                                    value: taken,
                                    picked,
                                });
                            }
                            None => {
                                unpicked.insert(parameter.clone());
                            }
                        }
                        used.insert(parameter);
                    }
                }
                if parameters > 0 {
                    *text = items
                        .iter()
                        .map(|(name, options)| stated(name, options))
                        .collect::<Vec<_>>()
                        .join(", ");
                }
            }
        }
        let stages = format!("[{}]", names.join("], ["));
        if !unpicked.is_empty() {
            let names: Vec<String> = unpicked.iter().map(|p| format!("${p}")).collect();
            return Err(Error::Parse(format!(
                "no pick for {}, which {stages} take",
                names.join(", ")
            )));
        }
        let unused: Vec<&str> = picks
            .keys()
            .map(String::as_str)
            .filter(|pick| !used.contains(*pick))
            .collect();
        if !unused.is_empty() {
            return Err(Error::Parse(format!(
                "{stages} take no pick {}: it would not apply",
                unused.join(", ")
            )));
        }
        Ok(Bound {
            stages: bound,
            bindings,
        })
    }

    /// Stage `name`'s texts, in order, if it is stated (it may be stated with none).
    pub fn stage(&self, name: &str) -> Option<&[String]> {
        self.stages
            .iter()
            .find(|(stage, _)| stage == name)
            .map(|(_, texts)| texts.as_slice())
    }

    /// The stages' names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.stages.iter().map(|(name, _)| name.as_str())
    }

    /// Refused, naming them, where a stage of `needed` is not stated or a stage is stated that is neither needed nor
    /// one of `known` (stages the program may be given): what a program that runs these stages checks before it runs
    /// any.
    pub fn check(&self, needed: &[&str], known: &[&str]) -> Result<(), Error> {
        let missing: Vec<&str> = needed
            .iter()
            .copied()
            .filter(|name| self.stage(name).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(Error::Parse(format!(
                "no stage [{}]: the stages needed are [{}]",
                missing.join("], ["),
                needed.join("], [")
            )));
        }
        let allowed: Vec<&str> = needed.iter().chain(known).copied().collect();
        if let Some(other) = self.names().find(|name| !allowed.contains(name)) {
            return Err(Error::Parse(format!(
                "stage [{other}] is none of [{}]",
                allowed.join("], [")
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|t| (*t).to_owned()).collect()
    }

    /// What is written reads back, the comment dropped; a stage may be stated with no texts.
    #[test]
    fn stages_read_back_from_their_text() {
        let stages = Stages::new()
            .with("selection", texts(&["select-fromsoft-area{region=rect:0:0:1:1}"]))
            .unwrap()
            .with("read", texts(&["read-toy{cell=4,detail=plain}"]))
            .unwrap()
            .with("writer", Vec::new())
            .unwrap()
            .with("lowering", texts(&["select-lod{form=finest}", "place-assets"]))
            .unwrap();
        let text = stages.text("written by a test\nfrom its values");
        assert!(
            text.starts_with("# written by a test\n# from its values\n[selection]\n"),
            "{text}"
        );
        assert_eq!(Stages::parse(&text).unwrap(), stages);
        assert_eq!(
            stages.names().collect::<Vec<_>>(),
            ["selection", "read", "writer", "lowering"]
        );
        assert_eq!(stages.stage("writer"), Some(&[][..]));
        assert_eq!(
            stages.stage("lowering").unwrap(),
            ["select-lod{form=finest}", "place-assets"]
        );
        assert_eq!(stages.stage("clips"), None);
    }

    /// Each refusal names the line.
    #[test]
    fn a_stages_text_refuses_naming_the_line() {
        let refusal = |text: &str| Stages::parse(text).unwrap_err().to_string();
        assert_eq!(
            refusal("# c\nplace-assets"),
            "line 2: \"place-assets\" comes before any stage: open one with [name]"
        );
        assert_eq!(refusal("[read]\n[read]"), "line 2: stage [read] twice");
        assert_eq!(
            refusal("[read\nx"),
            "line 1: \"[read\" opens no stage: write [name]"
        );
        assert!(refusal("[a b]").starts_with("line 1: \"a b\" cannot name a stage"));
        assert!(Stages::new().with("read", texts(&["a\nb"])).is_err());
        assert!(Stages::new().with("read", texts(&["# a"])).is_err());
    }

    fn picks(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// Each parameter of the stages bound takes its pick, as an option's whole value; the other stages keep theirs.
    #[test]
    fn parameters_take_the_picks() {
        let stages = Stages::parse(
            "[selection]\nselect-fromsoft-dressing{npc=$npc,empty=show}\n\
             [read]\nread-fromsoft-character{id=$id,role=npc,weapons=false}\n\
             [clips]\nbind-fromsoft-animation{clips=$clip,rate=2}",
        )
        .unwrap();
        let bound = stages
            .bind(
                &["selection", "read"],
                &picks(&[("npc", "30000014"), ("id", "c1010")]),
            )
            .unwrap()
            .stages;
        assert_eq!(
            bound.stage("selection").unwrap(),
            ["select-fromsoft-dressing{empty=show,npc=30000014}"]
        );
        assert_eq!(
            bound.stage("read").unwrap(),
            ["read-fromsoft-character{id=c1010,role=npc,weapons=false}"]
        );
        assert_eq!(bound.stage("clips"), stages.stage("clips"));
        let clip = bound
            .bind(&["clips"], &picks(&[("clip", "a000_000020")]))
            .unwrap()
            .stages;
        assert_eq!(
            clip.stage("clips").unwrap(),
            ["bind-fromsoft-animation{clips=a000_000020,rate=2}"]
        );
        // a stage with no parameter is kept as written
        let plain = Stages::parse("[lowering]\nplace-assets").unwrap();
        assert_eq!(plain.bind(&["lowering"], &BTreeMap::new()).unwrap().stages, plain);
    }

    /// A parameter's fallback stands where nothing is picked, and a pick replaces it; each parameter says which it
    /// took. A fallback that is empty or names a parameter is refused.
    #[test]
    fn a_fallback_stands_where_nothing_is_picked() {
        let stages = Stages::parse(
            "[selection]\nselect-fromsoft-dressing{npc=$npc|first-map,empty=show}\nselect-fromsoft-area{region=$region|preview}",
        )
        .unwrap();
        let bound = stages
            .bind(&["selection"], &picks(&[("region", "rect:0:0:10:10")]))
            .unwrap();
        assert_eq!(
            bound.stages.stage("selection").unwrap(),
            [
                "select-fromsoft-dressing{empty=show,npc=first-map}",
                "select-fromsoft-area{region=rect:0:0:10:10}"
            ]
        );
        let said: Vec<String> = bound.bindings.iter().map(ToString::to_string).collect();
        assert_eq!(
            said,
            [
                "[selection] select-fromsoft-dressing npc=$npc: first-map (fallback, no pick)",
                "[selection] select-fromsoft-area region=$region: rect:0:0:10:10 (picked)"
            ]
        );
        for written in ["$npc|", "$npc|$other"] {
            let text = format!("[selection]\nselect-fromsoft-dressing{{npc={written},empty=show}}");
            assert!(
                Stages::parse(&text)
                    .unwrap()
                    .bind(&["selection"], &BTreeMap::new())
                    .unwrap_err()
                    .to_string()
                    .contains("a fallback is a plain value"),
                "{written}"
            );
        }
    }

    /// A parameter with no pick, a pick no parameter takes, a stage that is not there and a pick that cannot be a value
    /// are refused, naming them; a stated value takes no pick.
    #[test]
    fn binding_refuses_naming_the_parameter_or_the_pick() {
        let stages = Stages::parse(
            "[selection]\nselect-fromsoft-dressing{npc=$npc,empty=show}\nselect-fromsoft-area{region=$region}",
        )
        .unwrap();
        let refusal = |names: &[&str], items: &[(&str, &str)]| {
            stages.bind(names, &picks(items)).unwrap_err().to_string()
        };
        assert_eq!(
            refusal(&["selection"], &[]),
            "no pick for $npc, $region, which [selection] take"
        );
        assert_eq!(
            refusal(
                &["selection"],
                &[("npc", "1"), ("region", "whole"), ("outfit", "x")]
            ),
            "[selection] take no pick outfit: it would not apply"
        );
        assert_eq!(refusal(&["read"], &[]), "no stage [read] to bind");
        assert!(
            refusal(&["selection"], &[("npc", "{1}"), ("region", "whole")])
                .contains("cannot stand as a value")
        );
        let fixed = Stages::parse("[selection]\nselect-fromsoft-dressing{npc=first-map,empty=show}").unwrap();
        assert_eq!(
            fixed
                .bind(&["selection"], &picks(&[("npc", "30000014")]))
                .map(|bound| bound.stages)
                .unwrap_err()
                .to_string(),
            "[selection] take no pick npc: it would not apply"
        );
    }

    /// A program refuses a file without a stage it needs, or with one it does not know, naming them.
    #[test]
    fn a_program_checks_the_stages_it_runs() {
        let stages = Stages::parse("[read]\nx\n[lowering]\ny").unwrap();
        assert!(stages.check(&["read", "lowering"], &["writer"]).is_ok());
        assert_eq!(
            stages
                .check(&["selection", "read", "lowering"], &[])
                .unwrap_err()
                .to_string(),
            "no stage [selection]: the stages needed are [selection], [read], [lowering]"
        );
        assert_eq!(
            stages.check(&["read"], &["writer"]).unwrap_err().to_string(),
            "stage [lowering] is none of [read], [writer]"
        );
    }
}
