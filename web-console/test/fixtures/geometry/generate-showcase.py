"""Original MIT-licensed mounting bracket; OCCT 7.9.3 authoring tools only.

Writes a named, colored STEP assembly and a self-contained GLB in meters.
Coordinates are Y-up; analytic assembly bounds are 120 x 72 x 80 mm.
"""
from pathlib import Path
import json
import math
import re
import struct

from OCP.BRep import BRep_Tool
from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut, BRepAlgoAPI_Fuse
from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeFace, BRepBuilderAPI_MakePolygon
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakeCylinder, BRepPrimAPI_MakePrism
from OCP.IFSelect import IFSelect_RetDone
from OCP.Quantity import Quantity_Color, Quantity_TOC_RGB
from OCP.STEPCAFControl import STEPCAFControl_Writer
from OCP.STEPControl import STEPControl_AsIs
from OCP.TCollection import TCollection_ExtendedString
from OCP.TDataStd import TDataStd_Name
from OCP.TDocStd import TDocStd_Document
from OCP.TopAbs import TopAbs_FACE, TopAbs_REVERSED
from OCP.TopExp import TopExp_Explorer
from OCP.TopLoc import TopLoc_Location
from OCP.TopoDS import TopoDS
from OCP.XCAFDoc import XCAFDoc_DocumentTool, XCAFDoc_ColorType
from OCP.gp import gp_Ax2, gp_Dir, gp_Pnt, gp_Trsf, gp_Vec

root = Path(__file__).parent
parts = []
steel = (0.68, 0.73, 0.79)
blue = (0.18, 0.43, 0.62)


def cylinder(x, y, z, radius, length, axis=(0, 1, 0)):
    return BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(x, y, z), gp_Dir(*axis)), radius, length).Shape()


base = BRepPrimAPI_MakeBox(gp_Pnt(-60, 0, -40), 120, 8, 80).Shape()
for x in (-48, 48):
    for z in (-28, 28):
        base = BRepAlgoAPI_Cut(base, cylinder(x, 0, z, 4.5, 8)).Shape()
parts.append(("Base plate", base, (0.49, 0.57, 0.65), 0.35))
upright = BRepPrimAPI_MakeBox(gp_Pnt(-40, 8, 24), 80, 64, 8).Shape()
upright = BRepAlgoAPI_Cut(upright, cylinder(0, 46, 24, 18, 8, (0, 0, 1))).Shape()
parts.append(("Upright", upright, blue, 0.2))
for x, name in [(-36, "Left gusset"), (30, "Right gusset")]:
    polygon = BRepBuilderAPI_MakePolygon()
    for point in [(x, 8, -24), (x, 8, 24), (x, 60, 24)]:
        polygon.Add(gp_Pnt(*point))
    polygon.Close()
    rib = BRepPrimAPI_MakePrism(BRepBuilderAPI_MakeFace(polygon.Wire()).Face(), gp_Vec(6, 0, 0)).Shape()
    parts.append((name, rib, blue, 0.2))
for index, (x, z) in enumerate([(x, z) for x in (-48, 48) for z in (-28, 28)], 1):
    washer = BRepAlgoAPI_Cut(cylinder(x, 8, z, 7, 1.2), cylinder(x, 8, z, 4.2, 1.2)).Shape()
    bolt = BRepAlgoAPI_Fuse(cylinder(x, 0, z, 4, 9.2), cylinder(x, 9.2, z, 6.5, 5)).Shape()
    parts.append((f"Washer {index}", washer, (0.35, 0.4, 0.46), 0.7))
    parts.append((f"Bolt {index}", bolt, steel, 0.75))

document = TDocStd_Document(TCollection_ExtendedString("BinXCAF"))
shapes = XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
colors = XCAFDoc_DocumentTool.ColorTool_s(document.Main())
assembly = shapes.NewShape()
TDataStd_Name.Set_s(assembly, TCollection_ExtendedString("Mounting bracket"))
for name, shape, color, _metallic in parts:
    label = shapes.AddShape(shape, False)
    TDataStd_Name.Set_s(label, TCollection_ExtendedString(name))
    colors.SetColor(label, Quantity_Color(*color, Quantity_TOC_RGB), XCAFDoc_ColorType.XCAFDoc_ColorGen)
    component = shapes.AddComponent(assembly, label, TopLoc_Location())
    TDataStd_Name.Set_s(component, TCollection_ExtendedString(name))
