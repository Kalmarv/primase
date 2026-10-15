import { parseCamera } from './geometry.js';

export const VIEWER_BUILD='1.2.0';

export function cameraFromUrl(value) {
  const url=new URL(value);
  const cam=url.searchParams.get('cam');
  if(cam===null)return null;
  const values=parseCamera(cam);
  const fov=Number(url.searchParams.get('fov')??55);
  if(!Number.isFinite(fov)||fov<=0||fov>=180)throw Error('fov must be between 0 and 180');
  return {position:values.slice(0,3),target:values.slice(3),fov};
}

export function viewReport({url,scene,position,target,fov,pick,note,viewport}) {
  const link=new URL(url);
  link.searchParams.set('cam',[...position,...target].join(','));
  link.searchParams.set('fov',String(fov));
  return {version:1,scene,viewer_build:VIEWER_BUILD,url:link.href,camera:{position,target,fov},viewport,pick,note:note??''};
}

export function screenshotName(scene,cam,time=new Date()) {
  const safe=value=>value.replace(/[^a-zA-Z0-9_.-]/g,'_');
  return `${safe(scene)}_${safe(cam).slice(0,150)}_${safe(time.toISOString())}.png`;
}
