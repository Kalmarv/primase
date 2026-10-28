//! Every Kraken DCX envelope in a game's archives, at any depth (`installs.sekiro`; `installs.elden` with `keys.elden`): the
//! archive entries, the members of the BND4 binders, and the members of the BHF4/BDF4 pairs. Each payload walks exactly
//! ([`oodle_framing::walk`]) in blocks that each restart the decoder, Kraken or LZB16, and the game's Oodle 6 library and
//! oozextract, the decoder where there is no game library, decode it to the same bytes. A stream that is framed otherwise or
//! that oozextract refuses is a failure named by archive entry. The game's library is needed, so this runs where it exists
//! (Windows).
//!
//! The gate refuses what it should refuse, not only agrees on what it accepts: the same envelope with its decoded-size
//! word or its payload-size word one more or one less, its payload one byte short and one byte over, must not decode
//! ([`Decoder::decode`]), whichever way the game's library would take it.
//!
//! The standing census of the streams checked is printed beside the report, so that a green run states its sample: how many
//! streams (archive entries and binder members), blocks, the block headers of the first block and of the later ones, the
//! decoder types, the restart and checksum bits, the stored blocks, the quantum kinds. A full run asserts the census of the
//! whole install (the counts of the original corpus census); a smoke run asserts the invariants only.
//!
//! `[run] gate = 'smoke'` checks a seeded sample (one archive entry in sixteen, with everything inside it), `'full'` every
//! entry. The debug key `oodle.dump` names a folder under the scratch root where the payload and the library's output of each
//! failing entry are kept.

#![allow(clippy::unwrap_used)]
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: real-data gates"
)]

mod inputs;

use std::collections::BTreeMap;

use fromsoft_formats::dcx::{Compression, Decoder, Envelope};
use fromsoft_formats::{Binder, Game};
use inputs::{Depth, Game as Data, OracleSet, Tool, debug, depth, oracle, scratch, seed, tool};
use primase_foundations::compress::Decompressor;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_foundations::compress::oodle_framing::{self, DecoderType, Framing};
use primase_foundations::compress::reference;
use primase_testdata::gate::{GateReport, Tolerance};
mod common;

type BoxError = Box<dyn std::error::Error>;

/// The sample's stride in smoke mode.
const STRIDE: u64 = 16;

/// How a payload's framing differs from that of every Kraken payload of both games: blocks that consume exactly the
/// payload (the walk also refuses the checksum bit), each restarting the decoder, Kraken or LZB16 only.
fn framing_fault(framing: &Framing) -> Option<String> {
    if framing.restart_bits() != framing.block_count() {
        return Some(format!(
            "{} of {} blocks restart the decoder",
            framing.restart_bits(),
            framing.block_count()
        ));
    }
    framing
        .decoders()
        .into_iter()
        .find(|d| !matches!(d, DecoderType::Kraken | DecoderType::Lzb16))
        .map(|d| format!("a {d:?} block"))
}

/// What the streams checked add up to: the standing census.
#[derive(Debug, Default, PartialEq, Eq)]
struct Census {
    /// Streams that are archive entries, and streams that are binder members.
    top: usize,
    members: usize,
    blocks: usize,
    restart_bits: usize,
    /// Blocks with the checksum bit (byte 1, bit 7) set.
    checksum_bits: usize,
    stored_blocks: usize,
    /// The two header bytes of each stream's first block, and of every later block, with how many.
    first_headers: BTreeMap<String, usize>,
    later_headers: BTreeMap<String, usize>,
    /// Streams by the decoder types their blocks name.
    decoder_sets: BTreeMap<String, usize>,
    /// Quanta: compressed, memset, whole-match, stored.
    quanta: [usize; 4],
}

