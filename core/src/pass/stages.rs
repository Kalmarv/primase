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

use super::Error;

/// Named stages of texts, in the order they are stated; each name once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stages {
    stages: Vec<(String, Vec<String>)>,
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
