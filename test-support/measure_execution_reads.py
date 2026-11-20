"""Linux wire probe for EXEC-01 (paired-lane revision). Run with the absolute
staged Client binary.

The probe acts as a measuring UDS peer, not the product Agent Server. Each
sample starts a new peer process and Client; fixtures are warm-cache. rchar
counts process-wide logical reads, not disk traffic or source-only reads.

Pairing revision (`"probe": "paired-v2"` in results): the Client handshake is
now two independently authenticated lanes — the data `Hello` is answered with
a fresh 256-bit pairing challenge that a second connection's `ControlHello`
must echo — and the Client paces a continuous `ControlPoll`/`ControlTurn`
loop on that lane for the whole session. Every result produced before this
revision is HISTORICAL: it predates the control lane entirely, so its
`client_rchar_ex_socket` excluded traffic that simply did not exist yet and
is not comparable with paired-v2 numbers. The measuring peer cannot align
socket sends with reads in a different process. Report observed rchar and
the scanner's expected source byte count separately, without subtracting
sent bytes from the former.
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
import uuid
import threading
import time


FRAME = 1024 * 1024
WIDTH = 128
MARKER = 'needle:"\\\t' + chr(0x96EA)
PROBE_REVISION = "paired-v2"


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
                turn = {
                    "sequence": poll["sequence"],
                    "health": poll["health"],
                    "request": None,
                }
                frame = pack(turn)
                self.conn.sendall(frame)
                with self.lock:
                    self.bytes_sent += len(frame)
        except (EOFError, OSError, socket.timeout):
            return

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
        listener.listen(2)
        listener.settimeout(10)
        identity = dict(version=1, session="probe", environment="probe-env",
                        instance="probe-client", generation="1", workspace=str(workspace))
        # server_incarnation is Server-assigned: it exists in the bootstrap
        # bundle and admission replies but never in a Client Hello.
        server_identity = dict(identity, server_incarnation=str(uuid.uuid4()))
        secret = secrets.token_hex(32)
        bundle = private / "b"
        with os.fdopen(os.open(bundle, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as f:
            f.write(pack(dict(server_identity, endpoint={
                "transport": "unix", "socket_path": address}, credential=secret)))
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
                        if operation == "lines":
                            request = {"read-lines": dict(relative_path="input", offset=str(rows + 1),
                                                          limit="1", expected_revision=None)}
                        else:
                            request = {"search": dict(relative_root="input", spec={"grep": dict(
                                pattern="needle:", glob=None, literal=True, ignore_case=False,
                                context=0, limit="1")})}
                        frame = pack(request)
                        before = counter(proc.pid, "io", "rchar")
                        control_before = responder.sent()
                        start = time.perf_counter_ns()
                        conn.sendall(frame)
                        reply, reply_size = receive(conn)
                        elapsed = time.perf_counter_ns() - start
                        read_bytes = counter(proc.pid, "io", "rchar") - before
                        control_tx = responder.sent() - control_before
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
                                      probe=PROBE_REVISION,
                                      selected_bytes=len(selection.encode()), tx=len(frame),
                                      rx=reply_size,
                                      client_process_rchar=read_bytes, control_tx_sample=control_tx,
                                      scan_bytes_by_algorithm=scan_bytes,
                                      elapsed_ns=elapsed,
                                      client_hwm_kib=counter(proc.pid, "status", "VmHWM"),
                                      probe_hwm_kib=counter("self", "status", "VmHWM"))
                    finally:
                        # Orderly end: the data lane's EOF is the Client's
                        # clean serve exit — it must close FIRST, because a
                        # dead control lane fences the data lane (and the
                        # exit status) via the worker's teardown guard.
                        conn.close()
                        responder.close()
                        responder.join(10)
                finally:
                    control.close()
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