impl Census {
    fn record(&mut self, framing: &Framing, top: bool) {
        if top {
            self.top += 1;
        } else {
            self.members += 1;
        }
        self.blocks += framing.block_count();
        self.restart_bits += framing.restart_bits();
        self.stored_blocks += framing.stored_blocks();
        for (i, block) in framing.blocks.iter().enumerate() {
            if block.header[1] & 0x80 != 0 {
                self.checksum_bits += 1;
            }
            let headers = if i == 0 {
                &mut self.first_headers
            } else {
                &mut self.later_headers
            };
            *headers
                .entry(format!("{:02X} {:02X}", block.header[0], block.header[1]))
                .or_default() += 1;
        }
        let set = framing
            .decoders()
            .iter()
            .map(|d| format!("{d:?}"))
            .collect::<Vec<_>>()
            .join("+");
        *self.decoder_sets.entry(set).or_default() += 1;
        let q = framing.quanta();
        for (slot, n) in self
            .quanta
            .iter_mut()
            .zip([q.compressed, q.memset, q.whole_match, q.stored])
        {
            *slot += n as usize;
        }
    }

    fn print(&self, game: Game) {
        println!(
            "{game:?} Kraken census: {} streams ({} archive entries, {} binder members), {} blocks, \
             {} restart bits, {} checksum bits, {} stored blocks",
            self.top + self.members,
            self.top,
            self.members,
            self.blocks,
            self.restart_bits,
            self.checksum_bits,
            self.stored_blocks
        );
        println!("  first-block headers {:?}", self.first_headers);
        println!("  later-block headers {:?}", self.later_headers);
        println!("  streams by decoder types {:?}", self.decoder_sets);
        println!(
            "  quanta: {} compressed, {} memset, {} whole-match, {} stored",
            self.quanta[0], self.quanta[1], self.quanta[2], self.quanta[3]
        );
    }
}

/// The census of a whole install, from the original corpus census of every Kraken envelope at any depth (walked to the
/// last byte: Sekiro's one truncated archive entry is not a stream).
fn whole_install(game: Game) -> Census {
    let map = |pairs: &[(&str, usize)]| -> BTreeMap<String, usize> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    };
    match game {
        Game::Sekiro => Census {
            top: 7_829,
            members: 19_244,
            blocks: 97_970,
            restart_bits: 97_970,
            checksum_bits: 0,
            stored_blocks: 2_472,
            first_headers: map(&[("8C 06", 24_959), ("CC 06", 1_180), ("8C 02", 934)]),
            later_headers: map(&[("8C 06", 69_605), ("CC 06", 1_292)]),
            decoder_sets: map(&[("Kraken", 26_139), ("Lzb16", 934)]),
            quanta: [95_431, 67, 0, 0],
        },
        Game::EldenRing => Census {
            top: 71_312,
            members: 45_809,
            blocks: 420_824,
            restart_bits: 420_824,
            checksum_bits: 0,
            stored_blocks: 7_759,
            first_headers: map(&[("8C 06", 111_741), ("CC 06", 4_420), ("8C 02", 960)]),
            later_headers: map(&[("8C 06", 300_364), ("CC 06", 3_339)]),
            decoder_sets: map(&[("Kraken", 116_161), ("Lzb16", 960)]),
            quanta: [412_950, 115, 0, 0],
        },
    }
}

struct Gate<'a> {
    dll: &'a Oodle,
    decoder: Decoder<'a>,
    report: GateReport,
    census: Census,
    dump: Option<std::path::PathBuf>,
    /// Malformed variants that were refused.
    refused: usize,
}

