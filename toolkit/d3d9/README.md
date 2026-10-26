# primase-d3d9

Reads complete raw `ps_3_0` streams and lifts their supported operations directly into
`primase_shader::Program`. The caller keeps the original bytes as `ShaderFormat::D3d9`;
no existing bytecode format or consumer is changed. This crate does no file I/O.

`read` returns declarations, instructions, immediate definitions, every comment and CTAB's
creator, target, flags, named register allocations, types and raw defaults. The reserved
constant-info word is preserved: Microsoft compilers use nonzero values in shipped data.
`lift` returns named varyings, resource layouts, D3D register provenance, source hash and
version, and a validated shared IR module. No source text is generated as an intermediate.

The subset is straight-line float arithmetic (MOV, ADD, SUB, MAD, MUL, RCP, RSQ, DP3, DP4,
MIN, MAX, EXP, LOG, LRP, FRC, POW, ABS, NRM, CMP, DP2ADD), NOP, DCL, DEF, TEX (plain,
projected or biased), and TEXKILL. Textures may be 2D, cube or volume. Source swizzles,
negation/absolute value, destination masks/saturation, and centroid declarations are kept.
Partial precision is a hardware request; f32 execution is reported in the program notes.
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

Unknown models, operations, register banks, relative addressing, predication, control
flow, structured/int/bool constant layouts and sampler arrays are refused with an offset
and reason. Uninitialized temporary lanes and undeclared input lanes are refused. This
does not promise general SM1/2, vertex shaders, or every legal SM3 program.

Tests use hand-worked token/number fixtures and one private shipped pixel program against
Microsoft D3DDisassemble. The real gate compares every executable opcode and operand,
input/sampler declarations, DEF values and CTAB register allocations, then validates and
emits the lift as WGSL. It is not GPU pixel equivalence; execution proofs in a consumer
are a later integration gate. `tests/oracles/disassemble.py` is a thin DLL harness with
no shader-format logic. Its inputs and game disassembly stay outside the repository.
