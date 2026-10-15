"""Persistent Pillow 11 PNG oracle, framed binary replies."""
import struct,sys
from PIL import Image, __version__
assert __version__ == '11.0.0', __version__
for path in sys.stdin:
    try:
        with Image.open(path.rstrip('\n')) as image:result=image.convert('RGBA').tobytes()
        status=0
    except Exception as exc:
        result=str(exc).encode();status=1
    sys.stdout.buffer.write(struct.pack('<BQ',status,len(result)))
    sys.stdout.buffer.write(result);sys.stdout.buffer.flush()
