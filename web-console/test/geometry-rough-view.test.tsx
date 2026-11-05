import {cleanup,fireEvent,render} from '@testing-library/react';import {afterEach,expect,it} from 'vitest';
import {GeometryRoughPreview} from '../src/app/components/documents/GeometryRoughPreview';import type {GeometryRoughPose} from '../src/client/geometry-rough';import {en} from '../src/locale/dictionaries/geometry';
afterEach(cleanup);
it('draws source-derived geometry, updates a normalized handoff pose and fences retired inputs',()=>{
  let current=true;const pose:{current:GeometryRoughPose|undefined}={current:undefined};
  const preview={version:'one',positions:new Float32Array([0,0,0,1,0,0,0,1,0,0,0,1])};
  const ui=render(<GeometryRoughPreview preview={preview} copyId="copy" current={()=>current} pose={pose} labels={en}/>),svg=ui.getByRole('img'),path=svg.querySelector('path')!;
  expect(path.getAttribute('d')).toContain('L');expect(svg.getAttribute('data-geometry-rough')).toBe('one');expect(pose.current).toBeUndefined();
  fireEvent.keyDown(svg,{code:'ArrowRight'});expect(pose.current?.target[0]).not.toBe(0);
  const before=path.getAttribute('d');current=false;fireEvent.keyDown(svg,{code:'ArrowRight'});expect(path.getAttribute('d')).toBe(before);
});
it('does not draw a placeholder for degenerate or invalid coordinate sets',()=>{
  const ui=render(<GeometryRoughPreview preview={{version:'one',positions:new Float32Array([NaN,0,0,1,0,0,0,1,0])}} copyId="copy" current={()=>true} pose={{current:undefined}} labels={en}/>);expect(ui.queryByRole('img')).toBeNull();
});
