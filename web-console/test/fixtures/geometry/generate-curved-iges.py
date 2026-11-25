"""MIT rustX curved IGES workload from the original three-sphere BREP fixture.

uv run --no-project --with cadquery-ocp-novtk==7.9.3.1.1 python \
  test/fixtures/geometry/generate-curved-iges.py --output /tmp/curved.iges

The three analytic spheres preserve the BREP fixture's complete geometry and
millimeter placement. IGES has its own grouping/name/color capabilities; final
comparisons are between runtime variants importing this identical source.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path
from OCP.BRep import BRep_Builder
from OCP.BRepTools import BRepTools
from OCP.IGESControl import IGESControl_Writer, IGESControl_Controller
from OCP.Interface import Interface_Static
from OCP.TopoDS import TopoDS_Shape

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
shape = TopoDS_Shape()
assert BRepTools.Read_s(shape, str(Path(__file__).with_name('lod-assembly.brep')), BRep_Builder())
IGESControl_Controller.Init_s()
assert Interface_Static.SetCVal_s('write.iges.header.author', 'rustX')
assert Interface_Static.SetCVal_s('write.iges.header.company', 'rustX')
writer = IGESControl_Writer('MM', 0)
assert writer.AddShape(shape)
writer.ComputeModel()
assert writer.Write(str(args.output))
raw, dates = re.subn(rb'\d{8}\.\d{6}', b'20261117.000000', args.output.read_bytes())
assert dates == 2
args.output.write_bytes(raw)
print(json.dumps({'kind':'three complete analytic spheres exported from lod-assembly.brep','format':'iges','bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}))
