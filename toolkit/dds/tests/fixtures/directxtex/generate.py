"""Make the DirectXTex layout fixtures: DDS files written by texconv and texassemble, and what DirectXTex reads back.

    python -I generate.py --texconv texconv.exe --texassemble texassemble.exe --dump directxtex-dump.exe --work <scratch>

Writes, beside this script, one `<case>.dds` per case below and `directxtex.json`: per case the commands that made it,
the header choice texconv used (`auto`, `legacy` for -dx9, `dx10` for -dx10) and what DirectXTex's library read from the
file (directxtex-dump `read`: its TexMetadata, every image's size and pitch, and the sha256 of every image's bytes in
DirectXTex's order). Source images are generated here (TGA, every image distinct), so nothing else is needed. The tools
are the DirectXTex may2026 release (texconv, texassemble 2026.5.8.1) and directxtex-dump built against its source
(primase-oracles dumps/directxtex).

A Windows Python, on Windows or under Wine (primase-oracles' `tools/wine/winpy`, with Wine paths). texconv converts
some formats and makes mips through WIC, and Wine's own WIC fails or differs there (B5G6R5, B5G5R5A1 and R10G10B10A2
fail; mips differ), so under Wine Microsoft's `windowscodecs.dll` (modding-archive/tools/windowscodecs-10.0.26100.9711)
must be in the prefix's system32 and chosen with `WINEDLLOVERRIDES=windowscodecs,windowscodecsext=n`: then every
fixture is the same, byte for byte, as on Windows. (Microsoft's WIC does not decode PNG under Wine, hence TGA sources.)
"""

import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent


