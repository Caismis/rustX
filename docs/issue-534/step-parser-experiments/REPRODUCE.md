# Reproducing the diagnostic runs

Use the [evidence archive extraction and checksum verification](../../issue-556/evidence.md)
to recover the complete diagnostic directory. Follow the
[archived reproduction instructions](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-534/step-parser-experiments/REPRODUCE.md)
from that directory with its bundled manifests, patches, helpers, fixtures,
and scripts. The instructions identify the historical O3 baseline and pin
Emscripten 3.1.69 and the importer and OCCT source commits.

For the production bulk lexer package, see the [browser benchmark](../step-browser-benchmark.md)
and the [source rebuild recipe](../../../web-console/vendor/occt-import-js/rebuild.sh).
