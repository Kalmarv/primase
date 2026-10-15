//! `primase`: thin compositions of readers, passes and writers. If a subcommand needs more than piping, the
//! missing piece belongs in a crate.

fn main() {
    eprintln!("primase {}: no subcommands yet", env!("CARGO_PKG_VERSION"));
    std::process::exit(2);
}
