"""Original MIT rustX whole-model LOD fixtures. Author with cadquery-ocp-novtk==7.9.3.1.1.

Three independently named/colored spheres have analytic bounds (-10,-12,-12)
to (88,12,12) mm. STEP has a root assembly and three part occurrences. This
fixture tests curved tessellation refinement, complete-scene part membership,
metadata and placement. BREP variants exercise source triangle preservation.
"""
from pathlib import Path
import re
from OCP.BRep import BRep_Builder
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.BRepPrimAPI import BRepPrimAPI_MakeSphere
from OCP.BRepTools import BRepTools
from OCP.IFSelect import IFSelect_RetDone
from OCP.Quantity import Quantity_Color, Quantity_TOC_RGB
from OCP.STEPCAFControl import STEPCAFControl_Writer
from OCP.STEPControl import STEPControl_AsIs
from OCP.TCollection import TCollection_ExtendedString
from OCP.TDataStd import TDataStd_Name
from OCP.TDocStd import TDocStd_Document
from OCP.TopLoc import TopLoc_Location
from OCP.TopoDS import TopoDS_Compound
from OCP.XCAFDoc import XCAFDoc_DocumentTool, XCAFDoc_ColorType
from OCP.gp import gp_Pnt

root=Path(__file__).parent
document=TDocStd_Document(TCollection_ExtendedString('BinXCAF'))
shapes=XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
colors=XCAFDoc_DocumentTool.ColorTool_s(document.Main())
assembly=shapes.NewShape()
TDataStd_Name.Set_s(assembly,TCollection_ExtendedString('Curved three-part assembly'))
compound=TopoDS_Compound();builder=BRep_Builder();builder.MakeCompound(compound)
for name,x,radius,color in [('Blue sphere',0,10,(0.15,0.4,0.85)),('Orange sphere',40,12,(0.9,0.4,0.1)),('Green sphere',80,8,(0.15,0.7,0.3))]:
    shape=BRepPrimAPI_MakeSphere(gp_Pnt(x,0,0),radius).Shape()
    builder.Add(compound,shape)
    label=shapes.AddShape(shape,False)
    TDataStd_Name.Set_s(label,TCollection_ExtendedString(name))
    colors.SetColor(label,Quantity_Color(*color,Quantity_TOC_RGB),XCAFDoc_ColorType.XCAFDoc_ColorGen)
    component=shapes.AddComponent(assembly,label,TopLoc_Location())
    TDataStd_Name.Set_s(component,TCollection_ExtendedString(name))
shapes.UpdateAssemblies()
writer=STEPCAFControl_Writer();writer.SetColorMode(True);writer.SetNameMode(True)
assert writer.Transfer(document,STEPControl_AsIs)
p=root/'lod-assembly.step';assert writer.Write(str(p))==IFSelect_RetDone
text=re.sub(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d','2026-11-05T00:00:00',p.read_text())
p.write_text('\n'.join(line.rstrip() for line in text.splitlines())+'\n',encoding='utf-8',newline='\n')
assert BRepTools.Write_s(compound,str(root/'lod-assembly.brep'))
BRepMesh_IncrementalMesh(compound,0.1,False,0.3)
assert BRepTools.Write_s(compound,str(root/'lod-assembly-meshed.brep'))
for name in ('lod-assembly.brep','lod-assembly-meshed.brep'):
    p=root/name
    p.write_text('\n'.join(line.rstrip() for line in p.read_text().splitlines())+'\n',encoding='utf-8',newline='\n')
print('Generated original curved fixtures',[(p.name,p.stat().st_size) for p in root.glob('lod-assembly*')])
