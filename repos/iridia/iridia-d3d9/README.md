# iridia-d3d9

Reads complete raw `ps_3_0` and `vs_3_0` streams and lifts their supported operations directly into
`iridia::Program`. The caller keeps the original bytes as `ShaderFormat::D3d9`;
no existing bytecode format or consumer is changed. This crate does no file I/O.

`read` returns declarations, instructions, immediate definitions, every comment and CTAB's
creator, target, flags, named register allocations, types and raw defaults. The reserved
constant-info word is preserved: Microsoft compilers use nonzero values in shipped data.
`lift` returns named varyings, resource layouts, D3D register provenance, source hash and
version, and a validated shared IR module. No source text is generated as an intermediate.

The subset is straight-line float arithmetic (MOV, ADD, SUB, MAD, MUL, RCP, RSQ, DP3, DP4,
MIN, MAX, EXP, LOG, LRP, FRC, POW, ABS, NRM), NOP, DCL and DEF. Pixel programs additionally
support CMP, DP2ADD, TEX (plain, projected or biased) and TEXKILL. Textures may be 2D, cube
or volume. Source swizzles, negation/absolute value and destination masks/saturation are
kept. Pixel centroid declarations are kept; partial precision is a hardware request,
whose f32 execution is reported in the program notes.
NRM includes its fourth-lane result and the documented zero-vector FLT_MAX scale.
SM2/3 TEXKILL tests all four lanes, unlike the older xyz-only description on Microsoft's
instruction page; see the [D9VK 0.40 release note](https://github.com/misyltoad/d9vk/releases/tag/0.40).

The float bank is `cb0`, an array of raw `vec4<u32>` rows: `cN` is byte offset `16*N`.
DEF values override it locally. Sampler `sN` becomes a texture at `tN` and a sampler at
`sN`; bindings retain their original slots. Sparse input registers are unpacked into
named varyings, with their lane masks retained for consumers using register coordinates.
CTAB float scalars, vectors, matrices and arrays get explicit offsets and strides.
An optimized array's allocation can be shorter than its declared type: the normalized
layout describes the allocated range, a note names it, and `read` retains the full type.

Vertex programs retain declared output semantics and masks, including distinct semantics
packed into disjoint lanes of one output register. POSITION0 becomes clip-space position;
other supported outputs stay named varyings. The bounded banks are 16 inputs, 32 temporaries,
12 outputs and 256 float constants. CTAB's version must agree with the stage; its allocation
limits follow the stage. Every declared output lane must be written. Vertex texture fetch,
point size, relative addressing and flow control remain unsupported. The existing RGB
`project` and `reencode` helpers are pixel-only.

`project_vertex_output(bytes, register, mask)` selects declared vertex output lanes by
backwards register/lane dataflow, preserving their original arithmetic and source hash.
It first validates the full VS, so unsupported or invalid dead code is not hidden. The
result is an explicitly derived pixel-stage invocation program: Target0's selected lanes
keep their original RGBA positions, with (0,0,0,1) placeholders elsewhere. Other original
outputs are excluded. It assigns no meaning to a semantic and does not choose bindings;
original input declarations and CTAB allocations remain, with actual reads visible in
`used` and the shared value graph. Its distinct lifter identity and notes name the slice.

Unknown models, operations, register banks, relative addressing, predication, control
flow, structured/int/bool constant layouts and sampler arrays are refused with an offset
and reason. Uninitialized temporary lanes and undeclared input lanes are refused. This
does not promise general SM1/2 or every legal SM3 program.

Tests use hand-worked token/number fixtures and one private shipped pixel program plus two
vertex programs against Microsoft D3DDisassemble. The real gate compares every executable
opcode and operand, input/output/sampler declarations, DEF values and CTAB allocations,
then validates and
emits the lift as WGSL. It is not GPU pixel equivalence; execution proofs in a consumer
are a later integration gate. `tests/oracles/disassemble.py` is a thin DLL harness with
no shader-format logic. Its inputs and game disassembly stay outside the repository.
