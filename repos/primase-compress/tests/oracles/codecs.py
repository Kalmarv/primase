"""The corpus oracle set's general/codecs.tsv: game streams paired with what third-party decoders make of them.

Reads a staging folder that the corpus_* example programs filled with the games' own bytes:
  zlib-dcx/*.dcx      Sekiro's DFLT DCX envelopes (corpus_sekiro_dcx --compression DFLT): the zlib stream is cut
                      here from the documented header (big-endian: decoded size at 28, stored size at 32, the stream
                      at 76)
  zlib-cas/*.casblock skate.'s single-block CAS frames of kind 2 (corpus_skate_cas --zlib): the stream after the
                      eight-byte block header (decoded size in bytes 1 to 3, stored size in byte 5's low nibble and
                      bytes 6 and 7)
  lz4/*.lz4           Riders Republic's raw LZ4 terrain blocks (corpus_riders_lz4)
  deflate/*.deflate   GTA V's compressed archive files as stored, decrypted (corpus_gta_deflate): raw deflate
and writes, under the output folder, codecs/<codec>/<name>.in (the stream) and .out (Python's zlib or the lz4
package's decoding of it) and codecs.tsv, one `codec<TAB>input<TAB>reference` row per stream with paths relative to
the TSV. A stream a decoder refuses, or whose decoding is not the size its header states, stops the script.

uv run --no-project --with lz4==4.4.5 python -I toolkit/compress/tests/oracles/codecs.py <staging> <oracles.root>/corpus/general

It runs on the Linux build machine over the staging folders copied there; its output is portable (relative paths).
"""

import struct
import sys
import zlib
from pathlib import Path

import lz4.block

LZ4_BOUND = 16 << 20


def dcx_stream(data: bytes, name: str) -> tuple[bytes, int]:
    if data[:4] != b"DCX\0" or data[40:44] != b"DFLT":
        raise SystemExit(f"{name}: not a DFLT DCX")
    expected, size = struct.unpack_from(">II", data, 28)
    return data[76 : 76 + size], expected


def cas_stream(data: bytes, name: str) -> tuple[bytes, int]:
    if len(data) < 8 or data[0] != 0 or data[4] != 2:
        raise SystemExit(f"{name}: not a zlib CAS block")
    expected = int.from_bytes(data[1:4], "big")
    size = ((data[5] & 15) << 16) | int.from_bytes(data[6:8], "big")
    if len(data) != 8 + size:
        raise SystemExit(f"{name}: a frame of {len(data)} bytes for a block of {size}")
    return data[8:], expected


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    staging, out = Path(sys.argv[1]), Path(sys.argv[2])
    rows = []
    sources = [
        ("zlib", "zlib-dcx", "*.dcx", dcx_stream),
        ("zlib", "zlib-cas", "*.casblock", cas_stream),
        ("lz4", "lz4", "*.lz4", None),
        ("deflate", "deflate", "*.deflate", None),
    ]
    for codec, folder, pattern, cut in sources:
        for path in sorted((staging / folder).glob(pattern)):
            data = path.read_bytes()
            if codec == "lz4":
                stream, expected = data, None
                decoded = lz4.block.decompress(stream, uncompressed_size=LZ4_BOUND)
            elif codec == "deflate":
                stream, expected = data, None
                inflater = zlib.decompressobj(-15)
                decoded = inflater.decompress(stream) + inflater.flush()
                if not inflater.eof:
                    raise SystemExit(f"{path}: a deflate stream that does not end")
            else:
                stream, expected = cut(data, path.name)
                decoded = zlib.decompress(stream)
            if expected is not None and len(decoded) != expected:
                raise SystemExit(f"{path}: {len(decoded)} bytes decoded, the header states {expected}")
            stem = f"{folder}-{path.stem}"
            target = out / "codecs" / codec
            target.mkdir(parents=True, exist_ok=True)
            (target / f"{stem}.in").write_bytes(stream)
            (target / f"{stem}.out").write_bytes(decoded)
            rows.append(f"{codec}\tcodecs/{codec}/{stem}.in\tcodecs/{codec}/{stem}.out")
    if not rows:
        raise SystemExit(f"no streams under {staging}")
    header = "# codec\tinput\treference (paths relative to this file); made by toolkit/compress/tests/oracles/codecs.py\n"
    (out / "codecs.tsv").write_text(header + "\n".join(rows) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(rows)} streams written to {out / 'codecs.tsv'}")


if __name__ == "__main__":
    main()
