"""Diagnostic scaling: fixed four control-net rows, widths N/2N/4N, every pole referenced."""
import pathlib,math,json,hashlib,re,time
from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeFace
from OCP.Geom import Geom_BSplineSurface
from OCP.IFSelect import IFSelect_RetDone
from OCP.Interface import Interface_Static
from OCP.STEPControl import STEPControl_AsIs,STEPControl_Writer
from OCP.TColgp import TColgp_Array2OfPnt
from OCP.TColStd import TColStd_Array1OfInteger,TColStd_Array1OfReal
from OCP.gp import gp_Pnt
out=pathlib.Path('/tmp/pr556-step-experiments/scaling');out.mkdir(exist_ok=True)
def knots(n):
 k=TColStd_Array1OfReal(1,n-2);m=TColStd_Array1OfInteger(1,n-2)
 for i in range(n-2):k.SetValue(i+1,i/(n-3));m.SetValue(i+1,4 if i in (0,n-3)else 1)
 return k,m
manifests=[]
for width in [2048,4096,8192]:
 start=time.perf_counter();poles=TColgp_Array2OfPnt(1,4,1,width)
 for u in range(4):
  for v in range(width):
   x=1000*u/3;y=1000*v/(width-1);z=5*math.sin(2*math.pi*u/3)*math.cos(2*math.pi*v/(width-1));poles.SetValue(u+1,v+1,gp_Pnt(x,y,z))
 uk,um=knots(4);vk,vm=knots(width);surface=Geom_BSplineSurface(poles,uk,vk,um,vm,3,3);face=BRepBuilderAPI_MakeFace(surface,1e-7).Face();writer=STEPControl_Writer();assert Interface_Static.SetCVal_s('write.step.unit','MM');assert writer.Transfer(face,STEPControl_AsIs)==IFSelect_RetDone;file=out/f'wide-4x{width}.step';assert writer.Write(str(file))==IFSelect_RetDone
 data=file.read_bytes();data=re.sub(rb'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d',b'2026-11-05T00:00:00',data);file.write_bytes(data);record={'rows':4,'width':width,'degreeU':3,'degreeV':3,'referencedControlPoles':4*width,'inputBytes':len(data),'sha256':hashlib.sha256(data).hexdigest(),'generationSeconds':time.perf_counter()-start};manifests.append(record);print(json.dumps(record),flush=True)
(out/'manifest.json').write_text(json.dumps(manifests,indent=2)+'\n')