impl Gate<'_> {
    /// One Kraken envelope: its framing, the library against oozextract, and the malformed variants. Returns what the
    /// library decoded it to.
    fn check(
        &mut self,
        item: String,
        stored: &[u8],
        envelope: &Envelope<'_>,
        top: bool,
    ) -> Result<Option<Vec<u8>>, BoxError> {
        let framing = match oodle_framing::walk(envelope.payload, envelope.decoded_size) {
            Ok(framing) => framing,
            Err(e) => {
                self.report.fail(item, format!("framing: {e}"));
                return Ok(None);
            }
        };
        if let Some(fault) = framing_fault(&framing) {
            self.report.fail(item, format!("framing: {fault}"));
            return Ok(None);
        }
        self.census.record(&framing, top);
        let expected = self.dll.decompress(envelope.payload, envelope.decoded_size)?;
        match reference::decode(envelope.payload, envelope.decoded_size) {
            Ok(decoded) if decoded == expected => self.report.record(item.clone(), 0.),
            outcome => {
                if let Some(dir) = &self.dump {
                    std::fs::create_dir_all(dir)?;
                    let tag = item
                        .chars()
                        .filter(char::is_ascii_alphanumeric)
                        .take(40)
                        .collect::<String>();
                    std::fs::write(dir.join(format!("{tag}.payload")), envelope.payload)?;
                    std::fs::write(dir.join(format!("{tag}.decoded")), &expected)?;
                }
                match outcome {
                    Ok(_) => self.report.fail(item.clone(), "oozextract decodes other bytes"),
                    Err(e) => self
                        .report
                        .fail(item.clone(), e.to_string().lines().next().unwrap_or_default()),
                }
            }
        }
        self.malformed(&item, &stored[..76 + envelope.payload.len()]);
        Ok(Some(expected))
    }

    /// The envelope with either size word one more or one less, its payload one byte short and one byte over: none may
    /// decode.
    fn malformed(&mut self, item: &str, envelope: &[u8]) {
        // a zero byte after the payload is alignment, so a payload size one over takes it
        let mut base = envelope.to_vec();
        base.push(0);
        let word = |b: &[u8], at: usize| u32::from_be_bytes(b[at..at + 4].try_into().unwrap());
        let mut cases = Vec::new();
        for (at, name) in [(28, "decoded size"), (32, "payload size")] {
            let was = word(&base, at);
            for (now, how) in [(was.wrapping_sub(1), "-1"), (was.wrapping_add(1), "+1")] {
                let mut bad = base.clone();
                bad[at..at + 4].copy_from_slice(&now.to_be_bytes());
                cases.push((format!("{name} {how}"), bad));
            }
        }
        let mut short = base.clone();
        short.truncate(base.len() - 2);
        cases.push(("payload one byte short".to_owned(), short));
        let mut over = base;
        over.push(1);
        cases.push(("payload one byte over".to_owned(), over));
        for (what, bad) in cases {
            if self.decoder.decode(&bad).is_ok() {
                self.report.fail(format!("{item} [{what}]"), "decodes");
            } else {
                self.refused += 1;
            }
        }
    }

    /// Any file found at `item`: a Kraken envelope is checked, an envelope is decoded and looked through, a BND4 binder's
    /// members are visited as stored.
    fn visit(&mut self, item: &str, bytes: &[u8], top: bool) -> Result<(), BoxError> {
        if bytes.starts_with(b"DCX\0") {
            // an envelope the reader refuses (Sekiro's truncated entry) is not a stream
            let Ok(envelope) = Envelope::parse(bytes, usize::MAX) else {
                return Ok(());
            };
            let decoded = if envelope.compression == Compression::Kraken {
                match self.check(item.to_owned(), bytes, &envelope, top)? {
                    Some(decoded) => decoded,
                    None => return Ok(()),
                }
            } else {
                self.decoder.decode(bytes)?
            };
            return self.visit(item, &decoded, false);
        }
        if bytes.starts_with(b"BND4") {
            let binder = Binder::read_stored(bytes)?;
            for (i, file) in binder.files.iter().enumerate() {
                let name = file.name.clone().unwrap_or_else(|| format!("#{i}"));
                self.visit(&format!("{item}|{name}"), &file.bytes, false)?;
            }
        }
        Ok(())
    }
}

