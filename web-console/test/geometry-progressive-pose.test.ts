import {expect,it} from 'vitest';
import {Box3,Vector3} from 'three';
import {cameraFromPose} from '../src/app/components/documents/geometry/progressive';
it.each(['perspective','orthographic'] as const)('keeps normalized framing, pan and the Top orbit basis across a narrow %s refinement',projection=>{
  const element=document.createElement('div');Object.defineProperties(element,{clientWidth:{value:320},clientHeight:{value:640}});
  const bounds=new Box3(new Vector3(0,0,0),new Vector3(10,10,10)),pose={direction:[0,1,0] as [number,number,number],target:[.1,0,0] as [number,number,number],up:[0,0,-1] as [number,number,number],zoom:2};
  const camera=cameraFromPose(pose,bounds,element,projection);
  expect(camera.up).toEqual([0,0,-1]);expect(camera.target).toEqual([1,0,0]);expect(camera.position[0]).toBe(1);expect(camera.position[2]).toBe(0);
  const radius=bounds.getSize(new Vector3()).length()/2;
  if(camera.projection==='perspective')expect(camera.position[1]).toBeCloseTo(radius/Math.sin(Math.PI/8)/.5*1.1/2);
  else expect(camera.verticalSpan).toBeCloseTo(2*radius/.5*1.1/2);
});

it('normalizes LOD camera handoff to orthographic projection by default', () => {
  const element = document.createElement('div'), bounds = new Box3(new Vector3(), new Vector3(10, 10, 10));
  expect(cameraFromPose({direction: [1,1,1], target: [0,0,0], zoom: 1}, bounds, element).projection).toBe('orthographic');
});
