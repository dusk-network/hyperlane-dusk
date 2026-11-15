#!/usr/bin/env python3
"""Request-logging reverse proxy for the local agents benchmark.

Each listener forwards one agent's HTTP/1.1 traffic to one local upstream,
Rusk or Anvil, and appends a JSON line per request to the log: when it
started, how long the upstream took, which listener (agent) sent it, what it
was, its status, and the bytes each way. What it was:

- Rusk: `q:<contract>.<method>` for a contract query, `gql:<root field>` for
  GraphQL (`/on/graphql/query` or `/graphql`), `tx:preverify`/`tx:propagate`,
  `account:status`, otherwise the path with IDs masked.
- Anvil: `eth:<JSON-RPC method>`, with the contract and function for
  eth_call, eth_estimateGas and eth_sendRawTransaction, and the event for
  eth_getLogs.

A path that starts with /dusk-tx/ comes from an agent's dusk-tx helper
(dusk-tx-shim.sh adds the prefix); it is logged with "src": "dusk-tx" and
stripped before forwarding. Hyperlane messages found in a Dusk `process`
query or an EVM `process` call are logged as "<origin>:<nonce>".

Local only: listeners bind 127.0.0.1 and upstreams must be on localhost.

  proxy.py --log requests.jsonl --names names.json --admin 18099 \\
      --rusk http://127.0.0.1:8080 --evm http://127.0.0.1:8545 \\
      --listen relayer=18100:rusk --listen relayer=18200:evm ...

`names.json` maps contract IDs (64 hex) and EVM addresses to names. The admin
port takes GET /down?names=a,b, which closes those listeners and their open
connections, so the agent gets "connection refused" as from a stopped node,
and GET /up?names=a,b, which reopens them. `--pace relayer@rusk=150` holds
that listener's contract queries to 150 a second, below the 180 a second
Rusk's default HTTP policy allows all clients together; time spent waiting
is logged as "paced" (ms) and is not part of "dur".
"""

import argparse
import asyncio
import json
import re
import signal
import sys
import time
from urllib.parse import parse_qs, urlsplit

LOCAL_HOSTS = {"127.0.0.1", "localhost", "::1"}
STREAM_LIMIT = 8 * 1024 * 1024

# Function selectors and event topics the agents use on the EVM side.
SELECTORS = {
    "e495f1d4": "delivered", "affed0e0": "nonce", "134fbb4f": "latestDispatchedId",
    "8d3638f4": "localDomain", "6e5f516e": "defaultIsm", "3d1250b7": "defaultHook",
    "d6d08a09": "requiredHook", "e70f48ac": "recipientIsm", "7c39d130": "process",
    "07a2fda1": "processedAt", "5d1fe5a9": "processor", "82ea7bfe": "deployedBlock",
    "06661abd": "count", "ebf0c717": "root", "907c0f92": "latestCheckpoint",
    "fd54b228": "tree", "690cb786": "getAnnouncedValidators",
    "51abe7cc": "getAnnouncedStorageLocations", "21f71781": "announce",
    "6465e69f": "moduleType", "2e0ed234": "validatorsAndThreshold", "f7e83aee": "verify",
    "de523cf3": "interchainSecurityModule", "81b4e8b4": "transferRemote",
    "56d5d475": "handle", "70a08231": "balanceOf", "18160ddd": "totalSupply",
    "2ead72f6": "routers", "f2ed8c53": "quoteGasPayment", "a6929793": "quoteGasPayment",
    "82ad56cb": "aggregate3", "d5438eae": "mailbox", "7f5a7c7b": "hook", "8da5cb5b": "owner",
    "313ce567": "decimals", "9c42bd18": "quoteDispatch", "81d2ea95": "quoteDispatch",
    "440df4f4": "domains", "42cde4e8": "threshold", "ca1e7819": "validators",
}
TOPICS = {
    "769f711d20c679153d382254f59892613b58a97cc876b249134ac25c80f9c814": "Dispatch",
    "788dbc1b7152732178210e7f4d9d010ef016f9eafbe66786bd7169f56e0c353a": "DispatchId",
    "1cae38cdd3d3919489272725a5ae62a4f48b2989b0dae843d3c279fee18073a9": "ProcessId",
    "0d381c2a574ae8f04e213db7cfb4df8df712cdbd427d9868ffef380660ca6574": "Process",
    "253a3a04cab70d47c1504809242d9350cd81627b4f1d50753e159cf8cd76ed33": "InsertedIntoTree",
    "65695c3748edae85a24cc2c60b299b31f463050bc259150d2e5802ec8d11720a": "GasPayment",
    "78066d8adb677a1353d1fc8be28cf03e2a8de7157bbab979953587d78076c11e": "ValidatorAnnouncement",
}
GRAPHQL_ROOT = re.compile(r"^\s*(?:query\b[^{]*)?\{\s*([A-Za-z_]\w*)")
# Small replies worth keeping, to see when a delivery check first says yes.
KEEP_REPLY = {"q:mailbox.delivered", "eth_call:mailbox.delivered"}


