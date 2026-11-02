"""Generate analytic 10-unit cubes. Run with cadquery-ocp-novtk==7.9.3.1.1.

The fixtures and this generator are original rustX test data (MIT).
OCCT exporters run independently of the browser importer. Bounds are analytic:
min=(0,0,0), max=(10,10,10), surface area=600, volume=1000.
"""
from pathlib import Path
import json
import struct
import base64
import zlib
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox
from OCP.BRepTools import BRepTools
from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs
from OCP.IGESControl import IGESControl_Writer
from OCP.Interface import Interface_Static

root = Path(__file__).parent
shape = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape()
BRepTools.Write_s(shape, str(root / "cube.brep"))
for unit, name in [("MM", "cube.step"), ("INCH", "cube-inch.step")]:
    Interface_Static.SetCVal_s("write.step.unit", unit)
    writer = STEPControl_Writer()
    writer.Transfer(shape, STEPControl_AsIs)
    writer.Write(str(root / name))
iges = IGESControl_Writer("MM", 1)
iges.AddShape(shape)
iges.ComputeModel()
iges.Write(str(root / "cube.iges"))

points = [(0,0,0), (10,0,0), (10,10,0), (0,10,0), (0,0,10), (10,0,10), (10,10,10), (0,10,10)]
faces = [(0,2,1),(0,3,2),(4,5,6),(4,6,7),(0,1,5),(0,5,4),(3,7,6),(3,6,2),(0,4,7),(0,7,3),(1,2,6),(1,6,5)]
vertices = [points[index] for face in faces for index in face]
lines = ["solid cube"]
binary = bytearray(80) + struct.pack("<I", len(faces))
for face in faces:
    a,b,c = [points[i] for i in face]
    u=[b[i]-a[i] for i in range(3)]; v=[c[i]-a[i] for i in range(3)]
    n=[u[1]*v[2]-u[2]*v[1],u[2]*v[0]-u[0]*v[2],u[0]*v[1]-u[1]*v[0]]
    length=sum(x*x for x in n)**0.5; n=[x/length for x in n]
    lines += ["facet normal " + " ".join(map(str,n)), "outer loop"]
    lines += ["vertex " + " ".join(map(str, p)) for p in (a,b,c)]
    lines += ["endloop", "endfacet"]
    binary += struct.pack("<12fH", *n, *a, *b, *c, 0)
(root / "cube-ascii.stl").write_text("\n".join(lines + ["endsolid cube", ""]), encoding="utf-8", newline="\n")
(root / "cube.stl").write_bytes(binary)
positions = struct.pack("<" + "f"*(len(vertices)*3), *(x for p in vertices for x in p))
document = {"asset":{"version":"2.0","generator":"rustX analytic fixture"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"name":"Cube assembly","children":[1]},{"name":"Analytic cube","mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0}]}],"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[0.15,0.55,0.85,1]}}],"buffers":[{"byteLength":len(positions)}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":len(positions)}],"accessors":[{"bufferView":0,"componentType":5126,"count":len(vertices),"type":"VEC3","min":[0,0,0],"max":[10,10,10]}]}
metadata = json.dumps(document,separators=(",", ":")).encode()
metadata += b" " * (-len(metadata)%4)
payload = struct.pack("<II",len(metadata),0x4E4F534A)+metadata+struct.pack("<II",len(positions),0x004E4942)+positions
(root / "cube.glb").write_bytes(struct.pack("<III",0x46546C67,2,len(payload)+12)+payload)

def png_chunk(kind, content):
    return struct.pack(">I", len(content)) + kind + content + struct.pack(">I", zlib.crc32(kind + content))

png = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0))
png += png_chunk(b"IDAT", zlib.compress(b"\x00\xff\xff\xff\xff")) + png_chunk(b"IEND", b"")
uv = struct.pack("<" + "f" * (len(vertices) * 2), *(v for p in vertices for v in (p[0]/10, p[1]/10)))
textured = json.loads(json.dumps(document))
textured["images"] = [{"uri": "data:image/png;base64," + base64.b64encode(png).decode()}]
textured["textures"] = [{"source": 0}]
textured["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"] = {"index": 0}
textured["buffers"][0]["byteLength"] = len(positions) + len(uv)
textured["bufferViews"].append({"buffer": 0, "byteOffset": len(positions), "byteLength": len(uv)})
textured["accessors"].append({"bufferView": 1, "componentType": 5126, "count": len(vertices), "type": "VEC2"})
textured["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = 1
metadata = json.dumps(textured, separators=(",", ":")).encode()
metadata += b" " * (-len(metadata) % 4)
payload = struct.pack("<II", len(metadata), 0x4E4F534A) + metadata + struct.pack("<II", len(positions) + len(uv), 0x004E4942) + positions + uv
(root / "cube-textured.glb").write_bytes(struct.pack("<III", 0x46546C67, 2, len(payload) + 12) + payload)

# These formats are whitespace-delimited. IGES retains its fixed-width records.
for name in ("cube.step", "cube-inch.step", "cube.brep"):
    path = root / name
    path.write_text("\n".join(line.rstrip(" \t") for line in path.read_text().splitlines()) + "\n", encoding="utf-8", newline="\n")
