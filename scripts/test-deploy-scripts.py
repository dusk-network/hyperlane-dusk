#!/usr/bin/env python3
"""Run deploy/testnet.sh and demo/gen-agent-configs.sh against stubbed tools.

No network, chain or Rust build is needed: dusk-tx, cast, curl and forge are
replaced by stubs. The dusk-tx stub answers queries from a fixture and
records what the transactions it is asked for would change; cast and forge
run against a small fake EVM chain with Hyperlane's contracts, kept in a JSON
file. A test that needs the real dusk-tx skips itself unless DUSK_TX_BIN or
target/release/dusk-tx exists.
"""
import base64
from fractions import Fraction
import json
from math import ceil
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TESTNET_SH = Path(os.environ.get("HYPERLANE_TESTNET_SH", ROOT / "deploy" / "testnet.sh"))
GEN_AGENT_CONFIGS_SH = Path(
    os.environ.get("HYPERLANE_GEN_AGENT_CONFIGS_SH", ROOT / "demo" / "gen-agent-configs.sh")
)

ZERO = "00" * 32
ROUTE = "e1" * 32
MAILBOX = "a1" * 32
IGP = "a2" * 32
VALIDATOR_ANNOUNCE = "a3" * 32
MERKLE_TREE_HOOK = "a4" * 32
PROTOCOL_FEE = "a5" * 32
MULTISIG = "b1" * 32
OTHER_MULTISIG = "b2" * 32
TEST_MOCK = "c1" * 32
DEPLOYER_H256 = "d0" * 32
DUSK_DOMAIN = 1146442572
EVM_DOMAIN = 11155111
VALIDATOR = "0x3c659e0fe8d01b80d7828b421630085777346e7c"

# Sepolia's Hyperlane contracts, as in deploy/testnet.env.example.
EVM_MAILBOX = "0xffaef09b3cd11d9b20d1a19becca54eec2884766"
EVM_MERKLE = "0x4917a9746a7b6e0a57159ccb7f5a6744247f2d0d"
EVM_VA = "0xe6105c59480a1b7dd3e4f28153afdbe12f4cfcd9"
EVM_ISM_FACTORY = "0xfeb9585b2f948c1ed74034205a7439261a9d27dd"
EVM_HOOK_FACTORY = "0xaedcf9c2bb12d9bb681cc55dbfe9ff30b2de8d77"
EVM_PROTOCOL_FEE = "0x13ac3349cb159fe86a22cf42dda803d9f7309db5"
EVM_DEFAULT_HOOK = "0x17dc724b7a2f09141c13b8ac33b396073785c2bc"
DEPLOYER_KEY = "0x" + "11" * 32
DEPLOYER = "0x000000000000000000000000000000000000dead"

# What deploy/testnet.env.example's defaults charge (docs/costs.md).
DEFAULT_DUSK_IGP = {"gas_overhead": "200000", "token_exchange_rate": "375000", "gas_price": "1000000000"}
DEFAULT_EVM_IGP = {"gas_overhead": "2170000", "token_exchange_rate": "600000000000000", "gas_price": "1"}


def validators(count):
    return [f"0x{'ab' * 19}{index:02x}" for index in range(1, count + 1)]


# Answers the dusk-tx subcommands the scripts use from FIXTURE_CHAIN, and
# writes back what the transactions it is asked for would change. `query`
# looks up "<contract>/<method>" (plus "/<arg>" for --arg-u32 and --arg-u64);
# a missing entry, or one with "error", fails as dusk-tx does.
DUSK_TX_STUB = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
with open(os.environ["FIXTURE_TRACE"], "a") as trace:
    trace.write("dusk-tx " + " ".join(args) + "\n")
def opt(name):
    return args[args.index(name) + 1] if name in args else None
def out(value, code=0):
    print(json.dumps(value))
    sys.exit(code)
path = os.environ["FIXTURE_CHAIN"]
chain = json.load(open(path))
queries = chain.setdefault("queries", {})
def save():
    with open(path, "w") as f:
        json.dump(chain, f, indent=1)
def set_pricing(igp, config):
    domain, overhead, rate, price = (int(part) for part in config.split(":"))
    queries[f"{igp}/domain_gas_config/{domain}"] = {"value": {
        "gas_overhead": overhead, "token_exchange_rate": rate, "gas_price": price}}
