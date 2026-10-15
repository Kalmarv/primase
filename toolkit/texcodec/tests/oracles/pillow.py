"""Generate small synthetic DDS/Pillow 11 fixtures; no game data. Run python -I."""
import io, pathlib, random, struct
from PIL import Image, __version__
assert __version__ == '11.0.0', __version__
root = pathlib.Path(__file__).resolve().parents[1] / 'fixtures'
rng = random.Random(20261015)
for name, four, size in [('bc1',b'DXT1',8),('bc2',b'DXT3',16),('bc3',b'DXT5',16),('bc4',b'ATI1',8),('bc5',b'ATI2',16),('bc7',b'DX10',16)]:
    blocks = bytes(rng.randrange(256) for _ in range(size*64))
    h=bytearray(128);h[:4]=b'DDS '
    for off,v in [(4,124),(8,0xA1007),(12,32),(16,32),(20,len(blocks)),(28,1),(76,32),(80,4),(108,0x1000)]:struct.pack_into('<I',h,off,v)
    h[84:88]=four
    if name=='bc7':h+=struct.pack('<IIIII',98,3,0,1,0)
    rgba=Image.open(io.BytesIO(h+blocks)).convert('RGBA').tobytes()
    (root/(name+'.blocks')).write_bytes(blocks)
    (root/(name+'.pillow.rgba')).write_bytes(rgba)
print('Pillow',__version__,'6 fixtures, 64 blocks each')
