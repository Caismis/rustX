"""Linux read, search and write wire probe. Run with the absolute
staged Client binary.

The probe acts as a measuring UDS peer. Its footprint is separate from the
product Agent Server measurements. Each
sample starts a new peer process and Client; fixtures are warm-cache. rchar
counts process-wide logical reads, not disk traffic or source-only reads.

The current results carry `"probe": "paired-v8"`. The data `Hello` receives
a fresh 256-bit pairing challenge, and the independent `ControlHello` must
echo it. The Client paces `ControlPoll` and `ControlTurn` for the whole
session. Older single-lane measurements exclude the control traffic.
The measuring peer cannot align socket sends with reads in another
process. Report observed rchar and the expected source bytes separately.
"""

import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import socket
import struct
import subprocess
import sys
import tempfile
import uuid
import threading
import time


FRAME = 1024 * 1024
WIDTH = 128
MARKER = 'needle:"\\\t' + chr(0x96EA)
PROBE_REVISION = "paired-v8"
WIRE_VERSION = 8
WRITE_CHUNK_BYTES = 64 * 1024


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


class PollResponder:
    """The Server side of the paired control lane, on its own thread.

    Every `ControlPoll` is answered with the `ControlTurn` echoing its
    sequence and fresh health nonce and no request — exactly what an idle
    paired lane exchanges. The thread counts the turn bytes it sends as
    wire traffic, not as Client-side reads. Socket
    deadlines bound every frame; `close()` shuts the lane and `join()`
    reaps the thread, both bounded.
    """

    def __init__(self, conn):
        self.conn = conn
        self.lock = threading.Lock()
        self.bytes_sent = 0
        self.revoking = threading.Event()
        self.revoked = threading.Event()
        self.thread = threading.Thread(target=self._serve, daemon=True)

    def start(self):
        self.thread.start()

    def sent(self):
        with self.lock:
            return self.bytes_sent

    def _serve(self):
        try:
            while True:
                poll, _ = receive(self.conn)
                settled = poll["receipt"] == "revoked"
                turn = {
                    "sequence": poll["sequence"],
                    "health": poll["health"],
                    "request": "revoke" if self.revoking.is_set() and not settled else None,
                }
                frame = pack(turn)
                self.conn.sendall(frame)
                with self.lock:
                    self.bytes_sent += len(frame)
                if settled:
                    self.revoked.set()
                    return
        except (EOFError, OSError, socket.timeout):
            return

    def revoke(self):
        self.revoking.set()
        assert self.revoked.wait(5), "Client did not acknowledge physical revocation"

    def close(self):
        try:
            self.conn.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        try:
            self.conn.close()
        except OSError:
            pass

    def join(self, timeout):
        self.thread.join(timeout)
        assert not self.thread.is_alive(), "control responder did not exit"


def write_sample(conn, workspace, size):
    assert 0 < size <= 2 * 1024 * 1024
    request_bytes = reply_bytes = requests = 0

    def exchange(operation, **content):
        nonlocal request_bytes, reply_bytes, requests
        frame = pack({operation: content})
        conn.sendall(frame)
        reply, received = receive(conn)
        requests += 1
        request_bytes += len(frame)
        reply_bytes += received
        return reply

    name = "output"
    chunk = b"w" * WRITE_CHUNK_BYTES
    encoded = base64.b64encode(chunk).decode()
    ticket = exchange("begin-write", relative_path=name, expectation="absent",
                      total=str(size))["write-prepared"]["ticket"]
    for offset in range(0, size, len(chunk)):
        next_offset = min(offset + len(chunk), size)
        data = encoded if next_offset - offset == len(chunk) else base64.b64encode(
            chunk[:next_offset - offset]).decode()
        reply = exchange("write-chunk", ticket=ticket, offset=str(offset), data_base64=data)
        assert reply == {"write-chunk-received": {"ticket": ticket, "offset": str(offset)}}
    assert not (workspace / name).exists()
    reply = exchange("commit-write", ticket=ticket)["written"]
    assert reply["ticket"] == ticket and reply["metadata"]["size"] == str(size)
    return request_bytes, reply_bytes, requests


