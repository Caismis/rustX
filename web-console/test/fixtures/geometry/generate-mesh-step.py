"""MIT rustX high-output fixture: named/colored independent analytic spheres.

uv run --no-project --with cadquery-ocp-novtk==7.9.3.1.1 python \
  test/fixtures/geometry/generate-mesh-step.py --output /tmp/curved-200.step

Independent free-shape bounds keep each shape's unchanged .001 deflection
meaningful even in a broad assembly; final output is hundreds of thousands of
triangles. Input references real analytic geometry, with no padding entities.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path
from OCP.BRepPrimAPI import BRepPrimAPI_MakeSphere
from OCP.IFSelect import IFSelect_RetDone
from OCP.Quantity import Quantity_Color, Quantity_TOC_RGB
from OCP.STEPCAFControl import STEPCAFControl_Writer
from OCP.STEPControl import STEPControl_AsIs
from OCP.TCollection import TCollection_ExtendedString
from OCP.TDataStd import TDataStd_Name
from OCP.TDocStd import TDocStd_Document
from OCP.XCAFDoc import XCAFDoc_DocumentTool, XCAFDoc_ColorType
from OCP.gp import gp_Pnt

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--parts', type=int, default=200)
args = parser.parse_args()
assert 1 <= args.parts <= 257  # 257 is the deliberate native/client rejection case.
model = TDocStd_Document(TCollection_ExtendedString('BinXCAF'))
shapes = XCAFDoc_DocumentTool.ShapeTool_s(model.Main())
colors = XCAFDoc_DocumentTool.ColorTool_s(model.Main())
for index in range(args.parts):
    shape = BRepPrimAPI_MakeSphere(gp_Pnt((index % 20) * 25, (index // 20) * 25, 0), 10).Shape()
    label = shapes.AddShape(shape, False)
    TDataStd_Name.Set_s(label, TCollection_ExtendedString(f'Curved part {index:03}'))
    color = ((.15,.4,.85),(.9,.4,.1),(.15,.7,.3))[index % 3]
    colors.SetColor(label, Quantity_Color(*color, Quantity_TOC_RGB), XCAFDoc_ColorType.XCAFDoc_ColorGen)
writer = STEPCAFControl_Writer(); writer.SetColorMode(True); writer.SetNameMode(True)
assert writer.Transfer(model, STEPControl_AsIs)
assert writer.Write(str(args.output)) == IFSelect_RetDone
text = re.sub(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d', '2026-11-17T00:00:00', args.output.read_text())
args.output.write_text('\n'.join(line.rstrip() for line in text.splitlines()) + '\n', encoding='utf-8', newline='\n')
raw = args.output.read_bytes()
print(json.dumps({'kind':'independent named colored analytic spheres','parts':args.parts,'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}))
