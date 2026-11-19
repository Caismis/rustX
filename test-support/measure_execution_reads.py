"""Linux wire probe for EXEC-01. Run with the absolute staged Client binary.

The probe acts as a measuring UDS peer, not the product Agent Server. Each
sample starts a new peer process and Client; fixtures are warm-cache. rchar
counts logical reads, not disk traffic. A no-file Ping calibrates whether
socket request bytes enter this kernel/build's rchar accounting.
"""

import argparse
import json
import os
from pathlib import Path
import secrets
import socket
import struct
import subprocess
import sys
import tempfile
import time


FRAME = 1024 * 1024
WIDTH = 128
MARKER = 'needle:"\\\t' + chr(0x96EA)


def pack(value):
    body = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()
    assert len(body) + 4 <= FRAME
    return struct.pack(">I", len(body)) + body


def receive(sock):
    def exact(length):
        result = bytearray()
        while len(result) < length:
            chunk = sock.recv(length - len(result))
            if not chunk:
                raise EOFError("partial or absent frame")
            result.extend(chunk)
        return result

    size, = struct.unpack(">I", exact(4))
    assert 0 < size <= FRAME - 4
    return json.loads(exact(size)), size + 4


def counter(pid, filename, name):
    fields = dict(line.split(":", 1) for line in
                  Path(f"/proc/{pid}/{filename}").read_text().splitlines()
                  if ":" in line)
    return int(fields[name].split()[0])


def sample(client, size, operation):
    with tempfile.TemporaryDirectory() as temporary, socket.socket(socket.AF_UNIX) as listener:
        private = Path(temporary).resolve()
        workspace = private / "workspace"
        workspace.mkdir()
        rows, rem = divmod(size, WIDTH)
        assert not rem and rows % 512 == 0
        with (workspace / "input").open("wb") as out:
            block = (b"x" * (WIDTH - 1) + b"\n") * 512
            for _ in range(rows // 512):
                out.write(block)
            out.write(MARKER.encode())
        source_size = size + len(MARKER.encode())
        address = str(private / "s")
        listener.bind(address)
        listener.listen(1)
        listener.settimeout(10)
        identity = dict(version=1, session="probe", environment="probe-env",
                        instance="probe-client", generation="1", workspace=str(workspace))
        secret = secrets.token_hex(32)
        bundle = private / "b"
        with os.fdopen(os.open(bundle, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as f:
            f.write(pack(dict(identity, socket_path=address, credential=secret)))
        proc = subprocess.Popen([client, "--binding-file", bundle, "--workspace", workspace],
                                cwd="/", env={}, stdin=subprocess.DEVNULL)
        try:
            with listener.accept()[0] as conn:
                conn.settimeout(65)
                hello, _ = receive(conn)
                assert all(hello[k] == v for k, v in identity.items())
                assert secrets.compare_digest(hello["credential"], secret)
                conn.sendall(pack({"Ok": identity}))
                conn.sendall(pack("ping"))
                assert receive(conn)[0] == "pong"
                baseline = counter(proc.pid, "io", "rchar")
                ping = pack("ping")
                conn.sendall(ping)
                assert receive(conn)[0] == "pong"
                socket_reads = counter(proc.pid, "io", "rchar") - baseline
                assert socket_reads in (0, len(ping)), socket_reads
                if operation == "lines":
                    request = {"read-lines": dict(relative_path="input", offset=str(rows + 1),
                                                  limit="1", expected_revision=None)}
                else:
                    request = {"search": dict(relative_root="input", spec={"grep": dict(
                        pattern="needle:", glob=None, literal=True, ignore_case=False,
                        context=0, limit="1")})}
                frame = pack(request)
                before = counter(proc.pid, "io", "rchar")
                start = time.perf_counter_ns()
                conn.sendall(frame)
                reply, reply_size = receive(conn)
                elapsed = time.perf_counter_ns() - start
                read_bytes = counter(proc.pid, "io", "rchar") - before
                if socket_reads:
                    read_bytes -= len(frame)
                if operation == "lines":
                    page = reply["lines"]
                    assert page["metadata"]["size"] == str(source_size)
                    assert page["total_lines"] == str(rows + 1) and page["end"] == "complete"
                    assert page["shown_lines"] == 1 and page["text"] == MARKER
                    selection = page["text"]
                else:
                    page = reply["search-page"]
                    assert page["matches"] == "1" and page["body"] == f"input:{rows+1}: {MARKER}"
                    assert not any(page[k] for k in ("short_lines", "hit_limit", "byte_limit"))
                    selection = page["body"]
                # rchar includes auxiliary descriptor/metadata reads, not just
                # bytes from input. Keep the observed value distinct from the
                # two known full passes (UTF-8 validation, then matching).
                scan_bytes = source_size * (1 if operation == "lines" else 2)
                assert read_bytes >= scan_bytes, (read_bytes, scan_bytes)
                result = dict(operation=operation, source_bytes=source_size, rpc=1,
                              selected_bytes=len(selection.encode()), tx=len(frame), rx=reply_size,
                              client_rchar_ex_socket=read_bytes, scan_bytes_by_algorithm=scan_bytes,
                              elapsed_ns=elapsed,
                              client_hwm_kib=counter(proc.pid, "status", "VmHWM"),
                              probe_hwm_kib=counter("self", "status", "VmHWM"))
            if proc.wait(timeout=10):
                raise RuntimeError("Client did not exit successfully")
            return result
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("client", type=Path)
    parser.add_argument("--one", choices=("lines", "grep"))
    parser.add_argument("--size", type=int)
    args = parser.parse_args()
    if args.one:
        print(json.dumps(sample(args.client.resolve(strict=True), args.size, args.one)))
    else:
        for mib in (1, 8, 64):
            for op in ("lines", "grep"):
                for _ in range(3):
                    subprocess.run([sys.executable, __file__, args.client, "--one", op,
                                    "--size", str(mib * 1024 * 1024)], check=True)
