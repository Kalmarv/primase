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

/// Whether the `current` table is the committed one; if not, what to do about it. A fixture whose outcome differs from
/// the committed one's needs a bump of the version that names the change; one added or taken away says nothing of what
/// the lifter makes of the others, so the committed table is replaced without a bump.
pub fn check(committed: &str, current: &str) -> Result<(), String> {
    if committed == current {
        return Ok(());
    }
    let name = |line: &str| line.split('\t').next().unwrap_or_default().to_owned();
    let committed_names: Vec<String> = entries(committed).map(name).collect();
    let current_names: Vec<String> = entries(current).map(name).collect();
    let changed: Vec<String> = entries(current)
        .filter(|line| !entries(committed).any(|c| c == *line))
        .map(name)
        .filter(|n| committed_names.contains(n))
        .collect();
    let added: Vec<&String> = current_names
        .iter()
        .filter(|n| !committed_names.contains(n))
        .collect();
    let removed: Vec<&String> = committed_names
        .iter()
        .filter(|n| !current_names.contains(n))
        .collect();
    let same_versions = versions(committed) == versions(current);
    let mut what = Vec::new();
    if !changed.is_empty() {
        what.push(format!(
            "the outcome of {} fixtures differs from the committed one: {}",
            changed.len(),
            changed.join(", ")
        ));
    }
    for (list, how) in [(&added, "added"), (&removed, "taken away")] {
        if !list.is_empty() {
            let names: Vec<&str> = list.iter().map(|n| n.as_str()).collect();
            what.push(format!("{} fixtures {how}: {}", names.len(), names.join(", ")));
        }
    }
    let what = if what.is_empty() {
        "the fixtures' outcomes are the committed ones".to_owned()
    } else {
        what.join("; ")
    };
    Err(if same_versions && !changed.is_empty() {
        format!(
            "what the lifter makes of its fixtures changed and its versions did not ({what}): bump LIFTER_VERSION (a change of \
             the lifter) or PROGRAM_VERSION (a change of the program representation), then replace the committed table by \
             the current one"
        )
    } else if same_versions {
        format!(
            "the fixtures are not the committed table's ({what}): replace the committed table by the current one"
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

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "# lifted fixtures of l\n# versions lifter=1 program=2\n";

    /// A changed outcome asks for a bump; an added or removed fixture only for the table; a bumped version for the table.
    #[test]
    fn a_changed_outcome_needs_a_bump_and_an_added_fixture_only_the_table() {
        let committed = format!("{HEAD}a\tlifted\t00\nb\trefused\tno\n");
        assert_eq!(check(&committed, &committed), Ok(()));
        let changed = check(&committed, &format!("{HEAD}a\tlifted\t01\nb\trefused\tno\n")).unwrap_err();
        assert!(
            changed.contains("bump") && changed.contains("1 fixtures differs") && changed.contains(": a"),
            "{changed}"
        );
        let added = check(
            &committed,
            &format!("{HEAD}a\tlifted\t00\nb\trefused\tno\nc\tlifted\t02\n"),
        )
        .unwrap_err();
        assert!(
            !added.contains("bump") && added.contains("1 fixtures added: c"),
            "{added}"
        );
        let removed = check(&committed, &format!("{HEAD}a\tlifted\t00\n")).unwrap_err();
        assert!(
            !removed.contains("bump") && removed.contains("1 fixtures taken away: b"),
            "{removed}"
        );
        let bumped = "# lifted fixtures of l\n# versions lifter=2 program=2\na\tlifted\t01\nb\trefused\tno\n";
        let e = check(&committed, bumped).unwrap_err();
        assert!(!e.contains("bump LIFTER") && e.contains("lifter=2"), "{e}");
    }
}
