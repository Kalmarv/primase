//! Uncompressed SDB2 version 39 shader databases (`ekshadersPC*.bin`).
//!
//! Header: `SDB2`, version, source count, active record count, source offset, table offset (six u32).
//! Each source is name[64], CRC-32, byte length, dependency[4], then the source bytes. A table row is
//! kind, permutation[2], dependency mask[4], payload offset and size. The unused table reserve is zero.
//! Payloads tile in table order, including retained zero-length records; no gap, overlap or trailer is accepted.
//!
//! Effect/state-only payloads begin with 48 bytes: BEEF u16, six count/reserved bytes, vertex/pixel byte lengths,
//! two u64 stage cache keys, and sixteen reserved zero bytes. The raw SM3 programs follow, then literal render
//! states, save/restore state placeholders, two groups of named sampler bindings, and runtime state-variable
//! names. A binding is two NUL strings, a packed native address, a u32 state count and (kind,value) u32 pairs.
//! All groups are read to the exact end; no tail is silently discarded. State values stay in the source's D3D9
//! vocabulary; choosing a destination interpretation is not this reader's job.
//!
//! Native loader evidence distinguishes kind 8 as a state-only pass, not a vertex declaration. Kinds 5..7
//! retain their utility-stage bytecode; kind 6 only marks its presence in the native loader, so no purpose is
//! inferred from its program. Compressed siblings and other database versions are refused by name.

use std::collections::BTreeSet;

use iridia_interface::Stage;
use primase_bytes::{Cursor, Endian, Field, FieldError, View, checked_span};

use crate::shaders::{Database, Effect, Key, Payload, Record, SamplerBinding, Source, State};

const LE: Endian = Endian::Little;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Field(#[from] FieldError),
    #[error("SDB2 {field} at {at:#x}: {why}")]
    Invalid {
        at: usize,
        field: &'static str,
        why: String,
    },
}
pub type Result<T> = std::result::Result<T, Error>;
fn invalid(at: usize, field: &'static str, why: impl Into<String>) -> Error {
    Error::Invalid {
        at,
        field,
        why: why.into(),
    }
}
fn size(n: u32, at: usize, field: &'static str) -> Result<usize> {
    usize::try_from(n).map_err(|_| invalid(at, field, "value exceeds address space"))
}

struct Reader<'a> {
    c: Cursor<'a>,
    origin: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], origin: usize) -> Self {
        Self {
            c: Cursor::with_origin(bytes, origin),
            origin,
        }
    }
    fn at(&self) -> usize {
        self.origin + self.c.position()
    }
    fn u32(&mut self, name: &'static str) -> Result<u32> {
        Ok(self.c.u32(LE).field(name)?)
    }
    fn u64(&mut self, name: &'static str) -> Result<u64> {
        Ok(self.c.u64(LE).field(name)?)
    }
    fn bytes(&mut self, n: usize, name: &'static str) -> Result<&'a [u8]> {
        Ok(self.c.take(n).field(name)?)
    }
    fn mask(&mut self) -> Result<[u32; 4]> {
        Ok([
            self.u32("dependency mask")?,
            self.u32("dependency mask")?,
            self.u32("dependency mask")?,
            self.u32("dependency mask")?,
        ])
    }
    fn string(&mut self, name: &'static str) -> Result<String> {
        let at = self.at();
        let rest = self
            .c
            .bytes()
            .get(self.c.position()..)
            .ok_or_else(|| invalid(at, name, "cursor outside data"))?;
        let n = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid(at, name, "missing NUL terminator"))?;
        let bytes = self.bytes(n, name)?;
        self.bytes(1, name)?;
        if n == 0 {
            return Err(invalid(at, name, "empty name"));
        }
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| invalid(at, name, "not UTF-8"))
    }
    fn states(&mut self, n: usize) -> Result<Vec<State>> {
        if n > self.c.remaining() / 8 {
            return Err(invalid(
                self.at(),
                "state pairs",
                format!("{n} pairs exceed remaining bytes"),
            ));
        }
        (0..n)
            .map(|_| {
                Ok(State {
                    kind: self.u32("state kind")?,
                    value: self.u32("state value")?,
                })
            })
            .collect()
    }
    fn samplers(&mut self, n: u8) -> Result<Vec<SamplerBinding>> {
        let mut bindings = Vec::new();
        for _ in 0..n {
            let sampler = self.string("sampler name")?;
            let texture = self.string("texture parameter")?;
            let address = self.u32("packed sampler address")?;
            let count = self.c.count(LE, 8).field("sampler state count")?;
            let states = self.states(count)?;
            bindings.push(SamplerBinding {
                sampler,
                texture,
                address,
                states,
            });
        }
        Ok(bindings)
    }
    fn end(&self, name: &'static str) -> Result<()> {
        if self.c.remaining() != 0 {
            return Err(invalid(
                self.at(),
                name,
                format!("{} unread bytes", self.c.remaining()),
            ));
        }
        Ok(())
    }
}

