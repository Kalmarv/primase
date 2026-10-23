//! The version gate of a lifter. A lifter's `LIFTER_VERSION` and the program representation's [`crate::PROGRAM_VERSION`] are
//! what a cache keyed by a bytecode's hash trusts to know that a program it holds is stale, so a change to what a lifter
//! makes of a bytecode must come with a bump. The gate makes that checkable: a lifter's tests lift its committed fixtures,
//! [`table`] hashes each outcome (the program's stable text form, or the refusal) under the versions the lifter has, the
//! table is committed, and [`check`] fails when the outcomes changed and the versions did not (bump the one that names the
//! change), or the versions changed and the committed table is not that version's.

use std::fmt::Write;

use sha2::{Digest, Sha256};

/// One fixture's outcome: the text form of the program lifted, or the refusal's reason.
pub type Outcome = Result<String, String>;

/// The table of `outcomes` (fixture name, outcome) for `lifter` at these versions: a header of the versions, then a line per
/// fixture in the order given.
pub fn table(lifter: &str, lifter_version: u32, outcomes: &[(String, Outcome)]) -> String {
    let mut out = format!(
        "# lifted fixtures of {lifter}\n# versions lifter={lifter_version} program={}\n",
        crate::PROGRAM_VERSION
    );
    for (name, outcome) in outcomes {
        let _ = match outcome {
            Ok(text) => writeln!(out, "{name}\tlifted\t{}", hex(&Sha256::digest(text.as_bytes()))),
            Err(reason) => writeln!(out, "{name}\trefused\t{reason}"),
        };
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn versions(table: &str) -> Option<&str> {
    table.lines().find(|l| l.starts_with("# versions "))
}

fn entries(table: &str) -> impl Iterator<Item = &str> {
    table.lines().filter(|l| !l.starts_with('#'))
}

/// Whether the `current` table is the committed one; if not, what to do about it.
pub fn check(committed: &str, current: &str) -> Result<(), String> {
    if committed == current {
        return Ok(());
    }
    let changed: Vec<&str> = entries(current)
        .filter(|line| !entries(committed).any(|c| c == *line))
        .filter_map(|line| line.split('\t').next())
        .collect();
    let same_versions = versions(committed) == versions(current);
    let what = if changed.is_empty() {
        "the fixtures' outcomes are the committed ones".to_owned()
    } else {
        format!(
            "the outcome of {} fixtures differs from the committed one: {}",
            changed.len(),
            changed.join(", ")
        )
    };
    Err(if same_versions {
        format!(
            "what the lifter makes of its fixtures changed and its versions did not ({what}): bump LIFTER_VERSION (a change of \
             the lifter) or PROGRAM_VERSION (a change of the program representation), then replace the committed table by \
             the current one"
        )
    } else {
        format!(
            "the versions are not the committed table's ({} now, {} committed; {what}): replace the committed table by the \
             current one",
            versions(current).unwrap_or("none"),
            versions(committed).unwrap_or("none"),
        )
    })
}