shapes.UpdateAssemblies()
writer = STEPCAFControl_Writer()
writer.SetColorMode(True)
writer.SetNameMode(True)
assert writer.Transfer(document, STEPControl_AsIs)
step_path = root / "mounting-bracket.step"
assert writer.Write(str(step_path)) == IFSelect_RetDone
text = step_path.read_text()
text = re.sub(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d", "2026-11-03T00:00:00", text)
step_path.write_text("\n".join(line.rstrip() for line in text.splitlines()) + "\n", encoding="utf-8", newline="\n")

glb = {"asset": {"version": "2.0", "generator": "rustX original bracket fixture"}, "scene": 0,
       "scenes": [{"nodes": [0]}], "nodes": [{"name": "Mounting bracket", "children": []}],
       "meshes": [], "materials": [], "bufferViews": [], "accessors": [], "buffers": []}
binary = bytearray()


def attribute(values, bounds=False):
    at = len(binary)
    binary.extend(struct.pack("<" + "f" * len(values), *values))
    view = len(glb["bufferViews"])
    glb["bufferViews"].append({"buffer": 0, "byteOffset": at, "byteLength": len(binary) - at})
    accessor = {"bufferView": view, "componentType": 5126, "count": len(values) // 3, "type": "VEC3"}
    if bounds:
        accessor["min"] = [min(values[i::3]) for i in range(3)]
        accessor["max"] = [max(values[i::3]) for i in range(3)]
    glb["accessors"].append(accessor)
    return len(glb["accessors"]) - 1


for name, shape, color, metallic in parts:
    BRepMesh_IncrementalMesh(shape, 0.1, False, 0.1, True)
    faces = TopExp_Explorer(shape, TopAbs_FACE)
    positions, normals = [], []
    while faces.More():
        face = TopoDS.Face_s(faces.Current())
        location = TopLoc_Location()
        mesh = BRep_Tool.Triangulation_s(face, location)
        for index in range(1, mesh.NbTriangles() + 1):
            indices = list(mesh.Triangle(index).Get())
            if face.Orientation() == TopAbs_REVERSED:
                indices[1], indices[2] = indices[2], indices[1]
            points = [mesh.Node(i).Transformed(location.Transformation()) for i in indices]
            points = [(p.X() / 1000, p.Y() / 1000, p.Z() / 1000) for p in points]
            a, b, c = points
            u, v = [[p[i] - a[i] for i in range(3)] for p in (b, c)]
            normal = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]]
            length = math.sqrt(sum(value * value for value in normal))
            if length > 0:
                normal = [value / length for value in normal]
                positions.extend(value for point in points for value in point)
                normals.extend(normal * 3)
        faces.Next()
    material = len(glb["materials"])
    glb["materials"].append({"name": name, "pbrMetallicRoughness": {"baseColorFactor": [*color, 1], "metallicFactor": metallic, "roughnessFactor": 0.48}})
    glb["meshes"].append({"primitives": [{"attributes": {"POSITION": attribute(positions, True), "NORMAL": attribute(normals)}, "material": material}]})
    glb["nodes"][0]["children"].append(len(glb["nodes"]))
    glb["nodes"].append({"name": name, "mesh": len(glb["meshes"]) - 1})
glb["buffers"] = [{"byteLength": len(binary)}]
metadata = json.dumps(glb, separators=(",", ":")).encode()
metadata += b" " * (-len(metadata) % 4)
payload = struct.pack("<II", len(metadata), 0x4E4F534A) + metadata + struct.pack("<II", len(binary), 0x004E4942) + binary
(root / "mounting-bracket.glb").write_bytes(struct.pack("<III", 0x46546C67, 2, len(payload) + 12) + payload)
print(f"Wrote {len(parts)} parts; STEP {step_path.stat().st_size} bytes; GLB {len(payload) + 12} bytes")

# A 96-part assembly exercises repeated CAD imports without changing tolerances.
array = shapes.NewShape()
TDataStd_Name.Set_s(array, TCollection_ExtendedString("Bracket array"))
for index in range(8):
    placement = gp_Trsf()
    placement.SetTranslation(gp_Vec(150 * (index % 4), 0, 110 * (index // 4)))
    component = shapes.AddComponent(array, assembly, TopLoc_Location(placement))
    TDataStd_Name.Set_s(component, TCollection_ExtendedString(f"Bracket {index + 1}"))
shapes.UpdateAssemblies()
writer = STEPCAFControl_Writer()
writer.SetColorMode(True)
writer.SetNameMode(True)
assert writer.Transfer(array, STEPControl_AsIs)
array_path = root / "bracket-array.step"
assert writer.Write(str(array_path)) == IFSelect_RetDone
text = re.sub(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d", "2026-11-03T00:00:00", array_path.read_text())
array_path.write_text("\n".join(line.rstrip() for line in text.splitlines()) + "\n", encoding="utf-8", newline="\n")
