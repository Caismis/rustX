"""Original dense B-spline STEP workload; requires cadquery-ocp-novtk 7.9.3.1.1.

Every pole belongs to the exported curved surface. No padding, unused entities,
duplicate comments or repeated whitespace is added to reach the source size.
The large file is generated outside Git; commit the generator and manifest.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import time

from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeFace
from OCP.Geom import Geom_BSplineSurface
from OCP.IFSelect import IFSelect_RetDone
from OCP.Interface import Interface_Static
from OCP.STEPControl import STEPControl_AsIs, STEPControl_Writer
from OCP.TColgp import TColgp_Array2OfPnt
from OCP.TColStd import TColStd_Array1OfInteger, TColStd_Array1OfReal
from OCP.gp import gp_Pnt

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--poles', type=int, default=1450)
args = parser.parse_args()
assert 4 <= args.poles <= 1600
args.output.parent.mkdir(parents=True, exist_ok=True)
start = time.perf_counter()
n = args.poles
poles = TColgp_Array2OfPnt(1, n, 1, n)
for u in range(n):
    x = 1000 * u / (n - 1)
    for v in range(n):
        y = 1000 * v / (n - 1)
        z = 5 * math.sin(2 * math.pi * u / (n - 1)) * math.cos(2 * math.pi * v / (n - 1))
        poles.SetValue(u + 1, v + 1, gp_Pnt(x, y, z))
knots = TColStd_Array1OfReal(1, n - 2)
multiplicities = TColStd_Array1OfInteger(1, n - 2)
for index in range(n - 2):
    knots.SetValue(index + 1, index / (n - 3))
    multiplicities.SetValue(index + 1, 4 if index in (0, n - 3) else 1)
surface = Geom_BSplineSurface(poles, knots, knots, multiplicities, multiplicities, 3, 3)
face = BRepBuilderAPI_MakeFace(surface, 1e-7).Face()
writer = STEPControl_Writer()
assert Interface_Static.SetCVal_s('write.step.unit', 'MM')
assert writer.Transfer(face, STEPControl_AsIs) == IFSelect_RetDone
assert writer.Write(str(args.output)) == IFSelect_RetDone
with args.output.open('r+b') as output:
    header = output.read(4096)
    normalized = re.sub(rb'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d', b'2026-11-05T00:00:00', header)
    assert len(header) == len(normalized)
    output.seek(0)
    output.write(normalized)
digest = hashlib.sha256()
with args.output.open('rb') as source:
    while chunk := source.read(1024 * 1024):
        digest.update(chunk)
record = {'kind': 'one curved degree-3 tensor B-spline face', 'polesPerAxis': n,
          'referencedControlPoles': n * n, 'bytes': args.output.stat().st_size,
          'sha256': digest.hexdigest(), 'units': 'millimeter',
          'controlHull': {'min': [0, 0, -5], 'max': [1000, 1000, 5]},
          'generationSeconds': time.perf_counter() - start}
args.output.with_suffix('.manifest.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record), flush=True)
