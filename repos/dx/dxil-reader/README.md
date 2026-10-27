# dxil-reader

A bounded reader of Shader Model 6 programs (DXIL) as Direct3D stores them in a `DXBC` container. It reads and checks
bytes and reports the records the file stores, with DXIL's and D3D's own numbers; mapping them into a graphics API's
vocabulary is the caller's (`iridia-dxil` maps them into iridia's interface and lifts the program). Everything it can't
read is an `Error` naming the bit where it is (in the bitcode, or eight times a byte of the container or part).

- Container (`parts`, over `dxcontainer`): the DXBC directory format; SM6 parts are not 4-byte aligned. Parts lie inside
  the container, do not overlap, and each kind appears once. The digest is not verified (not an authenticity check).
- Program (`program`, `part`): the `DXIL` part's header (program version, size, `DXIL` magic, DXIL version, bitcode
  offset and size); the `STAT` reflection part has the same header.
- Bitstream (`bitstream`): a bounded reader of LLVM's bitstream container: abbreviations (literal, fixed, VBR, array,
  char6, blob), BLOCKINFO, nesting up to 32 blocks, at most 4096 abbreviations per block and 2^26 record operands,
  every length checked against the bits that remain before allocating. (`llvm-bitcode` was considered; it sizes
  allocations from untrusted lengths.)
- Module (`module`, and `module()` for a container's part): LLVM 3.7 IR as DXIL stores it, relative value ids (module
  version 1): types, globals and functions, constants (no constant expressions), metadata (strings, values, nodes,
  named nodes), value names and function bodies. Instructions: binary operators, casts, comparisons, select,
  extractvalue and insertvalue, vector element and shuffle operations, call, phi, ret, branches, switch, unreachable,
  alloca, load and store; any other record is refused by name.
- A function body names only what the function has: every value an instruction reads is one of the function's (an
  earlier one, or a later one a phi or a loop reads), every branch target and phi predecessor a block of it, every
  instruction in a block, and a call has as many arguments as its callee has parameters.
- Reflection records (`reflection`): the entry point (`entry`, a library's `library_kind`) and what its properties state
  (`num_threads`, `geometry_state`, `mesh_state`, `hull_state`, `domain_state`); its signature elements and their
  packing (`elements`, `patch_elements`: element id, first register and column, size, interpolation); the bound
  resource ranges of `dx.resources` (`ranges`: class, range id, register, count, shape, space, element type and stride
  tags, counter); the field annotations of `dx.typeAnnotations` (`fields`: name, offset, component type, matrix); and
  the rows of an `ISG1`/`OSG1`/`PSG1` part (`signature`).
- Runtime data (`rdat`): a library's subobjects as its `RDAT` part stores them (the layout is in `src/rdat.rs`), and a
  function's export name from its decorated name.
- `operations`: the `dx.op` operations a container's module calls, by name, for surveying programs.

`tests/` reads every committed fixture (compiled from the HLSL beside it with the dxc command in each file) and data
built by hand to the runtime-data layout.
