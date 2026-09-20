#!/usr/bin/env python3
import argparse
import json
import os
import pathlib
import signal
import socket
import statistics
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

CLK_TCK = os.sysconf("SC_CLK_TCK")
SAMPLE_INTERVAL = 0.02
RUNS = 3
SESSION_COUNT = 150
READ_OPS = 5000
LIST_OPS = 300
IDLE_SECONDS = 4.0


def proc_tree(root_pid):
    seen = set()
    stack = [root_pid]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        proc = pathlib.Path("/proc") / str(pid)
        if not proc.exists():
            continue
        seen.add(pid)
        try:
            children = (proc / "task" / str(pid) / "children").read_text().strip()
            if children:
                stack.extend(int(x) for x in children.split())
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            pass
    return seen


def proc_ticks(pid):
    try:
        raw = (pathlib.Path("/proc") / str(pid) / "stat").read_text()
        tail = raw[raw.rfind(")") + 2 :].split()
        return int(tail[11]) + int(tail[12])
    except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError, IndexError):
        return None


def proc_rss_kib(pid):
    try:
        for line in (pathlib.Path("/proc") / str(pid) / "status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError):
        pass
    return 0


class Sampler:
    def __init__(self, root_pid):
        self.root_pid = root_pid
        self.lock = threading.Lock()
        self.stop_event = threading.Event()
        self.last_ticks = {}
        self.total_ticks = 0
        self.phase_peak_kib = 0
        self.phase_samples_kib = []
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self):
        while not self.stop_event.is_set():
            pids = proc_tree(self.root_pid)
            rss = 0
            current = {}
            for pid in pids:
                rss += proc_rss_kib(pid)
                ticks = proc_ticks(pid)
                if ticks is not None:
                    current[pid] = ticks
            with self.lock:
                for pid, ticks in current.items():
                    previous = self.last_ticks.get(pid)
                    if previous is not None and ticks >= previous:
                        self.total_ticks += ticks - previous
                self.last_ticks.update(current)
                self.phase_peak_kib = max(self.phase_peak_kib, rss)
                self.phase_samples_kib.append(rss)
            time.sleep(SAMPLE_INTERVAL)

    def begin(self):
        time.sleep(SAMPLE_INTERVAL * 2)
        with self.lock:
            self.phase_peak_kib = 0
            self.phase_samples_kib = []
            return self.total_ticks

    def end(self, start_ticks):
        time.sleep(SAMPLE_INTERVAL * 2)
        with self.lock:
            delta = self.total_ticks - start_ticks
            samples = list(self.phase_samples_kib)
            peak = self.phase_peak_kib
        return delta / CLK_TCK, samples, peak

    def stop(self):
        self.stop_event.set()
        self.thread.join(timeout=2)


def phase(sampler, name, fn=None, duration=None, ops=None):
    start_ticks = sampler.begin()
    started = time.perf_counter()
    result = None
    if duration is not None:
        time.sleep(duration)
    elif fn is not None:
        result = fn()
    wall = time.perf_counter() - started
    cpu_s, samples, peak_kib = sampler.end(start_ticks)
    avg_kib = statistics.fmean(samples) if samples else 0.0
    end_kib = samples[-1] if samples else 0
    record = {
        "phase": name,
        "wall_s": wall,
        "cpu_s": cpu_s,
        "cpu_pct_one_core": (cpu_s / wall * 100.0) if wall else 0.0,
        "peak_rss_mib": peak_kib / 1024.0,
        "avg_rss_mib": avg_kib / 1024.0,
        "end_rss_mib": end_kib / 1024.0,
    }
    if ops is not None:
        record["ops"] = ops
        record["ops_s"] = ops / wall if wall else 0.0
        record["cpu_ms_per_op"] = cpu_s * 1000.0 / ops if ops else 0.0
    return record, result


def reserve_port():
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