def find_messages(data, domains):
    """Yield "<origin>:<nonce>" for each Hyperlane message header in `data`
    whose origin and destination are two of the known domains."""
    found = []
    for i in range(len(data) - 45):
        if data[i] != 3:
            continue
        origin = int.from_bytes(data[i + 5:i + 9], "big")
        destination = int.from_bytes(data[i + 41:i + 45], "big")
        if origin in domains and destination in domains and origin != destination:
            found.append(f"{origin}:{int.from_bytes(data[i + 1:i + 5], 'big')}")
    return found


def rlp_item(data, pos):
    """Decode one RLP item at `pos`: (value, next position). Lists are lists."""
    b = data[pos]
    if b < 0x80:
        return data[pos:pos + 1], pos + 1
    if b < 0xb8:
        n = b - 0x80
        return data[pos + 1:pos + 1 + n], pos + 1 + n
    if b < 0xc0:
        ln = b - 0xb7
        n = int.from_bytes(data[pos + 1:pos + 1 + ln], "big")
        start = pos + 1 + ln
        return data[start:start + n], start + n
    if b < 0xf8:
        n, start = b - 0xc0, pos + 1
    else:
        ln = b - 0xf7
        n = int.from_bytes(data[pos + 1:pos + 1 + ln], "big")
        start = pos + 1 + ln
    items, p = [], start
    while p < start + n:
        item, p = rlp_item(data, p)
        items.append(item)
    return items, start + n


def raw_tx_to_and_data(raw):
    """`to` and calldata of a signed legacy or typed (EIP-2718) transaction."""
    if raw and raw[0] < 0x7f:
        fields, _ = rlp_item(raw, 1)
        # 1: [chainId, nonce, gasPrice, gas, to, value, data, ...]
        # 2: [chainId, nonce, maxPriority, maxFee, gas, to, value, data, ...]
        to_index = {1: 4, 2: 5}.get(raw[0], 5)
    else:
        fields, _ = rlp_item(raw, 0)
        to_index = 3  # [nonce, gasPrice, gas, to, value, data, v, r, s]
    return fields[to_index], fields[to_index + 2]


def abi_bytes(data, index):
    """The `index`th dynamic `bytes` argument of ABI-encoded call data."""
    offset = int.from_bytes(data[32 * index:32 * index + 32], "big")
    length = int.from_bytes(data[offset:offset + 32], "big")
    return data[offset + 32:offset + 32 + length]


