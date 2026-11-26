# Linux Workbench native STEP previews

The Product Host can convert an explicitly opened Workbench STEP or STP file
to one full-detail GLB. The default Workbench path uses the [bulk-lexer
OCCT WASM build](step-browser-benchmark.md). Operators can enable the native
path on Linux in `RUSTX_WORKSPACE_HOST_CONFIG`.

```json
{
  "stepPreviewConverter": {
    "executable": "/absolute/path/to/rustx-step-native",
    "threads": 1
  }
}
```

The helper accepts one or eight OCCT threads. Use one thread for an initial
deployment. The Host holds one converter admission, rejects concurrent
conversions with the preview capacity error, and uses the existing two-owner
and two-read bounds. The helper receives a read-only source descriptor and
reads at most 200 MiB. It rejects external STEP references before XCAF
transfer. The helper limits address space to 4 GiB and output to 100 MiB. The
converter deadline is 55 seconds within the existing 60-second preview
operation. Cancellation sends SIGKILL to the converter and waits for child
closure before returning the admission. Temporary assets are scoped to their
preview owner and removed on retirement.

The Host validates the original descriptor, secure path identity, source
version and current authority before exposing the GLB, while transporting
bounded ranges, and at final settlement. The browser uses the same geometry
admission, render and copy lifecycle as other GLBs. OCCT XCAF transfers names,
occurrences and colors. Root meshing uses the browser CAD importer
parameters of relative deflection 0.001 of the root's mean axis span and an
angular deflection of 0.5 radians. The exporter writes meters with the viewer's
Y-up presentation. Session-file delivery uses the existing browser path.

## Build and package

Use OCCT [`d2abb6d844231cb8f29be6894440874a4700e4a5`](https://github.com/Open-Cascade-SAS/OCCT/tree/d2abb6d844231cb8f29be6894440874a4700e4a5)
(7.6.1), the same version used by the browser runtime. Apply
`web-console/vendor/occt-import-js/step-bulk-lexer.patch` to an OCCT source
checkout. Build in Release with RapidJSON headers and the `TKXDESTEP` and
`TKRWMesh` toolkits. CMake can resolve the required dependent toolkits from
`BUILD_ADDITIONAL_TOOLKITS="TKXDESTEP TKRWMesh"` when the `BUILD_MODULE_*`
options are off. Enable `USE_RAPIDJSON` and disable `USE_FREETYPE`, `USE_TBB`
and `USE_OPENGL`. Configure the helper from the rustX checkout.

```sh
cmake -S web-console/host/step-native -B /tmp/rustx-step-helper -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DOCCT_HEADERS=/path/to/occt-build/include/opencascade \
  -DCMAKE_LIBRARY_PATH=/path/to/occt-build/lin64/gcc/lib
cmake --build /tmp/rustx-step-helper
```

Install the executable and its matching shared libraries together on the
Linux Product Host. Supply the library search path in the Host environment
where required. Record the OCCT and rustX commits, patch and executable
SHA-256, build flags, dependencies and OCCT license notices alongside the
operator package. Distribute that package with the Linux Product Host.

## Verify and compare

Set `RUSTX_NATIVE_STEP_EXECUTABLE` to the built executable when running
`test/e2e/native-step.spec.ts` under the existing Playwright configuration.
It opens both STEP and STP through the Workbench and checks the browser's
GLB viewer and units. `test/native-step-preview.test.ts` covers the preview
owner, physical child settlement and transport with a controlled converter.

Use the [STEP browser benchmark](step-browser-benchmark.md) with
`builds.lexer` pointing to the production build and
`native: { "executable": "/absolute/path/to/rustx-step-native" }`. Leave
`builds.baseline` out when comparing only the current WASM and native paths.
The same production UI and input digests are used for WASM, native-1 and
native-8. The harness keeps raw cold and warm samples, first complete and
full-detail GPU fence/frame proxies, range transport and admission timing,
converter phase timings, helper RSS, and failed opens. Repeat with a
constrained CPU affinity and the large reference assembly. The native path
remains opt-in pending throughput, fidelity and resource evidence on the
[#534 matrix](https://github.com/Caismis/rustX/issues/534).

See [#602](https://github.com/Caismis/rustX/issues/602) and
[#556](https://github.com/Caismis/rustX/pull/556).