fn raw(bytes: &[u8], at: usize, expected: Stage) -> Result<Vec<u8>> {
    let (stage, program) =
        iridia_d3d9::frame(bytes).map_err(|e| invalid(at, "stage bytecode", e.to_string()))?;
    if stage != expected || program.len() != bytes.len() {
        return Err(invalid(
            at,
            "stage bytecode",
            format!(
                "{stage:?}, {} of {} bytes; expected complete {expected:?}",
                program.len(),
                bytes.len()
            ),
        ));
    }
    Ok(program.to_vec())
}

fn effect(bytes: &[u8], origin: usize, state_only: bool) -> Result<Effect> {
    let mut r = Reader::new(bytes, origin);
    if r.c.u16(LE).field("effect tag")? != 0xbeef {
        return Err(invalid(origin, "effect tag", "expected BEEF"));
    }
    let [states, restore, variables, samplers, additional, reserved] =
        r.c.array::<6>().field("effect counts")?;
    if reserved != 0 {
        return Err(invalid(
            origin + 7,
            "effect reserved count",
            format!("{reserved}"),
        ));
    }
    let nv = size(r.u32("vertex byte length")?, r.at(), "vertex byte length")?;
    let np = size(r.u32("pixel byte length")?, r.at(), "pixel byte length")?;
    let vertex_key = r.u64("vertex cache key")?;
    let pixel_key = r.u64("pixel cache key")?;
    if r.bytes(16, "effect reserved header")?.iter().any(|&b| b != 0) {
        return Err(invalid(
            origin + 32,
            "effect reserved header",
            "nonzero reserved bytes",
        ));
    }
    if state_only && (nv != 0 || np != 0 || vertex_key != 0 || pixel_key != 0) {
        return Err(invalid(
            origin,
            "state-only pass",
            "stage bytecode or stage cache key present",
        ));
    }
    let at = r.at();
    let vertex = if nv == 0 {
        None
    } else {
        Some(raw(r.bytes(nv, "vertex program")?, at, Stage::Vertex)?)
    };
    let at = r.at();
    let pixel = if np == 0 {
        None
    } else {
        Some(raw(r.bytes(np, "pixel program")?, at, Stage::Pixel)?)
    };
    let states = r.states(states.into())?;
    let restore_states = r.states(restore.into())?;
    let samplers = r.samplers(samplers)?;
    let additional_samplers = r.samplers(additional)?;
    let variables = (0..variables)
        .map(|_| r.string("runtime state variable"))
        .collect::<Result<_>>()?;
    r.end("effect metadata tail")?;
    Ok(Effect {
        vertex,
        pixel,
        vertex_key,
        pixel_key,
        states,
        restore_states,
        samplers,
        additional_samplers,
        variables,
    })
}

