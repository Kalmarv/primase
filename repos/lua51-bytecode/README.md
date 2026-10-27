# Lua 5.1 precompiled chunks

`Chunk::parse(bytes)` reads a `luac` 5.1 chunk, in either byte order, strictly, and `calls(&chunk.main)` lists the calls
a script makes whose callee and arguments are constants, globals and fields of them: how a script that only configures
an engine (`change_fog(400, 700, ...)`, `level.LoadData(level, "path")`) reads. The reader is game-independent; a game's
crate says which calls mean what.

**Header.** `ESC Lua`, version `0x51`, format 0, a byte-order flag (0 big, 1 little), the sizes of `int` and `size_t`
(4 or 8 each), of an instruction (4) and of a number (8, a float: the integral flag must be 0). Both byte orders
read with the same code (the one game's scripts it was built for are little-endian with 4-byte `int` and `size_t`;
the fixtures cover big-endian too).

**Refused, naming the byte:** any other header; a count the rest of the chunk cannot hold (no allocation is sized by an
unchecked count); a string without its terminating NUL; a boolean other than 0 or 1; a constant of an unknown type;
`is_vararg` bits Lua 5.1 does not define; a stack past 250 registers; line numbers for some but not all instructions;
a local whose range leaves the code; upvalue names for some but not all upvalues; a chain of more than 200 functions
inside one another; a byte after the main function; and code the verifier rejects. The verifier is the subset of
`luaG_checkcode` that needs no symbolic execution: an opcode that does not exist, a register past the stack, a
constant, function or upvalue past its table, a jump out of the code, a test not followed by a jump, a `LOADBOOL` that
skips past the end, a `CLOSURE` not followed by its upvalue instructions, and a function that does not end in `RETURN`.

**The call lister** follows registers through the instructions that put constants in place and reports every `CALL`
and `TAILCALL` with what its callee and arguments held. It does not run branches or loops: every instruction a jump can
reach starts from nothing known, so a value that came through control flow is `Value::Unknown`, never a guess. A call
under a condition is listed as unconditional; its arguments are those on the straight path to it.

**Why this is written here.** No maintained crate reads these chunks the way a loader of untrusted bytes must (checked
on crates.io, 2026-10-26): `luac-parser` 0.5.3 (MIT) returns only the parsed function and drops any bytes after it, uses `unsafe` pointer arithmetic for its error offsets and brings nom,
serde, bincode and rmp-serde for a format of a few hundred lines; `lunify` 2.0.0 (MIT) only converts chunks byte to byte
between formats and exposes no parsed function; `lua-bytecode` 0.3.5 is GPL-3.0-only; `lua-assembler` 0.1.1 is MPL-2.0
on the `gaia` / `oak` crates. Nothing here uses their code.

**Checks.** `tests/chunks.rs` reads chunks written out byte by byte from `lopcodes.h` and `lundump.c` (a big-endian
function, the same function little-endian, a function that folds `-10`, jumps and nests a method call, constants of
every type), refuses each header field, every truncation of each of them and a trailing byte, and each rule of the
verifier at the byte that breaks it. Run over the 1,278 scripts of the one console game it was built for, the reader
refuses none, and the lister finds the same 89,819 calls in the same 1,223 scripts as an independent Python lister
written from the same format description (call counts per script; their arguments differ only where this lister
resets at jump targets).
