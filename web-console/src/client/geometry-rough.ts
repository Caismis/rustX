/** Small, version-bound coordinate envelope. It is never an admitted CAD mesh. */
export interface GeometryRoughPreview { version: string; positions: Float32Array }
export interface GeometryRoughPose { direction: [number, number, number]; target: [number, number, number]; zoom: number }
export const isStepName = (name: string) => /\.(?:step|stp)$/i.test(name);
export function hasRoughArea(points:number[]) {
  if(points.length<9)return false;
  const project=(at:number)=>[(points[at]-points[at+2])/Math.sqrt(2),(points[at]+points[at+2])/Math.sqrt(6)-points[at+1]*Math.sqrt(2/3)];
  const first=project(0);let base:number[]|undefined;
  for(let at=3;at<points.length;at+=3){const next=project(at);if(!base){if(Math.hypot(next[0]-first[0],next[1]-first[1])>1e-9)base=next;}else if(Math.abs((base[0]-first[0])*(next[1]-first[1])-(base[1]-first[1])*(next[0]-first[0]))>1e-12)return true;}
  return false;
}
export function isStepSource(bytes: Uint8Array) {
  return /^\s*ISO-10303-21\s*;/i.test(new TextDecoder().decode(bytes.subarray(0, 128)));
}
export function stepPointSamples(bytes: Uint8Array): number[] {
  const text = new TextDecoder().decode(bytes.subarray(0, 65536)), points: number[][] = [];
  const record = /#\d{1,12}\s*=\s*CARTESIAN_POINT\s*\(\s*'(?:[^'\r\n]|''){0,128}'\s*,\s*\(\s*([^,()]{1,64}),\s*([^,()]{1,64}),\s*([^,()]{1,64})\)\s*\)\s*;/gi;
  const numeric = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[Ee][+-]?\d+)?$/;
  for (const match of text.matchAll(record)) {
    const tokens = match.slice(1).map(value => value.trim());
    if (!tokens.every(value => numeric.test(value))) continue;
    const values = tokens.map(Number);
    if (values.every(value => Number.isFinite(value) && Math.abs(value) <= 1e12)) points.push(values);
  }
  const output: number[] = [], count = Math.min(128, points.length);
  for (let i = 0; i < count; i++) output.push(...points[Math.floor(i * (points.length - 1) / Math.max(1, count - 1))]);
  return output;
}
