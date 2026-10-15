# primase-fromsoft

Read-only Sekiro archive access: raw RSA BHD5 envelopes, bucket indexes,
AES-128-ECB entry ranges, DCX DFLT/KRAK envelopes, and BND4 file tables.

`Archive::open(root)` indexes the five archive pairs. `read_hash(hash, decoder)`
and `read(path, decoder)` return decrypted, decoded bytes. `read_stored(hash)`
returns decrypted stored bytes including padding. `Header::read` and
`unwrap_rsa` expose the lower-level index operations; `Entry::decrypt` applies
only its listed ranges. SHA metadata is parsed and retained, not authenticated.

`Decoder { oodle, max_output }` takes an optional borrowed, caller-loaded Oodle
library and an explicit decompressed-byte ceiling. KRAK needs the native decoder;
DFLT does not. `Binder::read(bytes, &decoder)` returns IDs, names, flags, and
member bytes, including decoded nested DCX. Names are UTF-16 or ASCII; non-ASCII
legacy Shift-JIS names are rejected. Unsupported layouts and unaccounted bytes
are errors. BDT padding for non-DCX entries is retained for reference-reader
parity: a zero BHD logical size does not imply an empty entry.

`NameDictionary::read(path)` reads a caller-supplied UTF-8 path list. Hash
collisions retain all distinct normalized candidates. No dictionary, native
library, or game assets are distributed with this crate.

## Gates

Synthetic tests cover valid and malformed BHD5/DCX/BND4, RSA block geometry,
a NIST AES vector, encrypted ranges, nested envelopes, hash tables, truncations,
and single-byte mutations. Real gates skip when their environment variables
are unset. Use `cargo test -p primase-fromsoft -- --nocapture`.

- `PRIMASE_SEKIRO_DIR`: directory containing the five BHD/BDT pairs.
- `PRIMASE_OODLE6`: explicitly trusted version-6 native library.
- `PRIMASE_FROMSOFT_NAMES`: optional path dictionary for the named-count gate.
- `PRIMASE_FROMSOFT_BHD_ORACLE`: directory of independent reference C# decoder
  output, generated from the same installation. Its files are:
  - `Data1.plain` through `Data5.plain`: complete RSA-unwrapped headers.
  - `entries.tsv`: shard, eight-digit hex hash, container label, decoded length,
    uppercase SHA-256, tab-separated.
  - `binders.tsv`: shard, hash, zero-based member index, signed ID, member name,
    decoded length, uppercase SHA-256.
  - `errors.tsv`: shard, hash, parser name, decimal failure offset, normalized
    error detail. Empty when the reference rejects nothing. Populate only from
    verified reference failures; this is never a list of ignored entries.
  - `samples.tsv`: hash and archive path, tab-separated; corresponding exact
    decoded bytes in `samples/<hash>.bin`. Include varied entries and every
    indexed path for a selected map area.

The census attempts every indexed entry. Accepted results must match reference
lengths and digests; samples compare actual bytes. Rejections count as agreement
only when both readers reject at the same parsing boundary with equivalent
errors. Unexpected acceptance, a missing reference entry, or a different error
fails the differential gate. Reports distinguish decoded entries from agreed
rejections; agreement does not make a corrupt stock entry readable.

## Texture packages and split binders

`Binder::read_split(header, payload, decoder)` shares the binder file-table,
name, hash-table, decompression and coverage checks with the single-file reader.
It checks the BHF4 and BDF4 halves independently.

`tpf::Package::read(bytes)` parses PC TPF containers, retaining each texture's
DDS, name, format, dimensional kind, mip count, flags and optional metadata words.
Section padding must be zero; PC producers use both name-end-relative and
first-payload-relative data sizes. Unknown platforms and compressed PC texture
records return errors. UTF-16 and ASCII-compatible legacy names are supported.
`tpf::Texture::image()` imports 2D DDS through `primase-dds`, preserving encoded
mips and colour-space tags. Cube/volume payloads are retained by the container
reader but explicitly rejected by this 2D import. Bindings assign semantic roles
and normal conventions; package parsing does not infer them from a name.

The TPF corpus gate extends the archive oracle directory with
`tpf/packages.jsonl`: one object per discovered package, with `id` (shard/hex
hash, followed by `/b<index>` or `/s<index>` for single/split binder children),
`area`, package size/SHA256, platform/flags/encoding, and a `textures` array.
Each texture records name/format/kind/mipmaps/flags, DDS size/SHA256 and, for area
2D images, width/height and decoded RGBA SHA256. Area DDS inputs reside at
`tpf/area-dds/<uppercase-sha256>.dds`; none belong in the repository.

The pixel gate additionally requires `PRIMASE_FROMSOFT_DDS_ORACLE`, an explicitly
trusted .NET assembly launched with `dotnet`. For each DDS path on stdin it
writes a binary response to stdout: status byte (0 success), little-endian u64
length, and that many RGBA8 bytes (UTF-8 error text for nonzero status). Diagnostics
must go to stderr. It uses the independent C# decoder with BC1 punch-through
alpha enabled; BC4 grayscale presentation is normalized to red-only by the gate.
Pillow 11 is queried through the shared texture-codec test oracle. Exact checks
use each decoder's own arithmetic convention; cross-convention channel-error
histograms are reported separately, without weakening the exact gates.
