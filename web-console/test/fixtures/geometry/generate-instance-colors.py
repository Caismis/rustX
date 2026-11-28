"""Generate the original MIT STEP instance-color fixture with cadquery-ocp-novtk 7.9.3.1.1."""

from pathlib import Path
import re

from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox
from OCP.IFSelect import IFSelect_RetDone
from OCP.Quantity import Quantity_Color, Quantity_TOC_RGB
from OCP.STEPCAFControl import STEPCAFControl_Writer
from OCP.STEPControl import STEPControl_AsIs
from OCP.TCollection import TCollection_ExtendedString
from OCP.TDataStd import TDataStd_Name
from OCP.TDocStd import TDocStd_Document
from OCP.TopLoc import TopLoc_Location
from OCP.XCAFDoc import XCAFDoc_ColorType, XCAFDoc_DocumentTool
from OCP.gp import gp_Trsf, gp_Vec


document = TDocStd_Document(TCollection_ExtendedString("BinXCAF"))
shapes = XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
colors = XCAFDoc_DocumentTool.ColorTool_s(document.Main())
surface = XCAFDoc_ColorType.XCAFDoc_ColorSurf

assembly = shapes.NewShape()
TDataStd_Name.Set_s(assembly, TCollection_ExtendedString("Colored instances"))
part = shapes.AddShape(BRepPrimAPI_MakeBox(10, 10, 10).Shape(), False)
TDataStd_Name.Set_s(part, TCollection_ExtendedString("Cube"))
colors.SetColor(part, Quantity_Color(1, 0, 0, Quantity_TOC_RGB), surface)

first = shapes.AddComponent(assembly, part, TopLoc_Location())
TDataStd_Name.Set_s(first, TCollection_ExtendedString("Inherited red"))
offset = gp_Trsf()
offset.SetTranslation(gp_Vec(30, 0, 0))
second = shapes.AddComponent(assembly, part, TopLoc_Location(offset))
TDataStd_Name.Set_s(second, TCollection_ExtendedString("Override blue"))
colors.SetColor(second, Quantity_Color(0, 0, 1, Quantity_TOC_RGB), surface)
shapes.UpdateAssemblies()

writer = STEPCAFControl_Writer()
writer.SetColorMode(True)
writer.SetNameMode(True)
assert writer.Transfer(document, STEPControl_AsIs)
output = Path(__file__).with_name("instance-colors.step")
assert writer.Write(str(output)) == IFSelect_RetDone
text = re.sub(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d", "2026-11-28T00:00:00", output.read_text())
output.write_text("\n".join(line.rstrip() for line in text.splitlines()) + "\n", encoding="utf-8", newline="\n")
