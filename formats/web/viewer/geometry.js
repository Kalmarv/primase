import { Box3, Vector3 } from 'three';

export function contentBounds(tiles) {
  const geometry=tiles.filter(tile=>tile.triangles>0);
  const bounds=new Box3();
  for(const tile of (geometry.length?geometry:tiles)) {
    bounds.expandByPoint(new Vector3(...tile.min));
    bounds.expandByPoint(new Vector3(...tile.max));
  }
  return bounds;
}

export function curvePoints(curve) {
  const points=curve.points.map(p=>new Vector3(...p));
  if(curve.closed && points.length>0)points.push(points[0].clone());
  return points;
}

export function parseCamera(value) {
  const values=value.split(',').map(Number);
  if(values.length!==6 || value.split(',').some(v=>v.trim()==='') || values.some(v=>!Number.isFinite(v)))throw Error('cam requires six finite numbers: x,y,z,tx,ty,tz');
  return values;
}
