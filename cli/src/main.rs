//! `primase`: thin compositions of readers, passes and writers. If a subcommand needs more than piping, the
//! missing piece belongs in a crate.
//!
//! `primase convert --from <reader> [reader options] [--select "<pipeline>"] [--passes "<pipeline>"] --to <writer>
//! <out> [writer options]` reads a world with one of [`convert::READERS`], runs the selection on it, and writes it
//! with one of [`convert::WRITERS`], each part the writer asks for loaded by the reader and run through the passes
//! (the reader's defaults without `--select` and `--passes`).
mod convert;
mod gta;
mod web;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("convert") => convert::run(&args[1..]),
        _ => Err(format!("primase {}\n{}", env!("CARGO_PKG_VERSION"), convert::USAGE).into()),
    };
    if let Err(e) = result {
        eprintln!("primase: {e}");
        std::process::exit(2);
    }
}
