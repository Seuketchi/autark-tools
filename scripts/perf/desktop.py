#!/usr/bin/env python3
"""What the running desktop costs: memory (RSS against PSS) and CPU per thread, for each process named on the command line.

    scripts/perf/desktop.py wayfire autark-bar autark-dock

RSS counts every shared library in full for every process; PSS shares each page among the processes that map it, so it
is the real cost. CPU is read over five seconds from each thread's own counters, to say which thread does the work."""
import os
import sys
import time

TICKS = os.sysconf("SC_CLK_TCK")
WINDOW = 5


def rollup(pid):
    values = {}
    try:
        for line in open(f"/proc/{pid}/smaps_rollup"):
            key, _, rest = line.partition(":")
            if rest.strip().endswith("kB"):
                values[key] = int(rest.split()[0]) // 1024
    except OSError:
        return None
    return values


def threads(pid):
    out = {}
    for tid in os.listdir(f"/proc/{pid}/task"):
        try:
            name = open(f"/proc/{pid}/task/{tid}/comm").read().strip()
            fields = open(f"/proc/{pid}/task/{tid}/stat").read().rsplit(")", 1)[1].split()
            out[tid] = (name, int(fields[11]) + int(fields[12]))
        except OSError:
            pass
    return out


def find(names):
    found = {}
    for entry in os.listdir("/proc"):
        if entry.isdigit():
            try:
                comm = open(f"/proc/{entry}/comm").read().strip()
            except OSError:
                continue
            if comm in names:
                found.setdefault(comm, entry)
    return found


def main():
    names = sys.argv[1:]
    if not names:
        sys.exit(__doc__)
    pids = find(names)
    before = {name: threads(pid) for name, pid in pids.items()}
    time.sleep(WINDOW)
    total_rss = total_pss = 0
    print(f"{'process':18} {'RSS':>7} {'PSS':>7} {'private':>8} {'anon':>6}  threads  busiest threads (of one core)")
    for name, pid in pids.items():
        memory = rollup(pid)
        if not memory:
            continue
        busy = []
        for tid, (thread, ticks) in threads(pid).items():
            used = (ticks - before[name].get(tid, (thread, ticks))[1]) * 100 / TICKS / WINDOW
            busy.append((used, thread))
        busy.sort(reverse=True)
        top = ", ".join(f"{thread} {used:.0f}%" for used, thread in busy[:3] if used >= 0.5) or "idle"
        private = memory.get("Private_Clean", 0) + memory.get("Private_Dirty", 0)
        print(f"{name:18} {memory['Rss']:5d} MB {memory['Pss']:5d} MB {private:6d} MB {memory.get('Anonymous', 0):4d} MB  {len(busy):7d}  {top}")
        total_rss += memory["Rss"]
        total_pss += memory["Pss"]
    print(f"{'total':18} {total_rss:5d} MB {total_pss:5d} MB")
    missing = [name for name in names if name not in pids]
    if missing:
        print("not running:", ", ".join(missing))


main()