def tga(path, w, h, k, alpha=False):
    """An uncompressed 32-bit BGRA TGA, top row first, whose every pixel depends on its position and on the image number
    `k`. DirectXTex reads TGA itself, not through WIC, so the sources read the same wherever the tools run."""
    pixels = bytearray()
    for y in range(h):
        for x in range(w):
            a = ((x + y + k) * 37) & 255 if alpha else 255
            pixels += bytes([(k * 53 + x * 7 + y * 13) & 255, y * 255 // max(h - 1, 1), x * 255 // max(w - 1, 1), a])
    # no ID, no colour map, type 2 (true colour), origin (0, 0), w x h, 32 bits, 8 alpha bits, top-left origin
    header = struct.pack("<BBBHHBHHHHBB", 0, 0, 2, 0, 0, 0, 0, 0, w, h, 32, 0x28)
    path.write_bytes(header + bytes(pixels))


def dds_1d(path, width, layers):
    """A DX10 1D R8G8B8A8_UNORM texture (no DirectXTex tool assembles one), every texel distinct."""
    header = bytearray(148)
    header[0:4] = b"DDS "
    for offset, value in [(4, 124), (8, 0x100F), (12, 1), (16, width), (20, width * 4), (28, 1), (76, 32), (80, 4),
                          (108, 0x1000), (128, 28), (132, 2), (140, layers)]:
        header[offset:offset + 4] = struct.pack("<I", value)
    header[84:88] = b"DX10"
    payload = bytes((i * 29 + 7) & 255 for i in range(width * 4 * layers))
    path.write_bytes(bytes(header) + payload)


def run(cmd):
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"{cmd}: exit {result.returncode}\n{result.stdout}\n{result.stderr}")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--texconv", required=True)
    p.add_argument("--texassemble", required=True)
    p.add_argument("--dump", required=True)
    p.add_argument("--work", required=True)
    a = p.parse_args()
    work = Path(a.work)
    if work.exists():
        shutil.rmtree(work)
    (work / "src").mkdir(parents=True)
    src = work / "src"

    # Sources: a 2D image with odd sides, three array layers, six cube faces twice over, four volume slices, 1D.
    tga(src / "odd.tga", 13, 7, 0, alpha=True)
    tga(src / "flat.tga", 16, 8, 0, alpha=True)
    layers = [src / f"layer{i}.tga" for i in range(3)]
    faces = [src / f"face{i}.tga" for i in range(12)]
    slices = [src / f"slice{i}.tga" for i in range(4)]
    for i, f in enumerate(layers):
        tga(f, 16, 8, 10 + i)
    for i, f in enumerate(faces):
        tga(f, 8, 8, 20 + i, alpha=True)
    for i, f in enumerate(slices):
        tga(f, 16, 8, 40 + i)
    dds_1d(src / "line.dds", 16, 1)
    dds_1d(src / "lines.dds", 16, 2)

    def assemble(name, command, inputs):
        out = src / f"{name}.dds"
        cmd = [a.texassemble, command, "-nologo", "-y", "-o", str(out), *map(str, inputs)]
        run(cmd)
        return out, ["texassemble", command, "-o", out.name, *[Path(i).name for i in inputs]]

    array, array_cmd = assemble("array", "array", layers)
    cube, cube_cmd = assemble("cube", "cube", faces[:6])
    cubes, cubes_cmd = assemble("cubearray", "cubearray", faces)
    volume, volume_cmd = assemble("volume", "volume", slices)
    made = {"array": array_cmd, "cube": cube_cmd, "cubearray": cubes_cmd, "volume": volume_cmd}

    # (case, input, texconv options, header choice)
    cases = [
        ("2d-rgba8-legacy", src / "odd.tga", ["-f", "R8G8B8A8_UNORM", "-m", "0"], "auto"),
        ("2d-rgba8-dx10", src / "odd.tga", ["-f", "R8G8B8A8_UNORM", "-m", "0", "-dx10"], "dx10"),
        ("2d-rgba8srgb-dx10", src / "odd.tga", ["-f", "R8G8B8A8_UNORM_SRGB", "-m", "1"], "auto"),
        ("2d-rgba8srgb-dx9", src / "odd.tga", ["-f", "R8G8B8A8_UNORM_SRGB", "-m", "1", "-dx9"], "legacy"),
        ("2d-bgra8", src / "odd.tga", ["-f", "B8G8R8A8_UNORM", "-m", "0"], "auto"),
        ("2d-bgrx8", src / "odd.tga", ["-f", "B8G8R8X8_UNORM", "-m", "1"], "auto"),
        ("2d-b5g6r5", src / "odd.tga", ["-f", "B5G6R5_UNORM", "-m", "0"], "auto"),
        ("2d-b5g5r5a1", src / "odd.tga", ["-f", "B5G5R5A1_UNORM", "-m", "1"], "auto"),
        ("2d-b4g4r4a4", src / "odd.tga", ["-f", "B4G4R4A4_UNORM", "-m", "1"], "auto"),
        ("2d-rgb10a2-dx10", src / "odd.tga", ["-f", "R10G10B10A2_UNORM", "-m", "1"], "auto"),
        ("2d-rgb10a2-dx9", src / "odd.tga", ["-f", "R10G10B10A2_UNORM", "-m", "1", "-dx9"], "legacy"),
        ("2d-r8", src / "odd.tga", ["-f", "R8_UNORM", "-m", "0"], "auto"),
        ("2d-rg8", src / "odd.tga", ["-f", "R8G8_UNORM", "-m", "1"], "auto"),
        ("2d-a8", src / "odd.tga", ["-f", "A8_UNORM", "-m", "1"], "auto"),
        ("2d-r16", src / "odd.tga", ["-f", "R16_UNORM", "-m", "1"], "auto"),
        ("2d-rg16", src / "odd.tga", ["-f", "R16G16_UNORM", "-m", "1"], "auto"),
        ("2d-rg8snorm", src / "odd.tga", ["-f", "R8G8_SNORM", "-m", "1"], "auto"),
        ("2d-rgba8snorm", src / "odd.tga", ["-f", "R8G8B8A8_SNORM", "-m", "1"], "auto"),
        ("2d-rg16snorm", src / "odd.tga", ["-f", "R16G16_SNORM", "-m", "1"], "auto"),
        ("2d-r8snorm", src / "odd.tga", ["-f", "R8_SNORM", "-m", "1"], "auto"),
        ("2d-rgba16", src / "odd.tga", ["-f", "R16G16B16A16_UNORM", "-m", "1"], "auto"),
        ("2d-rgba16f", src / "odd.tga", ["-f", "R16G16B16A16_FLOAT", "-m", "0"], "auto"),
        ("2d-rgba32f", src / "odd.tga", ["-f", "R32G32B32A32_FLOAT", "-m", "1"], "auto"),
        ("2d-rg32f", src / "odd.tga", ["-f", "R32G32_FLOAT", "-m", "1"], "auto"),
        ("2d-rg16f", src / "odd.tga", ["-f", "R16G16_FLOAT", "-m", "1"], "auto"),
        ("2d-r32f", src / "odd.tga", ["-f", "R32_FLOAT", "-m", "1"], "auto"),
        ("2d-r16f", src / "odd.tga", ["-f", "R16_FLOAT", "-m", "1"], "auto"),
        ("2d-bc1", src / "odd.tga", ["-f", "BC1_UNORM", "-m", "0"], "auto"),
        ("2d-bc1srgb-dx9", src / "odd.tga", ["-f", "BC1_UNORM_SRGB", "-m", "1", "-dx9"], "legacy"),
        ("2d-bc2", src / "flat.tga", ["-f", "BC2_UNORM", "-m", "0"], "auto"),
        ("2d-bc2-pm", src / "flat.tga", ["-f", "BC2_UNORM", "-m", "1", "-pmalpha"], "auto"),
        ("2d-bc3", src / "flat.tga", ["-f", "BC3_UNORM", "-m", "0"], "auto"),
        ("2d-bc3-pm", src / "flat.tga", ["-f", "BC3_UNORM", "-m", "1", "-pmalpha"], "auto"),
        ("2d-bc3srgb", src / "flat.tga", ["-f", "BC3_UNORM_SRGB", "-m", "1"], "auto"),
        ("2d-bc4", src / "flat.tga", ["-f", "BC4_UNORM", "-m", "0"], "auto"),
        ("2d-bc4-dx9", src / "flat.tga", ["-f", "BC4_UNORM", "-m", "1", "-dx9"], "legacy"),
        ("2d-bc4snorm", src / "flat.tga", ["-f", "BC4_SNORM", "-m", "1"], "auto"),
        ("2d-bc5", src / "flat.tga", ["-f", "BC5_UNORM", "-m", "1"], "auto"),
        ("2d-bc5-dx9", src / "flat.tga", ["-f", "BC5_UNORM", "-m", "1", "-dx9"], "legacy"),
        ("2d-bc5snorm", src / "flat.tga", ["-f", "BC5_SNORM", "-m", "1"], "auto"),
        ("2d-bc6h", src / "flat.tga", ["-f", "BC6H_UF16", "-m", "1"], "auto"),
        ("2d-bc6hs-dx10", src / "flat.tga", ["-f", "BC6H_SF16", "-m", "1", "-dx10"], "dx10"),
        ("2d-bc7", src / "flat.tga", ["-f", "BC7_UNORM", "-m", "0"], "auto"),
        ("2d-bc7-pm-dx10", src / "flat.tga", ["-f", "BC7_UNORM", "-m", "1", "-pmalpha", "-dx10"], "dx10"),
        ("2d-bc7srgb", src / "flat.tga", ["-f", "BC7_UNORM_SRGB", "-m", "1"], "auto"),
        ("array-rgba8", array, ["-f", "R8G8B8A8_UNORM", "-m", "0"], "auto"),
        ("array-bc1", array, ["-f", "BC1_UNORM", "-m", "0"], "auto"),
        ("cube-bc3-legacy", cube, ["-f", "BC3_UNORM", "-m", "0"], "auto"),
        ("cube-bc3-dx10", cube, ["-f", "BC3_UNORM", "-m", "0", "-dx10"], "dx10"),
        ("cube-rgba8-legacy", cube, ["-f", "R8G8B8A8_UNORM", "-m", "1"], "auto"),
        ("cube-bc7", cube, ["-f", "BC7_UNORM", "-m", "0"], "auto"),
        ("cube-rgba16f", cube, ["-f", "R16G16B16A16_FLOAT", "-m", "0"], "auto"),
        ("cubearray-bc7", cubes, ["-f", "BC7_UNORM", "-m", "0"], "auto"),
        ("cubearray-rgba8", cubes, ["-f", "R8G8B8A8_UNORM", "-m", "1"], "auto"),
        ("volume-rgba8-legacy", volume, ["-f", "R8G8B8A8_UNORM", "-m", "0"], "auto"),
        ("volume-rgba8-dx10", volume, ["-f", "R8G8B8A8_UNORM", "-m", "0", "-dx10"], "dx10"),
        ("volume-bc1", volume, ["-f", "BC1_UNORM", "-m", "0"], "auto"),
        ("volume-r16f", volume, ["-f", "R16_FLOAT", "-m", "0"], "auto"),
        ("1d-rgba8-dx10", src / "line.dds", ["-f", "R8G8B8A8_UNORM", "-m", "0", "-dx10"], "dx10"),
        ("1d-rgba8-default", src / "line.dds", ["-f", "R8G8B8A8_UNORM", "-m", "0"], "auto"),
        ("1d-array-r16f", src / "lines.dds", ["-f", "R16_FLOAT", "-m", "0"], "auto"),
    ]

    for old in HERE.glob("*.dds"):
        old.unlink()
    record = {"tools": {}, "cases": []}
    for tool in (a.texconv, a.texassemble, a.dump):
        record["tools"][Path(tool).name] = hashlib.sha256(Path(tool).read_bytes()).hexdigest()
    batch = []
    for name, source, options, header in cases:
        out = work / "out" / name
        out.mkdir(parents=True)
        run([a.texconv, "-nologo", "-nogpu", "-y", *options, "-o", str(out), str(source)])
        (written,) = out.glob("*.dds")
        shutil.copyfile(written, HERE / f"{name}.dds")
        made_by = made.get(source.stem, [])
        record["cases"].append({
            "name": name,
            "header": header,
            "commands": [c for c in [made_by, ["texconv", "-nogpu", *options, source.name]] if c],
        })
        batch.append(f"read\t{HERE / f'{name}.dds'}\t{work / 'dump' / name}")
    (work / "batch.tsv").write_text("\n".join(batch) + "\n", encoding="utf-8")
    run([a.dump, "batch", str(work / "batch.tsv")])
    for case in record["cases"]:
        dump = work / "dump" / case["name"]
        meta = json.loads((dump / "meta.json").read_text())
        if meta["hr"] != "0x00000000":
            raise SystemExit(f"{case['name']}: DirectXTex refused its own file: {meta['hr']}")
        data = (dump / "images.bin").read_bytes()
        at = 0
        for image in meta["images"]:
            image["sha256"] = hashlib.sha256(data[at:at + image["slicePitch"]]).hexdigest()
            at += image["slicePitch"]
        assert at == len(data), case["name"]
        case["directxtex"] = meta
    (HERE / "directxtex.json").write_text(json.dumps(record, indent=1) + "\n", encoding="utf-8", newline="\n")
    total = sum(f.stat().st_size for f in HERE.glob("*.dds"))
    print(f"{len(cases)} fixtures, {total} bytes")


if __name__ == "__main__":
    main()
