#!/usr/bin/env python3
"""Run fixture projections (or a load/fingerprint race) in an isolated MCP process."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import selectors
import subprocess
import time
import threading
import struct


class Client:
    def __init__(self, binary):
        self.process = subprocess.Popen([str(binary), "--in-process"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        env={**os.environ, "RMC_DAEMON": "0", "EXTRACT_TIMING": "1"})
        self.selector = selectors.DefaultSelector()
        self.buffers = {}
        self.log = []
        self.serial = 0
        for stream, name in [(self.process.stdout, "stdout"), (self.process.stderr, "stderr")]:
            self.selector.register(stream, selectors.EVENT_READ, name)
            self.buffers[name] = b""
        self.info = self.request("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                               "clientInfo": {"name": "graph-truth", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, value):
        self.process.stdin.write(json.dumps(value).encode() + b"\n")
        self.process.stdin.flush()

    def request(self, method, params, on_log=None):
        self.serial += 1
        request_id = self.serial
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            for key, _ in self.selector.select(timeout=1):
                name = key.data
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    self.selector.unregister(key.fileobj)
                    continue
                self.buffers[name] += chunk
                while b"\n" in self.buffers[name]:
                    line, self.buffers[name] = self.buffers[name].split(b"\n", 1)
                    if name == "stderr":
                        text = line.decode(errors="replace")
                        self.log.append(text)
                        if on_log:
                            on_log(text)
                    else:
                        message = json.loads(line)
                        if message.get("id") == request_id:
                            return message
            if self.process.poll() is not None:
                raise RuntimeError(f"MCP exited {self.process.returncode}: {self.log[-10:]}")
        raise TimeoutError(f"MCP request {method}")

    def tool(self, name, arguments, on_log=None):
        return self.request("tools/call", {"name": name, "arguments": arguments}, on_log)

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=10)
        self.selector.close()


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("workspace", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--race", action="store_true")
    parser.add_argument("--identity", action="store_true", help="assert v16 target, expansion and reuse contracts")
    parser.add_argument("--race-read", type=int, default=2,
                        help="mutate after this source close-read event (Linux)")
    args = parser.parse_args()
    root = args.workspace.resolve()
    client = Client(args.binary.resolve())
    result = {"initialize": client.info}
    source = root / "app/src/lib.rs"
    original = source.read_text()
    changed = False
    reads = 0
    try:
        if args.race:
            # IN_CLOSE_NOWRITE: edit after a loader has read the old file, while
            # the rest of workspace loading is still in progress. A log line
            # after load is too late on a tiny corpus (hashing takes < 1 ms).
            libc = ctypes.CDLL(None, use_errno=True)
            watcher = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
            if watcher < 0 or libc.inotify_add_watch(watcher, os.fsencode(source), 0x10) < 0:
                raise OSError(ctypes.get_errno(), "inotify watch failed")
            done = threading.Event()
            def edit_after_read():
                nonlocal changed, reads
                while not done.wait(0.001):
                    try:
                        events = os.read(watcher, 65536)
                    except BlockingIOError:
                        continue
                    cursor = 0
                    while cursor < len(events):
                        _, mask, _, length = struct.unpack_from("iIII", events, cursor)
                        cursor += 16 + length
                        if mask & 0x10:
                            reads += 1
                    if reads >= args.race_read:
                        source.write_text(original + "\npub fn race_added() { truth_dep::leaf(); }\n")
                        changed = True
                        return
            thread = threading.Thread(target=edit_after_read, daemon=True)
            thread.start()
            try:
                result["first"] = client.tool("build_hypergraph", {"directory": str(root), "force_rebuild": True})
            finally:
                done.set()
                thread.join()
                os.close(watcher)
            result["edit_triggered"] = changed
            result["source_read_events"] = reads
            result["immediately_after_first"] = client.tool("calls_from", {"directory": str(root), "caller": "truth_app::race_added"})
            result["reuse"] = client.tool("build_hypergraph", {"directory": str(root)})
            result["before_forced_repair"] = client.tool("calls_from", {"directory": str(root), "caller": "truth_app::race_added"})
            result["force"] = client.tool("build_hypergraph", {"directory": str(root), "force_rebuild": True})
            result["after_forced_repair"] = client.tool("calls_from", {"directory": str(root), "caller": "truth_app::race_added"})
        else:
            result["tools"] = client.request("tools/list", {})
            result["build"] = client.tool("build_hypergraph", {"directory": str(root)})
            for caller in ["direct", "helper", "data", "closures"]:
                result[caller] = client.tool("calls_from", {"directory": str(root), "caller": "truth_app::" + caller})
            if args.identity:
                def body(response):
                    assert "error" not in response, response
                    assert not response["result"].get("isError"), response
                    return json.loads(response["result"]["content"][0]["text"])
                result["identity_calls"] = {
                    caller: client.tool("calls_from", {"directory": str(root), "caller": caller})
                    for caller in ["truth_app[lib]::helper", "truth_app[bin]::helper", "truth_app::macros"]}
                lib, binary, macros = [body(response) for response in result["identity_calls"].values()]
                assert lib["total_match_count"] == 1 and lib["call_sites"][0]["callee_qualified_name"] == "truth_dep::leaf"
                assert binary["total_match_count"] == 1 and binary["call_sites"][0]["callee_qualified_name"] == "truth_dep::register"
                assert macros["total_match_count"] == 2
                assert len({json.dumps(site["expansion"], sort_keys=True) for site in macros["call_sites"]}) == 2
                assert "error" in result["helper"] or result["helper"]["result"].get("isError"), result["helper"]
                result["reuse"] = client.tool("build_hypergraph", {"directory": str(root)})
                built, reused = body(result["build"]), body(result["reuse"])
                assert reused["reused"] and reused["graph_id"] == built["graph_id"]
                assert built["usage_counts"]["observations"] == built["usage_count"]
                assert 0 < built["usage_counts"]["expanded_observations"] < built["usage_count"]
                result["identity_assertions"] = 8
        result["stderr"] = client.log
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"output": str(args.output), "race_edit": changed}))
    finally:
        if changed:
            source.write_text(original)
        client.close()


if __name__ == "__main__":
    main()
