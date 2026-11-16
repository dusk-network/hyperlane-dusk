#!/usr/bin/env python3
"""Summarise a local agents benchmark run (demo/bench/scenario.sh).

Reads the run directory and prints Markdown tables:

- requests and bytes per minute per agent, chain and phase, and per call kind,
  with the reads inside Multicall for Dusk aggregates, and the requests Rusk
  refused;
- per message: what each Dusk -> EVM and EVM -> Dusk message added over the
  idle rate, per agent and kind, and the dusk-tx helpers the agents ran;
- latency per stage, from the Rusk log's block times, checkpoint files,
  the proxy log, the dusk-tx shim log and the Mailbox's transactions, and
  how the relayer's deliveries to Dusk shared blocks;
- the agents' transactions and the gas they spent;
- CPU, memory, database and log growth per agent;
- what happened around the relayer restarts and the endpoint outage.

  python3 demo/bench/report.py <BENCH_DIR> [--json out.json]
"""

import argparse
import collections
import glob
import json
import os
import re
import statistics

AGENTS = ["relayer", "validator-dusk-1", "validator-dusk-2", "validator-dusk-3", "validator-evm"]
DUSK_VALIDATORS = AGENTS[1:4]

# ── Keccak-256 (for message IDs) ─────────────────────────────────────────────

_RC = [0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
       0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
       0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
       0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
       0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
       0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008]
_ROT = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56],
        [27, 20, 39, 8, 14]]
_MASK = (1 << 64) - 1


def _rol(v, n):
    return ((v << n) | (v >> (64 - n))) & _MASK if n else v


