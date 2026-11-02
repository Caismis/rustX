# Geometry fixtures

These files and `generate.py` are original rustX test data, licensed under the
repository's MIT license. No model was copied from a third-party model library.

The reference solid is an axis-aligned cube with minimum `(0, 0, 0)` and maximum
`(10, 10, 10)`, twelve outward-facing triangles, area `600` and volume `1000`.
`cube.glb`, `cube.stl` and `cube-ascii.stl` are written directly from that analytic
triangle list. GLB adds a named hierarchy and a blue material.
`cube-textured.glb` additionally embeds an original 1x1 white PNG and UV
coordinates to exercise image admission and disposal. GLB coordinates
are meters according to glTF; STL coordinates have no intrinsic physical unit.

STEP, inch-encoded STEP, IGES and BREP are exported independently with
cadquery-ocp-novtk 7.9.3.1.1 / OCCT 7.9.3, rather than with the browser importer.
The solid is a 10 mm cube in STEP/IGES. The inch variant encodes the same size in
inches, and must import to 10 mm. BREP preserves the ten native coordinate units
and supplies no reliable unit declaration. Browser tessellation must agree with
analytic bounds within `1e-5` coordinate units; triangle order is not prescribed.

Regenerate on a supported Python 3.11–3.14 host:

```sh
uv run --no-project --with cadquery-ocp-novtk==7.9.3.1.1 python web-console/test/fixtures/geometry/generate.py
```

Python and the native exporter are test-authoring tools only. Runtime previews
use the locally packaged browser assets. STEP exporter header timestamps can
differ on regeneration; tests assert geometry and unit semantics.
