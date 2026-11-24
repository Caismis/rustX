"""MIT rustX negative admission fixture; not a performance workload.

An embedded sphere face is referenced repeatedly inside one compound. Shared
native triangulation keeps input and native geometry small while the logical
export exceeds the stream budgets. This verifies bounded failure before
allocating huge JS output; repetition is deliberate test topology.
"""
import argparse
import hashlib
import json
from pathlib import Path
from OCP.BRep import BRep_Builder, BRep_Tool
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.BRepPrimAPI import BRepPrimAPI_MakeSphere
from OCP.BRepTools import BRepTools
from OCP.TopAbs import TopAbs_FACE
from OCP.TopExp import TopExp_Explorer
from OCP.TopLoc import TopLoc_Location
from OCP.TopoDS import TopoDS, TopoDS_Compound

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
sphere = BRepPrimAPI_MakeSphere(10).Shape()
BRepMesh_IncrementalMesh(sphere, .02, False, .5)
face = TopoDS.Face_s(TopExp_Explorer(sphere, TopAbs_FACE).Current())
triangles = BRep_Tool.Triangulation_s(face, TopLoc_Location())
vertices = triangles.NbNodes()
references = 13_000_000 // vertices + 1
compound = TopoDS_Compound(); builder = BRep_Builder(); builder.MakeCompound(compound)
for _ in range(references): builder.Add(compound, face)
assert BRepTools.Write_s(compound, str(args.output))
raw = args.output.read_bytes()
print(json.dumps({'kind':'negative shared-face stream-budget fixture','references':references,'verticesPerFace':vertices,'logicalVertices':vertices*references,'logicalTriangles':triangles.NbTriangles()*references,'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}))
