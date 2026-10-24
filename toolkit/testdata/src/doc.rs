//! CONTRIBUTING.md's real-data tables, made from the registry ([`tables`]): the architecture check compares the
//! document's block with them, so the two cannot drift.
use crate::registry::{self, Status};

/// The line before the tables in CONTRIBUTING.md.
pub const BEGIN: &str =
    "<!-- real-data tables: made by primase_testdata::doc::tables from toolkit/testdata/src/registry.rs -->";
/// The line after them.
pub const END: &str = "<!-- end of real-data tables -->";

/// A table cell: `|` escaped, an empty text a dash.
fn cell(text: &str) -> String {
    if text.is_empty() {
        "-".to_owned()
    } else {
        text.replace('|', "\\|")
    }
}

fn table(out: &mut String, title: &str, header: &[&str], rows: impl IntoIterator<Item = Vec<String>>) {
    out.push_str(&format!("#### {title}\n\n| {} |\n|", header.join(" | ")));
    out.push_str(&" --- |".repeat(header.len()));
    out.push('\n');
    for row in rows {
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out.push('\n');
}

/// The tables, between [`BEGIN`] and [`END`]: the configuration's keys, the oracle sets, every declared file and the
/// debug keys.
pub fn tables() -> String {
    let mut out = format!("{BEGIN}\n\n");
    let gate = |g: bool| if g { "yes" } else { "-" }.to_owned();
    table(
        &mut out,
        "Configuration keys",
        &["key", "gate", "about", "source"],
        registry::INSTALLS
            .iter()
            .chain(registry::KEYS)
            .chain(registry::TOOLS)
            .chain([&registry::SCRATCH])
            .map(|e| {
                vec![
                    format!("`{}`", e.key),
                    gate(e.gate),
                    cell(e.about),
                    cell(e.source),
                ]
            }),
    );
    table(
        &mut out,
        "Oracle sets (`<oracles.root>/<set>/<game>/`)",
        &["set", "about", "source"],
        registry::ORACLE_SETS
            .iter()
            .map(|s| vec![format!("`{}`", s.key), cell(s.about), cell(s.source)]),
    );
    table(
        &mut out,
        "Declared files",
        &["key", "status", "gate", "about", "source"],
        registry::DECLARED.iter().map(|d| {
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
        "Debug keys (`PRIMASE_DEBUG`)",
        &["key", "about"],
        registry::DEBUG
            .iter()
            .map(|(k, about)| vec![format!("`{k}`"), cell(about)]),
    );
    out.push_str(END);
    out.push('\n');
    out
}