def keccak256(data):
    rate = 136
    padded = bytearray(data) + b"\x01"
    padded += b"\x00" * ((-len(padded)) % rate)
    padded[-1] |= 0x80
    a = [0] * 25
    for off in range(0, len(padded), rate):
        for i in range(rate // 8):
            a[i] ^= int.from_bytes(padded[off + 8 * i:off + 8 * i + 8], "little")
        for rc in _RC:
            c = [a[x] ^ a[x + 5] ^ a[x + 10] ^ a[x + 15] ^ a[x + 20] for x in range(5)]
            d = [c[(x - 1) % 5] ^ _rol(c[(x + 1) % 5], 1) for x in range(5)]
            a = [a[i] ^ d[i % 5] for i in range(25)]
            b = [0] * 25
            for x in range(5):
                for y in range(5):
                    b[y + 5 * ((2 * x + 3 * y) % 5)] = _rol(a[x + 5 * y], _ROT[x][y])
            a = [b[i] ^ ((~b[(i % 5 + 1) % 5 + 5 * (i // 5)]) & b[(i % 5 + 2) % 5 + 5 * (i // 5)])
                 for i in range(25)]
            a[0] ^= rc
    return b"".join(a[i].to_bytes(8, "little") for i in range(4))


# ── loading ──────────────────────────────────────────────────────────────────

def jsonl(path):
    out = []
    if os.path.exists(path):
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line:
                    try:
                        out.append(json.loads(line))
                    except ValueError:
                        pass
    return out


def load_phases(run):
    """{phase: (start, end)} and the agent start/stop events."""
    phases, events = {}, []
    for rec in jsonl(os.path.join(run, "phases.jsonl")):
        if "phase" in rec:
            start, end = phases.get(rec["phase"], (None, None))
            if rec["event"] == "start":
                start = rec["ts"]
            else:
                end = rec["ts"]
            phases[rec["phase"]] = (start, end)
        else:
            events.append(rec)
    return phases, events


ANSI = re.compile(r"\x1b\[[0-9;]*m")


def load_blocks(run):
    """{height: {"accepted": t, "finalized": t}} from the Rusk log (wall clock)."""
    from datetime import datetime
    blocks = collections.defaultdict(dict)
    path = os.path.join(run, "logs", "rusk.log")
    if not os.path.exists(path):
        return blocks
    with open(path, errors="replace") as f:
        for line in f:
            if "block accepted" not in line and "block finalized" not in line:
                continue
            line = ANSI.sub("", line)
            stamp = line.split(" ", 1)[0]
            try:
                t = datetime.strptime(stamp[:26] + "Z", "%Y-%m-%dT%H:%M:%S.%fZ")
            except ValueError:
                continue
            from datetime import timezone
            t = t.replace(tzinfo=timezone.utc).timestamp()
            m = re.search(r'event="block (accepted|finalized)".*?\bheight=(\d+)', line)
            if m:
                blocks[int(m.group(2))].setdefault(m.group(1), t)
    return blocks


def in_window(ts, window):
    return window[0] is not None and window[1] is not None and window[0] <= ts < window[1]


def pct(values, q):
    if not values:
        return None
    values = sorted(values)
    k = (len(values) - 1) * q
    lo, hi = int(k), min(int(k) + 1, len(values) - 1)
    return values[lo] + (values[hi] - values[lo]) * (k - lo)


def fmt(v, digits=1):
    if v is None:
        return "-"
    if isinstance(v, int) or (isinstance(v, float) and v.is_integer() and abs(v) >= 100):
        return f"{int(v):,}"
    return f"{v:,.{digits}f}"


def table(headers, rows):
    out = ["| " + " | ".join(headers) + " |",
           "|" + "|".join("---" if i == 0 else "---:" for i in range(len(headers))) + "|"]
    for row in rows:
        out.append("| " + " | ".join(str(c) for c in row) + " |")
    return "\n".join(out)


# ── API usage ────────────────────────────────────────────────────────────────

def usage(requests, window):
    """{(agent, up): {"n", "bytes", "kinds": {kind: {...}}}} for requests in window."""
    agg = collections.defaultdict(lambda: {"n": 0, "rq": 0, "rs": 0, "dur": 0.0, "helper": 0,
                                           "kinds": collections.defaultdict(
                                               lambda: {"n": 0, "rq": 0, "rs": 0, "dur": 0.0,
                                                        "durs": [], "err": 0})})
    for r in requests:
        if "dur" not in r or not in_window(r["ts"], window):
            continue
        a = agg[(r["agent"], r.get("up", "?"))]
        a["n"] += 1
        a["rq"] += r.get("rq", 0)
        a["rs"] += r.get("rs", 0)
        a["dur"] += r["dur"]
        kind = r["kind"] + (" [dusk-tx]" if r.get("src") == "dusk-tx" else "")
        if r.get("src") == "dusk-tx":
            a["helper"] += 1
        k = a["kinds"][kind]
        k["n"] += 1
        k["rq"] += r.get("rq", 0)
        k["rs"] += r.get("rs", 0)
        k["dur"] += r["dur"]
        k["durs"].append(r["dur"])
        if r.get("st", 200) >= 400:
            k["err"] += 1
    return agg


def minutes(window):
    return (window[1] - window[0]) / 60.0


def report_usage(out, requests, phases, order):
    out.append("## Requests per minute, by agent and phase\n")
    rows = []
    for agent in AGENTS + ["loadgen", "harness"]:
        for up in ("rusk", "evm"):
            row = [f"{agent} -> {'Rusk' if up == 'rusk' else 'Anvil'}"]
            seen = False
            for phase in order:
                w = phases.get(phase)
                if not w or w[1] is None:
                    row.append("-")
                    continue
                a = usage(requests, w).get((agent, up))
                if a:
                    seen = True
                    row.append(f"{a['n'] / minutes(w):.1f} / {(a['rq'] + a['rs']) / 1024 / minutes(w):.1f}")
                else:
                    row.append("0")
            if seen:
                rows.append(row)
    out.append("Requests per minute / KiB per minute (both directions).\n")
    out.append(table(["Agent"] + order, rows) + "\n")


def report_kinds(out, requests, window, title, agents, top=12):
    agg = usage(requests, window)
    mins = minutes(window)
    out.append(f"### {title}\n")
    for agent in agents:
        for up in ("rusk", "evm"):
            a = agg.get((agent, up))
            if not a:
                continue
            rows = []
            for kind, k in sorted(a["kinds"].items(), key=lambda kv: -kv[1]["n"])[:top]:
                rows.append([kind, f"{k['n'] / mins:.2f}", f"{k['rs'] / 1024 / mins:.2f}",
                             f"{k['rq'] / 1024 / mins:.2f}", fmt(statistics.mean(k["durs"]), 1),
                             fmt(pct(k["durs"], 0.9), 1), k["err"]])
            out.append(f"**{agent} -> {'Rusk' if up == 'rusk' else 'Anvil'}**: "
                       f"{a['n'] / mins:.1f} req/min, {(a['rq'] + a['rs']) / 1024 / mins:.1f} KiB/min, "
                       f"node time {a['dur'] / 1000 / (mins * 60) * 100:.2f}% of wall clock\n")
            out.append(table(["Kind", "req/min", "reply KiB/min", "request KiB/min",
                              "mean ms", "p90 ms", "errors"], rows) + "\n")


def report_per_message(out, requests, helper_calls, idle, window, n_msgs, title):
    """Requests per message: phase count minus the idle rate over the phase."""
    base = usage(requests, idle)
    load = usage(requests, window)
    idle_min, load_min = minutes(idle), minutes(window)
    out.append(f"### {title}: {n_msgs} messages over {load_min:.1f} min\n")
    summary = []
    for agent in AGENTS:
        for up in ("rusk", "evm"):
            a, b = load.get((agent, up)), base.get((agent, up))
            if not a:
                continue
            bn = b["n"] / idle_min if b else 0
            brs = (b["rq"] + b["rs"]) / idle_min if b else 0
            extra_n = a["n"] - bn * load_min
            extra_bytes = (a["rq"] + a["rs"]) - brs * load_min
            summary.append([f"{agent} -> {'Rusk' if up == 'rusk' else 'Anvil'}",
                            f"{a['n'] / load_min:.1f}", f"{bn:.1f}",
                            f"{extra_n / n_msgs:.1f}", f"{extra_bytes / 1024 / n_msgs:.1f}",
                            f"{a['helper'] / n_msgs:.1f}"])
    out.append(table(["Agent", "req/min in phase", "req/min idle", "extra req/msg",
                      "extra KiB/msg", "dusk-tx req/msg"], summary) + "\n")
    # the kinds that grow with messages
    for agent in AGENTS:
        for up in ("rusk", "evm"):
            a, b = load.get((agent, up)), base.get((agent, up))
            if not a:
                continue
            rows = []
            for kind, k in a["kinds"].items():
                bk = b["kinds"].get(kind) if b else None
                bn = bk["n"] / idle_min if bk else 0
                brs = bk["rs"] / idle_min if bk else 0
                extra = (k["n"] - bn * load_min) / n_msgs
                if abs(extra) >= 0.05:
                    rows.append((extra, [kind, f"{extra:.2f}",
                                         f"{(k['rs'] - brs * load_min) / 1024 / n_msgs:.2f}",
                                         fmt(statistics.mean(k["durs"]), 1)]))
            if rows:
                rows.sort(key=lambda r: -r[0])
                out.append(f"**{agent} -> {'Rusk' if up == 'rusk' else 'Anvil'}**, per message:\n")
                out.append(table(["Kind", "extra req/msg", "extra reply KiB/msg", "mean ms"],
                                 [r[1] for r in rows[:14]]) + "\n")
    calls = [c for c in helper_calls if in_window(c["ts"], window)]
    if calls:
        by = collections.Counter((c["agent"], c["fn"], c["rc"]) for c in calls)
        rows = [[a, fn, rc, n, f"{n / n_msgs:.2f}"] for (a, fn, rc), n in sorted(by.items())]
        out.append("dusk-tx helper runs:\n")
        out.append(table(["Agent", "Function", "Exit", "Runs", "Per message"], rows) + "\n")


# ── messages and latency ─────────────────────────────────────────────────────

def find_message(data, origin, destination, body_len=64):
    """The first Hyperlane message from origin to destination in `data`."""
    for i in range(len(data) - 45):
        if data[i] == 3 and int.from_bytes(data[i + 5:i + 9], "big") == origin \
                and int.from_bytes(data[i + 41:i + 45], "big") == destination:
            return int.from_bytes(data[i + 1:i + 5], "big"), bytes(data[i:i + 77 + body_len])
    return None, None


def checkpoint_times(run, validator):
    times = {}
    for path in glob.glob(os.path.join(run, "checkpoints", validator, "*_with_id.json")):
        name = os.path.basename(path)
        try:
            times[int(name.split("_", 1)[0])] = os.stat(path).st_mtime
        except ValueError:
            pass
    return times


def stats_rows(stages, msgs, base_key):
    rows = []
    for key, label in stages:
        deltas = [m[key] - m[base_key] for m in msgs if m.get(key) is not None and m.get(base_key) is not None]
        if deltas:
            rows.append([label, len(deltas), fmt(pct(deltas, 0.5), 1), fmt(pct(deltas, 0.9), 1),
                         fmt(max(deltas), 1)])
    return rows


def step_rows(stages, msgs):
    rows = []
    for (k0, l0), (k1, l1) in zip(stages, stages[1:]):
        deltas = [m[k1] - m[k0] for m in msgs if m.get(k1) is not None and m.get(k0) is not None]
        if deltas:
            rows.append([f"{l0} -> {l1}", len(deltas), fmt(pct(deltas, 0.5), 1),
                         fmt(pct(deltas, 0.9), 1), fmt(max(deltas), 1)])
    return rows


def dusk_to_evm(run, requests, load, blocks, dusk_domain, evm_domain, heights):
    """Per-message stage times for Dusk -> EVM messages, keyed by nonce."""
    sent = [r for r in load if r.get("dir") == "dusk-to-evm" and r.get("rc") == 0]
    by_height = {r["height"]: r for r in sent if r.get("height") is not None}
    msgs = {}
    for nonce, height in heights.items():
        r = by_height.get(height)
        if r is None:
            continue
        b = blocks.get(height, {})
        msgs[nonce] = {"nonce": nonce, "sent": r["ts"], "dispatched": b.get("accepted"),
                       "origin_final": b.get("finalized"), "height": height}
    for v in DUSK_VALIDATORS:
        for idx, t in checkpoint_times(run, v).items():
            if idx in msgs:
                msgs[idx][f"signed:{v}"] = t
    for m in msgs.values():
        signed = sorted(m[k] for k in m if k.startswith("signed:"))
        m["signed_1st"] = signed[0] if signed else None
        m["signed_2nd"] = signed[1] if len(signed) > 1 else None
        m["signed_3rd"] = signed[2] if len(signed) > 2 else None
    ids = {}
    for r in requests:
        if r.get("agent") != "relayer" or r.get("up") != "evm" or "dur" not in r:
            continue
        for tag in r.get("msg", []):
            origin, nonce = map(int, tag.split(":"))
            if origin != dusk_domain or nonce not in msgs:
                continue
            m = msgs[nonce]
            if "process" in r["kind"] and "sendRawTransaction" not in r["kind"]:
                m.setdefault("metadata_ready", r["ts"])
            if "sendRawTransaction" in r["kind"]:
                m.setdefault("submitted", r["ts"])
                m.setdefault("included", r["ts"] + r["dur"] / 1000)
                m.setdefault("evm_tx", r.get("txhash"))
                m["sends"] = m.get("sends", 0) + 1
                if r.get("msgdata"):
                    ids[keccak256(bytes.fromhex(r["msgdata"])).hex()] = nonce
    for r in requests:
        if r.get("agent") == "relayer" and r.get("kind") == "eth_call:mailbox.delivered" and r.get("id"):
            nonce = ids.get(r["id"])
            if nonce is not None and str(r.get("rshex", "")).endswith("1"):
                if r["ts"] >= msgs[nonce].get("included", 0):
                    msgs[nonce].setdefault("confirmed", r["ts"] + r["dur"] / 1000)
    return msgs


def chain_outcome(call, dusk_txs):
    """(height, gas, error) of the transaction a dusk-tx run sent: from the
    chain (scenario.sh reads every one back), or from what dusk-tx printed. A
    pipelined delivery (--no-wait) prints only that it was submitted. None
    for a run that sent nothing."""
    res = call.get("result") or {}
    if call.get("rc") != 0 or not res.get("tx"):
        return None
    tx = dusk_txs.get(res["tx"])
    if tx is not None and not tx.get("missing"):
        return tx.get("blockHeight"), tx.get("gasSpent"), tx.get("err")
    if res.get("height") is not None:
        return res["height"], res.get("gas"), None
    return None, None, "in no block" if tx is not None else None


def evm_to_dusk(run, requests, load, helper_calls, blocks, dusk_domain, evm_domain, dusk_txs, mailbox_txs):
    """Per-message stage times for EVM -> Dusk messages, keyed by nonce. A
    delivery is the Mailbox `process` transaction that executed it, from the
    chain; the relayer's dusk-tx run that sent it gives when it was
    submitted. Other transactions for the same message failed in their block
    (a second copy, say) or never reached one."""
    msgs = {}
    for r in load:
        if not r.get("dir", "").startswith("evm-to-dusk") or r.get("rc") != 0 or not r.get("dispatch"):
            continue
        data = bytes.fromhex(r["dispatch"][2:])
        nonce, message = find_message(data, evm_domain, dusk_domain)
        if nonce is None:
            continue
        msgs[nonce] = {"nonce": nonce, "sent": r["ts"], "dispatched": r["end"],
                       "origin_final": r["end"], "id": keccak256(message).hex(), "kind": r["dir"]}
    for idx, t in checkpoint_times(run, "validator-evm").items():
        if idx in msgs:
            msgs[idx]["signed_1st"] = t
    by_id = {m["id"]: n for n, m in msgs.items()}
    for r in requests:
        if r.get("agent") != "relayer" or r.get("up") != "rusk" or "dur" not in r:
            continue
        simulated = r.get("kind") == "q:mailbox.process" or "mailbox.process" in r.get("det", "")
        if simulated and r.get("src") == "agent":
            for tag in r.get("msg", []):
                origin, nonce = map(int, tag.split(":"))
                if origin == evm_domain and nonce in msgs:
                    msgs[nonce].setdefault("metadata_ready", r["ts"])
                    msgs[nonce]["simulations"] = msgs[nonce].get("simulations", 0) + 1
        if r.get("kind") == "q:mailbox.delivered" and r.get("rqhex") and r.get("src") == "agent":
            nonce = by_id.get(r["rqhex"][-64:])
            if nonce is not None and r.get("rshex") == "01":
                msgs[nonce].setdefault("delivered_query_true", r["ts"] + r["dur"] / 1000)

    def included(m, height, gas, tx):
        b = blocks.get(height, {})
        m.update(height=height, gas=gas, tx=tx, included=b.get("accepted"), dest_final=b.get("finalized"))

    for t in mailbox_txs:
        if t.get("fn") != "process" or not t.get("data"):
            continue
        nonce, _ = find_message(bytes.fromhex(t["data"]), evm_domain, dusk_domain)
        if nonce is None or nonce not in msgs:
            continue
        m = msgs[nonce]
        if t.get("err") is None and "height" not in m:
            included(m, t["height"], t.get("gas"), t["id"])
        else:
            m["reverted_txs"] = m.get("reverted_txs", 0) + 1
            m["reverted_gas"] = m.get("reverted_gas", 0) + (t.get("gas") or 0)
    propagates = sorted(r["ts"] for r in requests
                        if r.get("agent") == "relayer" and r.get("src") == "dusk-tx"
                        and r.get("kind") == "tx:propagate")
    for c in sorted(helper_calls, key=lambda c: c["ts"]):
        if c.get("agent") != "relayer" or c.get("fn") != "process" or not c.get("args"):
            continue
        nonce, _ = find_message(bytes.fromhex(c["args"]), evm_domain, dusk_domain)
        if nonce is None or nonce not in msgs:
            continue
        m = msgs[nonce]
        m["helper_runs"] = m.get("helper_runs", 0) + 1
        m.setdefault("first_submit_attempt", c["ts"])
        tx = (c.get("result") or {}).get("tx")
        if not mailbox_txs and "height" not in m:
            # A run from before scenario.sh read the Mailbox's transactions.
            outcome = chain_outcome(c, dusk_txs)
            if outcome is not None and outcome[0] is not None and outcome[2] is None:
                included(m, outcome[0], outcome[1], tx)
        if tx is not None and tx == m.get("tx"):
            m["submitted"] = c["ts"]
            m["helper_done"] = c["end"]
            m["pipelined"] = bool(c.get("nowait"))
            prop = [t for t in propagates if c["ts"] <= t <= c["end"]]
            m["propagated"] = prop[0] if prop else None
        elif c.get("rc") != 0 or tx is None:
            m["failed_helper_runs"] = m.get("failed_helper_runs", 0) + 1
        else:
            m["other_txs"] = m.get("other_txs", 0) + 1
    return msgs


def report_latency(out, title, msgs, stages):
    out.append(f"### {title}: {len(msgs)} messages\n")
    if not msgs:
        return
    out.append("Seconds since the dispatch was included in its block:\n")
    out.append(table(["Stage", "n", "median", "p90", "max"], stats_rows(stages[1:], msgs, stages[0][0])) + "\n")
    out.append("Seconds per step:\n")
    out.append(table(["Step", "n", "median", "p90", "max"], step_rows(stages, msgs)) + "\n")


# ── resources ────────────────────────────────────────────────────────────────

def report_resources(out, resources, phases, order, n_msgs):
    procs = collections.defaultdict(list)
    paths = collections.defaultdict(list)
    for r in resources:
        if "proc" in r:
            procs[r["proc"]].append(r)
        elif "path" in r:
            paths[r["path"]].append(r)
    out.append("## CPU and memory\n")
    out.append("CPU is the share of one core over the phase (the process and the dusk-tx helpers it ran); "
               "RSS is the mean over the phase / the peak.\n")
    rows = []
    for name in AGENTS + ["rusk", "anvil", "proxy"]:
        samples = procs.get(name, [])
        if not samples:
            continue
        row = [name]
        for phase in order:
            w = phases.get(phase)
            s = [x for x in samples if w and w[1] and in_window(x["ts"], w)]
            if len(s) < 2:
                row.append("-")
                continue
            cpu = 0.0
            for x0, x1 in zip(s, s[1:]):
                if x0["pid"] == x1["pid"]:
                    cpu += (x1["cpu"] + x1["cpu_children"]) - (x0["cpu"] + x0["cpu_children"])
            span = s[-1]["ts"] - s[0]["ts"]
            rss = [x["rss_kb"] for x in s]
            row.append(f"{cpu / span * 100:.1f}% / {statistics.mean(rss) / 1024:.0f} / {max(rss) / 1024:.0f} MiB")
        rows.append(row)
    out.append(table(["Process"] + order, rows) + "\n")
    out.append("## Disk\n")
    rows = []
    for name, samples in sorted(paths.items()):
        row = [name]
        for phase in order:
            w = phases.get(phase)
            s = [x for x in samples if w and w[1] and in_window(x["ts"], w)]
            row.append(fmt(s[-1]["disk"] / 1024, 0) if s else "-")
        rows.append(row)
    out.append("Allocated KiB at the end of each phase.\n")
    out.append(table(["Path"] + order, rows) + "\n")


def size_at(samples, ts):
    best = None
    for x in samples:
        if x["ts"] <= ts:
            best = x
    return best


def report_growth(out, resources, phases, windows):
    paths = collections.defaultdict(list)
    for r in resources:
        if "path" in r:
            paths[r["path"]].append(r)
    out.append("### Growth per message\n")
    rows = []
    for name, samples in sorted(paths.items()):
        row = [name]
        for phase, n in windows:
            w = phases.get(phase)
            if not w or not w[1]:
                row.append("-")
                continue
            a, b = size_at(samples, w[0]), size_at(samples, w[1])
            if a and b:
                row.append(f"{(b['bytes'] - a['bytes']) / n / 1024:.2f} / {(b['disk'] - a['disk']) / n / 1024:.2f}")
            else:
                row.append("-")
        rows.append(row)
    out.append("KiB per message, apparent / allocated, from the first sample before the phase to the last.\n")
    out.append(table(["Path"] + [f"{p} ({n} msgs)" for p, n in windows], rows) + "\n")



def report_node_load(out, requests, window, title, top=15):
    """What the node spends its time on: all agents' Rusk requests by kind."""
    mins = minutes(window)
    kinds = collections.defaultdict(lambda: {"n": 0, "dur": 0.0, "rs": 0, "agents": set()})
    total_n = total_dur = total_rs = 0
    for r in requests:
        if r.get("up") != "rusk" or "dur" not in r or r["agent"] not in AGENTS:
            continue
        if not in_window(r["ts"], window):
            continue
        k = kinds[r["kind"] + (" [dusk-tx]" if r.get("src") == "dusk-tx" else "")]
        k["n"] += 1
        k["dur"] += r["dur"]
        k["rs"] += r.get("rs", 0)
        k["agents"].add(r["agent"])
        total_n += 1
        total_dur += r["dur"]
        total_rs += r.get("rs", 0)
    if not total_n:
        return
    out.append(f"### {title}: {total_n / mins:.0f} req/min, {total_rs / 1024 / mins:.0f} KiB/min of replies, "
               f"{total_dur / 1000 / (mins * 60) * 100:.2f}% of one node thread busy\n")
    rows = []
    for kind, k in sorted(kinds.items(), key=lambda kv: -kv[1]["dur"])[:top]:
        rows.append([kind, f"{k['n'] / mins:.1f}", f"{100 * k['n'] / total_n:.0f}%",
                     f"{100 * k['rs'] / total_rs:.0f}%", f"{100 * k['dur'] / total_dur:.0f}%",
                     fmt(k["dur"] / k["n"], 2), len(k["agents"])])
    out.append(table(["Kind", "req/min", "share of requests", "share of reply bytes",
                      "share of node time", "mean ms", "agents"], rows) + "\n")


def report_transactions(out, helper_calls, receipts, dusk_txs, mailbox_txs):
    out.append("## Transactions the agents sent\n")
    by = collections.defaultdict(lambda: {"runs": 0, "sent": 0, "ok": 0, "reverted": 0, "lost": 0,
                                          "fail": 0, "pipelined": 0, "gas": [], "lost_gas": 0})
    for c in helper_calls:
        b = by[(c.get("agent"), c.get("fn"))]
        b["runs"] += 1
        outcome = chain_outcome(c, dusk_txs)
        if outcome is None:
            b["fail"] += 1
            continue
        b["sent"] += 1
        b["pipelined"] += bool(c.get("nowait"))
        height, gas, err = outcome
        if height is None:
            b["lost"] += 1
        elif err is None:
            b["ok"] += 1
            b["gas"].append(gas or 0)
        else:
            b["reverted"] += 1
            b["lost_gas"] += gas or 0
    rows = []
    for (agent, fn), b in sorted(by.items(), key=lambda kv: (str(kv[0][0]), str(kv[0][1]))):
        if not fn:
            continue
        rows.append([agent, fn, b["runs"], b["sent"], b["pipelined"], b["ok"], b["reverted"], b["lost"], b["fail"],
                     fmt(sum(b["gas"]), 0), fmt(statistics.median(b["gas"]), 0) if b["gas"] else "-",
                     fmt(b["lost_gas"], 0)])
    out.append("Dusk transactions, through the dusk-tx helper. Sent: the helper submitted it; "
               "pipelined: with --no-wait; executed, failed in their block, and in no block: from the chain; "
               "gas spent by the executed ones and by the failed ones.\n")
    out.append(table(["Agent", "Function", "Helper runs", "Sent", "Pipelined", "Executed", "Failed in block",
                      "In no block", "Helper failed", "Gas (executed)", "Median gas", "Gas (failed)"], rows) + "\n")
    fails = [c for c in helper_calls if c.get("rc") != 0 and c.get("fn")]
    for c in fails[:12]:
        res = c.get("result") or {}
        out.append(f"- helper failed: {c.get('agent')} {c.get('fn')} at {c['ts']:.1f}: "
                   f"{(res.get('error') or '')[:160]}")
    if fails:
        out.append("")
    process = [t for t in mailbox_txs if t.get("fn") == "process"]
    if process:
        failed = [t for t in process if t.get("err")]
        out.append(f"Dusk Mailbox `process` transactions in blocks: {len(process)}, {len(failed)} failed there "
                   f"({fmt(sum(t.get('gas') or 0 for t in failed), 0)} gas); executed ones spent "
                   f"{fmt(sum(t.get('gas') or 0 for t in process if not t.get('err')), 0)} gas.\n")
        for t in failed[:8]:
            out.append(f"- failed in block {t.get('height')}: {str(t.get('err'))[:160]}")
        if failed:
            out.append("")
    gas = [int(r["gas"], 16) for r in receipts if r.get("gas")]
    ok = sum(1 for r in receipts if r.get("status") in ("0x1", 1, "1"))
    if gas:
        out.append(f"Relayer deliveries on Anvil: {len(gas)} transactions, {ok} succeeded, gas total "
                   f"{fmt(sum(gas), 0)}, median {fmt(statistics.median(gas), 0)}, max {fmt(max(gas), 0)}\n")


def report_logs(out, resources, phases, order):
    paths = collections.defaultdict(list)
    for r in resources:
        if "path" in r and r["path"].endswith("-log"):
            paths[r["path"]].append(r)
    out.append("## Log volume\n")
    rows = []
    for name, samples in sorted(paths.items()):
        row = [name[:-4]]
        for phase in order:
            w = phases.get(phase)
            if not w or not w[1]:
                row.append("-")
                continue
            a, b = size_at(samples, w[0]), size_at(samples, w[1])
            row.append(f"{(b['bytes'] - a['bytes']) / 1024 / minutes(w):.1f}" if a and b else "-")
        rows.append(row)
    out.append("KiB per minute written to each agent's log (default level, pretty format).\n")
    out.append(table(["Agent"] + order, rows) + "\n")


def burst_profile(requests, agent, up, start, seconds, step=10):
    """Requests per `step` seconds from `start`, for one agent and chain."""
    counts = [0] * (seconds // step)
    for r in requests:
        if r.get("agent") == agent and r.get("up") == up and "dur" in r:
            i = int((r["ts"] - start) // step)
            if 0 <= i < len(counts):
                counts[i] += 1
    return counts


def report_recovery(out, requests, phases, events, helper_calls, d2e, e2d, idle, dusk_txs):
    out.append("## Recovery\n")
    idle_rate = {}
    for agent in AGENTS:
        for up in ("rusk", "evm"):
            n = sum(1 for r in requests if r.get("agent") == agent and r.get("up") == up
                    and "dur" in r and in_window(r["ts"], idle))
            idle_rate[(agent, up)] = n / (minutes(idle) * 6)  # per 10 s
    starts = [e for e in events if e.get("event") == "start" and e.get("agent") == "relayer"]
    for phase, label in [("relayer-restart-1", "Relayer restart with Dusk -> EVM messages waiting"),
                         ("relayer-restart-2", "Relayer restart while delivering a burst to Dusk")]:
        w = phases.get(phase)
        if not w or not w[1]:
            continue
        start = next((e["ts"] for e in starts if e["ts"] >= w[0]), w[0])
        out.append(f"### {label}\n")
        for up in ("rusk", "evm"):
            prof = burst_profile(requests, "relayer", up, start, 180)
            out.append(f"- relayer -> {'Rusk' if up == 'rusk' else 'Anvil'}, requests per 10 s for 3 min "
                       f"after the start (idle: {idle_rate[('relayer', up)]:.1f}): {prof}")
        out.append("")
    if phases.get("relayer-restart-1", (None, None))[1]:
        w = phases["relayer-restart-1"]
        start = next((e["ts"] for e in starts if e["ts"] >= w[0]), w[0])
        backlog = [m for m in d2e.values() if in_window(m["sent"], phases.get("d2e-backlog", (None, None)))]
        done = [m["included"] for m in backlog if m.get("included")]
        if done:
            last = max(done)
            catch_up = [r for r in requests if r.get("agent") == "relayer" and "dur" in r and start <= r["ts"] <= last]
            rusk = [r for r in catch_up if r.get("up") == "rusk"]
            refused = [r for r in requests if r.get("st") == 429 and "dur" in r and start <= r["ts"] <= last]
            out.append(f"Backlog of {len(backlog)} Dusk -> EVM messages: first delivered "
                       f"{min(done) - start:.1f} s and the last {last - start:.1f} s after the relayer "
                       f"process started. Until the last, the relayer sent {len(rusk)} requests to Rusk "
                       f"({sum(1 for r in rusk if r['kind'].startswith('q:'))} contract queries, of them "
                       f"{sum(1 for r in rusk if r['kind'] == 'q:multicall.aggregate')} aggregates carrying "
                       f"{sum(len(r['det'].split(',')) for r in rusk if r['kind'] == 'q:multicall.aggregate' and r.get('det') not in (None, '', '?'))} "
                       f"reads; {sum(r.get('rq', 0) + r.get('rs', 0) for r in rusk) / 1024:.0f} KiB) and "
                       f"{len(catch_up) - len(rusk)} to Anvil; Rusk refused {len(refused)} requests (429), "
                       f"{sum(1 for r in refused if r['agent'] == 'relayer')} of them the relayer's.\n")
    if phases.get("relayer-restart-2", (None, None))[1]:
        w = phases["relayer-restart-2"]
        start = next((e["ts"] for e in starts if e["ts"] >= w[0]), w[0])
        around = sorted((c for c in helper_calls if c.get("agent") == "relayer" and c.get("fn") == "process"
                         and w[0] - 60 <= c["ts"] <= w[1] + 300), key=lambda c: c["ts"])
        out.append("Relayer deliveries sent around the restart (start, end, exit, nonce, block it executed "
                   "in or how it failed):\n")
        for c in around[:40]:
            res = c.get("result") or {}
            outcome = chain_outcome(c, dusk_txs)
            where = (f"block {outcome[0]}" + (f", failed: {str(outcome[2])[:80]}" if outcome[2] else "")
                     if outcome and outcome[0] is not None else (outcome[2] if outcome else "not sent"))
            out.append(f"- {c['ts'] - start:+.1f} s .. {c['end'] - start:+.1f} s, exit {c['rc']}, "
                       f"nonce {res.get('nonce')}, {where} {(res.get('error') or '')[:100]}")
        out.append("")
    w = phases.get("outage")
    r = phases.get("outage-restored")
    if w and r and r[1]:
        out.append("### Endpoint outage\n")
        for agent in AGENTS[:4]:
            prof = burst_profile(requests, agent, "rusk", r[0], 120)
            out.append(f"- {agent} -> Rusk, requests per 10 s for 2 min after the ports reopened "
                       f"(idle: {idle_rate[(agent, 'rusk')]:.1f}): {prof}")
        sent = [m for m in e2d.values() if in_window(m["sent"], w)]
        done = [m["included"] for m in sent if m.get("included")]
        if done:
            out.append(f"\n{len(sent)} EVM -> Dusk messages sent during the outage: last included "
                       f"{max(done) - r[0]:.1f} s after the ports reopened.\n")


def report_idle_summary(out, requests, phases, names):
    """Per agent: requests and KiB per minute to Rusk and to Anvil, per phase."""
    out.append("## Idle, per agent\n")
    headers = ["Agent"]
    for phase in names:
        headers += [f"{phase}: Rusk req/min", "KiB/min", "Anvil req/min", "KiB/min"]
    rows = []
    for agent in AGENTS:
        row = [agent]
        for phase in names:
            w = phases.get(phase)
            agg = usage(requests, w) if w and w[1] else {}
            for up in ("rusk", "evm"):
                a = agg.get((agent, up))
                if a:
                    row += [f"{a['n'] / minutes(w):.1f}", f"{(a['rq'] + a['rs']) / 1024 / minutes(w):.1f}"]
                else:
                    row += ["0", "0"]
        rows.append(row)
    out.append(table(headers, rows) + "\n")


def report_kind_matrix(out, requests, window, title, up="rusk"):
    """Requests per minute by kind: the relayer, one Dusk validator (mean of
    three) and the EVM validator."""
    mins = minutes(window)
    counts = collections.defaultdict(lambda: collections.Counter())
    replies = collections.Counter()
    durs = collections.defaultdict(list)
    for r in requests:
        if r.get("up") != up or "dur" not in r or r["agent"] not in AGENTS or not in_window(r["ts"], window):
            continue
        kind = r["kind"] + (" [dusk-tx]" if r.get("src") == "dusk-tx" else "")
        group = "validator-dusk" if r["agent"] in DUSK_VALIDATORS else r["agent"]
        counts[kind][group] += 1
        replies[kind] += r.get("rs", 0)
        durs[kind].append(r["dur"])
    groups = ["relayer", "validator-dusk"] if up == "rusk" else ["relayer", "validator-evm"]
    rows = []
    for kind in sorted(counts, key=lambda k: -sum(counts[k].values())):
        c = counts[kind]
        cells = []
        for g in groups:
            n = c.get(g, 0) / (3 if g == "validator-dusk" else 1)
            cells.append(f"{n / mins:.2f}" if n else "")
        total = sum(c.values())
        rows.append([f"`{kind}`"] + cells + [f"{replies[kind] / total:.0f}", fmt(statistics.mean(durs[kind]), 1)])
    labels = ["relayer", "each Dusk validator"] if up == "rusk" else ["relayer", "EVM validator"]
    out.append(f"### {title}\n")
    out.append(table(["Kind"] + [f"{l}, req/min" for l in labels] + ["reply bytes", "mean ms"], rows) + "\n")


def excess(requests, idle, window, n_msgs):
    """{(agent, up): (extra requests, extra bytes)} per message over the idle rate,
    and the same per kind."""
    base, load = usage(requests, idle), usage(requests, window)
    idle_min, load_min = minutes(idle), minutes(window)
    totals, kinds = {}, {}
    for key, a in load.items():
        b = base.get(key)
        bn = b["n"] / idle_min if b else 0
        bb = (b["rq"] + b["rs"]) / idle_min if b else 0
        totals[key] = ((a["n"] - bn * load_min) / n_msgs, (a["rq"] + a["rs"] - bb * load_min) / n_msgs)
        for kind, k in a["kinds"].items():
            bk = b["kinds"].get(kind) if b else None
            kn = bk["n"] / idle_min if bk else 0
            kb = (bk["rq"] + bk["rs"]) / idle_min if bk else 0
            kinds[key + (kind,)] = ((k["n"] - kn * load_min) / n_msgs,
                                    (k["rq"] + k["rs"] - kb * load_min) / n_msgs,
                                    statistics.mean(k["durs"]))
    return totals, kinds


def report_per_message_compact(out, requests, sets):
    """One table for both directions: requests and KiB per message over a
    baseline idle phase (one with the same Dusk history where there is one)."""
    out.append("## Per message, both directions\n")
    for label, _, n, base_name, _ in sets:
        out.append(f"- {label}: {n} messages, over the `{base_name}` rate")
    out.append("")
    results = {label: excess(requests, base, w, n) for label, w, n, _, base in sets}
    headers = ["Agent"]
    for label, *_ in sets:
        headers += [f"{label}: Rusk req", "Rusk KiB", "Anvil req", "Anvil KiB"]
    rows = []
    for agent in AGENTS + ["loadgen"]:
        row = [agent if agent != "loadgen" else "the sender (loadgen)"]
        for label, *_ in sets:
            totals = results[label][0]
            for up in ("rusk", "evm"):
                t = totals.get((agent, up))
                row += [f"{t[0]:.1f}", f"{t[1] / 1024:.1f}"] if t else ["", ""]
        rows.append(row)
    out.append(table(headers, rows) + "\n")
    for label, *_ in sets:
        kinds = results[label][1]
        rows = []
        for (agent, up, kind), (n, b, d) in sorted(kinds.items(), key=lambda kv: (kv[0][0], -kv[1][0])):
            if up == "rusk" and abs(n) >= 0.1 and agent != "harness":
                rows.append([agent, f"`{kind}`", f"{n:.1f}", f"{b / 1024:.2f}", fmt(d, 1)])
        out.append(f"### {label}: Rusk requests per message by kind\n")
        out.append(table(["Agent", "Kind", "req/msg", "KiB/msg", "mean ms"], rows) + "\n")


def report_config(out, run):
    """What the agents ran with: batched reads and the relayer's batch size."""
    rows = []
    for name in ["relayer"] + DUSK_VALIDATORS:
        try:
            with open(os.path.join(run, "agents", f"{name}.json")) as f:
                dusk = json.load(f).get("chains", {}).get("dusk", {})
        except (OSError, ValueError):
            continue
        rows.append([name, dusk.get("batchContractAddress") or "none (single queries)",
                     dusk.get("maxBatchSize", 1), dusk.get("contractQueriesPerSecond", "default")])
    if rows:
        out.append(table(["Agent", "batchContractAddress", "maxBatchSize", "contractQueriesPerSecond"],
                         rows) + "\n")


def report_refusals(out, requests, phases, order):
    """Requests Rusk refused for its rate limit (429), and other failures,
    per agent and phase."""
    out.append("## Rusk refusals\n")
    rows = []
    for agent in AGENTS + ["loadgen", "harness"]:
        row, seen = [agent], False
        for phase in order:
            w = phases.get(phase)
            rs = [r for r in requests if r.get("agent") == agent and r.get("up") == "rusk" and "dur" in r
                  and w and w[1] and in_window(r["ts"], w)]
            n429 = sum(1 for r in rs if r.get("st") == 429)
            other = sum(1 for r in rs if r.get("st", 200) >= 400 and r.get("st") != 429)
            seen = seen or n429 or other
            row.append(f"{n429} / {other}" if n429 or other else "0")
        if seen:
            rows.append(row)
    total = sum(1 for r in requests if r.get("up") == "rusk" and r.get("st") == 429)
    out.append(f"429s / other failed requests (5xx, and 502 from the proxy while a port is down). "
               f"429s over the whole run: {total}.\n")
    out.append((table(["Agent"] + order, rows) if rows else "None.") + "\n")


def report_aggregates(out, requests, window, title):
    """What the agents' Multicall for Dusk aggregates carried: aggregates and
    reads a minute, and the reads by kind (per Dusk validator: the mean of
    three)."""
    mins = minutes(window)
    groups = collections.defaultdict(lambda: {"n": 0, "calls": 0, "kinds": collections.Counter()})
    for r in requests:
        if r.get("kind") != "q:multicall.aggregate" or "dur" not in r or r["agent"] not in AGENTS:
            continue
        if not in_window(r["ts"], window):
            continue
        g = groups["validator-dusk" if r["agent"] in DUSK_VALIDATORS else r["agent"]]
        g["n"] += 1
        calls = [c for c in (r.get("det") or "").split(",") if c]
        g["calls"] += len(calls)
        g["kinds"].update(calls)
    if not groups:
        return
    out.append(f"### {title}: what the aggregates carried\n")
    for name, g in sorted(groups.items()):
        div = 3 if name == "validator-dusk" else 1
        label = "each Dusk validator" if name == "validator-dusk" else name
        out.append(f"**{label}**: {g['n'] / div / mins:.2f} aggregates a minute, {g['calls'] / div / mins:.2f} "
                   f"reads a minute, {g['calls'] / g['n']:.1f} reads per aggregate\n")
        rows = [[f"`{k}`", f"{n / div / mins:.2f}"] for k, n in g["kinds"].most_common(12)]
        out.append(table(["Read", "per minute"], rows) + "\n")


def report_deliveries_per_block(out, e2d, phases):
    """How the relayer's deliveries to Dusk shared blocks."""
    out.append("## Deliveries to Dusk per block\n")
    for phase, label in [("e2d", "paced"), ("e2d-burst", "burst"), ("outage", "outage")]:
        w = phases.get(phase)
        if not w or not w[1]:
            continue
        msgs = [m for m in e2d.values() if in_window(m["sent"], w)]
        per_block = collections.Counter(m["height"] for m in msgs if m.get("height") is not None)
        if not per_block:
            continue
        sizes = collections.Counter(per_block.values())
        times = sorted(m["included"] for m in msgs if m.get("included"))
        pipelined = sum(1 for m in msgs if m.get("pipelined"))
        out.append(f"- {label}: {len(msgs)} messages in {len(per_block)} blocks, at most {max(per_block.values())} "
                   f"in one; blocks by deliveries: "
                   + ", ".join(f"{n} deliver{'ies' if n > 1 else 'y'}: {c}" for n, c in sorted(sizes.items()))
                   + f"; {pipelined} sent pipelined"
                   + (f"; first to last inclusion {times[-1] - times[0]:.0f} s" if len(times) > 1 else ""))
        if phase == "e2d-burst":
            out.append(f"  - blocks: " + ", ".join(f"{h}: {n}" for h, n in sorted(per_block.items())))
    out.append("")


def report_gas(out, helper_calls, receipts, dusk_txs, mailbox_txs):
    """What the agents' transactions cost, in gas and in DUSK at the agents'
    gas price of 1 LUX."""
    out.append("## Gas the agents spent\n")
    rows = []
    for agent in AGENTS:
        for fn in sorted({c.get("fn") for c in helper_calls if c.get("agent") == agent and c.get("fn")}):
            spent = []
            for c in helper_calls:
                if c.get("agent") != agent or c.get("fn") != fn:
                    continue
                outcome = chain_outcome(c, dusk_txs)
                if outcome and outcome[0] is not None:
                    spent.append(outcome[1] or 0)
            if spent:
                rows.append([agent, f"Dusk `{fn}`", len(spent), fmt(sum(spent), 0), f"{sum(spent) / 1e9:.4f} DUSK"])
    gas = [int(r["gas"], 16) for r in receipts if r.get("gas")]
    if gas:
        rows.append(["relayer", "Anvil `process`", len(gas), fmt(sum(gas), 0), ""])
    out.append(table(["Agent", "Transactions", "In blocks", "Gas", "At 1 LUX a gas"], rows) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("run")
    parser.add_argument("--json")
    args = parser.parse_args()
    run = args.run
    deployment = json.load(open(os.path.join(run, "deploy", "deployment.json")))
    dusk_domain, evm_domain = deployment["dusk_domain"], deployment["evm_domain"]
    phases, events = load_phases(run)
    # requests.jsonl, plus any other proxy instance's log (requests-*.jsonl)
    requests = sorted((r for path in sorted(glob.glob(os.path.join(run, "logs", "requests*.jsonl")))
                       for r in jsonl(path)), key=lambda r: r.get("ts", 0))
    helper_calls = jsonl(os.path.join(run, "logs", "dusk-tx.jsonl"))
    load = jsonl(os.path.join(run, "logs", "load.jsonl"))
    resources = jsonl(os.path.join(run, "logs", "resources.jsonl"))
    heights = {r["nonce"]: r["height"] for r in jsonl(os.path.join(run, "logs", "dusk-dispatches.jsonl"))}
    dusk_txs = {t["id"]: t for t in jsonl(os.path.join(run, "logs", "dusk-txs.jsonl")) if t.get("id")}
    mailbox_txs = jsonl(os.path.join(run, "logs", "dusk-mailbox-txs.jsonl"))
    blocks = load_blocks(run)
    order = [p for p in ["baseline", "startup", "idle", "d2e", "d2e-backlog", "e2d", "e2d-burst",
                         "outage", "idle2"] if p in phases]
    out = ["# Agents benchmark report\n"]
    rows = [[p, fmt(minutes(phases[p]), 1)] for p in order if phases[p][1]]
    out.append(table(["Phase", "Minutes"], rows) + "\n")
    report_config(out, run)

    report_usage(out, requests, phases, order)
    report_refusals(out, requests, phases, order)
    report_idle_summary(out, requests, phases, [p for p in ("idle", "idle2") if p in phases])
    for phase in ("idle", "idle2"):
        if phases.get(phase, (None, None))[1]:
            report_kind_matrix(out, requests, phases[phase], f"{phase}: Rusk requests by kind")
            report_kind_matrix(out, requests, phases[phase], f"{phase}: Anvil requests by kind", up="evm")
            report_aggregates(out, requests, phases[phase], phase)
    if "idle" in phases:
        out.append("## Idle, by call kind\n")
        report_kinds(out, requests, phases["idle"], "Idle", AGENTS)
    if "idle2" in phases and phases["idle2"][1]:
        report_kinds(out, requests, phases["idle2"], "Idle after the load", AGENTS)

    d2e = dusk_to_evm(run, requests, load, blocks, dusk_domain, evm_domain, heights)
    e2d = evm_to_dusk(run, requests, load, helper_calls, blocks, dusk_domain, evm_domain, dusk_txs, mailbox_txs)
    n_d2e = sum(1 for m in d2e.values() if in_window(m["sent"], phases.get("d2e", (None, None))))
    n_e2d = sum(1 for m in e2d.values() if in_window(m["sent"], phases.get("e2d", (None, None))))
    # Dusk -> EVM grows the Dusk history from nothing, so it is measured over
    # the fresh idle phase. EVM -> Dusk leaves the Dusk history as it is, so
    # it is measured over idle2, which has the same history, when it ran.
    e2d_base = "idle2" if phases.get("idle2", (None, None))[1] else "idle"
    sets = [(label, phases[p], n, base, phases[base])
            for label, p, n, base in [("Dusk -> EVM", "d2e", n_d2e, "idle"),
                                      ("EVM -> Dusk", "e2d", n_e2d, e2d_base)]
            if n and phases.get(p, (None, None))[1]]
    if sets:
        report_per_message_compact(out, requests, sets)
    out.append("## Per message\n")
    if n_d2e:
        report_per_message(out, requests, helper_calls, phases["idle"], phases["d2e"], n_d2e,
                           "Dusk -> EVM")
        report_aggregates(out, requests, phases["d2e"], "Dusk -> EVM")
    if n_e2d:
        report_per_message(out, requests, helper_calls, phases[e2d_base], phases["e2d"], n_e2d,
                           f"EVM -> Dusk (over {e2d_base})")
        report_aggregates(out, requests, phases["e2d"], "EVM -> Dusk")

    out.append("## Latency per stage\n")
    d2e_stages = [("dispatched", "dispatch included"), ("origin_final", "finalized on Dusk"),
                  ("signed_1st", "1st validator signed"), ("signed_2nd", "2nd validator signed"),
                  ("signed_3rd", "3rd validator signed"), ("metadata_ready", "relayer metadata ready"),
                  ("submitted", "delivery submitted"), ("included", "included on Anvil")]
    e2d_stages = [("dispatched", "dispatch included"), ("signed_1st", "EVM validator signed"),
                  ("metadata_ready", "relayer metadata ready (simulation)"),
                  ("submitted", "dusk-tx started"), ("propagated", "transaction propagated"),
                  ("included", "included on Dusk"), ("dest_final", "finalized on Dusk")]
    for phase, msgs, stages, label in [
            ("d2e", d2e, d2e_stages, "Dusk -> EVM, back to back"),
            ("d2e-backlog", d2e, d2e_stages, "Dusk -> EVM, waiting for a stopped relayer"),
            ("e2d", e2d, e2d_stages, "EVM -> Dusk, paced"),
            ("e2d-burst", e2d, e2d_stages, "EVM -> Dusk, burst"),
            ("outage", e2d, e2d_stages, "EVM -> Dusk, sent while the agents could not reach Rusk")]:
        w = phases.get(phase)
        if w and w[1]:
            chosen = [m for m in msgs.values() if in_window(m["sent"], w)]
            report_latency(out, label, chosen, stages)
    retries = [m for m in e2d.values() if m.get("failed_helper_runs")]
    if retries:
        out.append(f"EVM -> Dusk messages whose delivery needed more than one dusk-tx run: {len(retries)} "
                   f"(failed runs: {sum(m['failed_helper_runs'] for m in retries)})\n")
    extra = [m for m in e2d.values() if m.get("reverted_txs") or m.get("other_txs")]
    if extra:
        out.append(f"EVM -> Dusk messages with more than one transaction: {len(extra)} "
                   f"({sum(m.get('reverted_txs', 0) for m in extra)} failed in a block, spending "
                   f"{fmt(sum(m.get('reverted_gas', 0) for m in extra), 0)} gas)\n")
    undelivered = [n for n, m in e2d.items() if m.get("height") is None]
    if undelivered:
        out.append(f"EVM -> Dusk messages with no delivery found on Dusk: {sorted(undelivered)[:20]}\n")
    gas = [m["gas"] for m in e2d.values() if m.get("gas")]
    if gas:
        out.append(f"Relayer `process` gas on Dusk: n={len(gas)}, median {fmt(pct(gas, 0.5), 0)}, "
                   f"max {fmt(max(gas), 0)}\n")

    report_deliveries_per_block(out, e2d, phases)
    receipts = jsonl(os.path.join(run, "logs", "evm-receipts.jsonl"))
    report_transactions(out, helper_calls, receipts, dusk_txs, mailbox_txs)
    report_gas(out, helper_calls, receipts, dusk_txs, mailbox_txs)

    out.append("## Node load by call kind\n")
    for phase, title in [("idle", "Idle"), ("d2e", "Dusk -> EVM"), ("e2d", "EVM -> Dusk"),
                         ("e2d-burst", "EVM -> Dusk burst"), ("idle2", "Idle after the load")]:
        w = phases.get(phase)
        if w and w[1]:
            report_node_load(out, requests, w, title)

    report_resources(out, resources, phases, order, None)
    windows = [(p, n) for p, n in [("d2e", n_d2e), ("e2d", n_e2d)] if n]
    report_growth(out, resources, phases, windows)
    report_logs(out, resources, phases, order)
    if "idle" in phases:
        report_recovery(out, requests, phases, events, helper_calls, d2e, e2d, phases["idle"], dusk_txs)

    print("\n".join(out))
    if args.json:
        with open(args.json, "w") as f:
            json.dump({"phases": phases, "d2e": d2e, "e2d": e2d}, f, indent=1, default=str)


if __name__ == "__main__":
    main()
