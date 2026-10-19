#!/usr/bin/env python3
"""Copy selected raw evidence into a verified compact archive; keep originals."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import tarfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--root',type=Path,required=True)
p.add_argument('--output',type=Path,required=True)
p.add_argument('patterns',nargs='+')
a = p.parse_args()
paths = sorted({path for pattern in a.patterns for path in a.root.glob(pattern) if path.is_file()})
assert paths, 'no evidence selected'
manifest = {str(path.relative_to(a.root)):{'bytes':path.stat().st_size,'sha256':hashlib.file_digest(path.open('rb'),'sha256').hexdigest()} for path in paths}
encoded = json.dumps(manifest,indent=2).encode()
with tarfile.open(a.output,'w:xz',preset=4) as archive:
    for path in paths:
        archive.add(path,arcname=str(path.relative_to(a.root)),recursive=False)
    info = tarfile.TarInfo('MANIFEST.json');info.size=len(encoded)
    archive.addfile(info,io.BytesIO(encoded))
with tarfile.open(a.output,'r:xz') as archive:
    for name,entry in manifest.items():
        data = archive.extractfile(name).read()
        assert len(data)==entry['bytes'] and hashlib.sha256(data).hexdigest()==entry['sha256'],name
a.output.with_suffix('.manifest.json').write_bytes(encoded)
print(json.dumps({'files':len(paths),'archive_bytes':a.output.stat().st_size,'verified':True}))