if args[0] == "query":
    key = f"{opt('--contract')}/{opt('--method')}"
    for flag in ("--arg-u32", "--arg-u64"):
        if opt(flag) is not None:
            key += "/" + opt(flag)
    if opt("--method") == "quote_gas_payment":
        config = queries.get(f"{opt('--contract')}/domain_gas_config/{opt('--arg-u32')}", {}).get("value")
        if config and config["gas_price"]:
            gas = int(opt("--arg-u64")) + config["gas_overhead"]
            out({"success": True, "value": gas * config["gas_price"] * config["token_exchange_rate"] // 10**10})
        out({"success": False, "error": "IGP: destination is not configured"}, 1)
    entry = queries.get(key)
    if entry is None and opt("--method") == "enrolled_router":
        entry = {"value": "00" * 32}
    if entry is None or "error" in entry:
        error = (entry or {}).get("error", f"Query {key} failed (500): ContractDoesNotExist")
        out({"success": False, "error": error}, 1)
    out({"success": True, "value": entry["value"]})
if args[0] == "account":
    if "--offline" in args:
        out({"success": True, "account_h256": chain.get("deployer_h256")})
    out({"success": True, "address": "fixture-deployer", "balance": "500000000000",
         "balance_dusk": "500.000000000"})
if args[0] == "deploy-hyperlane":
    contracts = chain["deploy"]
    set_pricing(contracts["igp"], opt("--igp-domain-config"))
    save()
    out({"success": True, "contracts": contracts})
if args[0] == "enroll-router":
    queries[f"{opt('--warp-contract')}/enrolled_router/{opt('--domain')}"] = {"value": opt("--router")}
    save()
    out({"success": True, "tx_id": "ab" * 32})
if args[0] == "set-igp-config":
    set_pricing(opt("--igp"), opt("--igp-domain-config"))
    save()
    out({"success": True, "tx_id": "cd" * 32, "block_height": 7})
if args[0] == "set-beneficiary":
    queries[f"{opt('--contract')}/beneficiary"] = {"value": "be" * 32}
    save()
    out({"success": True, "tx_id": "ce" * 32})
if args[0] == "claim-fees":
    key = f"{opt('--contract')}/claimable_fees"
    claimed = queries[key]["value"]
    queries[key] = {"value": 0}
    save()
    out({"success": True, "claimed": str(claimed), "claimed_dusk": f"{claimed / 1e9:.9f}",
         "tx_id": "cf" * 32})
out({"success": False, "error": f"stub dusk-tx: unexpected {args[0]}"}, 1)
'''

# A fake EVM chain for cast and forge: contracts are JSON objects in
# FIXTURE_EVM, calls read them and sends change them, as far as the scripts
# use Hyperlane's Mailbox, hooks, IGP, gas oracle, factories and HypERC20.
FAKE_EVM = r'''
import hashlib, json, os, re, sys

ZERO = "0x" + "00" * 20
SCALE = 10**10

def load():
    with open(os.environ["FIXTURE_EVM"]) as f:
        return json.load(f)

def save(state):
    with open(os.environ["FIXTURE_EVM"], "w") as f:
        json.dump(state, f, indent=1)

def trace(line):
    with open(os.environ["FIXTURE_TRACE"], "a") as f:
        f.write(line + "\n")

def die(message):
    print(message, file=sys.stderr)
    sys.exit(1)

def num(value):
    # cast adds the scientific form to large numbers.
    return f"{value} [{value:.4e}]" if value >= 10**6 else str(value)

def derive(*parts):
    return "0x" + hashlib.sha256(":".join(map(str, parts)).encode()).hexdigest()[:40]

def split(args):
    positional, options, flags = [], {}, set()
    i = 0
    while i < len(args):
        arg = args[i]
        if arg in ("--rpc-url", "--private-key", "--value"):
            options[arg] = args[i + 1]
            i += 2
        elif arg == "--constructor-args":
            options[arg] = args[i + 1:]
            break
        elif arg.startswith("--"):
            flags.add(arg)
            i += 1
        else:
            positional.append(arg)
            i += 1
    return positional, options, flags

def tuples(text):
    return [part.split(",") for part in re.findall(r"\(([^()]*(?:\([^()]*\))?[^()]*)\)", text)]

def contract(state, address):
    found = state["contracts"].get(address.lower())
    if found is None:
        die(f"Error: no contract at {address}")
    return found

def hook_quote(state, hook, domain, gas_limit):
    if hook.lower() == ZERO:
        return 0
    c = contract(state, hook)
    if c["type"] == "aggregation":
        return sum(hook_quote(state, child, domain, gas_limit) for child in c["hooks"])
    if c["type"] == "igp":
        oracle, overhead = c["configs"].get(str(domain), [ZERO, 0])
        if oracle == ZERO:
            die("execution reverted: IGP: no gas oracle for domain")
        rate, price = contract(state, oracle)["data"].get(str(domain), [0, 0])
        return (gas_limit + overhead) * price * rate // SCALE
    return c.get("fee", 0)

def call(state, address, signature, args):
    c = contract(state, address)
    name = signature.split("(")[0]
    if name in ("hookType", "localDomain"):
        return str(c["hook_type" if name == "hookType" else "domain"])
    if name in ("mailbox", "requiredHook", "defaultHook", "beneficiary"):
        return c[name]
    if name == "owner":
        return c.get("owner", ZERO)
    if name == "hook":
        return c.get("hook", ZERO)
    if name == "remoteGasData":
        rate, price = c["data"].get(args[0], [0, 0])
        return f"{num(rate)}\n{num(price)}"
    if name == "destinationGasConfigs":
        oracle, overhead = c["configs"].get(args[0], [ZERO, 0])
        return f"{oracle}\n{num(overhead)}"
    if name == "quoteGasPayment" and c["type"] == "igp":
        oracle, overhead = c["configs"][args[0]]
        rate, price = contract(state, oracle)["data"][args[0]]
        return num(int(args[1]) * price * rate // SCALE)
    if name == "quoteGasPayment" and c["type"] == "token":
        mailbox = contract(state, c["mailbox"])
        return num(hook_quote(state, mailbox["requiredHook"], args[0], 0)
                   + hook_quote(state, c["hook"], args[0], 0))
    if name == "hooks":
        return "[" + ", ".join(c["hooks"]) + "]"
    if name == "getAddress" and c["type"] == "aggregation_factory":
        return derive("aggregation", address.lower(), args[0].lower())
    if name == "getAddress" and c["type"] == "ism_factory":
        return derive("ism", address.lower(), args[0].lower(), args[1])
    if name == "routers":
        return c["routers"].get(args[0], "0x" + "00" * 32)
    die(f"stub cast: unexpected call {signature} on a {c['type']}")

def only_owner(c, sender):
    if c.get("owner", ZERO).lower() != sender.lower():
        die("execution reverted: Ownable: caller is not the owner")

def send(state, address, signature, args, sender):
    c = contract(state, address)
    name = signature.split("(")[0]
    if name == "setRemoteGasDataConfigs":
        only_owner(c, sender)
        for domain, rate, price in tuples(args[0]):
            c["data"][domain] = [int(rate), int(price)]
    elif name == "initialize" and c["type"] == "igp":
        if c["owner"] != ZERO:
            die("execution reverted: Initializable: contract is already initialized")
        c["owner"], c["beneficiary"] = args[0].lower(), args[1].lower()
    elif name == "setBeneficiary":
        only_owner(c, sender)
        c["beneficiary"] = args[0].lower()
    elif name == "setDestinationGasConfigs":
        only_owner(c, sender)
        for domain, oracle_and_overhead in ((t[0], t[1:]) for t in tuples(args[0])):
            oracle, overhead = (part.strip("() ") for part in oracle_and_overhead)
            c["configs"][domain] = [oracle.lower(), int(overhead)]
    elif name == "claim":
        balance = state["balances"].pop(address.lower(), 0)
        state["balances"][c["beneficiary"]] = state["balances"].get(c["beneficiary"], 0) + balance
    elif name == "deploy" and c["type"] == "aggregation_factory":
        hooks = [h.strip().lower() for h in args[0].strip("[]").split(",")]
        state["contracts"][derive("aggregation", address.lower(), args[0].lower())] = {
            "type": "aggregation", "hook_type": 2, "hooks": hooks}
    elif name == "deploy" and c["type"] == "ism_factory":
        state["contracts"][derive("ism", address.lower(), args[0].lower(), args[1])] = {"type": "ism"}
    elif name == "initialize" and c["type"] == "token":
        c.update(hook=args[3].lower(), ism=args[4].lower(), owner=args[5].lower())
    elif name == "setHook":
        only_owner(c, sender)
        c["hook"] = args[0].lower()
    elif name == "enrollRemoteRouter":
        only_owner(c, sender)
        c["routers"][args[0]] = args[1].lower()
    else:
        die(f"stub cast: unexpected send {signature} to a {c['type']}")

CREATED = {
    "StorageGasOracle": lambda sender, args: {"type": "oracle", "owner": sender, "data": {}},
    "InterchainGasPaymaster": lambda sender, args: {
        "type": "igp", "hook_type": 4, "owner": ZERO, "beneficiary": ZERO, "configs": {}},
    "StaticAggregationHookFactory": lambda sender, args: {"type": "aggregation_factory"},
    "HypERC20": lambda sender, args: {
        "type": "token", "mailbox": args[3].lower(), "hook": ZERO, "owner": ZERO, "routers": {}},
    "StorageMessageIdMultisigIsm": lambda sender, args: {"type": "ism"},
}

def main_cast(argv):
    state = load()
    positional, options, flags = split(argv)
    trace("cast " + " ".join(positional))
    command = positional[0]
    sender = state["keys"].get(options.get("--private-key", ""), ZERO)
    if command == "chain-id":
        print(state["chain_id"])
    elif command == "block-number":
        print(state["block"])
    elif command == "code":
        print("0x6080" if positional[1].lower() in state["contracts"] else "0x")
    elif command == "balance":
        wei = state["balances"].get(positional[1].lower(), 0)
        print(f"{wei / 10**18:.18f}" if "--ether" in flags else wei)
    elif command == "wallet":
        print(state["keys"][options["--private-key"]])
    elif command == "call":
        print(call(state, positional[1], positional[2], positional[3:]))
    elif command == "send":
        send(state, positional[1], positional[2], positional[3:], sender)
        state["block"] += 1
        save(state)
    else:
        die(f"stub cast: unexpected {command}")

def main_forge(argv):
    state = load()
    positional, options, flags = split(argv)
    trace("forge " + " ".join(positional + options.get("--constructor-args", [])))
    name = positional[1].split(":")[-1]
    sender = state["keys"][options["--private-key"]]
    state["nonce"] += 1
    address = derive("create", sender, state["nonce"])
    created = CREATED[name](sender, options.get("--constructor-args", []))
    if name == "InterchainGasPaymaster" and state.get("front_run"):
        created.update(owner=state["front_run"], beneficiary=state["front_run"])
    created["created_at"] = state["block"]
    state["contracts"][address] = created
    state["block"] += 1
    save(state)
    print(json.dumps({"deployedTo": address}))
'''

CAST_STUB = """#!/usr/bin/env python3
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fake_evm
fake_evm.main_cast(sys.argv[1:])
"""

FORGE_STUB = CAST_STUB.replace("main_cast", "main_forge")

# The RUES endpoints preflight reads: node info, a block height, the chain
# ID (one raw byte) and an archive query.
CURL_STUB = r'''#!/bin/sh
url=""
data=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --data) data="$2"; shift 2 ;;
        -m|-X|-H) shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
case "$url" in
    */on/node/info) echo '{"version_build":"fixture","vm_config":{"features":{"HQ_KECCAK256":0,"HQ_SECP256K1_RECOVER":0}}}' ;;
    */chain_id) printf '\002' ;;
    */on/graphql/query)
        case "$data" in
            *finalizedEvents*) echo '{"finalizedEvents":{"json":{"events":[]}}}' ;;
            *) echo '{"block":{"header":{"height":100}}}' ;;
        esac ;;
    *) exit 22 ;;