class Classifier:
    def __init__(self, names, domains):
        self.names = {k.lower().removeprefix("0x"): v for k, v in names.items()}
        self.domains = domains

    def name(self, ident):
        ident = (ident or "").lower().removeprefix("0x")
        return self.names.get(ident, ident[:8] or "?")

    def rusk(self, path, body):
        """(kind, detail, messages, keep_body) of a Rusk request."""
        path = path.split("?", 1)[0]
        m = re.match(r"^/on/contracts:([0-9a-fA-F]{64})/([^/]+)$", path)
        if m:
            kind = f"q:{self.name(m.group(1))}.{m.group(2)}"
            msgs = find_messages(body, self.domains) if m.group(2) == "process" else []
            return kind, "", msgs, kind in KEEP_REPLY
        if path in ("/on/graphql/query", "/graphql"):
            text = body.decode("utf-8", "replace")
            if text.lstrip().startswith("{"):
                try:
                    text = json.loads(text).get("query", "")
                except ValueError:
                    pass
            root = GRAPHQL_ROOT.match(text)
            kind = f"gql:{root.group(1) if root else '?'}"
            detail = ""
            contract = re.search(r'contractId:\s*"([0-9a-fA-F]{64})"', text)
            if contract:
                detail = self.name(contract.group(1))
                limit = re.search(r"limit:\s*(\d+)", text)
                if limit:
                    detail += f" limit={limit.group(1)}"
                if "cursor" in text:
                    detail += " cursor"
            return kind, detail, [], False
        m = re.match(r"^/on/transactions/(\w+)$", path)
        if m:
            return f"tx:{m.group(1)}", "", [], False
        if re.match(r"^/on/account:[^/]+/status$", path):
            return "account:status", "", [], False
        masked = re.sub(r"[0-9a-fA-F]{64}", "<id>", path)
        return masked, "", [], False

    def evm_one(self, call):
        """(kind, messages, extra fields) of one JSON-RPC call."""
        method = call.get("method", "?")
        params = call.get("params") or []
        kind, msgs, extra = f"eth:{method}", [], {}
        try:
            if method in ("eth_call", "eth_estimateGas", "eth_sendRawTransaction") and params:
                if method == "eth_sendRawTransaction":
                    to, data = raw_tx_to_and_data(bytes.fromhex(params[0][2:]))
                    to = to.hex()
                else:
                    to = params[0].get("to")
                    data = bytes.fromhex((params[0].get("data") or params[0].get("input") or "0x")[2:])
                fn = SELECTORS.get(data[:4].hex(), data[:4].hex() or "transfer")
                kind = f"{method}:{self.name(to)}.{fn}"
                if fn == "process":
                    msgs = find_messages(data, self.domains)
                    if method == "eth_sendRawTransaction":
                        extra["msgdata"] = abi_bytes(data[4:], 1).hex()
                elif fn == "delivered":
                    extra["id"] = data[4:36].hex()
            elif method == "eth_getLogs" and params:
                flt = params[0]
                topics = flt.get("topics") or []
                topic = topics[0] if topics else None
                if isinstance(topic, list):
                    topic = topic[0] if topic else None
                event = TOPICS.get((topic or "").lower().removeprefix("0x"), "any")
                address = flt.get("address")
                if isinstance(address, list):
                    address = address[0] if address else None
                kind = f"eth_getLogs:{self.name(address)}.{event}"
        except (ValueError, IndexError, KeyError, AttributeError, TypeError):
            pass
        return kind, msgs, extra

    def evm(self, body):
        """(kind, detail, messages, keep_reply, extra fields) of an Anvil request."""
        try:
            payload = json.loads(body)
        except ValueError:
            return "eth:?", "", [], False, {}
        if isinstance(payload, list):
            kinds, msgs = [], []
            for call in payload:
                k, m, _ = self.evm_one(call)
                kinds.append(k)
                msgs += m
            return f"eth:batch[{len(payload)}]", ",".join(kinds), msgs, False, {}
        kind, msgs, extra = self.evm_one(payload)
        return kind, "", msgs, kind in KEEP_REPLY, extra


async def read_head(reader):
    head = await reader.readuntil(b"\r\n\r\n")
    lines = head[:-4].split(b"\r\n")
    headers = []
    for line in lines[1:]:
        name, _, value = line.partition(b":")
        headers.append((name.strip().decode("latin-1"), value.strip().decode("latin-1")))
    return lines[0].decode("latin-1"), headers, len(head)


def header(headers, name):
    for key, value in headers:
        if key.lower() == name:
            return value
    return None


async def read_body(reader, headers, until_eof):
    """The body as sent (for forwarding) and decoded (for classifying)."""
    if "chunked" in (header(headers, "transfer-encoding") or "").lower():
        raw, body = bytearray(), bytearray()
        while True:
            line = await reader.readuntil(b"\r\n")
            raw += line
            size = int(line.split(b";")[0].strip(), 16)
            if size == 0:
                while True:
                    trailer = await reader.readuntil(b"\r\n")
                    raw += trailer
                    if trailer == b"\r\n":
                        return bytes(raw), bytes(body)
            chunk = await reader.readexactly(size + 2)
            raw += chunk
            body += chunk[:-2]
    length = header(headers, "content-length")
    if length is not None:
        data = await reader.readexactly(int(length)) if int(length) else b""
        return data, data
    if until_eof:
        data = await reader.read()
        return data, data
    return b"", b""


def build_head(start, headers):
    lines = [start] + [f"{k}: {v}" for k, v in headers]
    return ("\r\n".join(lines) + "\r\n\r\n").encode("latin-1")


class TokenBucket:
    """Holds requests back to `rate` a second, `rate` at once at most."""

    def __init__(self, rate):
        self.rate = float(rate)
        self.tokens = self.rate
        self.stamp = time.monotonic()
        self.lock = asyncio.Lock()

    async def take(self):
        async with self.lock:
            while True:
                now = time.monotonic()
                self.tokens = min(self.rate, self.tokens + (now - self.stamp) * self.rate)
                self.stamp = now
                if self.tokens >= 1:
                    self.tokens -= 1
                    return
                await asyncio.sleep((1 - self.tokens) / self.rate)