class RustX:
    def __init__(self, binary, root):
        self.binary = binary
        self.root = pathlib.Path(root)
        self.home = self.root / "home"
        self.workspace = self.root / "workspace"
        self.runtime = self.root / "runtime"
        self.workspace.mkdir(parents=True)
        subprocess.run(["git", "init", "-q", str(self.workspace)], check=True)
        self.home.mkdir(parents=True)
        self.env = os.environ.copy()
        self.env.update(
            {
                "HOME": str(self.home),
                "RUSTX_BENCH_KEY": "bench-only-unused",
            }
        )
        for key in ("XDG_CONFIG_HOME", "XDG_STATE_HOME"):
            self.env.pop(key, None)
        init = subprocess.run(
            [
                self.binary,
                "init",
                "--template",
                "openai-chat",
                "--provider",
                "local",
                "--model-id",
                "bench",
                "--endpoint",
                "http://127.0.0.1:9/v1",
                "--credential-env",
                "RUSTX_BENCH_KEY",
                "--context-window",
                "128000",
                "--max-output",
                "128",
                "--tool-calls",
                "false",
                "--reasoning",
                "false",
                "--compat",
                'chat_reasoning_replay = "omit"',
            ],
            cwd=self.workspace,
            env=self.env,
            capture_output=True,
            text=True,
            timeout=30,
        )
        if init.returncode != 0:
            raise RuntimeError(f"rustx init failed: {init.stdout}\n{init.stderr}")
        self.stderr = open(self.root / "rustx.stderr", "w", buffering=1)
        self.proc = subprocess.Popen(
            [
                self.binary,
                "app-server",
                "--runtime-root",
                str(self.runtime),
                "--listen",
                "stdio",
            ],
            cwd=self.workspace,
            env=self.env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr,
            text=True,
            bufsize=1,
        )
        self.next_id = 1
        initialized = self.request(
            "initialize",
            {
                "protocol_version": 13,
                "client": {"name": "resource-benchmark", "version": "1"},
                "presentation": {"images": False, "questionnaires": False, "reviews": False},
            },
        )
        if initialized.get("type") != "initialized":
            raise RuntimeError(f"unexpected initialize result: {initialized}")

    def request(self, method, params):
        request_id = self.next_id
        self.next_id += 1
        msg = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(msg, separators=(",", ":")) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            if not line:
                stderr = pathlib.Path(self.root / "rustx.stderr").read_text(errors="replace")
                raise RuntimeError(f"rustx closed while waiting for {method}; stderr={stderr[-4000:]}")
            payload = json.loads(line)
            if payload.get("id") != request_id:
                continue
            if "error" in payload:
                raise RuntimeError(f"rustx {method} error: {payload['error']}")
            return payload["result"]

    def warm(self):
        self.request("server/info", {})
        self.request("session/list", {"offset": 0, "limit": 100})

    def create_session(self):
        result = self.request(
            "session/create",
            {"settings": {"cwd": str(self.workspace)}},
        )
        session = result.get("session", result)
        return session["id"]

    def read_session(self, session_id):
        return self.request("session/read", {"session_id": session_id})

    def list_sessions(self):
        return self.request("session/list", {"offset": 0, "limit": 100})

    def close(self):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
        except BrokenPipeError:
            pass
        self.stderr.close()


class OpenCode:
    def __init__(self, binary, root):
        self.binary = binary
        self.root = pathlib.Path(root)
        self.home = self.root / "home"
        self.workspace = self.root / "workspace"
        self.workspace.mkdir(parents=True)
        subprocess.run(["git", "init", "-q", str(self.workspace)], check=True)
        self.home.mkdir(parents=True)
        self.env = os.environ.copy()
        self.env.update(
            {
                "HOME": str(self.home),
                "XDG_CONFIG_HOME": str(self.root / "xdg-config"),
                "XDG_DATA_HOME": str(self.root / "xdg-data"),
                "XDG_CACHE_HOME": str(self.root / "xdg-cache"),
                "OPENCODE_DISABLE_AUTOUPDATE": "1",
                "OPENCODE_DISABLE_PRUNE": "1",
                "OPENCODE_DISABLE_DEFAULT_PLUGINS": "1",
                "OPENCODE_DISABLE_LSP_DOWNLOAD": "1",
            }
        )
        self.port = reserve_port()
        self.stdout = open(self.root / "opencode.stdout", "w", buffering=1)
        self.stderr = open(self.root / "opencode.stderr", "w", buffering=1)
        self.proc = subprocess.Popen(
            [
                self.binary,
                "serve",
                "--hostname",
                "127.0.0.1",
                "--port",
                str(self.port),
            ],
            cwd=self.workspace,
            env=self.env,
            stdout=self.stdout,
            stderr=self.stderr,
            text=True,
        )
        self.base = f"http://127.0.0.1:{self.port}"
        deadline = time.time() + 30
        last_error = None
        while time.time() < deadline:
            if self.proc.poll() is not None:
                break
            try:
                with urllib.request.urlopen(self.base + "/global/health", timeout=1) as response:
                    if response.status == 200:
                        break
            except Exception as error:
                last_error = error
                time.sleep(0.05)
        else:
            raise RuntimeError(f"opencode health timeout: {last_error}")
        if self.proc.poll() is not None:
            raise RuntimeError(
                "opencode exited: "
                + pathlib.Path(self.root / "opencode.stderr").read_text(errors="replace")[-4000:]
            )
        self.dir_header = urllib.parse.quote(str(self.workspace), safe="")

    def request(self, method, path, payload=None):
        body = None
        headers = {"x-opencode-directory": self.dir_header}
        if payload is not None:
            body = json.dumps(payload, separators=(",", ":")).encode()
            headers["content-type"] = "application/json"
        req = urllib.request.Request(self.base + path, method=method, data=body, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=30) as response:
                raw = response.read()
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            raw = error.read().decode(errors="replace")
            raise RuntimeError(f"opencode {method} {path}: HTTP {error.code}: {raw}") from error

    def warm(self):
        self.request("GET", "/session?limit=100")

    def create_session(self):
        return self.request("POST", "/session", {})["id"]

    def read_session(self, session_id):
        return self.request("GET", "/session/" + urllib.parse.quote(session_id, safe=""))

    def list_sessions(self):
        return self.request("GET", "/session?limit=100")

    def close(self):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        self.stdout.close()
        self.stderr.close()


