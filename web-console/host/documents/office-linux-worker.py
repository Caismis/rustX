"""One private UNO Desktop, reused by every document in this converter slot."""
import json
import os
import re
import shutil
import subprocess
import sys
import time
import uno
from com.sun.star.beans import PropertyValue


def property_value(name, value):
    prop = PropertyValue()
    prop.Name = name
    prop.Value = value
    return prop


def send(value):
    sys.stdout.buffer.write(json.dumps(value, separators=(',', ':')).encode() + b'\n')
    sys.stdout.buffer.flush()


os.makedirs('/tmp/home', exist_ok=True)
os.makedirs('/tmp/profile/user', exist_ok=True)
shutil.copyfile('/input/registrymodifications.xcu', '/tmp/profile/user/registrymodifications.xcu')
engine = subprocess.Popen([
    '/usr/bin/libreoffice', '-env:UserInstallation=file:///tmp/profile', '--headless', '--nologo',
    '--nodefault', '--norestore', '--nofirststartwizard', '--nolockcheck',
    '--accept=pipe,name=rustx-office;urp;StarOffice.ComponentContext',
], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    local = uno.getComponentContext()
    resolver = local.ServiceManager.createInstanceWithContext('com.sun.star.bridge.UnoUrlResolver', local)
    until = time.monotonic() + 60
    while True:
        try:
            remote = resolver.resolve('uno:pipe,name=rustx-office;urp;StarOffice.ComponentContext')
            break
        except Exception:
            if engine.poll() is not None or time.monotonic() >= until:
                raise
            time.sleep(.01)
    desktop = remote.ServiceManager.createInstanceWithContext('com.sun.star.frame.Desktop', remote)
    send({'ready': True, 'engine_pid': engine.pid})
    while True:
        line = sys.stdin.buffer.readline(4097)
        if not line:
            break
        if len(line) > 4096 or not line.endswith(b'\n'):
            raise RuntimeError('invalid request')
        job = json.loads(line)
        identity, extension = job['id'], job['extension']
        if not re.fullmatch(r'[a-f0-9-]{36}', identity) or extension not in ('docx', 'pptx'):
            raise RuntimeError('invalid request')
        output = '/tmp/' + identity + '.pdf'
        document = None
        try:
            document = desktop.loadComponentFromURL(
                uno.systemPathToFileUrl('/input/' + identity + '.' + extension), '_blank', 0,
                tuple(property_value(k, v) for k, v in (
                    ('Hidden', True), ('ReadOnly', True), ('MacroExecutionMode', 0), ('UpdateDocMode', 0),
                )))
            if document is None:
                raise RuntimeError('load failed')
            document.storeToURL(uno.systemPathToFileUrl(output), (
                property_value('FilterName', 'writer_pdf_Export' if extension == 'docx' else 'impress_pdf_Export'),
                property_value('FilterData', uno.Any('[]com.sun.star.beans.PropertyValue', (
                    property_value('ReduceImageResolution', True), property_value('MaxImageResolution', 192),
                ))),
            ))
            document.close(True)
            document = None
            size = os.path.getsize(output)
            if size < 5 or size > 100 * 1024 * 1024:
                raise RuntimeError('invalid output')
            send({'id': identity, 'length': size})
            with open(output, 'rb') as file:
                if file.read(5) != b'%PDF-':
                    raise RuntimeError('invalid PDF')
                file.seek(0)
                shutil.copyfileobj(file, sys.stdout.buffer, 65536)
            sys.stdout.buffer.flush()
        finally:
            if document is not None:
                document.close(True)
            if os.path.exists(output):
                os.unlink(output)
        # Publication waits for this trailer: closing the document and removing
        # its private output must finish before the next slot can be admitted.
        send({'id': identity, 'settled': True, 'engine_pid': engine.pid})
finally:
    engine.kill()
    engine.wait()