class Listener:
    def __init__(self, proxy, name, port, upstream, pace=None):
        self.proxy, self.name, self.port, self.upstream = proxy, name, port, upstream
        self.server = None
        self.writers = set()
        # Contract queries only: the class Rusk limits most tightly.
        self.bucket = TokenBucket(pace) if pace else None

    async def open(self):
        if self.server is None:
            self.server = await asyncio.start_server(
                self.serve, "127.0.0.1", self.port, limit=STREAM_LIMIT)

    async def close(self):
        if self.server is not None:
            self.server.close()
            self.server = None
        for writer in list(self.writers):
            writer.close()
        self.writers.clear()

    async def serve(self, reader, writer):
        self.writers.add(writer)
        proxy = self.proxy
        conn = proxy.next_conn()
        kind_up, host, port = self.upstream
        up_reader = up_writer = None
        try:
            while True:
                try:
                    start, headers, head_len = await read_head(reader)
                except (asyncio.IncompleteReadError, ConnectionError):
                    return
                t0 = time.time()
                method, target, version = start.split(" ", 2)
                raw_body, body = await read_body(reader, headers, until_eof=False)
                src = "agent"
                if target.startswith("/dusk-tx/"):
                    src, target = "dusk-tx", target[len("/dusk-tx"):]
                headers = [(k, f"{host}:{port}" if k.lower() == "host" else v) for k, v in headers]
                request = build_head(f"{method} {target} {version}", headers) + raw_body
                extra = {}
                if kind_up == "rusk":
                    kind, detail, msgs, keep = proxy.classifier.rusk(target, body)
                else:
                    kind, detail, msgs, keep, extra = proxy.classifier.evm(body)
                record = {"ts": round(t0, 4), "agent": self.name, "src": src, "up": kind_up,
                          "kind": kind, "rq": len(request), "rqb": len(body), "conn": conn}
                if detail:
                    record["det"] = detail
                if msgs:
                    record["msg"] = msgs
                record.update(extra)
                if keep and kind_up == "rusk" and len(body) <= 256:
                    record["rqhex"] = body.hex()
                if self.bucket and target.startswith("/on/contracts:"):
                    waited = time.time()
                    await self.bucket.take()
                    waited = time.time() - waited
                    if waited >= 0.001:
                        record["paced"] = round(waited * 1000, 1)
                        t0 = time.time()
                response = None
                for attempt in (0, 1):
                    reused = up_writer is not None
                    try:
                        if up_writer is None:
                            up_reader, up_writer = await asyncio.open_connection(
                                host, port, limit=STREAM_LIMIT)
                        up_writer.write(request)
                        await up_writer.drain()
                        status_line, rheaders, rhead_len = await read_head(up_reader)
                        status = int(status_line.split(" ", 2)[1])
                        no_body = method == "HEAD" or status in (204, 304) or status < 200
                        if no_body:
                            raw_resp, resp_body = b"", b""
                        else:
                            raw_resp, resp_body = await read_body(up_reader, rheaders, until_eof=True)
                        response = (status_line, rheaders, raw_resp, resp_body, status)
                        break
                    except (asyncio.IncompleteReadError, ConnectionError, OSError) as error:
                        if up_writer is not None:
                            up_writer.close()
                        up_reader = up_writer = None
                        if attempt == 0 and reused:
                            continue  # a kept-alive upstream connection had closed
                        record["err"] = type(error).__name__
                        break
                if response is None:
                    reply = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    writer.write(reply)
                    await writer.drain()
                    record.update(dur=round((time.time() - t0) * 1000, 2), st=502,
                                  rs=len(reply), rsb=0)
                    proxy.log(record)
                    return
                status_line, rheaders, raw_resp, resp_body, status = response
                reply = build_head(status_line, rheaders) + raw_resp
                writer.write(reply)
                await writer.drain()
                record.update(dur=round((time.time() - t0) * 1000, 2), st=status,
                              rs=len(reply), rsb=len(resp_body))
                if keep and len(resp_body) <= 256:
                    if kind_up == "rusk":
                        record["rshex"] = resp_body.hex()
                    else:
                        try:
                            record["rshex"] = json.loads(resp_body).get("result", "")
                        except (ValueError, AttributeError):
                            pass
                if kind_up == "evm" and kind.startswith("eth_sendRawTransaction"):
                    try:
                        record["txhash"] = json.loads(resp_body).get("result")
                    except (ValueError, AttributeError):
                        pass
                proxy.log(record)
                close = ((header(headers, "connection") or "").lower() == "close"
                         or (header(rheaders, "connection") or "").lower() == "close"
                         or version == "HTTP/1.0")
                if close or (header(rheaders, "content-length") is None
                             and "chunked" not in (header(rheaders, "transfer-encoding") or "")):
                    if up_writer is not None:
                        up_writer.close()
                    up_reader = up_writer = None
                    if close:
                        return
        except (ConnectionError, asyncio.IncompleteReadError, asyncio.LimitOverrunError) as error:
            proxy.log({"ts": round(time.time(), 4), "agent": self.name, "kind": "proxy:error",
                       "err": type(error).__name__, "conn": conn})
        finally:
            self.writers.discard(writer)
            writer.close()
            if up_writer is not None:
                up_writer.close()


