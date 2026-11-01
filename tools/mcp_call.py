#!/usr/bin/env python3
"""Call a tool on a stdio MCP server directly: initialize → initialized → tools/call.

WHY. To check the server outside a client: run indexing under a specific
profile, take a health_check, run a search query, without restarting the
Claude session and without relying on whichever binary it started at launch.

    RMC_EMBEDDING_PROFILE=local-gpu-bge \
      python3 tools/mcp_call.py ./rust-code-mcp index_codebase \
        '{"directory": "/path/to/repo", "force_reindex": true}' idx.err

Positional arguments: binary, tool name, arguments JSON, and an OPTIONAL file
for the server's stderr (default /dev/null). The response is printed as text.

⚠ Waits for EXACTLY one response line, so it is good for one call per run;
keep long tools (indexing a large repo takes tens of minutes) in the background.
"""
import json, subprocess, sys, time

binary, tool = sys.argv[1], sys.argv[2]
args = json.loads(sys.argv[3])

# 🚨 stderr goes TO A FILE, not to a PIPE nobody reads: the server logs
# to stderr, the 64 KB pipe buffer overflows on the very first large
# indexing, and the server deadlocks on the write. It looks not like
# an error but like indexing just taking long: the process is alive, threads sit in futex_do_wait,
# no I/O moves (caught on a large workspace: 22 minutes of silence after the BM25 phase).
err = open(sys.argv[4] if len(sys.argv) > 4 else '/dev/null', 'w')
p = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=err, text=True, bufsize=1)

def send(obj):
    p.stdin.write(json.dumps(obj) + "\n")
    p.stdin.flush()

send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
      "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                 "clientInfo": {"name": "probe", "version": "0"}}})
print("init:", p.stdout.readline()[:120])
send({"jsonrpc": "2.0", "method": "notifications/initialized"})

t0 = time.time()
send({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
      "params": {"name": tool, "arguments": args}})
line = p.stdout.readline()
print(f"elapsed: {time.time()-t0:.1f}s")
try:
    resp = json.loads(line)
    # 🚨 IN FULL, no slicing. There used to be `[:2000]` here, and the response was truncated
    # SILENTLY, mid-line: the header said 'Found 35 reference(s)', while the list
    # delivered 17, i.e. the probe lied in exactly the way it is meant
    # to catch. Need it shorter? Pipe into head, so it is visible in the command.
    for c in resp.get("result", {}).get("content", []):
        print(c.get("text", ""))
    if "error" in resp:
        print("ERROR:", resp["error"])
except Exception:
    print("raw:", line)
p.terminate()
