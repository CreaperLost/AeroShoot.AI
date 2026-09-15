#!/usr/bin/env python3
"""Summarize a finished AeroShoot project as Markdown.

Reports segment durations against the 60-second limit, per-track media
coverage and shortfall, capture gaps and stalls shared across tracks, mouse
telemetry (event kinds, gap reasons, cursor shapes) and the qualification result.

Usage: script/analyze-recording.py <project.aero>
"""
import json
import sys
from collections import Counter, defaultdict
from pathlib import Path

SEGMENT_LIMIT_S = 60.0
GAP_S = 0.1
STALL_BIN_S = 0.25
STALL_MIN_TRACKS = 3
TRACKS = ("screen", "webcam", "system", "mic")


def load_jsonl(path):
    rows = []
    if not path.exists():
        return rows
    for line in path.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


def seconds(us):
    return us / 1_000_000


def segments_section(root, say):
    segments = defaultdict(list)
    for record in load_jsonl(root / "journal.jsonl"):
        if "start_us" in record and "end_us" in record and "track_id" in record:
            segments[record["track_id"]].append(record)
    say("## Segments")
    if not segments:
        say("No committed segments in journal.jsonl.\n")
        return
    say("| track | segments | longest | over limit | media | span | shortfall |")
    say("|---|---|---|---|---|---|---|")
    for track in sorted(segments, key=lambda t: TRACKS.index(t) if t in TRACKS else 99):
        rows = sorted(segments[track], key=lambda r: r["start_us"])
        durations = [seconds(r["end_us"] - r["start_us"]) for r in rows]
        over = [r.get("relative_path", "?") for r, d in zip(rows, durations) if d > SEGMENT_LIMIT_S]
        media = sum(durations)
        span = seconds(rows[-1]["end_us"] - rows[0]["start_us"])
        say(f"| {track} | {len(rows)} | {max(durations):.3f}s | {', '.join(over) or 'none'} "
            f"| {media:.2f}s | {span:.2f}s | {span - media:.2f}s |")
    say("")


def gaps_section(root, say):
    times = defaultdict(list)
    anomalies = []
    for record in load_jsonl(root / "telemetry" / "media_timestamps.jsonl"):
        if record.get("type") is None and "mapped_us" in record and "track" in record:
            times[record["track"]].append(record["mapped_us"])
        elif record.get("type") in ("delivery_gap", "slow_callback"):
            anomalies.append(record)
    say("## Capture gaps")
    if not times:
        say("No media_timestamps.jsonl samples.\n")
        return
    say(f"Gaps longer than {GAP_S * 1000:.0f} ms between captured samples.\n")
    say("| track | samples | span | time in gaps | largest gaps |")
    say("|---|---|---|---|---|")
    missing = defaultdict(set)
    for track in sorted(times, key=lambda t: TRACKS.index(t) if t in TRACKS else 99):
        ts = sorted(times[track])
        gaps = [(b - a, a) for a, b in zip(ts, ts[1:]) if b - a > GAP_S * 1_000_000]
        for length, start in gaps:
            first_bin = int(seconds(start) / STALL_BIN_S) + 1
            last_bin = int(seconds(start + length) / STALL_BIN_S)
            missing[track].update(range(first_bin, last_bin))
        largest = ", ".join(f"{seconds(g):.2f}s at {seconds(a):.1f}s" for g, a in sorted(gaps, reverse=True)[:3])
        say(f"| {track} | {len(ts)} | {seconds(ts[0]):.2f}–{seconds(ts[-1]):.2f}s "
            f"| {seconds(sum(g for g, _ in gaps)):.2f}s | {largest or 'none'} |")
    say("")
    bins = Counter(b for track_bins in missing.values() for b in track_bins)
    stalled = sorted(b for b, count in bins.items() if count >= min(STALL_MIN_TRACKS, len(times)))
    stalls, start, previous = [], None, None
    for b in stalled:
        if start is None:
            start = previous = b
        elif b == previous + 1:
            previous = b
        else:
            stalls.append((start, previous))
            start = previous = b
    if start is not None:
        stalls.append((start, previous))
    stalls = [(a, b) for a, b in stalls if (b - a + 1) * STALL_BIN_S >= 1.0]
    say("### Capture timing anomalies")
    say("`delivery_gap`: the source delivered nothing for a while (upstream). "
        "`slow_callback`: AeroShoot's own capture callback blocked its queue.\n")
    if anomalies:
        for record in sorted(anomalies, key=lambda r: r.get("at_us", 0)):
            length = record.get("gap_ms", record.get("duration_ms"))
            say(f"- {seconds(record.get('at_us', 0)):.2f}s {record['type']} on {record.get('track')}: {length} ms")
    else:
        say("- none recorded")
    say("")
    say("### Shared stalls")
    if stalls:
        for a, b in stalls:
            say(f"- {a * STALL_BIN_S:.1f}s–{(b + 1) * STALL_BIN_S:.1f}s: "
                f"{min(STALL_MIN_TRACKS, len(times))}+ tracks received no samples")
    else:
        say("- none (no second-long gap shared by several tracks)")
    say("")


def telemetry_section(root, say):
    events = load_jsonl(root / "telemetry" / "events.jsonl")
    say("## Mouse telemetry")
    if not events:
        say("No telemetry/events.jsonl (tracking off or no screen track).\n")
        return
    kinds = Counter((e.get("payload") or {}).get("kind", e.get("kind", "?")) for e in events)
    reasons = Counter((e.get("payload") or {}).get("reason") for e in events
                      if (e.get("payload") or {}).get("kind") == "gap")
    shapes = Counter((e.get("payload") or {}).get("name") or (e.get("payload") or {}).get("cursor_id")
                     for e in events if (e.get("payload") or {}).get("kind") == "cursor_changed")
    images = list((root / "telemetry" / "cursors").glob("*.png"))
    say(f"- events: {len(events)} ({', '.join(f'{k} {v}' for k, v in kinds.most_common())})")
    say(f"- gap reasons: {', '.join(f'{k} {v}' for k, v in reasons.most_common()) or 'none'}")
    say(f"- cursor changes: {sum(shapes.values())}; shapes: "
        f"{', '.join(f'{k} {v}' for k, v in shapes.most_common()) or 'none'}")
    say(f"- stored cursor images: {len(images)}")
    say("")


def qualification_section(root, say):
    path = root / "qualification.json"
    say("## Qualification")
    if not path.exists():
        say("No qualification.json (Stop did not complete).\n")
        return
    report = json.loads(path.read_text())
    say(f"- passed: {report.get('passed')}")
    for failure in report.get("failures", []):
        say(f"- {failure.get('code')}: {failure.get('message')}")
    say("")


def main(argv):
    if len(argv) != 2:
        print(__doc__.strip())
        return 2
    root = Path(argv[1]).expanduser()
    if not (root / "journal.jsonl").exists():
        print(f"Not an AeroShoot project (no journal.jsonl): {root}", file=sys.stderr)
        return 1
    lines = []
    say = lines.append
    manifest_path = root / "manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    say(f"# Recording analysis: {root.name}\n")
    tracks = ", ".join(t.get("id", "?") for t in manifest.get("tracks", [])) or "unknown"
    say(f"- tracks: {tracks}")
    say(f"- cursor_mode: {manifest.get('cursorMode', manifest.get('cursor_mode'))}\n")
    segments_section(root, say)
    gaps_section(root, say)
    telemetry_section(root, say)
    qualification_section(root, say)
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
