// Transcode every embedded KTX2 mip with the exact WASM shipped by pinned Three.js.
// Output hashes can be compared with the independent Rust transcoder.
import fs from 'node:fs/promises';
import path from 'node:path';
import vm from 'node:vm';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
const basis = new URL('./node_modules/three/examples/jsm/libs/basis/', import.meta.url);
const script = new URL('basis_transcoder.js', basis);
const context = vm.createContext({module:{exports:{}},exports:{},require:createRequire(import.meta.url),process,console,TextDecoder,TextEncoder,WebAssembly,Uint8Array,ArrayBuffer,setTimeout,clearTimeout,__dirname:fileURLToPath(basis),__filename:fileURLToPath(script)});
vm.runInContext(await fs.readFile(script,'utf8'),context);
const module = await context.module.exports({wasmBinary:await fs.readFile(new URL('basis_transcoder.wasm',basis))});
module.initializeBasis();
const folder = process.argv[2];
assert.ok(folder,'usage: node audit-ktx2.mjs <tile folder>');
const index = JSON.parse(await fs.readFile(path.join(folder,'tiles.json'),'utf8'));
let images=0,levels=0;
for(const tile of index.tiles) {
  const glb=await fs.readFile(path.join(folder,tile.file));
  const jsonLength=glb.readUInt32LE(12),doc=JSON.parse(glb.subarray(20,20+jsonLength));
  if(!doc.images)continue;
  const bin=glb.subarray(28+jsonLength);
  for(const [imageIndex,image] of doc.images.entries()) {
    if(image.mimeType==='image/png')continue;
    assert.equal(image.mimeType,'image/ktx2');
    const view=doc.bufferViews[image.bufferView];
    const bytes=bin.subarray(view.byteOffset??0,(view.byteOffset??0)+view.byteLength);
    const file=new module.KTX2File(new Uint8Array(bytes));
    try {
      assert.ok(file.isValid());assert.ok(file.isUASTC());assert.ok(file.startTranscoding());
      const hashes=[];
      for(let mip=0;mip<file.getLevels();mip++) {
        const rgba32=13;
        const pixels=new Uint8Array(file.getImageTranscodedSizeInBytes(mip,0,0,rgba32));
        assert.ok(file.transcodeImage(pixels,mip,0,0,rgba32,0,-1,-1));
        hashes.push(createHash('sha256').update(pixels).digest('hex'));levels++;
      }
      console.log(JSON.stringify({file:tile.file,image:imageIndex,width:file.getWidth(),height:file.getHeight(),hashes}));images++;
    } finally {file.close();file.delete();}
  }
}
console.error(JSON.stringify({images,levels,failed:0,decoder:'three WASM'}));