/// Read a complete uncompressed SDB2 v39 database. Raw stage programs are framed, not lifted; an operation
/// outside a lifter's subset remains bytecode. Source CRCs, identities, all spans, reserves and tails are checked.
pub fn read(bytes: &[u8]) -> Result<Database> {
    let mut h = Reader::new(bytes, 0);
    if h.bytes(4, "SDB2 magic")? != b"SDB2" {
        return Err(invalid(
            0,
            "magic",
            "expected uncompressed SDB2 (compressed siblings are not read here)",
        ));
    }
    let version = h.u32("version")?;
    if version != 39 {
        return Err(invalid(4, "version", format!("{version}, expected 39")));
    }
    let ns = size(h.u32("source count")?, 8, "source count")?;
    let nr = size(h.u32("record count")?, 12, "record count")?;
    let source_at = size(h.u32("source offset")?, 16, "source offset")?;
    let table_at = size(h.u32("table offset")?, 20, "table offset")?;
    if source_at != 24 || table_at < source_at || ns > 128 {
        return Err(invalid(
            16,
            "source region",
            "expected offset 24, ordered regions, at most 128 source identities",
        ));
    }
    let view = View::new(bytes);
    let mut r = Reader::new(
        view.slice(source_at, table_at - source_at)
            .field("source region")?,
        source_at,
    );
    checked_span(0, ns, 88, r.c.bytes().len()).field("source headers")?;
    let mut sources = Vec::new();
    let mut names = BTreeSet::new();
    let mut declared = [0u32; 4];
    for _ in 0..ns {
        let at = r.at();
        let name_bytes = r.bytes(64, "source name")?;
        let end = name_bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid(at, "source name", "missing NUL"))?;
        if end == 0 || name_bytes.get(end..).is_some_and(|s| s.iter().any(|&b| b != 0)) {
            return Err(invalid(at, "source name", "empty name or nonzero padding"));
        }
        let name = std::str::from_utf8(
            name_bytes
                .get(..end)
                .ok_or_else(|| invalid(at, "source name", "invalid span"))?,
        )
        .map_err(|_| invalid(at, "source name", "not UTF-8"))?
        .to_owned();
        if !names.insert(name.clone()) {
            return Err(invalid(at, "source name", format!("duplicate {name}")));
        }
        let crc32 = r.u32("source CRC-32")?;
        let n = size(r.u32("source byte length")?, r.at(), "source byte length")?;
        let dependency = r.mask()?;
        if dependency.iter().map(|w| w.count_ones()).sum::<u32>() != 1
            || dependency.iter().zip(declared).any(|(a, b)| a & b != 0)
        {
            return Err(invalid(at, "source dependency", "not a unique one-bit identity"));
        }
        for (d, b) in declared.iter_mut().zip(dependency) {
            *d |= b;
        }
        let data = r.bytes(n, "source bytes")?;
        let actual = primase_hash::crc32(data);
        if actual != crc32 {
            return Err(invalid(
                at,
                "source CRC-32",
                format!("{name}: stored {crc32:#x}, actual {actual:#x}"),
            ));
        }
        sources.push(Source {
            name,
            crc32,
            dependency,
            bytes: data.to_vec(),
        });
    }
    r.end("source region")?;
    let span = checked_span(table_at, nr, 36, bytes.len()).field("record table")?;
    let table_end = span.end;
    let mut table = Reader::new(
        view.slice(span.start, span.len()).field("record table")?,
        table_at,
    );
    let mut records = Vec::new();
    let mut keys = BTreeSet::new();
    for _ in 0..nr {
        let at = table.at();
        let key = Key {
            kind: table.u32("record kind")?,
            permutation: table.u64("permutation key")?,
            dependencies: table.mask()?,
        };
        if !matches!(key.kind, 4..=8) {
            return Err(invalid(
                at,
                "record kind",
                format!("{} is not effect/utility/state-only (4..8)", key.kind),
            ));
        }
        if key
            .dependencies
            .iter()
            .zip(declared)
            .any(|(used, known)| used & !known != 0)
        {
            return Err(invalid(at, "record dependencies", "unknown source identity"));
        }
        if !keys.insert((key.kind, key.permutation)) {
            return Err(invalid(at, "permutation key", "duplicate kind/permutation"));
        }
        records.push(Record {
            key,
            offset: table.u32("payload offset")?,
            bytes: table.u32("payload length")?,
            payload: Payload::Missing,
        });
    }
    let payload_at = records
        .first()
        .map(|r| size(r.offset, table_at, "first payload offset"))
        .transpose()?
        .unwrap_or(bytes.len());
    if payload_at < table_end {
        return Err(invalid(payload_at, "payload region", "overlaps table"));
    }
    let padding = view
        .slice(table_end, payload_at - table_end)
        .field("table reserve")?;
    if padding.iter().any(|&b| b != 0) {
        return Err(invalid(table_end, "table reserve", "nonzero bytes"));
    }
    let mut payloads = Reader::new(
        view.slice(payload_at, bytes.len() - payload_at)
            .field("payload region")?,
        payload_at,
    );
    for record in &mut records {
        if size(record.offset, payloads.at(), "payload offset")? != payloads.at() {
            return Err(invalid(
                payloads.at(),
                "payload offset",
                format!("stored {:#x}; records must tile in table order", record.offset),
            ));
        }
        let at = payloads.at();
        let n = size(record.bytes, at, "payload length")?;
        let data = payloads.bytes(n, "record payload")?;
        record.payload = if n == 0 {
            if record.key.kind != 4 {
                return Err(invalid(
                    at,
                    "empty payload",
                    "only effect records may lack compiled data",
                ));
            }
            Payload::Missing
        } else {
            match record.key.kind {
                4 | 8 => Payload::Effect(effect(data, at, record.key.kind == 8)?),
                5 => Payload::Program {
                    stage: Stage::Vertex,
                    bytes: raw(data, at, Stage::Vertex)?,
                },
                6 | 7 => Payload::Program {
                    stage: Stage::Pixel,
                    bytes: raw(data, at, Stage::Pixel)?,
                },
                _ => return Err(invalid(at, "record kind", "unhandled record")),
            }
        };
    }
    payloads.end("database")?;
    Ok(Database {
        sources,
        records,
        table_padding: padding.len(),
    })
}

/// Attach a chosen database as `shaunwhite.shader_library`, replacing a prior one. A failure leaves the world
/// unchanged. The caller chooses the quality file; no path, quality policy, binding or permutation is inferred.
pub fn read_into(world: &mut primase_core::World, bytes: &[u8]) -> Result<()> {
    let database = read(bytes)?;
    world.insert_resource(crate::components::ShaderLibrary(std::sync::Arc::new(database)));
    Ok(())
}
