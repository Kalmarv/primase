// Geometry/meshopt audit without a GPU. PNG dimensions are read for loader setup;
// texture rendering is deliberately left to the browser proof.
import fs from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { MeshoptDecoder } from 'three/addons/libs/meshopt_decoder.module.js';
import { Box3, Matrix4, Vector3, BufferGeometry } from 'three';
import { contentBounds, curvePoints, parseCamera } from './geometry.js';
// Box3.setFromObject's precise path excludes InstancedMesh. Measure actual
// transformed vertices on both sides instead of comparing conservative boxes.
function vertexBounds(root) {
  const bounds=new Box3(), point=new Vector3(), instance=new Matrix4(), world=new Matrix4();
  root.traverse(o=>{
    if(!o.isMesh)return;
    const position=o.geometry.attributes.position;
    for(let n=0;n<(o.isInstancedMesh?o.count:1);n++) {
      if(o.isInstancedMesh){o.getMatrixAt(n,instance);world.multiplyMatrices(o.matrixWorld,instance);}
      else world.copy(o.matrixWorld);
      for(let i=0;i<position.count;i++)bounds.expandByPoint(point.fromBufferAttribute(position,i).applyMatrix4(world));
    }
  });
  return bounds;
}
globalThis.self = globalThis;
assert.deepEqual(parseCamera('1,2,3,4,5,6'),[1,2,3,4,5,6]);
assert.throws(()=>parseCamera('1,2,3,4,5,NaN'));
assert.throws(()=>parseCamera('1,2,3,,5,6'));
globalThis.createImageBitmap = async blob => {
  const b = Buffer.from(await blob.arrayBuffer());
  assert.equal(b.subarray(1, 4).toString(), 'PNG');
  return {width:b.readUInt32BE(16), height:b.readUInt32BE(20), close(){}};
};
const hash = b => createHash('sha256').update(b).digest('hex');
const root = process.argv[2];
if (!root) throw Error('usage: node audit.mjs <tile folder>');
const index = JSON.parse(await fs.readFile(path.join(root, 'tiles.json')));
const cameraBounds=contentBounds(index.tiles);
assert.ok(!cameraBounds.isEmpty());
assert.deepEqual(contentBounds([{triangles:1,min:[10,20,30],max:[20,30,40]},{triangles:0,min:[0,0,0],max:[0,0,0]}]).min.toArray(),[10,20,30]);
const proof = JSON.parse(await fs.readFile(path.join(root, 'proof.json')));
await MeshoptDecoder.ready;
const loader = new GLTFLoader().setMeshoptDecoder(MeshoptDecoder);
let streams=0, meshes=0, triangles=0, instances=0, maxBoundError=0, transformedSlots=0;
for (const tile of index.tiles) {
  if(tile.collision){
    const payload=await fs.readFile(path.join(root,tile.collision.file));
    assert.equal(payload.length,tile.collision.bytes);
    const collision=await loader.parseAsync(payload.buffer.slice(payload.byteOffset,payload.byteOffset+payload.length),'');
    let records=0;
    const surfaceChecks=[];
    // GLTFLoader puts primitive extras on BufferGeometry; mesh extras belong to Mesh.
    collision.scene.traverse(o=>{if(o.isMesh){records++;const ref=o.geometry.userData['primase.collision_surfaces'];assert.equal(ref.domain,'triangle');surfaceChecks.push(collision.parser.getDependency('accessor',ref.accessor).then(a=>assert.equal(a.count,o.geometry.index.count/3)));}});
    await Promise.all(surfaceChecks);
    assert.equal(records,tile.collision.records);
  }
  const bytes = await fs.readFile(path.join(root,tile.file));
  const expected = proof.find(p=>p.file===tile.file);
  assert.equal(hash(bytes), expected.sha256);
  const jsonLength=bytes.readUInt32LE(12);
  const doc=JSON.parse(bytes.subarray(20,20+jsonLength).toString());
  let closedCurves=0;
  for(const node of doc.nodes??[]) {
    const record=node.extras?.primase?.['web.viewer']?.value;
    if(record?.kind==='grind_curve'){
      const points=curvePoints(record.data);
      const geometry=new BufferGeometry().setFromPoints(points);
      assert.equal(geometry.attributes.position.count,record.data.points.length+(record.data.closed&&record.data.points.length?1:0));
      if(record.data.closed){closedCurves++;assert.ok(points[0].equals(points.at(-1)));}
      geometry.dispose();
    }
  }
  const bin=bytes.subarray(28+jsonLength);
  for (const stream of expected.streams) {
    const e=doc.bufferViews[stream.view].extensions.EXT_meshopt_compression;
    const out=new Uint8Array(e.count*e.byteStride);
    MeshoptDecoder.decodeGltfBuffer(out,e.count,e.byteStride,bin.subarray(e.byteOffset,e.byteOffset+e.byteLength),e.mode,e.filter??'NONE');
    assert.equal(hash(out),stream.sha256,`${tile.file} view ${stream.view}`);
    streams++;
  }
  const result=await loader.parseAsync(bytes.buffer.slice(bytes.byteOffset,bytes.byteOffset+bytes.length),'');
  // KHR_texture_transform: the loaded texture of every transformed slot carries the slot's offset, scale, rotation.
  const slots=[['map',m=>m.pbrMetallicRoughness?.baseColorTexture],['metalnessMap',m=>m.pbrMetallicRoughness?.metallicRoughnessTexture],
    ['normalMap',m=>m.normalTexture],['aoMap',m=>m.occlusionTexture],['emissiveMap',m=>m.emissiveTexture]];
  const checkedMaterials=new Set();
  result.scene.traverse(o=>{
    if(!o.isMesh)return;
    for(const material of (Array.isArray(o.material)?o.material:[o.material])){
      const index=result.parser.associations.get(material)?.materials;
      if(index===undefined||checkedMaterials.has(material))continue;
      checkedMaterials.add(material);
      for(const [slot,info] of slots){
        const t=info(doc.materials[index])?.extensions?.KHR_texture_transform;
        if(!t)continue;
        const map=material[slot];
        assert.ok(map,`${tile.file} material ${index} ${slot} not loaded`);
        assert.deepEqual([map.offset.x,map.offset.y,map.repeat.x,map.repeat.y,map.rotation],
          [...(t.offset??[0,0]),...(t.scale??[1,1]),t.rotation??0],`${tile.file} material ${index} ${slot} transform`);
        transformedSlots++;
      }
    }
  });
  let tileMeshes=0,tileTriangles=0,tileInstances=0;
  result.scene.traverse(o=>{
    if (o.isMesh) {
      tileMeshes++;
      const n=o.isInstancedMesh?o.count:1;
      tileInstances+=n;
      tileTriangles+=(o.geometry.index?.count??o.geometry.attributes.position.count)/3*n;
    }
  });
  assert.equal(tileTriangles,tile.triangles,`${tile.file} triangles`);
  const plainBytes=await fs.readFile(path.join(root,'plain',tile.file));
  const plainJsonLength=plainBytes.readUInt32LE(12);
  const plainDoc=JSON.parse(plainBytes.subarray(20,20+plainJsonLength));
  const plainBin=plainBytes.subarray(28+plainJsonLength);
  assert.deepEqual(doc.materials,plainDoc.materials,`${tile.file} materials including doubleSided`);
  const viewBytes=(document,binary,view)=>{const v=document.bufferViews[view];return binary.subarray(v.byteOffset??0,(v.byteOffset??0)+v.byteLength);};
  for(let m=0;m<(doc.meshes?.length??0);m++)for(let p=0;p<doc.meshes[m].primitives.length;p++){
    const a=doc.accessors[doc.meshes[m].primitives[p].indices],b=plainDoc.accessors[plainDoc.meshes[m].primitives[p].indices];
    assert.deepEqual(viewBytes(doc,bin,a.bufferView),viewBytes(plainDoc,plainBin,b.bufferView),`${tile.file} triangle order`);
  }
  for(let i=0;i<(doc.images?.length??0);i++)assert.equal(hash(viewBytes(doc,bin,doc.images[i].bufferView)),hash(viewBytes(plainDoc,plainBin,plainDoc.images[i].bufferView)),`${tile.file} unchanged PNG`);
  const plain=await loader.parseAsync(plainBytes.buffer.slice(plainBytes.byteOffset,plainBytes.byteOffset+plainBytes.length),'');
  plain.scene.updateMatrixWorld(true);result.scene.updateMatrixWorld(true);
  let mirroredPlain=0,mirroredOutput=0;
  plain.scene.traverse(o=>{if(o.isMesh&&o.matrixWorld.determinant()<0)mirroredPlain++;});
  result.scene.traverse(o=>{
    if(!o.isMesh)return;
    if(o.isInstancedMesh){const instance=new Matrix4();for(let i=0;i<o.count;i++){o.getMatrixAt(i,instance);assert.ok(o.matrixWorld.determinant()*instance.determinant()>0,`${tile.file} mirrored instanced mesh`);}}
    else if(o.matrixWorld.determinant()<0)mirroredOutput++;
  });
  assert.equal(mirroredOutput,mirroredPlain,`${tile.file} mirrored plain meshes retained`);
  let maxBasis=1;
  plain.scene.traverse(o=>{if(o.isMesh){const m=o.matrixWorld.elements;for(let r=0;r<3;r++)maxBasis=Math.max(maxBasis,Math.abs(m[r])+Math.abs(m[r+4])+Math.abs(m[r+8]));}});
  const referenceBounds=vertexBounds(plain.scene), actualBounds=vertexBounds(result.scene);
  let boundError=0;
  if(!referenceBounds.isEmpty()) {
    const a=[...actualBounds.min,...actualBounds.max], b=[...referenceBounds.min,...referenceBounds.max];
    boundError=Math.max(...a.map((v,i)=>Math.abs(v-b[i])));
    // Position quantization plus f32 instance TRS reconstruction; relative term
    // accounts for rounding in transformed coordinates, not a visual tolerance.
    const budget=(expected.quantization?.position_max_local_error??0)*maxBasis+1e-5*Math.max(1,...b.map(Math.abs));
    assert.ok(boundError<=budget,`${tile.file} bounds error ${boundError} > derived budget ${budget}`);
  }
  maxBoundError=Math.max(maxBoundError,boundError);
  console.log(JSON.stringify({file:tile.file,meshes:tileMeshes,mesh_instances:tileInstances,triangles:tileTriangles,streams:expected.streams.length,bounds_max_error:boundError,closed_curves:closedCurves,mirrored_plain:mirroredOutput}));
  meshes+=tileMeshes;triangles+=tileTriangles;instances+=tileInstances;
  result.scene.traverse(o=>{o.geometry?.dispose(); for(const m of (Array.isArray(o.material)?o.material:[o.material])) if(m){for(const v of Object.values(m))if(v?.isTexture)v.dispose();m.dispose();}});
}
console.log(JSON.stringify({tiles:index.tiles.length,meshes,mesh_instances:instances,triangles,streams,exact:streams,failed:0,max_bounds_error:maxBoundError,transformed_texture_slots:transformedSlots}));
