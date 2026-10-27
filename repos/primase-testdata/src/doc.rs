//! A consumer's real-data tables, made from its registry ([`tables`]): its README holds them between [`BEGIN`] and
//! [`END`], and [`crate::check_declared`] compares that block with them, so the two cannot drift.
use crate::registry::{Registry, Status};

/// The line before the tables.
pub const BEGIN: &str =
    "<!-- real-data inputs: made by primase_testdata::doc::tables from tests/inputs/mod.rs -->";
/// The line after them.
pub const END: &str = "<!-- end of real-data inputs -->";

/// A table cell: `|` escaped, an empty text a dash.
fn cell(text: &str) -> String {
    if text.is_empty() {
        "-".to_owned()
    } else {
        text.replace('|', "\\|")
    }
}

/// A table under a heading; none when it has no rows.
fn table(out: &mut String, title: &str, header: &[&str], rows: impl IntoIterator<Item = Vec<String>>) {
    let rows: Vec<Vec<String>> = rows.into_iter().collect();
    if rows.is_empty() {
        return;
    }
    out.push_str(&format!("#### {title}\n\n| {} |\n|", header.join(" | ")));
    out.push_str(&" --- |".repeat(header.len()));
    out.push('\n');
    for row in rows {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out.push('\n');
}

/// The tables, between [`BEGIN`] and [`END`]: the configuration's keys, the oracle sets, every declared file and the
/// debug keys (a table with no rows is left out).
pub fn tables(registry: &Registry) -> String {
    let mut out = format!("{BEGIN}\n\n");
    let gate = |g: bool| if g { "yes" } else { "-" }.to_owned();
    table(
        &mut out,
        "Configuration keys",
        &["key", "gate", "about", "source"],
        registry
            .entries()
            .map(|(key, e)| vec![format!("`{key}`"), gate(e.gate), cell(e.about), cell(e.source)]),
    );
    table(
        &mut out,
        "Oracle sets (`<oracles.root>/<set>/<game>/`)",
        &["set", "about", "source"],
        registry
            .oracle_sets
            .iter()
            .map(|s| vec![format!("`oracles.{}`", s.name), cell(s.about), cell(s.source)]),
    );
    table(
        &mut out,
        "Declared files",
        &["key", "status", "gate", "about", "source"],
        registry.declared.iter().map(|d| {
            let status = match d.status {
                Status::Live => "live".to_owned(),
                Status::Blocked => "blocked".to_owned(),
                Status::Unheld { why } => format!("unheld: {}", cell(why)),
            };
            vec![
                format!("`{}`", d.query.key()),
                status,
                gate(d.gate),
                cell(d.about),
                cell(d.source),
            ]
        }),
    );
    table(
        &mut out,
        "Debug keys (`GAME_DATA_DEBUG`)",
        &["key", "about"],
        registry
            .debug
            .iter()
            .map(|(k, about)| vec![format!("`{k}`"), cell(about)]),
    );
    out.push_str(END);
    out.push('\n');
    out
}
