#!/usr/bin/env python3
"""Sample a macOS process tree using metadata only; never read process arguments.

RSS totals may count shared pages repeatedly. Launchd-owned WebKit helpers are
not descendants and must not be claimed as measured without independent proof.
"""

import argparse
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import statistics
import subprocess
import time


def cpu_seconds(value):
    days = 0
    if "-" in value:
        day, value = value.split("-", 1)
        days = int(day)
    parts = value.split(":")
    seconds = 0.0
    for part in parts:
        seconds = seconds * 60 + float(part)
    return days * 86400 + seconds


def process_table():
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,rss=,time=,comm="], text=True
    )
    result = {}
    for line in output.splitlines():
        parts = line.split(None, 4)
        if len(parts) != 5:
            continue
        pid, parent, rss, cpu, executable = parts
        result[int(pid)] = {
            "pid": int(pid),
            "parent_pid": int(parent),
            "rss_bytes": int(rss) * 1024,
            "cpu_seconds": cpu_seconds(cpu),
            "executable": Path(executable).name,
        }
    return result


def selected_processes(table, pid):
    if pid not in table:
        raise RuntimeError("Selected process exited; refusing to report a full interval")
    selected = {pid}
    while True:
        children = {p for p, row in table.items() if row["parent_pid"] in selected}
        updated = selected | children
        if updated == selected:
            return {p: table[p] for p in sorted(selected)}
        selected = updated


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--duration", type=float, default=120)
    parser.add_argument("--interval", type=float, default=1)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.pid <= 0 or not math.isfinite(args.duration) or not math.isfinite(args.interval) or args.duration < 1 or args.interval < 0.1:
        parser.error("Use positive PID, duration >= 1, interval >= 0.1")
    start = time.monotonic()
    start_utc = datetime.now(timezone.utc).isoformat()
    initial = selected_processes(process_table(), args.pid)
    root_name = initial[args.pid]["executable"]
    samples = [{"elapsed_seconds": 0.0, "processes": list(initial.values())}]
    previous = initial
    previous_time = start
    cpu_rates = []
    rss_values = [sum(p["rss_bytes"] for p in initial.values())]
    total_cpu = 0.0
    observed_pids = set(initial)
    missed_exits = set()
    while time.monotonic() - start < args.duration:
        remaining = args.duration - (time.monotonic() - start)
        time.sleep(min(args.interval, max(0, remaining)))
        current = selected_processes(process_table(), args.pid)
        now = time.monotonic()
        if current[args.pid]["executable"] != root_name:
            raise RuntimeError("Selected process identity changed")
        missed_exits.update(set(previous) - set(current))
        # Newly observed children have no earlier reliable counter baseline.
        # Exited children have no final counter; record this limitation.
        delta = 0.0
        for pid in set(previous) & set(current):
            difference = current[pid]["cpu_seconds"] - previous[pid]["cpu_seconds"]
            if difference < 0:
                raise RuntimeError("CPU counter decreased; process identity may have changed")
            delta += difference
        rate = delta / (now - previous_time) * 100
        total_cpu += delta
        cpu_rates.append(rate)
        rss = sum(p["rss_bytes"] for p in current.values())
        rss_values.append(rss)
        observed_pids.update(current)
        samples.append({"elapsed_seconds": now - start, "cpu_percent_one_core": rate,
                        "rss_bytes": rss, "processes": list(current.values())})
        previous = current
        previous_time = now
    elapsed = previous_time - start
    report = {
        "started_at_utc": start_utc,
        "ended_at_utc": datetime.now(timezone.utc).isoformat(),
        "root_pid": args.pid,
        "duration_seconds": elapsed,
        "interval_seconds": args.interval,
        "sample_count": len(samples),
        "scope": "Selected PID and observed descendants only; not all application helpers",
        "limitations": [
            "No UI state, account count, refresh setting, or workload is inferred",
            "macOS launchd-owned WebKit helpers are outside the process tree",
            "RSS is resident bytes, not physical footprint; shared pages may double count",
            "ps CPU time has finite precision; short intervals are quantized",
            "New/exited descendants may have unobserved CPU time between samples",
        ],
        "observed_pids": sorted(observed_pids),
        "exited_between_samples": sorted(missed_exits),
        "cpu_seconds": total_cpu,
        "cpu_mean_percent_one_core": total_cpu / elapsed * 100,
        "cpu_median_percent_one_core": statistics.median(cpu_rates),
        "cpu_p95_percent_one_core": percentile(cpu_rates, 0.95),
        "rss_initial_bytes": rss_values[0],
        "rss_final_bytes": rss_values[-1],
        "rss_median_bytes": statistics.median(rss_values),
        "rss_p95_bytes": percentile(rss_values, 0.95),
        "rss_max_bytes": max(rss_values),
        "samples": samples,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + chr(10))
    print(json.dumps({k: v for k, v in report.items() if k != "samples"}, indent=2))


if __name__ == "__main__":
    main()