def sample(client, size, operation):
    with tempfile.TemporaryDirectory() as temporary, socket.socket(socket.AF_UNIX) as listener:
        private = Path(temporary).resolve()
        workspace = private / "workspace"
        workspace.mkdir()
        rows, rem = divmod(size, WIDTH)
        source_size = size + len(MARKER.encode())
        if operation != "write":
            assert not rem and rows % 512 == 0
            with (workspace / "input").open("wb") as out:
                block = (b"x" * (WIDTH - 1) + b"\n") * 512
                for _ in range(rows // 512):
                    out.write(block)
                out.write(MARKER.encode())
        address = str(private / "s")
        listener.bind(address)
        listener.listen(2)
        listener.settimeout(10)
        identity = dict(version=WIRE_VERSION, session="probe", environment="probe-env",
                        instance="probe-client", generation="1", workspace=str(workspace))
        # server_incarnation is Server-assigned: it exists in the bootstrap
        # bundle and admission replies but never in a Client Hello.
        server_identity = dict(identity, server_incarnation=str(uuid.uuid4()))
        secret = secrets.token_hex(32)
        bundle = private / "b"
        with os.fdopen(os.open(bundle, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as f:
            f.write(pack(dict(server_identity, endpoint={
                "transport": "unix", "socket_path": address}, credential=secret)))
        started = time.perf_counter_ns()
        proc = subprocess.Popen([client, "--binding-file", bundle, "--workspace", workspace],
                                cwd="/", env={}, stdin=subprocess.DEVNULL)
        try:
            with listener.accept()[0] as conn:
                conn.settimeout(65)
                # The data lane: a typed `data` Hello answered with the
                # half-admission plus a fresh 256-bit pairing challenge.
                hello, _ = receive(conn)
                assert set(hello.keys()) == {"data"}, hello.keys()
                data_hello = hello["data"]
                assert all(data_hello[k] == v for k, v in identity.items())
                assert secrets.compare_digest(data_hello["credential"], secret)
                challenge = secrets.token_hex(32)
                conn.sendall(pack({"Ok": {"admission": server_identity, "challenge": challenge}}))

                # The control lane: a second connection whose `control`
                # Hello must echo the exact challenge — no nonce, no pair.
                control = listener.accept()[0]
                control.settimeout(65)
                try:
                    control_hello, _ = receive(control)
                    assert set(control_hello.keys()) == {"control"}, control_hello.keys()
                    pairing = control_hello["control"]
                    assert all(pairing[k] == v for k, v in identity.items())
                    assert secrets.compare_digest(pairing["credential"], secret)
                    assert secrets.compare_digest(pairing["connection_nonce"], challenge)
                    control.sendall(pack({"Ok": server_identity}))
                    responder = PollResponder(control)
                    responder.start()
                    try:
                        conn.sendall(pack("ping"))
                        assert receive(conn)[0] == "pong"
                        startup_ns = time.perf_counter_ns() - started
                        if operation == "lines":
                            request = {"read-lines": dict(relative_path="input", offset=str(rows + 1),
                                                          limit="1", expected_revision=None)}
                        elif operation == "grep":
                            request = {"search": dict(relative_root="input", spec={"grep": dict(
                                pattern="needle:", glob=None, literal=True, ignore_case=False,
                                context=0, limit="1")})}
                        before = counter(proc.pid, "io", "rchar")
                        writes_before = counter(proc.pid, "io", "wchar")
                        control_before = responder.sent()
                        start = time.perf_counter_ns()
                        if operation == "write":
                            tx, rx, rpc = write_sample(conn, workspace, size)
                            selected_bytes = size
                            scan_bytes = 0
                        else:
                            frame = pack(request)
                            conn.sendall(frame)
                            reply, rx = receive(conn)
                            tx, rpc = len(frame), 1
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
                            selected_bytes = len(selection.encode())
                            scan_bytes = source_size * (1 if operation == "lines" else 2)
                        elapsed = time.perf_counter_ns() - start
                        read_bytes = counter(proc.pid, "io", "rchar") - before
                        written_bytes = counter(proc.pid, "io", "wchar") - writes_before
                        control_tx = responder.sent() - control_before
                        assert read_bytes >= scan_bytes, (read_bytes, scan_bytes)
                        result = dict(operation=operation,
                                      source_bytes=size if operation == "write" else source_size,
                                      rpc=rpc, probe=PROBE_REVISION,
                                      selected_bytes=selected_bytes, tx=tx, rx=rx,
                                      client_process_rchar=read_bytes, client_process_wchar=written_bytes,
                                      control_tx_sample=control_tx,
                                      scan_bytes_by_algorithm=scan_bytes,
                                      startup_ns=startup_ns, elapsed_ns=elapsed,
                                      client_hwm_kib=counter(proc.pid, "status", "VmHWM"),
                                      probe_hwm_kib=counter("self", "status", "VmHWM"))
                        if operation == "write":
                            with (workspace / "output").open("rb") as output:
                                for offset in range(0, size, WRITE_CHUNK_BYTES):
                                    length = min(size - offset, WRITE_CHUNK_BYTES)
                                    assert output.read(length) == b"w" * length
                                assert output.read(1) == b""
                        shutdown = time.perf_counter_ns()
                        responder.revoke()
                    finally:
                        # Close the data lane after the explicit physical receipt.
                        conn.close()
                        responder.close()
                        responder.join(10)
                finally:
                    control.close()
            if proc.wait(timeout=10):
                raise RuntimeError("Client did not exit successfully")
            result["shutdown_ns"] = time.perf_counter_ns() - shutdown
            return result
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("client", type=Path)
    parser.add_argument("--one", choices=("lines", "grep", "write"))
    parser.add_argument("--size", type=int)
    args = parser.parse_args()
    if args.one:
        if args.size is None:
            parser.error("--one requires --size")
        print(json.dumps(sample(args.client.resolve(strict=True), args.size, args.one)))
    else:
        for mib in (1, 8, 64):
            for op in ("lines", "grep"):
                for _ in range(3):
                    subprocess.run([sys.executable, __file__, args.client, "--one", op,
                                    "--size", str(mib * 1024 * 1024)], check=True)
