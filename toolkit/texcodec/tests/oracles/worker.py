"""Persistent Pillow 11 oracle, framed binary replies; game data stays in memory."""
import io,struct,sys
from PIL import Image, __version__
assert __version__ == '11.0.0', __version__
for path in sys.stdin:
    try:
        with open(path.rstrip('\n'), 'rb') as f:data=bytearray(f.read())
        if data[84:88]==b'DX10':
            dxgi=struct.unpack_from('<I',data,128)[0]
            dxgi={72:71,75:74,78:77,99:98}.get(dxgi,dxgi)
            struct.pack_into('<I',data,128,dxgi)
        image=Image.open(io.BytesIO(data)).convert('RGBA')
        result=image.tobytes();status=0
    except Exception as exc:
        result=str(exc).encode();status=1
    sys.stdout.buffer.write(struct.pack('<BQ',status,len(result)))
    sys.stdout.buffer.write(result);sys.stdout.buffer.flush()