def one_run(product, run_index, binary):
    with tempfile.TemporaryDirectory(prefix=f"bench-{product}-{run_index}-") as root:
        impl = RustX(binary, root) if product == "rustx" else OpenCode(binary, root)
        sampler = Sampler(impl.proc.pid)
        try:
            impl.warm()
            time.sleep(1.0)
            records = []

            metric, _ = phase(sampler, "idle_empty", duration=IDLE_SECONDS)
            records.append(metric)

            ids = []

            def creates():
                for _ in range(SESSION_COUNT):
                    ids.append(impl.create_session())

            metric, _ = phase(
                sampler,
                "create_sessions",
                fn=creates,
                ops=SESSION_COUNT,
            )
            records.append(metric)

            metric, _ = phase(sampler, "idle_after_sessions", duration=IDLE_SECONDS)
            records.append(metric)

            def reads():
                for i in range(READ_OPS):
                    impl.read_session(ids[i % len(ids)])

            metric, _ = phase(sampler, "read_sessions", fn=reads, ops=READ_OPS)
            records.append(metric)

            def lists():
                for _ in range(LIST_OPS):
                    impl.list_sessions()

            metric, _ = phase(sampler, "list_sessions", fn=lists, ops=LIST_OPS)
            records.append(metric)

            for rec in records:
                rec["product"] = product
                rec["run"] = run_index
            return records
        finally:
            sampler.stop()
            impl.close()


def summarize(raw):
    grouped = {}
    for record in raw:
        grouped.setdefault((record["product"], record["phase"]), []).append(record)
    summary = []
    fields = [
        "wall_s",
        "cpu_s",
        "cpu_pct_one_core",
        "peak_rss_mib",
        "avg_rss_mib",
        "end_rss_mib",
        "ops_s",
        "cpu_ms_per_op",
    ]
    for (product, phase_name), records in sorted(grouped.items()):
        item = {"product": product, "phase": phase_name, "runs": len(records)}
        for field in fields:
            values = [float(r[field]) for r in records if field in r]
            if values:
                item[field + "_median"] = statistics.median(values)
                item[field + "_min"] = min(values)
                item[field + "_max"] = max(values)
        summary.append(item)
    return summary


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rustx", required=True)
    parser.add_argument("--opencode", required=True)
    args = parser.parse_args()

    raw = []
    # Alternate products to reduce one-sided runner drift.
    for run_index in range(1, RUNS + 1):
        order = ["rustx", "opencode"] if run_index % 2 else ["opencode", "rustx"]
        for product in order:
            binary = args.rustx if product == "rustx" else args.opencode
            print(f"BENCH_PROGRESS product={product} run={run_index}", flush=True)
            rows = one_run(product, run_index, binary)
            raw.extend(rows)
            for row in rows:
                print(
                    "BENCH_ROW "
                    + " ".join(
                        [
                            f"product={row['product']}",
                            f"run={row['run']}",
                            f"phase={row['phase']}",
                            f"wall_s={row['wall_s']:.6f}",
                            f"cpu_s={row['cpu_s']:.6f}",
                            f"cpu_pct={row['cpu_pct_one_core']:.3f}",
                            f"peak_rss_mib={row['peak_rss_mib']:.3f}",
                            f"avg_rss_mib={row['avg_rss_mib']:.3f}",
                            f"ops_s={row.get('ops_s', 0.0):.3f}",
                            f"cpu_ms_op={row.get('cpu_ms_per_op', 0.0):.6f}",
                        ]
                    ),
                    flush=True,
                )

    summary = summarize(raw)
    print("BENCH_SUMMARY_BEGIN", flush=True)
    for row in summary:
        print(json.dumps(row, sort_keys=True), flush=True)
    print("BENCH_SUMMARY_END", flush=True)
    print(
        "BENCHMARK_JSON="
        + json.dumps(
            {
                "parameters": {
                    "runs": RUNS,
                    "session_count": SESSION_COUNT,
                    "read_ops": READ_OPS,
                    "list_ops": LIST_OPS,
                    "idle_seconds": IDLE_SECONDS,
                    "sample_interval_s": SAMPLE_INTERVAL,
                },
                "summary": summary,
                "raw": raw,
            },
            separators=(",", ":"),
            sort_keys=True,
        ),
        flush=True,
    )


if __name__ == "__main__":
    main()
