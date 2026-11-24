# STEP lexer browser benchmark

Measure a STEP file opened in the production Workbench with the real Product
Host and App Server. Build both variants from the same Web sources. Change
only the reviewed OCCT lexer WASM asset between variants. Build the baseline
with the parent revision's WASM and the candidate with the lexer WASM, then
verify each emitted asset's digest before measuring.

Set `RUSTX_BINARY` to an App Server built from the matching sources. Install
the Web, TUI and fake provider dependencies used by the existing browser
tests. With both production builds available, save a local JSON manifest like
the one below.

```json
{
  "builds": {
    "baseline": "/absolute/path/to/baseline-dist",
    "lexer": "/absolute/path/to/lexer-dist"
  },
  "fixtures": [
    { "id": "industrial-part", "path": "/absolute/path/to/part.step" },
    { "id": "industrial-assembly", "path": "/absolute/path/to/assembly.step" },
    { "id": "pressure-control", "path": "/absolute/path/to/control.step", "control": true }
  ],
  "output": "/absolute/path/to/browser-samples.json",
  "repetitions": 3,
  "diagnosticRepetitions": 1
}
```

Run the benchmark from `web-console`.

```sh
RUSTX_STEP_BROWSER_BENCHMARK=/absolute/path/to/manifest.json \
  pnpm exec playwright test --config playwright.step-browser-benchmark.config.ts
```

Set `executablePath` to use a locally installed Chrome. Set `softwareWebGL`
to `true` for SwiftShader or `false` to require hardware WebGL. On Linux the
hardware option selects ANGLE OpenGL. Report the actual renderer with the
results. Keep the machine free of benchmark-owned build and test activity
during the timed comparison.

- A cold sample is the first STEP open in a fresh browser process. Warm uses
  the same page after closing and releasing the cold scene. A second copy of
  the exact same input under a new file name forces another authorized read
  and OCCT instance, while retaining the compiled WASM module. Both samples
  use the operating system's normal filesystem cache.
- The clock starts at the trusted click. File transport covers the
  browser-observed Host open to final close response. WASM fetch and module
  compilation can overlap the file transport. Each worker separately reports
  the time to instantiate the module and initialize the importer.
- Primary rows leave native profiling off. Diagnostic repetitions enable
  existing bounded native phase spans for parse, document transfer and
  healing, coarse and final tessellation. Inclusive spans may overlap. Use
  `repetitions: 0` for a diagnostic-only pass.
- Upload observations record submitted WebGL buffer bytes and time spent in
  upload API calls. The fence after a whole-model draw includes queued GPU
  work and drawing. The subsequent animation frame is a presentation proxy.
  These timestamps cannot isolate device transfer time or physical scan-out.
- An in-product failure is recorded with the completed timing boundaries.
  Keep the production deadline and distinguish incomplete runs from measured
  first-display latencies. The optional pressure control runs once per mode
  and variant in each pass. Only use local fixture copies and keep them out of
  the repository.
