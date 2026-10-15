import test from 'node:test';
import assert from 'node:assert/strict';
import {cameraFromUrl,viewReport,screenshotName} from './report.js';

test('report JSON URL restores the exact camera and scene',()=>{
  const camera={position:[235.123456789,-58.125,1040.00000001],target:[206.352,31.236,1008.799],fov:43.75};
  const report=viewReport({url:'https://example.test/viewer/?scene=legion/&cam=0,0,0,0,0,0',scene:'legion/',...camera,pick:[{object:'mesh',tile:'tile.glb',point:[1,2,3]}],note:'A "face" disappears\nwhen orbiting',viewport:{width:1600,height:900}});
  const pasted=JSON.parse(JSON.stringify(report));
  assert.deepEqual(cameraFromUrl(pasted.url),camera);
  assert.equal(new URL(pasted.url).searchParams.get('scene'),'legion/');
  assert.deepEqual(pasted.pick,report.pick);
  assert.equal(pasted.note,report.note);
  assert.ok(pasted.viewer_build);
});

test('invalid camera parameters are rejected and filenames are portable',()=>{
  assert.equal(cameraFromUrl('https://example.test/'),null);
  assert.throws(()=>cameraFromUrl('https://example.test/?cam=1,2,3,4,5,6&fov=NaN'));
  assert.throws(()=>cameraFromUrl('https://example.test/?cam=1,2,3,4,5,6&fov=180'));
  const name=screenshotName('legion/','1,2,3,4,5,6',new Date('2026-01-01T12:34:56Z'));
  assert.ok(name.startsWith('legion__1_2_3_4_5_6_'));
  assert.ok(name.endsWith('.png'));
  assert.doesNotMatch(name,/[\\/:*?"<>|]/);
});
