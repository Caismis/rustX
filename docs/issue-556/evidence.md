# Geometry preview evidence archive

The [pr556-evidence-v1 tag](https://github.com/MiNisterPaX/rustX/tree/pr556-evidence-v1)
pins commit `efef9f0441de1a31752187ed9287217ec53816fb`. Its two evidence trees
retain the raw samples, unsuccessful runs, diagnostic source and patches,
fixture identities, source/build provenance, and original reports. The Git
ref keeps the evidence reachable independently of time-limited CI artifacts.
[evidence.sha256](evidence.sha256) records the bytes of every archived file.
[archives.json](archives.json) also records the compressed and uncompressed
identities of the three streaming STL sample files.

Extract the archive into a separate directory and verify it from its root.
Run these commands from a rustX Git checkout with the current checksum file.

```sh
archive=$(mktemp -d)
checksums="$PWD/docs/issue-556/evidence.sha256"
git fetch https://github.com/MiNisterPaX/rustX.git refs/tags/pr556-evidence-v1
test "$(git rev-parse FETCH_HEAD)" = efef9f0441de1a31752187ed9287217ec53816fb
git archive FETCH_HEAD docs/issue-534 docs/issue-556 | tar -x -C "$archive"
(cd "$archive" && sha256sum -c "$checksums")
```

See the [STL benchmark](README.md), [CAD validation](../issue-534/native-cad-validation.md),
and [parser reproduction entry point](../issue-534/step-parser-experiments/REPRODUCE.md)
for commands and measurement scope. The archived diagnostic commands use the
historical baseline revisions identified in their manifests.

Use a squash merge for #556 to keep the archived raw evidence out of main's
reachable history. The archive ref preserves the original research history.
