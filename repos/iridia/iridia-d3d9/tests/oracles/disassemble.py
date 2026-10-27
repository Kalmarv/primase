"""Independent text oracle: Microsoft D3DDisassemble, with no shader format logic.

Windows: python disassemble.py input.bin output.asm
Input must be a privately extracted raw shader; neither it nor game disassembly belongs in this repository.
"""
import ctypes
import sys
from pathlib import Path

data = Path(sys.argv[1]).read_bytes()
compiler = ctypes.WinDLL("d3dcompiler_47.dll")
compiler.D3DDisassemble.argtypes = [
    ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint, ctypes.c_char_p,
    ctypes.POINTER(ctypes.c_void_p),
]
compiler.D3DDisassemble.restype = ctypes.c_long
blob = ctypes.c_void_p()
buffer = ctypes.create_string_buffer(data)
result = compiler.D3DDisassemble(buffer, len(data), 0, None, ctypes.byref(blob))
if result < 0:
    raise RuntimeError(f"D3DDisassemble failed: {result & 0xffffffff:#x}")
table = ctypes.cast(blob, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
try:
    pointer = ctypes.WINFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)(table[3])(blob)
    size = ctypes.WINFUNCTYPE(ctypes.c_size_t, ctypes.c_void_p)(table[4])(blob)
    Path(sys.argv[2]).write_bytes(ctypes.string_at(pointer, size).rstrip(b"\0"))
finally:
    ctypes.WINFUNCTYPE(ctypes.c_ulong, ctypes.c_void_p)(table[2])(blob)
