"""Generate synthetic PNG inputs independently with Python's struct/zlib.
8-bit expected pixels also verified with Pillow 11; no external asset data.
"""
import pathlib,struct,zlib,io
from PIL import Image
root=pathlib.Path(__file__).resolve().parents[1]/'fixtures'
def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
for depth in [8,16]:
    for color,ch in [(0,1),(2,3),(4,2),(6,4)]:
        w,h=5,3;maximum=(1<<depth)-1
        samples=[(i*1901+17)&maximum for i in range(w*h*ch)]
        pixels=bytes(samples) if depth==8 else struct.pack('>'+str(len(samples))+'H',*samples)
        stride=w*ch*(depth//8)
        data=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',w,h,depth,color,0,0,0))
        data+=chunk(b'IDAT',zlib.compress(b''.join(b'\0'+pixels[y*stride:(y+1)*stride] for y in range(h))))+chunk(b'IEND',b'')
        rgba=[]
        for i in range(0,len(samples),ch):
            c=samples[i:i+ch]
            rgba.extend([c[0],c[0],c[0],maximum] if color==0 else [*c,maximum] if color==2 else [c[0],c[0],c[0],c[1]] if color==4 else c)
        expected=bytes(rgba) if depth==8 else struct.pack('<'+str(len(rgba))+'H',*rgba)
        name=f'c{color}d{depth}'
        (root/(name+'.png')).write_bytes(data);(root/(name+'.rgba')).write_bytes(expected)
        if depth==8:assert Image.open(io.BytesIO(data)).convert('RGBA').tobytes()==expected
for mode in ['L','RGB','P','LA','RGBA','1']:
    im=Image.new(mode,(7,5))
    if mode=='P':
        im.putpalette([v for i in range(256) for v in (i,255-i,(i*17)%256)])
        im.putdata([i%4 for i in range(35)]);im.info['transparency']=bytes([0,80,255,255])
    elif mode=='L':im.putdata([i%256 for i in range(35)]);im.info['transparency']=4
    elif mode=='RGB':im.putdata([(i,i*3,i*5) for i in range(35)]);im.info['transparency']=(4,12,20)
    elif mode=='1':im.putdata([i%2 for i in range(35)])
    else:im.putdata([tuple((i*7+c*31)%256 for c in range(len(mode))) for i in range(35)])
    stream=io.BytesIO();im.save(stream,format='PNG',bits=2 if mode=='P' else 8)
    data=stream.getvalue();(root/(mode+'.png')).write_bytes(data)
    (root/(mode+'.rgba')).write_bytes(Image.open(io.BytesIO(data)).convert('RGBA').tobytes())
print('14 PNG fixtures')
# Malformed palette index: valid CRC/zlib, but the pixel addresses absent entry 2.
data=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',1,1,8,3,0,0,0))+chunk(b'PLTE',bytes([10,20,30]))+chunk(b'IDAT',zlib.compress(bytes([0,2])))+chunk(b'IEND',b'')
(root/'bad_palette.png').write_bytes(data)
