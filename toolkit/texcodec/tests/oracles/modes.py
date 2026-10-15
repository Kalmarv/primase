"""Explicit BC6H (14 valid + 4 reserved) and BC7 (8 valid + zero) mode fixtures."""
import pathlib, random, subprocess
root=pathlib.Path(__file__).resolve().parents[1]/'fixtures'
rng=random.Random(12345)
modes=[0,1,2,6,10,14,18,22,26,30,3,7,11,15,19,23,27,31]
blocks=bytearray(rng.randrange(256) for _ in range(16*len(modes)*16))
for i in range(len(blocks)//16):
    mode=modes[i%len(modes)]
    mask=3 if mode in (0,1) else 31
    blocks[i*16]=(blocks[i*16]&~mask)|mode
(root/'bc6.blocks').write_bytes(blocks)
# Pass an absolute scratch-built native harness path as argv[1].
import sys
for signed in (0,1):
    subprocess.run([sys.argv[1],str(root/'bc6.blocks'),str(root/f'bc6.{signed}.rgb32f'),str(signed)],check=True)
# Existing Pillow fixture includes all eight BC7 modes and a reserved all-zero block.
from PIL import Image
import io,struct
blocks=bytearray(rng.randrange(256) for _ in range(16*64))
for i in range(64):
    m=i%8
    blocks[i*16]=(blocks[i*16]&~((1<<(m+1))-1))|(1<<m)

h=bytearray(128);h[:4]=b'DDS '
for off,v in [(4,124),(8,0xA1007),(12,32),(16,32),(20,len(blocks)),(28,1),(76,32),(80,4),(108,0x1000)]:struct.pack_into('<I',h,off,v)
h[84:88]=b'DX10';h+=struct.pack('<IIIII',98,3,0,1,0)
(root/'bc7.blocks').write_bytes(blocks)
(root/'bc7.pillow.rgba').write_bytes(Image.open(io.BytesIO(h+blocks)).convert('RGBA').tobytes())