esac
'''


def write_executable(path, text):
    path.write_text(text)
    path.chmod(0o700)


def sepolia():
    """Sepolia's Hyperlane core as the fake EVM holds it: the Mailbox
    requires a ProtocolFee hook, and its default hook falls back to the
    MerkleTreeHook for Dusk."""
    return {
        "chain_id": EVM_DOMAIN, "block": 100, "nonce": 0, "front_run": None,
        "keys": {DEPLOYER_KEY: DEPLOYER},
        "balances": {DEPLOYER: 10**18},
        "contracts": {
            EVM_MAILBOX: {"type": "mailbox", "domain": EVM_DOMAIN,
                          "requiredHook": EVM_PROTOCOL_FEE, "defaultHook": EVM_DEFAULT_HOOK},
            EVM_MERKLE: {"type": "merkle", "hook_type": 3, "mailbox": EVM_MAILBOX},
            EVM_VA: {"type": "validator_announce", "mailbox": EVM_MAILBOX},
            EVM_PROTOCOL_FEE: {"type": "protocol_fee", "hook_type": 6, "fee": 1},
            EVM_DEFAULT_HOOK: {"type": "fallback_routing", "hook_type": 5},
            EVM_ISM_FACTORY: {"type": "ism_factory"},
            EVM_HOOK_FACTORY: {"type": "aggregation_factory"},
        },
    }


def dusk_queries():
    """A Dusk deployment as deploy/testnet.sh leaves it: every contract at
    this checkout's state_version, the route with no ISM of its own, the
    Mailbox's default a 1-of-1 multisig ISM, and the IGP priced as the
    example settings say."""
    return {
        f"{MAILBOX}/local_domain": {"value": DUSK_DOMAIN},
        f"{MAILBOX}/default_ism": {"value": MULTISIG},
        f"{MAILBOX}/state_version": {"value": 5},
        f"{MERKLE_TREE_HOOK}/state_version": {"value": 2},
        f"{IGP}/state_version": {"value": 3},
        f"{IGP}/domain_gas_config/{EVM_DOMAIN}": {"value": {
            key: int(value) for key, value in DEFAULT_DUSK_IGP.items()}},
        f"{IGP}/claimable_fees": {"value": 0},
        f"{IGP}/beneficiary": {"value": DEPLOYER_H256},
        f"{PROTOCOL_FEE}/claimable_fees": {"value": 0},
        f"{PROTOCOL_FEE}/beneficiary": {"value": DEPLOYER_H256},
        f"{VALIDATOR_ANNOUNCE}/state_version": {"value": 2},
        f"{ROUTE}/state_version": {"value": 4},
        f"{ROUTE}/interchain_security_module": {"value": ZERO},
        f"{MULTISIG}/module_type": {"value": 5},
        f"{MULTISIG}/validators_and_threshold": {"value": {"validators": [VALIDATOR], "threshold": 1}},
        f"{MULTISIG}/threshold": {"value": 1},
        f"{TEST_MOCK}/module_type": {"value": 6},
    }


DUSK_CONTRACTS = {
    "mailbox": MAILBOX, "igp": IGP, "validator_announce": VALIDATOR_ANNOUNCE,
    "merkle_tree_hook": MERKLE_TREE_HOOK, "protocol_fee": PROTOCOL_FEE, "warp_native": ROUTE,
    "ism_multisig": MULTISIG, "test_mock": TEST_MOCK,
}


class TestnetScript(unittest.TestCase):
    """deploy/testnet.sh in a temporary checkout, against the stubs."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="hyperlane-testnet-checks-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "deploy").mkdir()
        shutil.copy2(TESTNET_SH, self.root / "deploy" / "testnet.sh")
        shutil.copy2(ROOT / "deploy" / "testnet.env.example", self.root / "deploy" / "testnet.env.example")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        write_executable(self.bin / "dusk-tx", DUSK_TX_STUB)
        (self.bin / "fake_evm.py").write_text(FAKE_EVM)
        write_executable(self.bin / "cast", CAST_STUB)
        write_executable(self.bin / "forge", FORGE_STUB)
        write_executable(self.bin / "curl", CURL_STUB)
        wasm = self.root / "wasm"
        wasm.mkdir()
        (wasm / "hyperlane_dusk_mailbox.wasm").write_bytes(b"\0asm")
        (self.root / "solidity").mkdir()
        keys = self.root / "deployer.keys"
        keys.write_text("{}")
        self.chain = self.root / "chain.json"
        self.evm_file = self.root / "evm.json"
        self.trace = self.root / "stub.trace"
        self.settings = {
            "DEPLOY_NAME": "fixture",
            "DUSK_KEYS": str(keys),
            "WASM_DIR": str(wasm),
            "SOLIDITY_DIR": str(self.root / "solidity"),
            "DUSK_ORIGIN_VALIDATORS": VALIDATOR,
            "DUSK_ORIGIN_THRESHOLD": "1",
            "EVM_ORIGIN_VALIDATORS": VALIDATOR,
            "EVM_ORIGIN_THRESHOLD": "1",
        }
        self.out = self.root / "deploy" / "out" / "fixture"
        self.out.mkdir(parents=True)
        self.relayer_config = self.out / "relayer.json"
        self.queries = dusk_queries()
        self.evm = sepolia()

    def write_state(self, state):
        (self.out / "deployment.json").write_text(json.dumps(state))

    def state(self):
        return json.loads((self.out / "deployment.json").read_text())

    def run_script(self, *command, **settings):
        env_file = self.root / "deploy" / "testnet.env"
        values = {**self.settings, **settings}
        env_file.write_text('source "$DUSK_DIR/deploy/testnet.env.example"\n'
                            + "".join(f"{key}={shlex.quote(str(value))}\n" for key, value in values.items()))
        self.chain.write_text(json.dumps({"queries": self.queries, "deployer_h256": DEPLOYER_H256,
                                          "deploy": DUSK_CONTRACTS}))
        self.evm_file.write_text(json.dumps(self.evm))
        if self.relayer_config.exists():
            self.relayer_config.unlink()
        self.trace.write_text("")
        env = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "HOME": str(self.root),
            "DUSK_TX": str(self.bin / "dusk-tx"),
            "DUSK_CONSENSUS_PASSWORD": "fixture",
            "EVM_DEPLOYER_KEY": DEPLOYER_KEY,
            "FIXTURE_CHAIN": str(self.chain),
            "FIXTURE_EVM": str(self.evm_file),
            "FIXTURE_TRACE": str(self.trace),
        }
        result = subprocess.run(["bash", str(self.root / "deploy" / "testnet.sh"), *command],
                                env=env, capture_output=True, text=True, timeout=120)
        # What the run left on the fake chains, for the next run and the asserts.
        self.queries = json.loads(self.chain.read_text())["queries"]
        self.evm = json.loads(self.evm_file.read_text())
        return result

    def traced(self, prefix):
        return [line for line in self.trace.read_text().splitlines() if line.startswith(prefix)]

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def assert_fails(self, result, expected):
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn(expected, output)


class DeployedTestnet(TestnetScript):
    """A deployment testnet.sh finished: the Dusk side, and on the EVM side
    our ISM, IGP and oracle priced as the example settings say, the route's
    aggregation hook and the token dispatching through it."""

    def setUp(self):
        super().setUp()
        evm = self.evm["contracts"]
        self.ism, self.token = "0x" + "55" * 20, "0x" + "11" * 20
        self.oracle, self.evm_igp, self.hook = "0x" + "66" * 20, "0x" + "44" * 20, "0x" + "77" * 20
        evm[self.ism] = {"type": "ism"}
        evm[self.oracle] = {"type": "oracle", "owner": DEPLOYER, "data": {
            str(DUSK_DOMAIN): [int(DEFAULT_EVM_IGP["token_exchange_rate"]), int(DEFAULT_EVM_IGP["gas_price"])]}}
        evm[self.evm_igp] = {"type": "igp", "hook_type": 4, "owner": DEPLOYER, "beneficiary": DEPLOYER,
                             "configs": {str(DUSK_DOMAIN): [self.oracle, int(DEFAULT_EVM_IGP["gas_overhead"])]}}
        evm[self.hook] = {"type": "aggregation", "hook_type": 2, "hooks": [EVM_MERKLE, self.evm_igp]}
        evm[self.token] = {"type": "token", "mailbox": EVM_MAILBOX, "hook": self.hook, "owner": DEPLOYER,
                           "routers": {str(DUSK_DOMAIN): "0x" + ROUTE}}
        self.queries[f"{ROUTE}/enrolled_router/{EVM_DOMAIN}"] = {"value": "00" * 12 + "11" * 20}
        self.write_state({
            "evm_chain": "sepolia",
            "evm_domain": EVM_DOMAIN,
            "evm": {"mailbox": EVM_MAILBOX, "merkle_tree_hook": EVM_MERKLE, "validator_announce": EVM_VA,
                    "ism": self.ism, "storage_gas_oracle": self.oracle, "igp": self.evm_igp,
                    "igp_beneficiary": DEPLOYER, "route_hook": self.hook, "token": self.token,
                    "owner": DEPLOYER, "index_from": 1},
            "dusk": DUSK_CONTRACTS,
            "dusk_domain": DUSK_DOMAIN,
        })


class TestnetDeployedIsmChecks(DeployedTestnet):
    """`preflight` and `agent-configs` against a recorded deployment."""

    def assert_both_commands(self, passes, expected_text, **settings):
        for command in ["agent-configs", "preflight"]:
            with self.subTest(command=command):
                result = self.run_script(command, **settings)
                output = result.stdout + result.stderr
                if passes:
                    self.assertEqual(result.returncode, 0, output)
                else:
                    self.assertNotEqual(result.returncode, 0, output)
                    self.assertFalse(self.relayer_config.exists(), "no agent config after a failed check")
                self.assertIn(expected_text, output)

    def set_default_multisig(self, addresses, threshold, ism=MULTISIG):
        self.queries[f"{ism}/module_type"] = {"value": 5}
        self.queries[f"{ism}/validators_and_threshold"] = {
            "value": {"validators": addresses, "threshold": threshold}}
        self.queries[f"{ism}/threshold"] = {"value": threshold}

    def test_deployed_multisig_matching_the_settings_passes(self):
        self.assert_both_commands(True, f"a MessageIdMultisig ISM: 1 of [{VALIDATOR}]")
        result = self.run_script("agent-configs")
        self.assertEqual(result.returncode, 0, result.stderr)
        relayer = json.loads(self.relayer_config.read_text())
        self.assertEqual(relayer["chains"]["dusk"]["gasLimit"], 5000000)

    def test_failed_module_type_query_fails_closed(self):
        # The review's case: the owner raised the threshold to 20 and the
        # module type can't be read. Treating that as "no signatures to
        # check" used the settings' threshold of 1 and a 5M gas limit that
        # can't deliver a 20-signature message.
        self.set_default_multisig(validators(20), 20)
        self.queries[f"{MULTISIG}/module_type"] = {"error": "Query failed (500): transient"}
        self.assert_both_commands(False, "Can't read the module type of the Dusk ISM")

    def test_missing_module_type_value_fails_closed(self):
        self.queries[f"{MULTISIG}/module_type"] = {"value": None}
        self.assert_both_commands(False, "the query returned no value")

    def test_test_mock_ism_is_refused(self):
        self.queries[f"{MAILBOX}/default_ism"] = {"value": TEST_MOCK}
        self.assert_both_commands(False, "a TestMock ISM that accepts every message")

    def test_test_mock_ism_is_refused_on_remote_endpoints_even_with_the_flag(self):
        self.queries[f"{MAILBOX}/default_ism"] = {"value": TEST_MOCK}
        self.assert_both_commands(False, "only counts when DUSK_RUES_URL and EVM_RPC are both",
                                  LOCAL_REHEARSAL_ALLOW_TEST_MOCK=1)
        # Local-looking userinfo in front of a remote host doesn't count.
        for url in ["http://localhost:8080@node.invalid/", "http://127.0.0.1.node.invalid/"]:
            with self.subTest(url=url):
                self.assert_both_commands(False, "only counts when DUSK_RUES_URL and EVM_RPC are both",
                                          LOCAL_REHEARSAL_ALLOW_TEST_MOCK=1, DUSK_RUES_URL=url,
                                          EVM_RPC="http://127.0.0.1:8545")

    def test_local_rehearsal_flag_allows_test_mock(self):
        self.queries[f"{MAILBOX}/default_ism"] = {"value": TEST_MOCK}
        self.assert_both_commands(True, "Allowed for this local rehearsal only",
                                  LOCAL_REHEARSAL_ALLOW_TEST_MOCK=1, DUSK_RUES_URL="http://localhost:8080/",
                                  EVM_RPC="http://127.0.0.1:8545")

    def test_unexpected_module_types_are_refused(self):
        for module_type in [0, 1, 2, 4, 7, "five"]:
            with self.subTest(module_type=module_type):
                self.queries[f"{MULTISIG}/module_type"] = {"value": module_type}
                self.assert_both_commands(False, f"of module type {module_type}; it must be a MessageIdMultisig")

    def test_raised_on_chain_threshold_is_refused(self):
        self.set_default_multisig(validators(20), 20)
        self.settings["EVM_ORIGIN_VALIDATORS"] = ",".join(validators(20))
        self.assert_both_commands(False, "needs 20 signatures, but EVM_ORIGIN_THRESHOLD is 1")

    def test_matching_threshold_still_needs_a_gas_limit_that_covers_it(self):
        self.set_default_multisig(validators(20), 20)
        self.settings["EVM_ORIGIN_VALIDATORS"] = ",".join(validators(20))
        self.settings["EVM_ORIGIN_THRESHOLD"] = 20
        # A relayed process at 20 signatures takes 6.35M gas, so the EVM IGP
        # must charge for that much, and the relayer's limit can't be less.
        self.settings["EVM_IGP_GAS_OVERHEAD"] = 6350000
        self.evm["contracts"][self.evm_igp]["configs"][str(DUSK_DOMAIN)][1] = 6350000
        self.assert_both_commands(False, "is above the relayer's gas limit DUSK_RELAYER_GAS_LIMIT (5,000,000)")
        self.assert_both_commands(False, "DUSK_RELAYER_GAS_LIMIT is 6400000; with EVM_ORIGIN_THRESHOLD of 20",
                                  DUSK_RELAYER_GAS_LIMIT=6400000)
        self.assert_both_commands(True, "a MessageIdMultisig ISM: 20 of", DUSK_RELAYER_GAS_LIMIT=9525000)

    def test_other_validator_set_is_refused(self):
        self.set_default_multisig(["0x" + "77" * 20], 1)
        self.assert_both_commands(False, f"but EVM_ORIGIN_VALIDATORS is [{VALIDATOR}]")

    def test_validator_order_and_case_do_not_matter(self):
        on_chain = validators(3)
        self.set_default_multisig(on_chain, 2)
        self.settings["EVM_ORIGIN_VALIDATORS"] = " ,".join(
            address.upper().replace("0X", "0x") for address in reversed(on_chain))
        self.settings["EVM_ORIGIN_THRESHOLD"] = 2
        self.settings["EVM_IGP_GAS_OVERHEAD"] = 2390000
        self.evm["contracts"][self.evm_igp]["configs"][str(DUSK_DOMAIN)][1] = 2390000
        self.assert_both_commands(True, "a MessageIdMultisig ISM: 2 of")

    def test_route_ism_overrides_the_mailbox_default(self):
        self.queries[f"{ROUTE}/interchain_security_module"] = {"value": OTHER_MULTISIG}
        self.set_default_multisig(["0x" + "77" * 20], 1, ism=OTHER_MULTISIG)
        self.assert_both_commands(False, f"uses its own ISM, {OTHER_MULTISIG}, which trusts")
        self.set_default_multisig([VALIDATOR], 1, ism=OTHER_MULTISIG)
        self.assert_both_commands(True, f"uses its own ISM, {OTHER_MULTISIG}, a MessageIdMultisig ISM")

    def test_failed_ism_lookups_fail_closed(self):
        for key, expected in [
            (f"{ROUTE}/interchain_security_module", f"Can't read the ISM of the Dusk route {ROUTE}"),
            (f"{MAILBOX}/default_ism", f"Can't read the default ISM of the Dusk Mailbox {MAILBOX}"),
            (f"{MULTISIG}/validators_and_threshold", "Can't read the validators and threshold"),
        ]:
            with self.subTest(query=key):
                saved = self.queries.pop(key)
                try:
                    self.assert_both_commands(False, expected)
                finally:
                    self.queries[key] = saved

    def test_unset_default_ism_is_refused(self):
        self.queries[f"{MAILBOX}/default_ism"] = {"value": ZERO}
        self.assert_both_commands(False, "the Mailbox's default ISM, which is not set")


def expected_fees(ratio="25000", margin=5000, evm_gas_price=10**9, dusk_overhead=200000,
                  evm_overhead=2170000, dusk_gas_price=1):
    """docs/costs.md's formula, worked out here independently of the script."""
    ratio = Fraction(ratio)
    markup = Fraction(10_000 + margin, 10_000)
    # LUX per wei: 1 ETH (1e18 wei) buys `ratio` DUSK (ratio x 1e9 LUX).
    dusk_rate = ceil(ratio * 10**9 / 10**18 * 10**10 * markup)
    evm_rate = ceil(Fraction(10**18) / (ratio * 10**9) * 10**10 * markup)
    gas = 50_000 + dusk_overhead
    return {
        "dusk_igp": {"gas_overhead": str(dusk_overhead), "token_exchange_rate": str(dusk_rate),
                     "gas_price": str(evm_gas_price)},
        "evm_igp": {"gas_overhead": str(evm_overhead), "token_exchange_rate": str(evm_rate),
                    "gas_price": str(dusk_gas_price)},
        "dusk_quote_lux": gas * evm_gas_price * dusk_rate // 10**10,
        # What delivering costs, in LUX, at the prices, before the margin.
        "dusk_cost_lux": gas * evm_gas_price * ratio * 10**9 / 10**18,
        "evm_quote_wei": evm_overhead * dusk_gas_price * evm_rate // 10**10,
        "evm_cost_wei": evm_overhead * dusk_gas_price * Fraction(10**18) / (ratio * 10**9),
    }


class TestnetFees(TestnetScript):
    """`fees`: what the settings charge, from the formula in docs/costs.md."""

    def fees(self, **settings):
        result = self.run_script("fees", "--json", **settings)
        self.assert_ok(result)
        return json.loads(result.stdout)

    def test_default_settings_charge_the_documented_amounts(self):
        fees = self.fees()
        self.assertEqual(fees["dusk_igp"]["config"], f"{EVM_DOMAIN}:200000:375000:1000000000")
        self.assertEqual(fees["evm_igp"], {"domain": DUSK_DOMAIN, **DEFAULT_EVM_IGP})
        # 250,000 EVM gas at 1 gwei is 0.00025 ETH, 6.25 DUSK at 25,000 DUSK
        # per ETH; with the 50% margin, 9.375 DUSK.
        self.assertEqual(fees["dusk_to_evm"]["quote_lux"], "9375000000")
        self.assertEqual(fees["dusk_to_evm"]["cost_lux"], "6250000000")
        # 2,170,000 Dusk gas at 1 LUX is 0.00217 DUSK, 8.68e10 wei; with the
        # margin, 1.302e11 wei.
        self.assertEqual(fees["evm_to_dusk"]["quote_wei"], "130200000000")
        self.assertEqual(fees["evm_to_dusk"]["cost_wei"], "86800000000")
        self.assertEqual(fees["relayer"], {"dusk_origin_gas_fraction": "2/3",
                                           "evm_origin_gas_fraction": "217/500",
                                           "evm_origin_required_gas": 2170000})
        summary = self.run_script("fees")
        self.assert_ok(summary)
        self.assertIn("quote 9.375 DUSK", summary.stdout)
        self.assertIn("quote 0.0000001302 ETH (130,200,000,000 wei)", summary.stdout)

    def test_pricing_follows_the_formula(self):
        for ratio, margin, gas_price in [("25000", 5000, 10**9), ("30000", 0, 10**9),
                                         ("12345.678", 2500, 2 * 10**9), ("1000", 1, 3 * 10**9),
                                         ("250000", 10000, 10**8), ("0.5", 300, 10**9)]:
            with self.subTest(ratio=ratio, margin=margin, gas_price=gas_price):
                fees = self.fees(ETH_DUSK_PRICE_RATIO=ratio, OPERATOR_MARGIN_BPS=margin,
                                 EVM_GAS_PRICE_WEI=gas_price)
                expected = expected_fees(ratio, margin, gas_price)
                for side in ("dusk_igp", "evm_igp"):
                    for key, value in expected[side].items():
                        self.assertEqual(fees[side][key], value, (side, key))
                quote = int(fees["dusk_to_evm"]["quote_lux"])
                self.assertEqual(quote, expected["dusk_quote_lux"])
                # Rounding the rate up never charges less than the margin.
                markup = Fraction(10_000 + margin, 10_000)
                self.assertGreaterEqual(quote + 1, expected["dusk_cost_lux"] * markup)
                self.assertGreaterEqual(int(fees["evm_to_dusk"]["quote_wei"]) + 1,
                                        expected["evm_cost_wei"] * markup)
                self.assertEqual(fees["relayer"]["dusk_origin_gas_fraction"],
                                 str(Fraction(10_000, 10_000 + margin)).replace(" ", "")
                                 if margin else "1/1")

    def test_relayer_requirement_follows_the_evm_overhead(self):
        fees = self.fees(EVM_IGP_GAS_OVERHEAD=2390000, EVM_ORIGIN_THRESHOLD=2,
                         EVM_ORIGIN_VALIDATORS=",".join(validators(2)))
        self.assertEqual(fees["relayer"]["evm_origin_gas_fraction"], "239/500")
        self.assertEqual(fees["relayer"]["evm_origin_required_gas"], 2390000)

    def test_invalid_settings_are_refused(self):
        for settings, expected in [
            ({"ETH_DUSK_PRICE_RATIO": "abc"}, "ETH_DUSK_PRICE_RATIO must be a positive number"),
            ({"ETH_DUSK_PRICE_RATIO": "0"}, "ETH_DUSK_PRICE_RATIO must be a positive number"),
            ({"OPERATOR_MARGIN_BPS": "-1"}, "OPERATOR_MARGIN_BPS must be a whole number"),
            ({"EVM_GAS_PRICE_WEI": "0"}, "EVM_GAS_PRICE_WEI must be between 1"),
            ({"DUSK_IGP_TOKEN_EXCHANGE_RATE": "1"}, "DUSK_IGP_TOKEN_EXCHANGE_RATE is no longer read"),
            ({"EVM_ROUTE_HOOK": "0x" + "00" * 20}, "EVM_ROUTE_HOOK is no longer read"),
        ]:
            with self.subTest(settings=settings):
                self.assert_fails(self.run_script("fees", **settings), expected)

    def test_gas_overheads_must_cover_delivery(self):
        six = ",".join(validators(6))
        self.assert_fails(
            self.run_script("fees", DUSK_ORIGIN_VALIDATORS=six, DUSK_ORIGIN_THRESHOLD=6),
            "Set it to at least 204,000")
        self.assert_ok(self.run_script("fees", DUSK_ORIGIN_VALIDATORS=six, DUSK_ORIGIN_THRESHOLD=6,
                                       DUSK_IGP_GAS_OVERHEAD=204000))
        self.assert_fails(
            self.run_script("fees", EVM_ORIGIN_THRESHOLD=2, EVM_ORIGIN_VALIDATORS=",".join(validators(2))),
            "takes about 2,390,000 Dusk gas")
        self.assert_fails(self.run_script("fees", EVM_IGP_GAS_OVERHEAD=6000000),
                          "is above the relayer's gas limit")


class TestnetFeeCommands(DeployedTestnet):
    """agent-configs' enforcement, the pricing updates, revenue and claims."""

    def test_agent_configs_relay_any_message_once_it_is_paid(self):
        self.assert_ok(self.run_script("agent-configs"))
        relayer = json.loads(self.relayer_config.read_text())
        self.assertNotIn("whitelist", relayer)
        self.assertEqual(relayer["gasPaymentEnforcement"], [
            {"type": "onChainFeeQuoting", "gasFraction": "2/3", "matchingList": [{"originDomain": DUSK_DOMAIN}]},
            {"type": "onChainFeeQuoting", "gasFraction": "217/500",
             "matchingList": [{"originDomain": EVM_DOMAIN}]},
        ])
        self.assertTrue(relayer["igpIndexingEnabled"])
        # The relayer reads EVM-origin payments from our IGP, not Hyperlane's.
        self.assertEqual(relayer["chains"]["sepolia"]["interchainGasPaymaster"], self.evm_igp)
        self.assertEqual(relayer["chains"]["dusk"]["interchainGasPaymaster"], "0x" + IGP)

    def test_agent_configs_follow_the_margin(self):
        self.settings["OPERATOR_MARGIN_BPS"] = 2000
        fees = expected_fees(margin=2000)
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"] = {"value": {
            key: int(value) for key, value in fees["dusk_igp"].items()}}
        self.evm["contracts"][self.oracle]["data"][str(DUSK_DOMAIN)] = [
            int(fees["evm_igp"]["token_exchange_rate"]), 1]
        self.assert_ok(self.run_script("agent-configs"))
        relayer = json.loads(self.relayer_config.read_text())
        self.assertEqual(relayer["gasPaymentEnforcement"][0]["gasFraction"], "5/6")

    def test_agent_configs_refuse_pricing_the_chain_doesnt_have(self):
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"] = 1
        result = self.run_script("agent-configs")
        self.assert_fails(result, "bash deploy/testnet.sh dusk-igp")
        self.assertFalse(self.relayer_config.exists())
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"] = 375000
        self.evm["contracts"][self.evm_igp]["configs"][str(DUSK_DOMAIN)][1] = 1950000
        self.assert_fails(self.run_script("agent-configs"), "bash deploy/testnet.sh evm-igp")
        self.assertFalse(self.relayer_config.exists())

    def test_agent_configs_refuse_a_route_that_pays_no_igp_of_ours(self):
        self.evm["contracts"][self.token]["hook"] = "0x" + "00" * 20
        self.assert_fails(self.run_script("agent-configs"), "dispatches through hook")
        # preflight only warns: deploy sets the hook.
        result = self.run_script("preflight")
        self.assert_ok(result)
        self.assertIn("deploy sets", result.stderr)

    def test_agent_configs_refuse_a_hook_that_skips_the_merkle_tree(self):
        self.evm["contracts"][self.hook]["hooks"] = [self.evm_igp]
        self.assert_fails(self.run_script("agent-configs"), "the Merkle hook validators sign, then the IGP")

    def test_dusk_igp_dry_run_shows_both_quotes_and_sends_nothing(self):
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"] = {"value": {
            "gas_overhead": 200000, "token_exchange_rate": 1, "gas_price": 1000000000}}
        result = self.run_script("dusk-igp")
        self.assert_ok(result)
        rows = [line.split() for line in result.stderr.splitlines() if line.split()[:1] in (["now"], ["new"])]
        # The pricing deploy/testnet.sh used to deploy charged 25,000 LUX.
        self.assertEqual(rows, [["now", "200000", "1", "1000000000", "25000"],
                                ["new", "200000", "375000", "1000000000", "9375000000"]])
        self.assertIn("Dry run: nothing was sent", result.stderr)
        self.assertEqual(self.traced("dusk-tx set-igp-config"), [])

    def test_dusk_igp_apply_sets_the_settings_pricing(self):
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"] = 1
        self.assert_ok(self.run_script("dusk-igp", "--apply"))
        [sent] = self.traced("dusk-tx set-igp-config")
        self.assertIn(f"--igp {IGP} --igp-domain-config {EVM_DOMAIN}:200000:375000:1000000000", sent)
        self.assertEqual(self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"],
                         375000)
        result = self.run_script("dusk-igp")
        self.assert_ok(result)
        self.assertIn("already charges what the settings say", result.stderr)

    def test_evm_igp_dry_run_and_apply(self):
        self.settings["ETH_DUSK_PRICE_RATIO"] = "20000"
        # The Dusk IGP follows the ratio too; this test is about the EVM one.
        fees = expected_fees(ratio="20000")
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"] = {"value": {
            key: int(value) for key, value in fees["dusk_igp"].items()}}
        result = self.run_script("evm-igp")
        self.assert_ok(result)
        rows = [line.split() for line in result.stderr.splitlines() if line.split()[:1] in (["now"], ["new"])]
        self.assertEqual(rows, [["now", "2170000", "600000000000000", "1", "130200000000"],
                                ["new", "2170000", "750000000000000", "1", "162750000000"]])
        self.assertEqual(self.traced("cast send"), [])
        self.assert_ok(self.run_script("evm-igp", "--apply"))
        [sent] = self.traced("cast send")
        self.assertIn(f"setRemoteGasDataConfigs((uint32,uint128,uint128)[]) [({DUSK_DOMAIN},750000000000000,1)]",
                      sent)
        self.assertEqual(self.evm["contracts"][self.oracle]["data"][str(DUSK_DOMAIN)], [750000000000000, 1])
        self.assert_ok(self.run_script("agent-configs"))

    def test_evm_igp_apply_changes_the_overhead(self):
        self.settings.update(EVM_IGP_GAS_OVERHEAD=2390000, EVM_ORIGIN_THRESHOLD=2,
                             EVM_ORIGIN_VALIDATORS=",".join(validators(2)))
        self.assert_ok(self.run_script("evm-igp", "--apply"))
        [sent] = self.traced("cast send")
        self.assertIn(f"setDestinationGasConfigs((uint32,(address,uint96))[]) "
                      f"[({DUSK_DOMAIN},({self.oracle},2390000))]", sent)
        self.assertEqual(self.evm["contracts"][self.evm_igp]["configs"][str(DUSK_DOMAIN)],
                         [self.oracle, 2390000])

    def test_claim_collects_both_chains(self):
        self.queries[f"{IGP}/claimable_fees"] = {"value": 9375000000}
        self.queries[f"{PROTOCOL_FEE}/claimable_fees"] = {"value": 1000000}
        beneficiary = "0x" + "be" * 20
        self.evm["contracts"][self.evm_igp]["beneficiary"] = beneficiary
        self.evm["balances"][self.evm_igp] = 130200000000
        result = self.run_script("revenue")
        self.assert_ok(result)
        self.assertIn(f"holds 9375000000 LUX for the account with hash {DEPLOYER_H256}", result.stderr)
        self.assertIn(f"holds 130200000000 wei for {beneficiary}", result.stderr)
        self.assert_ok(self.run_script("claim"))
        self.assertEqual(self.traced("cast send"), [f"cast send {self.evm_igp} claim()"])
        self.assertEqual(len(self.traced("dusk-tx claim-fees")), 2)
        self.assertEqual(self.evm["balances"][beneficiary], 130200000000)
        self.assertEqual(self.queries[f"{IGP}/claimable_fees"]["value"], 0)

    def test_claim_leaves_another_beneficiary_its_own_claim(self):
        self.queries[f"{IGP}/claimable_fees"] = {"value": 5}
        self.queries[f"{IGP}/beneficiary"] = {"value": "be" * 32}
        result = self.run_script("claim")
        self.assert_ok(result)
        self.assertIn(f"dusk-tx claim-fees --rues-url", result.stderr)
        self.assertEqual(self.traced("dusk-tx claim-fees"), [])


class TestnetDeploy(TestnetScript):
    """`deploy` from nothing, and on top of an older deployment."""

    def test_deploy_wires_the_route_through_the_merkle_tree_and_our_igp(self):
        result = self.run_script("deploy")
        self.assert_ok(result)
        [deploy] = self.traced("dusk-tx deploy-hyperlane")
        self.assertIn(f"--igp-domain-config {EVM_DOMAIN}:200000:375000:1000000000", deploy)
        state = self.state()
        evm = self.evm["contracts"]
        oracle, igp, hook, token = (state["evm"][key] for key in
                                    ("storage_gas_oracle", "igp", "route_hook", "token"))
        self.assertEqual(evm[oracle]["data"][str(DUSK_DOMAIN)], [600000000000000, 1])
        self.assertEqual(evm[igp]["owner"], DEPLOYER)
        self.assertEqual(evm[igp]["beneficiary"], DEPLOYER)
        self.assertEqual(evm[igp]["configs"][str(DUSK_DOMAIN)], [oracle, 2170000])
        # The agents index from before the IGP and the token existed.
        self.assertLessEqual(state["evm"]["index_from"], min(evm[igp]["created_at"], evm[token]["created_at"]))
        # Sepolia's Mailbox requires only its ProtocolFee: the route's hook
        # replaces the default one, so it inserts into the Merkle tree first.
        self.assertEqual(evm[hook], {"type": "aggregation", "hook_type": 2, "hooks": [EVM_MERKLE, igp]})
        self.assertEqual(evm[token]["hook"], hook)
        # A transfer pays the 1 wei protocol fee and the IGP's quote.
        self.assertIn("charges 130200000000 wei for a transfer to Dusk", result.stderr)
        # Running it again sends nothing new.
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.traced("cast send"), [])
        self.assertEqual(self.traced("forge"), [])
        self.assertEqual(self.traced("dusk-tx deploy-hyperlane"), [])
        self.assert_ok(self.run_script("agent-configs"))

    def test_a_required_merkle_hook_makes_the_igp_the_route_hook(self):
        self.evm["contracts"][EVM_MAILBOX]["requiredHook"] = EVM_MERKLE
        self.assert_ok(self.run_script("deploy"))
        state = self.state()
        self.assertEqual(state["evm"]["route_hook"], state["evm"]["igp"])
        self.assertEqual(self.evm["contracts"][state["evm"]["token"]]["hook"], state["evm"]["igp"])

    def test_without_a_factory_on_chain_deploy_brings_one(self):
        self.assert_ok(self.run_script("deploy", EVM_AGGREGATION_HOOK_FACTORY=""))
        state = self.state()
        self.assertIn("aggregation_hook_factory", state["evm"])
        self.assertEqual(self.evm["contracts"][state["evm"]["route_hook"]]["hooks"],
                         [EVM_MERKLE, state["evm"]["igp"]])

    def test_a_front_run_igp_initialization_stops_the_deploy(self):
        self.evm["front_run"] = "0x" + "ee" * 20
        self.assert_fails(self.run_script("deploy"), "Remove .evm.igp from")

    def test_beneficiaries_are_configurable(self):
        beneficiary = "0x" + "be" * 20
        self.assert_ok(self.run_script("deploy", EVM_IGP_BENEFICIARY=beneficiary,
                                       DUSK_FEE_BENEFICIARY="fixture-treasury"))
        self.assertEqual(self.evm["contracts"][self.state()["evm"]["igp"]]["beneficiary"], beneficiary)
        sent = self.traced("dusk-tx set-beneficiary")
        self.assertEqual(len(sent), 2)
        self.assertTrue(all("--beneficiary fixture-treasury" in line for line in sent))

    def test_an_older_deployment_gets_our_igp_and_a_route_hook(self):
        # deploy/testnet.sh before relaying fees: Hyperlane's IGP recorded as
        # .evm.igp, and a token on the Mailbox's default hook.
        evm = self.evm["contracts"]
        hyperlane_igp, ism, token = "0x" + "6f" * 20, "0x" + "55" * 20, "0x" + "11" * 20
        evm[hyperlane_igp] = {"type": "igp", "hook_type": 4, "owner": "0x" + "ab" * 20,
                              "beneficiary": "0x" + "ab" * 20, "configs": {}}
        evm[ism] = {"type": "ism"}
        evm[token] = {"type": "token", "mailbox": EVM_MAILBOX, "hook": "0x" + "00" * 20,
                      "owner": DEPLOYER, "routers": {str(DUSK_DOMAIN): "0x" + ROUTE}}
        self.queries[f"{ROUTE}/enrolled_router/{EVM_DOMAIN}"] = {"value": "00" * 12 + "11" * 20}
        self.write_state({
            "evm_chain": "sepolia", "evm_domain": EVM_DOMAIN,
            "evm": {"mailbox": EVM_MAILBOX, "merkle_tree_hook": EVM_MERKLE, "validator_announce": EVM_VA,
                    "igp": hyperlane_igp, "ism": ism, "token": token, "owner": DEPLOYER, "index_from": 1},
            "dusk": DUSK_CONTRACTS, "dusk_domain": DUSK_DOMAIN,
        })
        self.assert_ok(self.run_script("deploy"))
        state = self.state()
        self.assertEqual(state["evm"]["hyperlane_igp"], hyperlane_igp)
        self.assertNotEqual(state["evm"]["igp"], hyperlane_igp)
        self.assertEqual(state["evm"]["index_from"], 1)
        self.assertEqual(self.evm["contracts"][token]["hook"], state["evm"]["route_hook"])
        self.assertIn(f"cast send {token} setHook(address) {state['evm']['route_hook']}",
                      self.trace.read_text())


# Encrypt a consensus key pair the way rusk-wallet's export does.
def consensus_keys(secret_key, public_key, password):
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC

    salt, iv = os.urandom(32), os.urandom(12)
    key = PBKDF2HMAC(algorithm=hashes.SHA256(), length=32, salt=salt, iterations=10000).derive(password.encode())
    plaintext = json.dumps({"secret_key_bls": base64.b64encode(secret_key).decode(),
                            "public_key_bls": base64.b64encode(public_key).decode()}).encode()
    return json.dumps({"salt": base64.b64encode(salt).decode(), "iv": base64.b64encode(iv).decode(),
                       "key_pair": list(AESGCM(key).encrypt(iv, plaintext, None))})


# Passwords Bash quoting (${var@Q}) and Python string literals disagree on.
TRICKY_PASSWORDS = [
    "it's",
    'say "hi" \\n \\x41 \\\\ $HOME `id` ${PATH}',
    "trailing backslash \\",
    "''' triple \"\"\" quotes",
    "tab\tand unicode é中",
]

