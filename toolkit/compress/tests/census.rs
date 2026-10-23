//! Optional private real-data gate. The TSV is produced by independent existing
//! Python/C# container readers; bytes stay outside the repository. Codec names:
//! lz4, deflate, zlib, zlib-unterminated. Oodle uses the DLL module's gate instead.
use primase_compress::{Decompressor, Deflate, Lz4, Zlib, ZlibUnterminated};
use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::{Game, OracleSet, oracle};
#[test]
fn corpus() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = oracle(OracleSet::Corpus, Game::General, "codecs.tsv") else {
        return Ok(());
    };
    let mut report = GateReport::new("independent real codec corpus", Tolerance::Exact);
    for line in std::fs::read_to_string(path)?
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
            name => return Err(format!("unknown codec {name}").into()),
        };
        let input = std::fs::read(fields[1])?;
        let reference = std::fs::read(fields[2])?;
        match codec.decompress(&input, reference.len()) {
            Ok(output) => report.compare_bytes(fields[1], &output, &reference),
            Err(e) => report.fail(fields[1], e.to_string()),
        }
    }
    println!("{report}");
    report.assert_passed();
    Ok(())
}
