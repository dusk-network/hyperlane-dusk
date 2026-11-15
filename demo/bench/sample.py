#!/usr/bin/env python3
"""Sample agent resource use for the local agents benchmark (Linux /proc).

Every --interval seconds, for each process named by a PID file, appends a
JSON line with its CPU time (own and reaped children's, so the dusk-tx
helpers an agent ran count towards it), RSS, peak RSS, threads and I/O.
Every --dir-every samples it also records the size of each directory and
file given (databases, event cursors, checkpoints, logs). PID files are
re-read every time, so a restarted agent is followed.

  sample.py --out resources.jsonl --interval 5 \\
      --pidfile relayer=agents/relayer.pid --pidfile rusk=rusk.pid \\
      --path relayer-db=state/relayer-db --path relayer-log=logs/relayer.log
"""

import argparse
import json
import os
import signal
import time

CLK_TCK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")


def read_pid(path):
    try:
        with open(path) as f:
            return int(f.read().split()[0])
    except (OSError, ValueError, IndexError):
        return None


def proc_sample(pid):
    try:
        with open(f"/proc/{pid}/stat") as f:
            stat = f.read()
        fields = stat[stat.rindex(")") + 2:].split()
        status = {}
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                key, _, value = line.partition(":")
                status[key] = value.strip()
        io = {}
        try:
            with open(f"/proc/{pid}/io") as f:
                for line in f:
                    key, _, value = line.partition(":")
                    io[key] = int(value)
        except OSError:
            pass
    except (OSError, ValueError):
        return None
    # fields[0] is field 3 (state) of /proc/pid/stat
    utime, stime, cutime, cstime = (int(fields[i]) / CLK_TCK for i in (11, 12, 13, 14))
    return {
        "pid": pid,
        "cpu": round(utime + stime, 2),
        "cpu_children": round(cutime + cstime, 2),
        "rss_kb": int(status.get("VmRSS", "0 kB").split()[0]),
        "hwm_kb": int(status.get("VmHWM", "0 kB").split()[0]),
        "threads": int(status.get("Threads", "0")),
        "read_bytes": io.get("read_bytes"),
        "write_bytes": io.get("write_bytes"),
    }


def size_of(path):
    """Apparent bytes and allocated bytes of a file or directory tree."""
    if not os.path.exists(path):
        return None
    if os.path.isfile(path):
        st = os.stat(path)
        return {"bytes": st.st_size, "disk": st.st_blocks * 512, "files": 1}
    total = disk = files = 0
    for root, _, names in os.walk(path):
        for name in names:
            try:
                st = os.lstat(os.path.join(root, name))
            except OSError:
                continue
            total += st.st_size
            disk += st.st_blocks * 512
            files += 1
    return {"bytes": total, "disk": disk, "files": files}


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True)
    parser.add_argument("--interval", type=float, default=5.0)
    parser.add_argument("--dir-every", type=int, default=6)
    parser.add_argument("--pidfile", action="append", default=[], metavar="NAME=PATH")
    parser.add_argument("--path", action="append", default=[], metavar="NAME=PATH")
    args = parser.parse_args()
    pidfiles = [spec.split("=", 1) for spec in args.pidfile]
    paths = [spec.split("=", 1) for spec in args.path]

    running = True

    def stop(*_):
        nonlocal running
        running = False

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    n = 0
    with open(args.out, "a", buffering=1) as out:
        while running:
            now = time.time()
            for name, path in pidfiles:
                pid = read_pid(path)
                sample = proc_sample(pid) if pid else None
                if sample:
                    out.write(json.dumps({"ts": round(now, 3), "proc": name, **sample}) + "\n")
            if n % args.dir_every == 0:
                for name, path in paths:
                    size = size_of(path)
                    if size:
                        out.write(json.dumps({"ts": round(now, 3), "path": name, **size}) + "\n")
            n += 1
            deadline = now + args.interval
            while running and time.time() < deadline:
                time.sleep(min(0.5, max(0.0, deadline - time.time())))


if __name__ == "__main__":
    main()