# Records the password export-signer-key gets, and writes a key file.
EXPORT_STUB = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
assert args[0] == "export-signer-key", args
assert "--password" not in args, "the password must not be on the command line"
out = args[args.index("--out") + 1]
with open(os.environ["FIXTURE_PASSWORD_SEEN"], "w", encoding="utf-8") as seen:
    seen.write(os.environ["DUSK_CONSENSUS_PASSWORD"])
fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
os.write(fd, b"0x" + b"11" * 32 + b"\n")
os.close(fd)
print(json.dumps({"success": True, "out": out}))
'''


class GenAgentConfigsPassword(unittest.TestCase):
    """demo/gen-agent-configs.sh passes the consensus password as data."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="hyperlane-gen-agent-configs-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.demo = self.root / "demo"
        self.demo.mkdir()
        shutil.copy2(GEN_AGENT_CONFIGS_SH, self.demo / "gen-agent-configs.sh")
        # The live-chain check gen-agent-configs.sh runs first.
        write_executable(self.demo / "deploy.sh", "#!/bin/sh\nexit 0\n")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        write_executable(self.bin / "cast", "#!/bin/sh\necho 0x000000000000000000000000000000000000dEaD\n")
        self.state = self.root / "state.json"
        self.state.write_text(json.dumps({
            "evm": {"mailbox": "0x" + "01" * 20, "token": "0x" + "02" * 20, "igp": "0x" + "03" * 20,
                    "validator_announce": "0x" + "04" * 20, "merkle_tree_hook": "0x" + "05" * 20,
                    "igp_dusk_gas_overhead": 2170000},
            "dusk": {"mailbox": "a1" * 32, "igp": "a2" * 32, "validator_announce": "a3" * 32,
                     "merkle_tree_hook": "a4" * 32, "test_mock": TEST_MOCK, "default_ism": TEST_MOCK},
            "evm_domain": 31338, "dusk_domain": 4242, "dusk_default_ism": "testMock", "dusk_chain_id": "02",
        }))
        self.keys = self.root / "consensus.keys"
        self.keys.write_text("{}")
        self.seen = self.root / "password-seen"
        self.tmp = self.root / "tmp"
        self.tmp.mkdir()

    def generate(self, password, dusk_tx):
        (self.demo / ".env.bridge").write_text("".join(f"{key}={shlex.quote(str(value))}\n" for key, value in {
            "BRIDGE_STATE_FILE": self.state, "CONSENSUS_KEYS": self.keys, "DUSK_TX": dusk_tx,
            "DUSK_RUES_URL": "http://localhost:8080/", "ANVIL_RPC": "http://localhost:8545",
            "ANVIL_CHAIN_ID": 31338, "ANVIL_PRIVATE_KEY": "0x" + "22" * 32,
        }.items()) + 'CONSENSUS_PASSWORD="${CONSENSUS_PASSWORD:-password}"\n')
        env = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "HOME": str(self.root),
            "TMPDIR": str(self.tmp),
            "CONSENSUS_PASSWORD": password,
            "FIXTURE_PASSWORD_SEEN": str(self.seen),
        }
        return subprocess.run(["bash", str(self.demo / "gen-agent-configs.sh"), "--ism", "testMock"],
                              env=env, capture_output=True, text=True, timeout=60)

    def assert_generated(self, result, password, secret_key_hex):
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertNotIn(password, output, "the password must not be printed")
        generated = json.loads(result.stdout)
        key_file = Path(generated["duskSignerKeyFile"])
        self.assertEqual(key_file.read_text(), f"0x{secret_key_hex}\n")
        self.assertEqual(key_file.stat().st_mode & 0o077, 0)
        shutil.rmtree(generated["runDir"])

    def test_password_reaches_the_key_export_unchanged(self):
        stub = self.bin / "dusk-tx"
        write_executable(stub, EXPORT_STUB)
        for password in TRICKY_PASSWORDS:
            with self.subTest(password=password):
                result = self.generate(password, stub)
                self.assert_generated(result, password, "11" * 32)
                self.assertEqual(self.seen.read_text(encoding="utf-8"), password)

    def test_relayer_relays_any_message_once_it_is_paid(self):
        stub = self.bin / "dusk-tx"
        write_executable(stub, EXPORT_STUB)
        result = self.generate("password", stub)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        generated = json.loads(result.stdout)
        relayer = json.loads(Path(generated["relayer"]).read_text())
        shutil.rmtree(generated["runDir"])
        self.assertNotIn("whitelist", relayer)
        self.assertTrue(relayer["igpIndexingEnabled"])
        self.assertEqual(relayer["gasPaymentEnforcement"], [
            {"type": "onChainFeeQuoting", "gasFraction": "2/3", "matchingList": [{"originDomain": 4242}]},
            {"type": "onChainFeeQuoting", "gasFraction": "2170000/5000000",
             "matchingList": [{"originDomain": 31338}]},
        ])
        self.assertEqual(relayer["chains"]["anvil"]["interchainGasPaymaster"], "0x" + "03" * 20)

    def test_real_dusk_tx_decrypts_with_a_tricky_password(self):
        dusk_tx = Path(os.environ.get("DUSK_TX_BIN", ROOT / "target" / "release" / "dusk-tx"))
        if not os.access(dusk_tx, os.X_OK):
            self.skipTest(f"no dusk-tx binary at {dusk_tx}")
        try:
            import cryptography  # noqa: F401
        except ImportError:
            self.skipTest("python3 cryptography is not installed")
        secret_key = bytes.fromhex("11" * 31 + "05")
        account = subprocess.run([str(dusk_tx), "account", "--offline", "--secret-key-stdin"],
                                 input=secret_key.hex() + "\n", capture_output=True, text=True, check=True)
        public_key = bytes.fromhex(json.loads(account.stdout)["public_key"])
        for password in TRICKY_PASSWORDS:
            with self.subTest(password=password):
                self.keys.write_text(consensus_keys(secret_key, public_key, password))
                self.assert_generated(self.generate(password, dusk_tx), password, secret_key.hex())


if __name__ == "__main__":
    unittest.main()