class Proxy:
    def __init__(self, args):
        names = {}
        if args.names:
            with open(args.names) as f:
                names = json.load(f)
        self.classifier = Classifier(names, set(args.domain))
        self.out = open(args.log, "a", buffering=1 << 16)
        self.conns = 0
        upstreams = {}
        for kind, url in (("rusk", args.rusk), ("evm", args.evm)):
            parts = urlsplit(url)
            if parts.hostname not in LOCAL_HOSTS:
                sys.exit(f"refusing non-local upstream {url}")
            upstreams[kind] = (kind, parts.hostname, parts.port or 80)
        pace = {}
        for spec in args.pace:
            key, _, rate = spec.partition("=")
            pace[key] = float(rate)
        self.listeners = {}
        for spec in args.listen:
            name, _, rest = spec.partition("=")
            port, _, kind = rest.partition(":")
            key = f"{name}@{kind}"
            self.listeners[key] = Listener(self, name, int(port), upstreams[kind], pace.get(key))
        self.admin_port = args.admin

    def next_conn(self):
        self.conns += 1
        return self.conns

    def log(self, record):
        self.out.write(json.dumps(record, separators=(",", ":")) + "\n")

    def select(self, query):
        wanted = set(filter(None, parse_qs(query).get("names", [""])[0].split(",")))
        return [l for key, l in self.listeners.items() if not wanted or l.name in wanted or key in wanted]

    async def admin(self, reader, writer):
        try:
            start, _, _ = await read_head(reader)
            path = start.split(" ")[1]
            parts = urlsplit(path)
            chosen = self.select(parts.query)
            if parts.path == "/down":
                for listener in chosen:
                    await listener.close()
            elif parts.path == "/up":
                for listener in chosen:
                    await listener.open()
            names = ",".join(sorted({l.name + "@" + l.upstream[0] for l in chosen}))
            self.log({"ts": round(time.time(), 4), "agent": "admin", "kind": f"admin:{parts.path}",
                      "det": names})
            body = (names + "\n").encode()
            writer.write(b"HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n"
                         % len(body) + body)
            await writer.drain()
        finally:
            writer.close()

    async def flusher(self):
        while True:
            await asyncio.sleep(1)
            self.out.flush()

    async def run(self):
        for listener in self.listeners.values():
            await listener.open()
        if self.admin_port:
            await asyncio.start_server(self.admin, "127.0.0.1", self.admin_port)
        stop = asyncio.Event()
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, stop.set)
        flush = asyncio.create_task(self.flusher())
        print(f"proxy listening: {', '.join(f'{k}:{l.port}' for k, l in self.listeners.items())}",
              flush=True)
        await stop.wait()
        flush.cancel()
        self.out.flush()


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--log", required=True)
    parser.add_argument("--names")
    parser.add_argument("--rusk", default="http://127.0.0.1:8080")
    parser.add_argument("--evm", default="http://127.0.0.1:8545")
    parser.add_argument("--domain", type=int, action="append", default=[],
                        help="Hyperlane domain to recognise in messages (repeat)")
    parser.add_argument("--listen", action="append", default=[], metavar="NAME=PORT:rusk|evm")
    parser.add_argument("--admin", type=int)
    parser.add_argument("--pace", action="append", default=[], metavar="NAME@rusk=RPS",
                        help="hold one listener's contract queries to RPS a second (repeat)")
    asyncio.run(Proxy(parser.parse_args()).run())


if __name__ == "__main__":
    main()
