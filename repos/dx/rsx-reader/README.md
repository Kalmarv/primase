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

`fragment::decode` reads the unchanged big-endian fragment microcode into typed
instructions. It rotates each word's halfwords, preserves sources, swizzles,
predicates, destination masks, modifier bits, texture units, control offsets,
and inline float bits. Constants consume a full extra slot shared by the
instruction's constant sources. END must be at the exact end; reserved source
types/opcodes, incomplete constants, and branches into constants refuse.
Unproven bump-environment opcodes refuse explicitly. `disassemble` produces
stable inspection text; this is not an assembler, executor or shader lifter.

`fragment::flow::analyze` reports possible texture and interpolator influences
into caller-selected registers, discard conditions, sampling coordinates, and
downstream operations. It tracks component writes, full/half register overlap,
conditional writes, forward IF/ELSE regions, KIL and RET. Arithmetic unions all
referenced source components, and condition dependencies survive joins. Thus
these are conservative dependencies, not proof that a particular texel/channel
affects a rendered pixel. Calls, loops, BRK and indexed interpolators explicitly
refuse analysis. The decoder can still inspect their encoded instructions.
No sampler is automatically assigned a colour, normal or material role.
The binary register overlap follows the RSX consumer's byte ranges
([register-file ranges](https://github.com/RPCS3/rpcs3/blob/979a52415003fd7a74ddd216e1a5782b3731281f/rpcs3/Emu/RSX/Program/Assembler/FPOpcodes.cpp)):
four 2-byte lanes per H register and four 4-byte lanes per R register.
This is physical binary addressing, separate from the non-overlapping named
temporaries presented by the high-level NV program language.

Instruction encodings are described by the same pinned RPCS3 public RSX field
and opcode declarations; semantics and condition vocabulary are described by
NVIDIA's published [NV_fragment_program](https://registry.khronos.org/OpenGL/extensions/NV/NV_fragment_program.txt)
and [NV_fragment_program2](https://registry.khronos.org/OpenGL/extensions/NV/NV_fragment_program2.txt).
No RPCS3 implementation was copied into this crate. Independently built and run
RPCS3 TaskFP outputs are an oracle in a world crate's corpus test. The test
compares all operands and controls actually printed by that tool, not omitted
saturation, precision, scale, ALU predicates or condition-register selection.
It also omits TXL/TXB's separate LOD/bias source and TXD's second gradient.
Those operands are retained by this decoder (and counted as unprinted by the
comparison); hand-worked constant/flow fixtures check them. The runtime
consumer's [sampling templates](https://github.com/RPCS3/rpcs3/blob/979a52415003fd7a74ddd216e1a5782b3731281f/rpcs3/Emu/RSX/Program/GLSLCommon.cpp)
describe these separate sources. A TXL with a source-1 inline constant is a
discriminating control: original TaskFP stops after 16 of its 32 bytes, whereas
the fixture requires that the decoded instruction include the constant.
When another instruction follows, TaskFP can print that constant as a NOP.
The corpus comparison validates and counts this specific discrepancy rather
than accepting the extra line as an instruction; the original oracle is kept
unchanged. One such constant slot occurs in the current disc corpus.
Hand-worked fixtures check those fields and dependency analysis. The oracle
does not establish hardware correctness: its condition spellings for test bits
0/2/7 disagree with NV's FL/EQ/TR. That discrepancy is kept explicit and counted
by the comparison (none occurs in the current corpus's printed conditions).
