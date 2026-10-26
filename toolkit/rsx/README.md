# RSX shader containers

`cgbinary::parse` reads exactly one big-endian revision-6 CgBinaryProgram;
`parse_prefix` reads its advertised size when a caller owns concatenation/padding.
Profiles 7003 (vertex) and 7004 (fragment) have a 32-byte header, 48-byte
parameter records, and a 24-byte stage descriptor. Microcode remains encoded.
The fragment instruction count includes inline constant slots.

Parameter strings keep their source bytes. Resource, type, variability and
direction keep the original Cg enum values. Resource indices and parameter
numbers remain signed. `texture_unit` recognises resources CG_TEXUNIT0..15;
it does not reinterpret a constant-register index as a sampler. Defaults retain
float bits for float scalars, vectors and matrices; defaults of other types
refuse explicitly. Embedded constant tables identify 16-byte microcode spans.

The reader bounds every structure against the advertised size, rejects
overlapping fixed structures and metadata inside those structures, and checks
alignment, boolean values, string termination and embedded constant offsets.
Shared metadata and string suffixes are allowed. Padding bytes in the fragment
descriptor are retained; no undocumented requirement that they be zero is added.
This is container validation, not microcode validation or disassembly.

Layout reference: RPCS3's public
[CgBinaryProgram declarations](https://github.com/RPCS3/rpcs3/blob/979a52415003fd7a74ddd216e1a5782b3731281f/rpcs3/Emu/RSX/Program/CgBinaryProgram.h).
It is used as a description; no RPCS3 implementation code is copied.
Cg enum values are described in the published
[Cg header](https://help.autodesk.com/cloudhelp/ENU/Mudbox-SDK/cpp_ref/cg_8h_source.html).
The tests lay out small containers by hand, including metadata fields that a
compiler may omit. A world crate can run its archive corpus through this reader
without introducing a toolkit dependency on that world's archive format.
