//! Optional private real-data gate over `oracles.corpus.general/codecs.tsv`: game streams with what third-party decoders
//! make of them (`tests/oracles/codecs.py`: Python's zlib and the lz4 package; LZO 2.10's own `lzo2a_decompress_safe`
//! for the lzo2a rows, through `tests/oracles/lzo2a.c`). Each row is
//! `codec<TAB>input<TAB>reference`, a relative path relative to the TSV's folder; bytes stay outside the repository.
//! Codec names: lz4, deflate, zlib, zlib-unterminated, lzo2a. Oodle uses the DLL module's gate instead.
use primase_core::gate::{GateReport, Tolerance};
use primase_foundations::compress::{Decompressor, Deflate, Lz4, Lzo2a, Zlib, ZlibUnterminated};
use primase_testdata::{Game, OracleSet, oracle};
#[test]
fn corpus() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = oracle(OracleSet::Corpus, Game::General, "codecs.tsv") else {
        return Ok(());
    };
    let mut report = GateReport::new("independent real codec corpus", Tolerance::Exact);
    let folder = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let mut rows = 0;
    for line in std::fs::read_to_string(&path)?
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 {
            return Err(format!("invalid TSV line {line}").into());
        }
        let codec: &dyn Decompressor = match fields[0] {
            "lz4" => &Lz4,
            "deflate" => &Deflate,
            "zlib" => &Zlib,
            "zlib-unterminated" => &ZlibUnterminated,
            "lzo2a" => &Lzo2a,
            name => return Err(format!("unknown codec {name}").into()),
        };
        // a relative path is the TSV's folder's
        let input = std::fs::read(folder.join(fields[1]))?;
        let reference = std::fs::read(folder.join(fields[2]))?;
        rows += 1;
        match codec.decompress(&input, reference.len()) {
            Ok(output) => report.compare_bytes(fields[1], &output, &reference),
            Err(e) => report.fail(fields[1], e.to_string()),
        }
    }
    primase_testdata::expect_found(
        primase_testdata::Query::Oracle(OracleSet::Corpus, Game::General, "codecs.tsv"),
        "streams",
        rows,
    );
    println!("{report}");
    report.assert_passed();
    Ok(())
}
