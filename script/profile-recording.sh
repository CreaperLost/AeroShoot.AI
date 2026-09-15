#!/bin/sh
# Profile AeroShoot while it records. Captures per-process CPU and memory,
# per-thread CPU inside AeroShoot (from a `sample` call tree), GPU utilization,
# thermal state and the active project's disk write rate, then writes a
# Markdown report.
#
#   script/profile-recording.sh [seconds]      default 30, minimum 10
#
# AEROSHOOT_PROCESS profiles another process name (smoke tests).
# AEROSHOOT_PROFILE_DIR overrides the report folder.
set -eu

duration=${1:-30}
case "$duration" in ''|*[!0-9]*) echo "seconds must be a whole number" >&2; exit 2 ;; esac
if [ "$duration" -lt 10 ]; then echo "profile for at least 10 seconds" >&2; exit 2; fi

process=${AEROSHOOT_PROCESS:-AeroShoot}
pid=$(pgrep -x "$process" | head -n 1 || true)
if [ -z "$pid" ]; then
  echo "$process is not running. Start a recording, then run this script." >&2
  exit 1
fi

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
projects_root="$HOME/Documents/AeroShootRec"
out_dir=${AEROSHOOT_PROFILE_DIR:-"$projects_root/_profiles"}
mkdir -p "$out_dir"
report="$out_dir/profile-$(date +%Y%m%d-%H%M%S).md"
work=$(mktemp -d "${TMPDIR:-/tmp}/aeroshoot-profile.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

# The project being recorded is the one whose media changed most recently.
active_media=$(find "$projects_root" -type f -path '*/media/*' -mmin -2 2>/dev/null | head -n 1 || true)
project=""
if [ -n "$active_media" ]; then project=${active_media%%/media/*}; fi
disk_start=0
if [ -n "$project" ]; then disk_start=$(du -sk "$project/media" | cut -f1); fi

echo "Profiling $process (pid $pid) for ${duration}s..."
# Per-process GPU time comes from each graphics client's accumulated GPU time.
ioreg -r -c IOGPUDeviceUserClient -l -w0 -a > "$work/gpu_start.plist" 2>/dev/null || true
samples=$((duration / 2))
top -l $((samples + 1)) -s 2 -stats pid,command,cpu,mem,threads > "$work/top.txt" 2>/dev/null &
top_job=$!
(
  i=0
  while [ "$i" -lt "$samples" ]; do
    ioreg -r -d 1 -c IOAccelerator 2>/dev/null | grep -o '"Device Utilization %"=[0-9]*' | head -n 1
    sleep 2
    i=$((i + 1))
  done
) > "$work/gpu.txt" &
gpu_job=$!

sample_seconds=10
if [ "$duration" -lt 24 ]; then sample_seconds=$((duration / 2)); fi
sleep 2
sample "$pid" "$sample_seconds" 1 -file "$work/sample.txt" > /dev/null 2>&1 || true
wait "$top_job" "$gpu_job" || true
ioreg -r -c IOGPUDeviceUserClient -l -w0 -a > "$work/gpu_end.plist" 2>/dev/null || true

disk_end=0
if [ -n "$project" ] && [ -d "$project/media" ]; then disk_end=$(du -sk "$project/media" | cut -f1); fi
pmset -g therm > "$work/therm.txt" 2>&1 || true
alive=yes
kill -0 "$pid" 2>/dev/null || alive=no

python3 - "$work" "$process" "$pid" "$duration" "$project" "$disk_start" "$disk_end" "$alive" > "$report" <<'PY'
import plistlib
import re
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path

work, process, pid, duration, project, disk_start, disk_end, alive = sys.argv[1:]
work, duration = Path(work), int(duration)
out = []
say = out.append
say(f"# AeroShoot profile — {datetime.now():%Y-%m-%d %H:%M:%S}\n")
say(f"- process: {process} (pid {pid}), {duration}s, still running at end: {alive}")
say(f"- project: {project or 'no media written in the last 2 minutes'}\n")

# Per-process CPU. top's first sample has no CPU delta, so skip it.
watched = {process[:16], "WindowServer", "kernel_task", "replayd", "VTEncoderXPCServ",
           "coreaudiod", "appleh13camerad", "cameracaptured"}
row = re.compile(r"^\s*(\d+)\s+(.+?)\s+([\d.]+)\s+(\S+)\s+(\S+)\s*$")
stats = defaultdict(lambda: {"cpu": [], "mem": "", "threads": ""})
blocks = (work / "top.txt").read_text(errors="replace").split("Processes:")[2:]
for block in blocks:
    for line in block.splitlines():
        m = row.match(line)
        if not m:
            continue
        proc_pid, command, cpu, mem, threads = m.groups()
        command = command.strip()
        if command.startswith("com.apple.WebKit"):
            command = f"{command} ({proc_pid})"
        elif command not in watched:
            continue
        entry = stats[command]
        entry["cpu"].append(float(cpu))
        entry["mem"], entry["threads"] = mem, threads
say("## Process CPU (100% = one core)\n")
say("| process | avg CPU % | max CPU % | memory | threads |")
say("|---|---|---|---|---|")
for command, entry in sorted(stats.items(), key=lambda kv: -sum(kv[1]["cpu"]) / len(kv[1]["cpu"])):
    avg = sum(entry["cpu"]) / len(entry["cpu"])
    if command.startswith("com.apple.WebKit") and avg < 0.5:
        continue
    say(f"| {command} | {avg:.1f} | {max(entry['cpu']):.1f} | {entry['mem']} | {entry['threads']} |")
say("")

# Per-thread CPU: samples whose innermost frame is not a blocking wait.
WAITS = {"mach_msg2_trap", "mach_msg_trap", "__psynch_cvwait", "__workq_kernreturn",
         "semaphore_wait_trap", "semaphore_timedwait_trap", "__semwait_signal", "kevent_id",
         "kevent", "__ulock_wait", "__ulock_wait2", "__select", "poll", "__psynch_mutexwait",
         "__recvfrom", "__sigsuspend", "semaphore_wait_signal_trap", "__psynch_rw_rdlock",
         "__psynch_rw_wrlock"}
sample_path = work / "sample.txt"
text = sample_path.read_text(errors="replace") if sample_path.exists() else ""
if "Call graph:" in text:
    graph = text.split("Call graph:", 1)[1].split("Total number in stack", 1)[0]
    frame = re.compile(r"^(\s*[+!:| ]*)(\d+)\s+(.*)$")
    entries = [(len(m.group(1)), int(m.group(2)), m.group(3))
               for m in (frame.match(line) for line in graph.splitlines()) if m]
    threads, current = [], None
    for index, (depth, count, label) in enumerate(entries):
        if label.startswith("Thread_"):
            current = {"label": label, "total": count, "waiting": 0}
            threads.append(current)
            continue
        if current is None:
            continue
        following = entries[index + 1] if index + 1 < len(entries) else None
        leaf = following is None or following[0] <= depth or following[2].startswith("Thread_")
        symbol = label.split("  (in ", 1)[0].strip()
        if leaf and symbol in WAITS:
            current["waiting"] += count
    samples_taken = max((t["total"] for t in threads), default=0)
    say(f"## {process} threads (CPU % of one core, {samples_taken} samples at 1 ms)\n")
    say("| thread | CPU % |")
    say("|---|---|")
    busy = sorted(((t["total"] - t["waiting"]) / samples_taken * 100, t["label"]) for t in threads if samples_taken)
    for cpu, label in reversed(busy[-15:]):
        if cpu < 0.5:
            continue
        name = re.sub(r"^Thread_(\d+|<multiple>)\s*", "", label).replace("(serial)", "").strip(" :") or "unnamed thread"
        say(f"| {name} | {cpu:.1f} |")
    say("")
    hot = text.split("Sort by top of stack", 1)[1].split("Binary Images", 1)[0] if "Sort by top of stack" in text else ""
    say("### Hottest non-waiting frames\n")
    shown = 0
    for line in hot.splitlines()[1:]:
        parts = line.strip().rsplit(None, 1)
        if len(parts) != 2 or not parts[1].isdigit():
            continue
        symbol = parts[0].split("  (in ", 1)[0].strip()
        if symbol in WAITS or symbol == "start_wqthread":
            continue
        say(f"- {parts[1]} × {parts[0][:120]}")
        shown += 1
        if shown == 12:
            break
    say("")
else:
    say(f"## {process} threads\n\n`sample` produced no call graph (process exited or access denied).\n")

gpu = [int(v) for v in re.findall(r"=(\d+)", (work / "gpu.txt").read_text(errors="replace"))]
say("## GPU\n")
say(f"- device utilization (all processes): avg {sum(gpu) / len(gpu):.0f}%, max {max(gpu)}%" if gpu else "- device utilization unavailable")


def gpu_times(path):
    """Accumulated GPU nanoseconds per (pid, name) from an `ioreg -a` snapshot."""
    try:
        entries = plistlib.loads(path.read_bytes())
    except Exception:
        return {}
    totals = defaultdict(int)
    for entry in entries if isinstance(entries, list) else []:
        match = re.match(r"pid (\d+), (.*)", str(entry.get("IOUserClientCreator", "")))
        if not match:
            continue
        for usage in entry.get("AppUsage") or []:
            totals[(int(match.group(1)), match.group(2))] += int(usage.get("accumulatedGPUTime", 0))
    return totals


start_path, end_path = work / "gpu_start.plist", work / "gpu_end.plist"
before = gpu_times(start_path) if start_path.exists() else {}
after = gpu_times(end_path) if end_path.exists() else {}
if before and after:
    elapsed_ns = max(end_path.stat().st_mtime_ns - start_path.stat().st_mtime_ns, 1)
    shares = sorted((((after[key] - before.get(key, 0)) / elapsed_ns * 100), key)
                    for key in after if after[key] > before.get(key, 0))
    say("- GPU time by process (share of wall time):")
    for share, (proc_pid, name) in reversed(shares[-6:]):
        say(f"  - {name} (pid {proc_pid}): {share:.1f}%")
    if not any(proc_pid == int(pid) for _, (proc_pid, _) in shares):
        say(f"  - {process}: no GPU time measured (it may have exited before the end snapshot)")
say("")

say("## Disk\n")
if project:
    rate_kb = (int(disk_end) - int(disk_start)) / duration
    say(f"- media grew {int(disk_end) - int(disk_start)} KB in {duration}s "
        f"({rate_kb / 1024:.2f} MB/s, {rate_kb * 60 / 1024:.0f} MB/min)\n")
else:
    say("- no active recording found\n")

therm = [line for line in (work / "therm.txt").read_text(errors="replace").splitlines()
         if line.strip() and not line.startswith("Note: No")]
say("## Thermal\n")
say("\n".join(f"- {line.strip()}" for line in therm) if therm else "- no thermal or performance warnings recorded")
print("\n".join(out))
PY

cat "$report"
echo
echo "Report saved to $report"
if [ -n "$project" ]; then
  echo "After you stop recording, run: $repo_dir/script/analyze-recording.py \"$project\""
fi
