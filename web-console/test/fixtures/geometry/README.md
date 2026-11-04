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
`cube-embedded.glb` stores the same PNG in a BIN buffer view, exercising generated
image-URL cleanup when native decoding fails.

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

## Product sidebar assembly

`mounting-bracket.step` and `mounting-bracket.glb` are original, named and colored
12-part assemblies: a drilled base plate, an upright with a circular opening,
two gussets, four washers and four bolts. Their Y-up bounds are
`120 × 72 × 80 mm`; the GLB stores the same coordinates in meters. The STEP uses
XCAF assembly labels and colors. The GLB uses independently exported OCCT
triangles and PBR materials. Both exercise the actual production sidebar with
the native App Server, committed delivery, Workbench, part hierarchy and
mixed Markdown/model tabs.

`bracket-array.step` repeats the same assembly eight times as 96 parts, with
analytic bounds `570 × 72 × 190 mm`. It exercises repeated imports, camera input,
picking and the shared two-body resource budget under the unchanged CAD settings.

The interaction-roadmap tests derive a transformed, repeated two-part GLB from
the analytic cube without changing its vertex/material data. The curved-outline
case writes an original unit-sphere STL from Three.js 0.187.0 SphereGeometry
(48 longitudinal / 32 latitudinal subdivisions), including outward facet normals.
These procedural derivatives remain MIT test data. They exercise occurrence
coordinates, curved silhouettes, shared resources and disposal; they are not CAD
topology or engineering-accuracy references.

```sh
uv run --no-project --with cadquery-ocp-novtk==7.9.3.1.1 python web-console/test/fixtures/geometry/generate-showcase.py
```
