"""The corpus oracle set's general/dxbc: the game's pixel programs with Microsoft's disassembly and what it states.

Reads a staging folder of `<name>.ppo` (a program as the game ships it) and `<name>.ppo.asm` (Microsoft's
disassembly of it: `fxc /nologo /dumpbin /Fc <name>.ppo.asm <name>.ppo`, d3dcompiler's D3DDisassemble) pairs and
writes, for every pair whose program is a ps_4_0, ps_4_1 or ps_5_0 pixel program, the two files and
`<name>.ppo.reflection.json` into the output folder. The reflection is read from the disassembly's own text, not
from the binary: the Resource Bindings table (each texture's and constant buffer's register), the Input signature
(each input's register, keyed by its upper-case name and index) and the Buffer Definitions (each constant's byte
offset and array count, by its leaf name). Nothing of primase reads the text; the tests hold primase's binary
reader to it (toolkit/dxbc/tests/program.rs, reflection.rs).

python3 -I toolkit/dxbc/tests/oracles/dxbc_corpus.py <staging> <oracles.root>/corpus/general/dxbc

Standard library only; it runs on the Linux build machine over the pairs copied there.
"""

import json
import re
import shutil
import sys
from pathlib import Path

PIXEL = {"ps_4_0", "ps_4_1", "ps_5_0"}


def comment_lines(asm: str) -> list[str]:
    """The header's comment lines, without their `//`, up to the shader model line."""
    out = []
    for line in asm.splitlines():
        stripped = line.strip()
        if not stripped.startswith("//"):
            break
        out.append(stripped[2:].rstrip())
    return out


def section(lines: list[str], title: str) -> list[str]:
    """The lines of the section titled `title:` up to the next title (a line ending in a colon at column 1)."""
    out, inside = [], False
    for line in lines:
        text = line.strip()
        if text == f"{title}:":
            inside = True
            continue
        if inside and re.fullmatch(r"[A-Z][A-Za-z ]+:", text):
            break
        if inside:
            out.append(line)
    return out


def table_rows(lines: list[str]) -> list[list[str]]:
    """A table's rows after its dashed rule, split on whitespace."""
    rows, ruled = [], False
    for line in lines:
        text = line.strip()
        if text.startswith("---"):
            ruled = True
            continue
        if ruled and text:
            rows.append(text.split())
    return rows


def register(bind: str) -> int:
    """`t2`, `cb1`, `s0` or a bare slot number: the number."""
    match = re.fullmatch(r"[a-z]*(\d+)", bind)
    if not match:
        raise SystemExit(f"a binding {bind!r} that is no register")
    return int(match[1])


def bindings(lines: list[str]) -> tuple[dict, dict]:
    textures, buffers = {}, {}
    for row in table_rows(section(lines, "Resource Bindings")):
        # Name Type Format Dim (HLSL Bind | Slot) (Count | Elements)
        name, kind, bind = row[0], row[1], row[-2]
        if kind == "texture":
            textures[name] = register(bind)
        elif kind == "cbuffer":
            buffers[name] = register(bind)
    return textures, buffers


def inputs(lines: list[str]) -> dict:
    out = {}
    for row in table_rows(section(lines, "Input signature")):
        # Name Index Mask Register SysValue Format Used (Used may be blank, so read from the left)
        name, index, reg = row[0], row[1], row[3]
        if not (index.isdigit() and reg.isdigit()):
            raise SystemExit(f"an input signature row {row!r} this reader does not know")
        out[f"{name.upper()}{index}"] = int(reg)
    return out


MEMBER = re.compile(r"^(?:row_major |column_major )?([\w]+)\s+([\w]+)(?:\[(\d+)\])?;\s*// Offset:\s*(\d+)")
STRUCT_END = re.compile(r"^\}\s*([\w]+)(?:\[(\d+)\])?;\s*// Offset:\s*(\d+)")


def layout(lines: list[str]) -> dict:
    """Every constant buffer's leaves: name to byte offset, type and array count. The disassembly states a struct
    member's offset within the buffer already (its struct's start included), so a struct only nests the listing."""
    out, buffer, stack = {}, None, []
    for line in section(lines, "Buffer Definitions"):
        text = line.strip()
        start = re.fullmatch(r"cbuffer (\S+)", text)
        if start:
            buffer, stack = start[1], []
            out[buffer] = {}
            continue
        if buffer is None or not text:
            continue
        if text.startswith("struct"):
            stack.append([])
            continue
        end = STRUCT_END.match(text)
        if end and stack:
            members = stack.pop()
            target = stack[-1] if stack else None
            for name, offset, kind, count in members:
                if target is None:
                    out[buffer][name] = {"offset": offset, "type": kind, "count": count}
                else:
                    target.append((name, offset, kind, count))
            continue
        member = MEMBER.match(text)
        if member:
            kind, name, count, offset = member[1], member[2], int(member[3] or 1), int(member[4])
            if stack:
                stack[-1].append((name, offset, kind, count))
            else:
                out[buffer][name] = {"offset": offset, "type": kind, "count": count}
    return out


def model(asm: str) -> str | None:
    for line in asm.splitlines():
        text = line.strip()
        if text and not text.startswith("//"):
            return text
    return None


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    staging, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    written, other = 0, {}
    for asm_path in sorted(staging.glob("*.ppo.asm")):
        ppo = asm_path.with_name(asm_path.name[: -len(".asm")])
        if not ppo.exists():
            raise SystemExit(f"{asm_path.name} without its program")
        asm = asm_path.read_text(encoding="utf-8", errors="strict")
        shader_model = model(asm)
        if shader_model not in PIXEL:
            other[shader_model] = other.get(shader_model, 0) + 1
            continue
        lines = comment_lines(asm)
        textures, buffers = bindings(lines)
        reflection = {
            "inputs": inputs(lines),
            "textures": textures,
            "buffers": buffers,
            "layout": {name: leaves for name, leaves in layout(lines).items() if name in buffers},
        }
        shutil.copyfile(ppo, out / ppo.name)
        shutil.copyfile(asm_path, out / asm_path.name)
        (out / f"{ppo.name}.reflection.json").write_text(json.dumps(reflection, sort_keys=True) + "\n", encoding="utf-8")
        written += 1
    if not written:
        raise SystemExit(f"no pixel program pairs under {staging}")
    print(f"{written} pixel programs written to {out}; other shader models left out: {other}")


if __name__ == "__main__":
    main()
