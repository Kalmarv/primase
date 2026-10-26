# primase-shred

Tony Hawk: SHRED (PS3, 2010), RME archive containers only. World and mesh reading are not implemented.

`read::pak::Pak::open` reads one PAK v4; `from_bytes` accepts caller-owned bytes. Metadata validates the header,
name hashes, supported attributes/alignment, zero on-disc runtime offsets, bounded NUL-terminated names and exact
payload tiling. Names remain byte strings, including original case and separators. Sections include their marker
entries; entries before a marker have no section. Allocation alignment is a runtime property, not a file constraint.
Stored entries have `compressed_size == 0` and occupy `size` bytes; compressed entries occupy `compressed_size` bytes.

`payload(index)` borrows stored bytes or lazily decodes one CSTR v1. `encoded(index)` exposes its encoded bytes.
`read::cstr::decode` checks the endian tag, version, threshold, block count, block bounds, exact DEFLATE consumption,
each output length, total output length and exact stream end. Errors retain byte offsets. Neither API extracts paths
to disk. The archive bytes remain in memory; decoded payloads are not cached.

The format contracts come from the disc's PAK/CSTR headers and its native RME loader. Raw DEFLATE decoding uses
the toolkit's flate2 adapter, independently of that loader. Tests include hand-built framing and RFC 1951 blocks,
malformed/truncated inputs, and a real-disc census. The latter is a structural check of the original data, not an
independent semantic decode of the enclosed resources.

Configure `installs.shred` in the local test-data file as the extracted disc root containing `PS3_GAME/USRDIR`.
Run `cargo test --release -p primase-shred -- --nocapture` with that configuration. Smoke depth selects eight archives
by a seeded rotation of sorted paths; full depth opens every archive and decodes every stream, checking the known
disc census: 336 archives, 67,320 entry occurrences, 31,227 streams, 111,391 blocks, 4,221 stored blocks.