fn gate(game: Game, data: Data) -> std::result::Result<(), BoxError> {
    let Some(archive) = common::archive(game)? else {
        return Ok(());
    };
    let Some(path) = tool(Tool::Oodle6) else {
        return Ok(());
    };
    // SAFETY: explicitly configured trusted version-6 game library.
    #[allow(unsafe_code)]
    let dll = unsafe { Oodle::load(path, Version::V6)? };
    let names: BTreeMap<u64, String> = oracle(OracleSet::Uxm, data, "dictionary.txt")
        .map(std::fs::read_to_string)
        .transpose()?
        .map(|text| {
            text.lines()
                .map(|l| (archive.hash(l.trim()), l.trim().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    // the entries that are the index of a split binder, and the payload entry of each
    let mut payload_of = BTreeMap::new();
    for (&hash, name) in &names {
        if let Some(stem) = name.strip_suffix("bhd") {
            let payload = archive.hash(&format!("{stem}bdt"));
            if archive.entries.contains_key(&hash) && archive.entries.contains_key(&payload) {
                payload_of.insert(hash, payload);
            }
        }
    }
    let payloads: std::collections::BTreeSet<u64> = payload_of.values().copied().collect();

    let seed = seed();
    let smoke = depth() == Depth::Smoke;
    println!("depth {} seed {seed}", if smoke { "smoke" } else { "full" });
    let mut gate = Gate {
        dll: &dll,
        decoder: Decoder {
            oodle: Some(&dll),
            max_output: usize::MAX >> 1,
        },
        report: GateReport::new(
            format!("{game:?} KRAK DCX: framing, DLL 6 vs oozextract, malformed variants refused"),
            Tolerance::Exact,
        ),
        census: Census::default(),
        dump: debug("oodle.dump").map(|name| scratch().join(name)),
        refused: 0,
    };
    for (&hash, entry) in &archive.entries {
        if smoke && hash.wrapping_add(seed).wrapping_mul(0x9E37_79B9_7F4A_7C15) % STRIDE != 0 {
            continue;
        }
        if payloads.contains(&hash) {
            continue;
        }
        let item = format!(
            "shard {} {hash:016x} {}",
            entry.shard,
            names.get(&hash).map_or("?", String::as_str)
        );
        if let Some(&payload) = payload_of.get(&hash) {
            // a split pair: the members of the payload, read with the index
            let (index, bytes) = (
                archive.read_hash(hash, &gate.decoder)?,
                archive.read_hash(payload, &gate.decoder)?,
            );
            let binder = Binder::read_split_stored(&index, &bytes)?;
            for (i, file) in binder.files.iter().enumerate() {
                let name = file.name.clone().unwrap_or_else(|| format!("#{i}"));
                gate.visit(&format!("{item}|{name}"), &file.bytes, false)?;
            }
            continue;
        }
        let stored = archive.read_stored(hash)?;
        gate.visit(&item, &stored, true)?;
    }
    let Gate {
        report,
        census,
        refused,
        ..
    } = gate;
    census.print(game);
    println!(
        "{} malformed variants refused (each size word one more and one less, the payload one byte short and over)\n{report}",
        refused
    );
    assert!(census.top + census.members > 0, "no Kraken stream was checked");
    report.assert_passed();
    // the standing invariants, whatever the sample: six malformed variants of each stream refused, every block restarting
    // the decoder, no block with the checksum bit
    assert_eq!(
        refused,
        6 * (census.top + census.members),
        "malformed variants refused"
    );
    assert_eq!(
        census.restart_bits, census.blocks,
        "a block that does not restart the decoder"
    );
    assert_eq!(census.checksum_bits, 0, "a block with the checksum bit");
    if !smoke {
        assert_eq!(
            census,
            whole_install(game),
            "{game:?}: the whole install's census"
        );
    }
    Ok(())
}

#[test]
fn sekiro_archives() -> std::result::Result<(), BoxError> {
    gate(Game::Sekiro, Data::Sekiro)
}

#[test]
fn elden_ring_archives() -> std::result::Result<(), BoxError> {
    gate(Game::EldenRing, Data::Elden)
}
