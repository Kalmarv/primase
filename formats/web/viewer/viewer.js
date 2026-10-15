import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { MeshoptDecoder } from 'three/addons/libs/meshopt_decoder.module.js';
import { contentBounds, curvePoints } from './geometry.js';
import { cameraFromUrl, viewReport, screenshotName, VIEWER_BUILD } from './report.js';
const status=document.querySelector('#status');
const base=new URL(new URLSearchParams(location.search).get('scene')??'./',location.href);
const renderer=new THREE.WebGLRenderer({antialias:true});renderer.setPixelRatio(Math.min(devicePixelRatio,2));renderer.setSize(innerWidth,innerHeight);renderer.setClearColor(0x182331);document.body.append(renderer.domElement);
const scene=new THREE.Scene();const camera=new THREE.PerspectiveCamera(55,innerWidth/innerHeight,0.1,15000);
const controls=new OrbitControls(camera,renderer.domElement);controls.enableDamping=true;
window.primaseCamera=()=>[...camera.position.toArray(),...controls.target.toArray()].join(',');
scene.add(new THREE.HemisphereLight(0xdcecff,0x737061,2.5));const sun=new THREE.DirectionalLight(0xfff4dd,3);sun.position.set(100,250,80);scene.add(sun);
const loader=new GLTFLoader().setMeshoptDecoder(MeshoptDecoder);
const loaded=new Map();const pending=new Set();const failed=new Set();let failures=0;let renderedFrames=0;
// Diagnostic picking for reproducible visual investigations (CSS pixel coordinates).
window.primasePick=(x,y)=>{
  const rect=renderer.domElement.getBoundingClientRect(),ray=new THREE.Raycaster();
  ray.setFromCamera(new THREE.Vector2((x-rect.left)/rect.width*2-1,1-(y-rect.top)/rect.height*2),camera);
  return ray.intersectObjects([...loaded.values()].map(r=>r.scene),true).slice(0,4).map(hit=>{
    let owner,tile,instance;
    for(let node=hit.object;node;node=node.parent){
      const instances=node.userData?.primase?.['web.instances']?.value;
      if(instances&&hit.instanceId!==undefined)instance=instances[hit.instanceId];
      for(const [file,result] of loaded)if(result.scene===node){owner=result;tile=file;}
    }
    const mapping=owner?.parser.associations.get(hit.object);
    const mesh=owner?.parser.json.meshes?.[mapping?.meshes];
    const prototype=mesh?.extras?.primase?.['skate.prototype']?.value;
    return {object:hit.object.name,prototype:prototype?{id:prototype.id,name:mesh.name,glb:prototype.glb,key:prototype.key}:null,tile,instance:instance??hit.instanceId??null,point:hit.point.toArray(),face:hit.faceIndex,source_record:prototype?.parts?.[mapping?.primitives]??null,material:(Array.isArray(hit.object.material)?hit.object.material:[hit.object.material]).map(m=>({name:m.name,side:m.side,map:m.map?.name??null,source_record:m.userData})),determinant:hit.object.matrixWorld.determinant()};
  });
};
let lastClick=null;
renderer.domElement.addEventListener('click',event=>{lastClick=[event.clientX,event.clientY];});
const reportStatus=document.querySelector('#reportStatus');
window.primaseViewReport=()=>{
  const [x,y]=lastClick??[innerWidth/2,innerHeight/2];
  return viewReport({url:location.href,scene:new URLSearchParams(location.search).get('scene')??'./',position:camera.position.toArray(),target:controls.target.toArray(),fov:camera.fov,pick:{screen:[x,y],hits:window.primasePick(x,y)},note:document.querySelector('#note').value,viewport:{width:innerWidth,height:innerHeight}});
};
async function copyView(){
  const text=JSON.stringify(window.primaseViewReport(),null,2);
  try{await navigator.clipboard.writeText(text);reportStatus.textContent='View copied';document.querySelector('#reportText').hidden=true;}
  catch{const area=document.querySelector('#reportText');area.hidden=false;area.value=text;area.focus();area.select();reportStatus.textContent='Clipboard unavailable. Copy the selected JSON.';}
}
function screenshot(){
  renderer.render(scene,camera);
  renderer.domElement.toBlob(blob=>{
    if(!blob){reportStatus.textContent='Screenshot failed';return;}
    const url=URL.createObjectURL(blob),link=document.createElement('a');link.href=url;link.download=screenshotName(new URLSearchParams(location.search).get('scene')??'scene',window.primaseCamera());link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);reportStatus.textContent='Screenshot downloaded';
  },'image/png');
}
document.querySelector('#copyView').addEventListener('click',copyView);
document.querySelector('#screenshot').addEventListener('click',screenshot);
addEventListener('keydown',event=>{if(event.repeat||event.ctrlKey||event.metaKey||event.altKey||event.target.isContentEditable||/^(INPUT|TEXTAREA|SELECT)$/.test(event.target.tagName))return;const key=event.key.toLowerCase();if(key==='c'||key==='s'){event.preventDefault();if(key==='c')copyView();else screenshot();}});
document.querySelector('#build').textContent=`Viewer ${VIEWER_BUILD}`;
function viewerExtras(root){const group=new THREE.Group();group.name='ViewerExtras';root.add(group);const records=[];root.traverse(o=>{const r=o.userData?.primase?.['web.viewer']?.value;if(r)records.push(r);});
for(const r of records){if(r.kind==='collision_reference')continue;if(r.kind==='grind_curve'){const g=new THREE.BufferGeometry().setFromPoints(curvePoints(r.data));group.add(new THREE.Line(g,new THREE.LineBasicMaterial({color:0xffcb5b})));}
else if(r.kind==='collision'){const g=new THREE.BufferGeometry();g.setAttribute('position',new THREE.Float32BufferAttribute(r.data.positions.flat(),3));g.setIndex(r.data.triangles.flat());group.add(new THREE.Mesh(g,new THREE.MeshBasicMaterial({color:0x45d7ce,wireframe:true,transparent:true,opacity:0.2})));}
else {const marker=new THREE.Mesh(new THREE.SphereGeometry(0.8,8,6),new THREE.MeshBasicMaterial({color:r.kind==='spawn'?0x8bff7b:0xff88bd}));marker.position.fromArray(r.anchor);group.add(marker);}}
group.visible=document.querySelector('#extras').checked;return group;}
function dispose(root){root.traverse(o=>{o.geometry?.dispose();for(const m of(Array.isArray(o.material)?o.material:[o.material]))if(m){for(const v of Object.values(m))if(v?.isTexture)v.dispose();m.dispose();}});}
function updateCollision(file, result, tile) {
  const enabled=document.querySelector('#collision').checked;
  if(!enabled && result.collision){result.scene.remove(result.collision);dispose(result.collision);result.collision=null;}
  if(!enabled || !tile.collision || result.collision || result.collisionPending || result.collisionFailed)return;
  result.collisionPending=true;
  loader.loadAsync(new URL(tile.collision.file,base).href).then(collision=>{
    const group=collision.scene;group.name='Collision';
    if(loaded.get(file)!==result || !document.querySelector('#collision').checked){dispose(group);return;}
    group.traverse(o=>{if(o.isMesh){o.material?.dispose();o.material=new THREE.MeshBasicMaterial({color:0x45d7ce,wireframe:true,transparent:true,opacity:0.2});}});
    result.collision=group;result.scene.add(group);
  }).catch(e=>{result.collisionFailed=true;failures++;console.error(e);}).finally(()=>{result.collisionPending=false;});
}
try {const response=await fetch(new URL('tiles.json',base));if(!response.ok)throw Error(`index ${response.status}`);const index=await response.json();if(!index.tiles.length)throw Error('empty tile set');
const bounds=contentBounds(index.tiles);const centre=bounds.getCenter(new THREE.Vector3());const extent=bounds.getSize(new THREE.Vector3()).length();controls.target.copy(centre);camera.position.copy(centre).add(new THREE.Vector3(extent*.35,extent*.4,extent*.35));camera.far=Math.max(15000,extent*4);camera.updateProjectionMatrix();
const cameraParameter=cameraFromUrl(location.href);
if(cameraParameter){camera.position.fromArray(cameraParameter.position);controls.target.fromArray(cameraParameter.target);camera.fov=cameraParameter.fov;camera.updateProjectionMatrix();}
controls.update();
function update(){const radius=Number(document.querySelector('#radius').value);document.querySelector('#radiusLabel').textContent=`${radius} m`;const wanted=new Set(index.tiles.filter(t=>Math.hypot(Math.max(t.min[0]-controls.target.x,0,controls.target.x-t.max[0]),Math.max(t.min[2]-controls.target.z,0,controls.target.z-t.max[2]))<=radius).map(t=>t.file));
for(const [file,r]of loaded)if(!wanted.has(file)){scene.remove(r.scene);dispose(r.scene);loaded.delete(file);}
for(const file of wanted)if(!loaded.has(file)&&!pending.has(file)&&!failed.has(file)){pending.add(file);loader.loadAsync(new URL(file,base).href).then(g=>{g.extras=viewerExtras(g.scene);loaded.set(file,g);scene.add(g.scene);}).catch(e=>{failed.add(file);failures++;console.error(e);}).finally(()=>pending.delete(file));}
let triangles=0;for(const [file,r]of loaded){const tile=index.tiles.find(t=>t.file===file);triangles+=tile.triangles;r.extras.visible=document.querySelector('#extras').checked;updateCollision(file,r,tile);}status.textContent=`${loaded.size}/${index.tiles.length} tiles · ${triangles.toLocaleString()} triangles · ${pending.size} loading${failures?` · ${failures} failures`:''}`;
window.primaseProof={loaded:loaded.size,total:index.tiles.length,triangles,pending:pending.size,failures,renderedFrames};}
setInterval(update,500);update();renderer.setAnimationLoop(()=>{controls.update();renderer.render(scene,camera);renderedFrames++;});
}catch(e){status.textContent=String(e);console.error(e);}
addEventListener('resize',()=>{camera.aspect=innerWidth/innerHeight;camera.updateProjectionMatrix();renderer.setSize(innerWidth,innerHeight);});
