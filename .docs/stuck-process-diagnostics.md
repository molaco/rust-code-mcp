# Stuck Process Diagnostics Runbook

This runbook is for rust-code-mcp test or MCP-server processes that may be
wedged in kernel sleep, especially the rust-analyzer graph-test class that can
end in `D` state while exiting. The goal is to collect enough information to
decide what to do without starting more diagnostic commands that also wedge.

The core rule is: once `D` state is suspected, do not run diagnostics that read
the target process command line, environment, or remote memory.

## Safe-ish Reads

For a known PID, prefer only these reads:

```sh
pid=12345
cat "/proc/$pid/status"
cat "/proc/$pid/stat"
cat "/proc/$pid/wchan"
readlink "/proc/$pid/exe"
readlink "/proc/$pid/cwd"
```

Avoid these after `D` state is suspected:

```sh
ps ... cmd
pgrep -af ...
pkill -f ...
tr '\0' ' ' < "/proc/$pid/cmdline"
cat "/proc/$pid/cmdline"
cat "/proc/$pid/environ"
cat "/proc/$pid/mem"
gcore "$pid"
strace -p "$pid"
gdb -p "$pid"
```

`/proc/$pid/cmdline` and related memory-backed files can block in
`__access_remote_vm` when the target is already stuck. That can turn a harmless
diagnostic into a second stuck process.

## Find Candidate PIDs Without Cmdline

Use `/proc/$pid/status` or `/proc/$pid/comm`; both expose the short process
name (`comm`) and do not require reading the target command line.

Find a process by exact or prefix `comm`:

```sh
comm_prefix=rmc_graph

for d in /proc/[0-9]*; do
    pid=${d##*/}
    [ -r "$d/status" ] || continue

    name=$(sed -n 's/^Name:[[:space:]]*//p' "$d/status" 2>/dev/null) || continue
    state=$(sed -n 's/^State:[[:space:]]*//p' "$d/status" 2>/dev/null) || state=unknown

    case "$name" in
        "$comm_prefix"|"$comm_prefix"*)
            printf '%s\t%s\t%s\n' "$pid" "$name" "$state"
            ;;
    esac
done
```

List likely blockers by process state:

```sh
for d in /proc/[0-9]*; do
    pid=${d##*/}
    [ -r "$d/status" ] || continue

    name=$(sed -n 's/^Name:[[:space:]]*//p' "$d/status" 2>/dev/null) || continue
    state=$(sed -n 's/^State:[[:space:]]*//p' "$d/status" 2>/dev/null) || state=unknown

    case "$state" in
        D*|Z*)
            printf '%s\t%s\t%s\n' "$pid" "$name" "$state"
            ;;
    esac
done | sort -n
```

`Name` and `comm` are truncated by the kernel, so a graph test binary such as
`rmc_graph-c878c3b05c79b3a8` may appear as `rmc_graph-c878c`.

## Snapshot A Known PID

Paste this function into the current shell when you already know the PID. It
does not read `cmdline`, `environ`, `/proc/$pid/mem`, or any debugger interface.

```sh
stuck_proc_snapshot() {
    pid=${1:?usage: stuck_proc_snapshot PID}
    d="/proc/$pid"

    if [ ! -d "$d" ]; then
        printf 'no such process: %s\n' "$pid" >&2
        return 1
    fi

    printf '== pid %s status ==\n' "$pid"
    cat "$d/status" 2>&1

    printf '\n== pid %s stat ==\n' "$pid"
    stat=$(cat "$d/stat" 2>&1)
    printf '%s\n' "$stat"

    case "$stat" in
        *") "*)
            rest=${stat##*) }
            set -- $rest
            printf '\n== pid %s stat parsed ==\n' "$pid"
            printf 'state=%s ppid=%s pgrp=%s session=%s tty_nr=%s tpgid=%s\n' \
                "$1" "$2" "$3" "$4" "$5" "$6"
            ;;
    esac

    printf '\n== pid %s wchan ==\n' "$pid"
    cat "$d/wchan" 2>&1

    printf '\n== pid %s exe ==\n' "$pid"
    readlink "$d/exe" 2>&1 || true

    printf '\n== pid %s cwd ==\n' "$pid"
    readlink "$d/cwd" 2>&1 || true
}
```

Usage:

```sh
stuck_proc_snapshot 12345
```

If the target disappears while the function runs, keep the partial output and
do not retry with broader process searches.

## State Decision Tree

Start with the `State:` line from `/proc/$pid/status` and `wchan`.

`S` - interruptible sleep:

- The process is not in the hard stuck class yet.
- Prefer application-level shutdown first, such as closing the MCP client or
  stopping the job that owns the process.
- If termination is required, use an exact PID. Do not use `pkill -f`.
- If a whole test process group must be stopped, derive the process group from
  `/proc/$pid/stat` and inspect it before sending a signal.

`D` - uninterruptible sleep:

- Collect the safe snapshot once.
- Do not read `/proc/$pid/cmdline`, run `pgrep -af`, or run `ps ... cmd`.
- Signals may remain pending until the kernel wait resolves. Repeated
  `SIGTERM` or `SIGKILL` usually adds no value.
- Check `wchan` and kernel logs for NVIDIA, freezer, and hung-task messages.
- Treat the process as a possible suspend or hibernate blocker. If it remains
  in `D`, plan a controlled reboot rather than repeated hibernation attempts.

`Z` - zombie:

- The process has exited and is waiting for its parent to reap it.
- Do not try to kill the zombie; it cannot run signal handlers.
- Use `PPid:` from `/proc/$pid/status` or `ppid` parsed from
  `/proc/$pid/stat` to identify the parent.
