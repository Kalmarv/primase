"""Independent synthetic DX10 / mask fixtures following Microsoft's DDS structs.
No Rust writer is called. Run with python -I in this directory.
"""
import struct
from pathlib import Path
out = Path(__file__).resolve().parent.parent / 'fixtures'
# 4x4, one mip, 16 bytes, BC7_UNORM_SRGB=99, TEXTURE2D=3, arraySize=1.
header = [124, 0x81007, 4, 4, 16, 0, 1] + [0] * 11
header += [32, 4, int.from_bytes(b'DX10', 'little'), 0, 0, 0, 0, 0]
header += [0x1000, 0, 0, 0, 0]
assert len(header) == 31
(out / 'dx10-bc7-srgb.dds').write_bytes(b'DDS ' + struct.pack('<36I', *header, 99, 3, 0, 1, 0) + bytes(range(16)))
# 2x1, 8-bit alpha-only, tight pitch of 2 bytes.
header = [124, 0x100f, 1, 2, 2, 0, 0] + [0] * 11
header += [32, 2, 0, 8, 0, 0, 0, 255] + [0x1000, 0, 0, 0, 0]
(out / 'legacy-a8.dds').write_bytes(b'DDS ' + struct.pack('<31I', *header) + bytes([0, 255]))