- If the parent is yours and is unhealthy, stop or restart the parent by exact
  PID after collecting any needed state.

## Killing By PID Or Process Group

Use this only when the process is still killable, or when an operator has
decided to stop the whole owned process group. Do not use pattern kills.

Exact PID:

```sh
pid=12345
kill -TERM -- "$pid"
sleep 2
kill -KILL -- "$pid"
```

Process group from `/proc/$pid/stat`:

```sh
pid=12345
stat=$(cat "/proc/$pid/stat") || exit 1
rest=${stat##*) }
set -- $rest
pgrp=$3

printf 'pid=%s pgrp=%s\n' "$pid" "$pgrp"

case "$pgrp" in
    ''|0|1)
        printf 'refusing suspicious process group: %s\n' "$pgrp" >&2
        exit 1
        ;;
esac

printf '\n== process group members from /proc/*/stat ==\n'
for d in /proc/[0-9]*; do
    member=${d##*/}
    member_stat=$(cat "$d/stat" 2>/dev/null) || continue
    member_rest=${member_stat##*) }
    set -- $member_rest
    member_state=$1
    member_ppid=$2
    member_pgrp=$3
    [ "$member_pgrp" = "$pgrp" ] || continue
    member_name=$(sed -n 's/^Name:[[:space:]]*//p' "$d/status" 2>/dev/null)
    member_wchan=$(cat "$d/wchan" 2>/dev/null || true)
    printf '%s\tname=%s\tstate=%s\tppid=%s\twchan=%s\n' \
        "$member" "$member_name" "$member_state" "$member_ppid" "$member_wchan"
done | sort -n

printf '\nReview the group member list above before sending a group signal.\n'
printf 'Type the process group number to continue: '
read confirm
[ "$confirm" = "$pgrp" ] || {
    printf 'aborted; confirmation did not match pgrp=%s\n' "$pgrp" >&2
    exit 1
}

kill -TERM -- "-$pgrp"
sleep 2
kill -KILL -- "-$pgrp"
```

If the process is already in `D`, signals may be recorded as pending but not
delivered until the kernel wait completes. That is expected; do not escalate to
`pkill -f`.

## Hibernation And Suspend Blockers

Before hibernating a machine that has had graph-test or CUDA-linked test
failures, check for `D` and `Z` processes without reading command lines:

```sh
for d in /proc/[0-9]*; do
    pid=${d##*/}
    [ -r "$d/status" ] || continue

    name=$(sed -n 's/^Name:[[:space:]]*//p' "$d/status" 2>/dev/null) || continue
    state=$(sed -n 's/^State:[[:space:]]*//p' "$d/status" 2>/dev/null) || state=unknown

    case "$state" in
        D*|Z*)
            wchan=$(cat "$d/wchan" 2>/dev/null || true)
            printf '%s\t%s\t%s\twchan=%s\n' "$pid" "$name" "$state" "$wchan"
            ;;
    esac
done | sort -n
```

Check kernel logs without touching the target process memory:

```sh
journalctl -b -k -g 'freez|hibernate|suspend|hung task|blocked for more than|NVRM|nvidia' --no-pager
```

Check systemd policy inhibitors separately from kernel freezer blockers:

```sh
systemd-inhibit --list --mode=block
```

Decision tree:

- If there are no `D` tasks and only ordinary inhibitors exist, resolve the
  inhibitor normally.
- If a process is in `S` or `R`, gracefully stop the owning test, server, or
  terminal session; then re-check.
- If a process is in `D`, especially with a pending signal, hibernation may
  hang or fail. Save work and plan a reboot instead of repeatedly attempting
  hibernation.
- If a previous hibernation attempt failed, collect the safe PID snapshot and
  kernel log excerpt, then stop probing. Repeated probes can create additional
  wedged diagnostics.

## NVIDIA-Specific Notes

These `wchan` values have been observed or are relevant for CUDA-linked Rust
and rust-analyzer workloads:

| `wchan` | What it suggests | What not to do |
|---|---|---|
| `do_exit` | The target is stuck during process teardown, often while kernel or driver-owned mappings are being released. If a signal is pending, more signals are unlikely to help. | Do not spam `kill`, run `pkill -f`, or start command-line reads to "confirm" the binary. |
| `do_mprotect_pkey` | The task is inside a memory-protection change. CUDA, JIT, allocator, or rust-analyzer teardown paths can make this relevant when large mappings are active. | Do not attach debuggers or dump core; those add more memory-management pressure. |
| `__vma_start_write` | The task is waiting on VMA write-side coordination. Other memory readers or writers may pile up behind it. | Do not run broad `/proc` memory readers, `gcore`, or `strace -p`. |
| `__access_remote_vm` | A diagnostic process is reading another process's address space, commonly through procfs command-line or environment reads. | Stop using `ps ... cmd`, `pgrep -af`, `/proc/$pid/cmdline`, and `/proc/$pid/environ` for this incident. |

For the Phase 4 failure class, a full rust-analyzer graph test can create a
large Rust process with RA worker threads and CUDA-capable libraries in its
history. If that process reaches `D` state in `do_exit`, treat it as a kernel or
driver teardown problem, not as an application-level shutdown bug that can be
fixed with a more forceful process-name kill.

`nvidia-smi` can be useful before a failure, but if the NVIDIA driver appears
wedged or kernel logs show `NVRM` errors, avoid piling on GPU reset, module
unload, or process-name cleanup commands from the same desktop session. Capture
the safe `/proc` snapshot and kernel logs, then use a controlled reboot plan.
