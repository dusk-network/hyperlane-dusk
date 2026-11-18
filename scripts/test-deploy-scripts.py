#!/usr/bin/env python3
"""Run deploy/testnet.sh and demo/gen-agent-configs.sh against stubbed tools.

No network, chain or Rust build is needed: dusk-tx, cast, curl and forge are
replaced by stubs. The dusk-tx stub answers queries from a fixture and
records what the transactions it is asked for would change; cast and forge
run against a small fake EVM chain with Hyperlane's contracts, kept in a JSON
file. A test that needs the real dusk-tx skips itself unless DUSK_TX_BIN or
target/release/dusk-tx exists. AnvilEvmDeploy deploys the EVM side on a
local Anvil with Hyperlane's real contracts, and skips itself without Anvil,
Foundry and the Solidity sources (HYPERLANE_SOLIDITY_DIR, by default
../hyperlane-monorepo/solidity).
"""
import base64
import fcntl
from fractions import Fraction
import hashlib
import json
from math import ceil
import os
from pathlib import Path
import re
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
AGGREGATION_HOOK = "a6" * 32
MULTISIG = "b1" * 32
OTHER_MULTISIG = "b2" * 32
TEST_MOCK = "c1" * 32
TEST_RECIPIENT = "c2" * 32
DEPLOYER_H256 = "d0" * 32
# Multicall for Dusk, at the ID it has on every chain.
MULTICALL = "93de287b7d349d678a0038fe7772e8960e1a84a1b5da2d0b1f62f532616ce1d4"
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
ATTACKER = "0x" + "ee" * 20
# Events deploy/testnet.sh reads from receipts: Initialized(uint8) and
# Upgraded(address).
INITIALIZED = "0x7f26b83ff96e1f2b6a682f133852f6798a09c465da95921460cefb3847402498"
UPGRADED = "0xbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b"
# QuoteSignerAdded(address) and QuoteSignerRemoved(address), from the IGP.
QUOTE_SIGNER_ADDED = "0x1999e59a6c4f2d603b5236b4b6a9ab4caecb598d4a59fe663b6eae45ef79a3d6"
QUOTE_SIGNER_REMOVED = "0xf1e7459dfbea75ed7b558903ed9cf48b6c71a30d598a87ca5335d87663a2f70a"

# What deploy/testnet.env.example's defaults charge (docs/costs.md).
DEFAULT_DUSK_IGP = {"gas_overhead": "405000", "token_exchange_rate": "375000", "gas_price": "1000000000"}
DEFAULT_EVM_IGP = {"gas_overhead": "2200000", "token_exchange_rate": "600000000000000", "gas_price": "1"}
# The deployer's public key, as the dusk-tx stub has it.
DEPLOYER_KEY_HEX = "d1" * 96
# deploy/testnet.env.example's token routes.
EXAMPLE_TOKEN_ROUTES = "EURd|Dusk Euro (testnet)|6|1000000000000\nUSDd|Dusk Dollar (testnet)|6|1000000000000"


def validators(count):
    return [f"0x{'ab' * 19}{index:02x}" for index in range(1, count + 1)]


def proxy_creation(evm, address, implementation, contract, creator=DEPLOYER, initialized=True):
    """Put `contract` at `address` behind an ERC1967Proxy of
    `implementation` on the fake EVM, with the receipt of the transaction
    that created it: sent by `creator`, and with the contract's
    initialization in it unless `initialized` is false. Returns the
    transaction's hash."""
    tx = "0x" + hashlib.sha256(f"creation:{address}".encode()).hexdigest()
    evm["contracts"][address] = {**contract, "implementation": implementation, "initialized": True}
    logs = [{"address": address, "topics": [UPGRADED, "0x" + "00" * 12 + implementation[2:]], "data": "0x"}]
    if initialized:
        logs.append({"address": address, "topics": [INITIALIZED], "data": "0x"})
    evm.setdefault("receipts", {})[tx] = {
        "transactionHash": tx, "status": "0x1", "from": creator, "to": None,
        "contractAddress": address, "blockNumber": "0x1", "logs": logs}
    return tx


def quote_signer_history(evm, igp, signer, block):
    """A quote signer the IGP's owner added and removed again, in two
    transactions in `block`, as the IGP's events show it."""
    for topic in (QUOTE_SIGNER_ADDED, QUOTE_SIGNER_REMOVED):
        tx = "0x" + hashlib.sha256(f"{topic}:{igp}:{signer}:{block}".encode()).hexdigest()
        evm.setdefault("receipts", {})[tx] = {
            "transactionHash": tx, "status": "0x1", "from": DEPLOYER, "to": igp, "contractAddress": None,
            "blockNumber": hex(block),
            "logs": [{"address": igp, "topics": [topic, "0x" + "00" * 12 + signer[2:]], "data": "0x"}]}


# Answers the dusk-tx subcommands the scripts use from FIXTURE_CHAIN, and
# writes back what the transactions it is asked for would change. `query`
# looks up "<contract>/<method>" (plus "/<arg>" for --arg-u32 and --arg-u64);
# a missing entry, or one with "error", fails as dusk-tx does. FIXTURE_CHAIN's
# "interleave", {"key": key, "after_reads": n, "value": v}, makes another
# process's change to that answer once it has been read n times, before the
# next read; "reads" counts the reads. Its "on_read", {"key": key, "path":
# path, "content": text}, writes the file at path the first time that key
# is read, as a process that doesn't take the deployment's lock could.
DUSK_TX_STUB = r'''#!/usr/bin/env python3
import hashlib, json, os, sys
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
# As dusk-tx does: every command that signs needs --expected-chain-id, and
# refuses an endpoint on another chain (FIXTURE_DUSK_CHAIN_ID, or 2).
SIGNING = ("call", "deploy-hyperlane", "deploy-wasm", "fund-dispatch", "withdraw-dispatch", "dispatch",
           "process", "enroll-router", "claim-pending", "set-igp-config",
           "set-beneficiary", "claim-fees", "pay-for-gas", "drc20-approve", "transfer-remote",
           "deploy-token-route", "drc20-mint")
# FIXTURE_CHAIN's "fail", {command: n}, makes the next n runs of a command
# fail, as one the node refuses does, sending nothing.
failing = chain.get("fail") or {}
if failing.get(args[0], 0) > 0 and "--plan" not in args:
    failing[args[0]] -= 1
    save()
    out({"success": False, "error": f"stub dusk-tx: {args[0]} failed, as the fixture asked"}, 1)
if args[0] in SIGNING and "--quote-only" not in args and "--plan" not in args:
    expected, endpoint = opt("--expected-chain-id"), os.environ.get("FIXTURE_DUSK_CHAIN_ID", "2")
    if expected is None:
        out({"success": False, "error": f"stub dusk-tx: {args[0]} needs --expected-chain-id"}, 2)
    if expected != endpoint:
        out({"success": False, "error": f"Configured Dusk chain ID {expected} does not match endpoint chain ID {endpoint}"}, 1)
if args[0] == "query":
    key = f"{opt('--contract')}/{opt('--method')}"
    for flag in ("--arg-u32", "--arg-u64"):
        if opt(flag) is not None:
            key += "/" + opt(flag)
    plan = chain.get("interleave")
    if plan and plan["key"] == key:
        plan["reads"] = plan.get("reads", 0) + 1
        if plan["reads"] == plan["after_reads"] + 1:
            queries[key] = {"value": plan["value"]}
        save()
    write = chain.get("on_read")
    if write and write["key"] == key and not write.get("written"):
        with open(write["path"], "w") as f:
            f.write(write["content"])
        write["written"] = True
        save()
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
def account_hash(text):
    return hashlib.sha256(text.encode()).hexdigest()
if args[0] == "account":
    if "--offline" in args:
        if opt("--public-key") is not None:
            key = opt("--public-key")
            out({"success": True, "public_key": key,
                 "account_h256": chain.get("deployer_h256") if key == chain.get("deployer_key") else account_hash(key)})
        if opt("--address") is not None:
            out({"success": True, "address": opt("--address"), "account_h256": account_hash(opt("--address"))})
        out({"success": True, "account_h256": chain.get("deployer_h256"), "public_key": chain.get("deployer_key")})
    balance = int(chain.get("balance", 1000 * 10**9))
    out({"success": True, "address": "fixture-deployer", "balance": str(balance),
         "balance_dusk": f"{balance // 10**9}.{balance % 10**9:09d}"})
if args[0] == "deploy-hyperlane":
    contracts = chain["deploy"]
    set_pricing(contracts["igp"], opt("--igp-domain-config"))
    save()
    out({"success": True, "domain": int(opt("--domain")), "chain_id": int(opt("--expected-chain-id")),
         "contracts": contracts})
if args[0] == "deploy-wasm":
    contract = opt("--expected-id")
    queries[f"{contract}/version"] = {"value": 1}
    save()
    out({"success": True, "contract_id": contract, "deployed": True, "gas_spent": 6899157, "tx_id": "dd" * 32})
if args[0] == "enroll-router":
    queries[f"{opt('--warp-contract')}/enrolled_router/{opt('--domain')}"] = {"value": opt("--router")}
    save()
    out({"success": True, "tx_id": "ab" * 32})
if args[0] == "set-igp-config":
    set_pricing(opt("--igp"), opt("--igp-domain-config"))
    save()
    out({"success": True, "tx_id": "cd" * 32, "block_height": 7})
if args[0] == "set-beneficiary":
    queries[f"{opt('--contract')}/beneficiary"] = {"value": account_hash(opt("--beneficiary"))}
    save()
    out({"success": True, "tx_id": "ce" * 32})
# A token route's token and route, at IDs the deploy nonce gives. A token
# exists when FIXTURE_CHAIN's "tokens" has it, a route when a query names it.
tokens = chain.setdefault("tokens", {})
def route_ids(nonce):
    return (hashlib.sha256(f"token:{nonce}".encode()).hexdigest(),
            hashlib.sha256(f"route:{nonce}".encode()).hexdigest())
def route_exists(route):
    return any(key.startswith(route + "/") for key in queries)
if args[0] == "deploy-token-route":
    nonce = int(opt("--deploy-nonce"))
    token, route = route_ids(nonce)
    exists = {"token": token in tokens, "route": route_exists(route)}
    contracts = {"token": token, "route": route}
    if "--plan" in args:
        out({"success": True, "deploy_nonce": nonce, "contracts": contracts, "exists": exists})
    for flag, value in (("--expected-token", token), ("--expected-route", route)):
        if opt(flag) is not None and opt(flag) != value:
            out({"success": False, "error": f"The Wasm gives {value}, not {opt(flag)}, which was planned: nothing was sent"}, 1)
    supply = opt("--token-supply")
    # FIXTURE_CHAIN's "txs" is the ledger of the stub's deploy transactions,
    # {hash: {"id": contract, "executed": bool}}. A contract at a planned ID
    # is this deployment's only if --token-tx or --route-tx names one that
    # executed: deploy-token-route refuses any other, sending nothing more,
    # and names it as "taken".
    txs = chain.setdefault("txs", {})
    def taken(what, reason):
        save()
        out({"success": False, "taken": what, "deploy_nonce": nonce, "contracts": contracts,
             "error": f"The {what}'s planned ID {contracts[what]} isn't this deployment's: {reason}. "
                      "Nothing more was sent: plan the route at other deploy nonces."}, 1)
    ours = {}
    for what in ("token", "route"):
        recorded = txs.get(opt(f"--{what}-tx") or "")
        ours[what] = bool(recorded and recorded["executed"] and recorded["id"] == contracts[what])
        if exists[what] and not ours[what]:
            taken(what, "a contract is at its planned ID that no transaction of this deployment's created")
    def occupy(what):
        # Someone else's contract, as deploy-token-route would have made it
        # but with another admin.
        if what == "token":
            tokens[token] = {"name": opt("--token-name"), "symbol": opt("--token-symbol"),
                             "decimals": int(opt("--token-decimals")), "total_supply": supply, "cap": "0",
                             "paused": False, "accounts": {"ee" * 96: {
                                 "balance": supply, "admin": True, "minter": True, "pauser": True}}}
        else:
            queries[f"{route}/owner"] = {"value": "ee" * 32}
    def send(what):
        # The transaction goes to --tx-log before it is sent. FIXTURE_CHAIN's
        # "front_run_deploy", [id, ...], lands someone else's contract at
        # each of those IDs right before this deployment's deploy to it,
        # which then fails.
        chain["deploys_sent"] = chain.get("deploys_sent", 0) + 1
        tx = hashlib.sha256(f"deploy:{chain['deploys_sent']}".encode()).hexdigest()
        if opt("--tx-log"):
            with open(opt("--tx-log"), "a") as log:
                log.write(json.dumps({"contract": what, "id": contracts[what], "tx": tx,
                                      "nonce": chain["deploys_sent"]}) + "\n")
        front = chain.get("front_run_deploy") or []
        if contracts[what] in front:
            front.remove(contracts[what])
            occupy(what)
            txs[tx] = {"id": contracts[what], "executed": False}
            taken(what, f"this deployment's transaction {tx} deploying it failed (contract already exists): "
                        "another contract got to its ID first")
        txs[tx] = {"id": contracts[what], "executed": True}
    if not exists["token"]:
        send("token")
        tokens[token] = {"name": opt("--token-name"), "symbol": opt("--token-symbol"),
                         "decimals": int(opt("--token-decimals")), "total_supply": supply, "cap": "0",
                         "paused": False, "accounts": {chain["deployer_key"]: {
                             "balance": supply, "admin": True, "minter": True, "pauser": True}}}
        # FIXTURE_CHAIN's "stop_after_token" plays a run that stops once the
        # token is deployed, before the route is sent.
        if chain.pop("stop_after_token", False):
            save()
            out({"success": False, "error": "stub dusk-tx: stopped after the token, as the fixture asked"}, 1)
    if not exists["route"]:
        send("route")
        for method, value in (("state_version", 7), ("owner", chain["deployer_h256"]), ("mailbox", opt("--mailbox")),
                              ("wrapped_token", token), ("hook", "00" * 32),
                              ("interchain_security_module", "00" * 32), ("pending_total", 0)):
            queries[f"{route}/{method}"] = {"value": value}
    # FIXTURE_CHAIN's "tamper", {"token": {...}, "route": {...}}, leaves the
    # contracts at the IDs reading back otherwise than as sent: token
    # fields, and route queries by method, that differ.
    tamper = chain.pop("tamper", None) or {}
    for key, value in tamper.get("token", {}).items():
        if key.startswith("account."):
            tokens[token]["accounts"][chain["deployer_key"]][key[len("account."):]] = value
        else:
            tokens[token][key] = value
    for method, value in tamper.get("route", {}).items():
        queries[f"{route}/{method}"] = value
    save()
    out({"success": True, "chain_id": int(opt("--expected-chain-id")), "deploy_nonce": nonce, "contracts": contracts,
         "deployed": {"token": not exists["token"], "route": not exists["route"]}, "read_back": 25})
if args[0] == "drc20-info":
    info = tokens.get(opt("--token"))
    if info is None or "error" in info:
        out({"success": False, "error": (info or {}).get("error", "Can't read the token's name (is it a canonical DRC20?)")}, 1)
    result = {"success": True, "token": opt("--token"), **{key: info[key] for key in
              ("name", "symbol", "decimals", "total_supply", "cap", "paused")}}
    if opt("--account") is not None:
        result["account"] = info["accounts"].get(opt("--account"), {
            "balance": "0", "admin": False, "minter": False, "pauser": False})
    out(result)
if args[0] == "claim-fees":
    key = f"{opt('--contract')}/claimable_fees"
    claimed = queries[key]["value"]
    queries[key] = {"value": 0}
    save()
    out({"success": True, "claimed": str(claimed), "claimed_dusk": f"{claimed / 1e9:.9f}",
         "tx_id": "cf" * 32})
out({"success": False, "error": f"stub dusk-tx: unexpected {args[0]}"}, 1)
'''

# mktemp on a full disk: the temporary record deploy/testnet.sh writes,
# .state.XXXXXX in the deployment's directory, is a link to /dev/full, where
# every write fails. Any other template gets the real mktemp's file.
MKTEMP_FULL = r'''#!/usr/bin/env python3
import os, sys
directory, name = os.path.split(sys.argv[-1])
if name.startswith(".state."):
    path = os.path.join(directory, ".state.full." + str(os.getpid()))
    os.symlink("/dev/full", path)
    print(path)
else:
    os.execv("@REAL@", ["mktemp", *sys.argv[1:]])
'''

# A fake EVM chain for cast and forge: contracts are JSON objects in
# FIXTURE_EVM, calls read them and sends change them, as far as the scripts
# use Hyperlane's Mailbox, hooks, IGP, gas oracle, factories, HypERC20 and
# OpenZeppelin's ERC1967Proxy. Every transaction leaves a receipt with the
# events the scripts look for. `cast calldata` and `cast abi-encode` encode
# as hex of JSON, which only this fake reads back.
#
# FIXTURE_EVM's "lag", "stale_latest", "pinned_misses" and "nonce_refusals"
# play a load-balanced endpoint whose nodes are a block or two apart, as the
# public Sepolia endpoints are (went_stale and the reads below).
#
# FIXTURE_EVM's "front_run" plays an attacker whose transaction lands right
# after the deployer creates a bare IGP or HypERC20, before anything else the
# deployer sends: {"attacker": address, "igp": mode, "token": mode}, with igp
# mode "owner" (initialize(attacker, attacker)), "beneficiary"
# (initialize(deployer, attacker)) or "standing_quote" (initialize with a
# helper owner, a quote signer that sets a free standing quote for any
# sender to Dusk, the signer removed and the IGP handed to the deployer),
# and token mode "owner" (initialize with the attacker as owner and a supply
# minted to it). Its "interleave" changes the token between two reads of its
# routers, as another process could (interleave() below).
FAKE_EVM = r"""
import hashlib, json, os, re, sys

ZERO = "0x" + "00" * 20
WILDCARD_SENDER = "0x" + "ff" * 20
WILDCARD_DEST = str(2**32 - 1)
SCALE = 10**10
DUSK_DOMAIN = "1146442572"
NOW = 1_800_000_000
# Event topics: Initialized(uint8), Upgraded(address),
# OwnershipTransferred(address,address), BeneficiarySet(address).
INITIALIZED = "0x7f26b83ff96e1f2b6a682f133852f6798a09c465da95921460cefb3847402498"
UPGRADED = "0xbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b"
OWNERSHIP_TRANSFERRED = "0x8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e0"
BENEFICIARY_SET = "0x04d55a8be181fb8d75b76f2d48aa0b2ee40f47e53d6e61763eeeec46feea8a24"
# QuoteSignerAdded(address), QuoteSignerRemoved(address).
QUOTE_SIGNER_ADDED = "0x1999e59a6c4f2d603b5236b4b6a9ab4caecb598d4a59fe663b6eae45ef79a3d6"
QUOTE_SIGNER_REMOVED = "0xf1e7459dfbea75ed7b558903ed9cf48b6c71a30d598a87ca5335d87663a2f70a"
IMPLEMENTATION_SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
ADMIN_SLOT = "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103"
# What `forge inspect` gives as ERC1967Proxy's creation code.
PROXY_CODE = "0x" + b"ERC1967Proxy".hex()

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

def word(address):
    return "0x" + "00" * 12 + address.lower()[2:]

def encode(value):
    return "0x" + json.dumps(value).encode().hex()

def decode(data):
    return json.loads(bytes.fromhex(data[2:] if data.startswith("0x") else data))

def split(args):
    positional, options, flags = [], {}, set()
    i = 0
    while i < len(args):
        arg = args[i]
        if arg in ("--rpc-url", "--private-key", "--value", "--from", "--create", "--gas-limit",
                   "--from-block", "--to-block", "--address", "--block", "--nonce"):
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

def log(address, *topics):
    return {"address": address.lower(), "topics": list(topics), "data": "0x"}

def multisig(args):
    # A MessageIdMultisig ISM of "[validator,...]" and a threshold.
    validators = [v.strip().lower() for v in args[0].strip("[]").split(",") if v.strip()]
    return {"type": "ism", "module_type": 5, "validators": validators, "threshold": int(args[1])}

def igp_rate(state, c, domain, sender):
    # Signed offchain quotes come first: for the sender to the domain, for
    # anyone to the domain, for the sender to anywhere.
    for key in (f"{domain}/{sender.lower()}", f"{domain}/{WILDCARD_SENDER}", f"{WILDCARD_DEST}/{sender.lower()}"):
        quote = c.get("quotes", {}).get(key)
        if quote and quote["expiry"] >= NOW:
            return quote["rate"], quote["price"]
    oracle, overhead = c["configs"].get(str(domain), [ZERO, 0])
    if oracle == ZERO:
        die("execution reverted: IGP: no gas oracle for domain")
    return contract(state, oracle)["data"].get(str(domain), [0, 0])

def routed(state, c, domain):
    route = c.get("routes", {}).get(str(domain), ZERO)
    if route == ZERO and c["type"] == "fallback_routing":
        route = c["fallback"]
    if route == ZERO:
        die(f"execution reverted: No hook configured for destination: {domain}")
    return route

def hook_quote(state, hook, domain, gas_limit, sender):
    if hook.lower() == ZERO:
        return 0
    c = contract(state, hook)
    if c["type"] == "aggregation":
        return sum(hook_quote(state, child, domain, gas_limit, sender) for child in c["hooks"])
    if c["type"] in ("routing", "fallback_routing"):
        return hook_quote(state, routed(state, c, domain), domain, gas_limit, sender)
    if c["type"] == "igp":
        rate, price = igp_rate(state, c, domain, sender)
        overhead = c["configs"].get(str(domain), [ZERO, 0])[1]
        return (gas_limit + overhead) * price * rate // SCALE
    return c.get("fee", 0)

def interleave(state, address):
    # FIXTURE_EVM's "interleave", {"token": address, "after_reads": n,
    # "change": {...}}, makes another process's change to the token once its
    # routers have been read n times, before the next read; "reads" counts
    # them.
    plan = state.get("interleave")
    if not plan or plan["token"] != address.lower():
        return
    plan["reads"] = plan.get("reads", 0) + 1
    if plan["reads"] == plan["after_reads"] + 1:
        state["contracts"][address.lower()].update(plan["change"])
    state["changed"] = True

def call(state, address, signature, args, sender):
    c = contract(state, address)
    name = signature.split("(")[0]
    if name == "hookType" and "hook_type" not in c:
        die("execution reverted")
    if name in ("hookType", "localDomain"):
        return str(c["hook_type" if name == "hookType" else "domain"])
    if name == "protocolFee" and "fee" in c:
        return num(c["fee"])
    if name in ("mailbox", "requiredHook", "defaultHook", "beneficiary"):
        return c[name]
    if name == "owner":
        return c.get("owner", ZERO)
    if name == "hook":
        return c.get("hook", ZERO)
    if name == "interchainSecurityModule":
        return c.get("ism", ZERO)
    if name == "remoteGasData":
        rate, price = c["data"].get(args[0], [0, 0])
        return f"{num(rate)}\n{num(price)}"
    if name == "destinationGasConfigs":
        oracle, overhead = c["configs"].get(args[0], [ZERO, 0])
        return f"{oracle}\n{num(overhead)}"
    if name == "quoteSigners":
        return "[" + ", ".join(c.get("quote_signers", [])) + "]"
    if name == "quoteGasPayment" and c["type"] == "igp":
        rate, price = igp_rate(state, c, args[0], sender)
        return num(int(args[1]) * price * rate // SCALE)
    if name == "quoteGasPayment" and c["type"] == "token":
        mailbox = contract(state, c["mailbox"])
        return num(hook_quote(state, mailbox["requiredHook"], args[0], 0, address)
                   + hook_quote(state, c["hook"], args[0], 0, address))
    if name == "hooks" and signature.startswith("hooks(uint32)"):
        return c.get("routes", {}).get(args[0], ZERO)
    if name == "hooks":
        return "[" + ", ".join(c["hooks"]) + "]"
    if name == "fallbackHook":
        return c["fallback"]
    if name == "getAddress" and c["type"] == "aggregation_factory":
        return derive("aggregation", address.lower(), args[0].lower())
    if name == "getAddress" and c["type"] == "ism_factory":
        return derive("ism", address.lower(), args[0].lower(), args[1])
    if name == "routers":
        interleave(state, address)
        return c["routers"].get(args[0], "0x" + "00" * 32)
    if name == "moduleType" and c["type"] == "ism":
        return str(c["module_type"])
    if name == "validatorsAndThreshold" and c["type"] == "ism":
        return "[" + ", ".join(c["validators"]) + "]\n" + str(c["threshold"])
    if name == "totalSupply":
        return num(c.get("supply", 0))
    if name in ("name", "symbol") and c["type"] == "token":
        return ("string", c.get(name, ""))
    if name == "decimals" and c["type"] == "token":
        return str(c.get("decimals", 18))
    if name in ("scaleNumerator", "scaleDenominator") and c["type"] == "token":
        return str(c.get("scale", [1, 1])[0 if name == "scaleNumerator" else 1])
    die(f"stub cast: unexpected call {signature} on a {c['type']}")

def only_owner(c, sender):
    if c.get("owner", ZERO).lower() != sender.lower():
        die("execution reverted: Ownable: caller is not the owner")

def initialize(c, address, sender, args):
    if c.get("initialized"):
        die("execution reverted: Initializable: contract is already initialized")
    c["initialized"] = True
    logs = [log(address, OWNERSHIP_TRANSFERRED, word(c.get("owner", ZERO)), word(sender))]
    if c["type"] == "igp":
        c["owner"], c["beneficiary"] = args[0].lower(), args[1].lower()
        logs.append(log(address, OWNERSHIP_TRANSFERRED, word(sender), word(c["owner"])))
        logs.append(log(address, BENEFICIARY_SET))
    elif c["type"] == "token":
        c["supply"] = c.get("supply", 0) + int(args[0])
        c["holders"] = {sender.lower(): int(args[0])} if int(args[0]) else {}
        c.update(name=args[1], symbol=args[2], hook=args[3].lower(), ism=args[4].lower(), owner=args[5].lower())
        logs.append(log(address, OWNERSHIP_TRANSFERRED, word(sender), word(c["owner"])))
    else:
        die(f"stub cast: unexpected initialize of a {c['type']}")
    return logs + [log(address, INITIALIZED)]

def send(state, address, signature, args, sender):
    c = contract(state, address)
    name = signature.split("(")[0]
    logs = []
    if name == "setRemoteGasDataConfigs":
        only_owner(c, sender)
        for domain, rate, price in tuples(args[0]):
            c["data"][domain] = [int(rate), int(price)]
    elif name == "initialize":
        logs = initialize(c, address, sender, args)
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
            "type": "aggregation", "hook_type": 2, "hooks": hooks, "created_at": state["block"]}
    elif name == "deploy" and c["type"] == "ism_factory":
        state["contracts"][derive("ism", address.lower(), args[0].lower(), args[1])] = {
            **multisig(args), "created_at": state["block"]}
    elif name == "addQuoteSigner":
        only_owner(c, sender)
        c["quote_signers"].append(args[0].lower())
        logs.append(log(address, QUOTE_SIGNER_ADDED, word(args[0])))
    elif name == "removeQuoteSigner":
        only_owner(c, sender)
        c["quote_signers"].remove(args[0].lower())
        logs.append(log(address, QUOTE_SIGNER_REMOVED, word(args[0])))
    elif name == "setHook":
        only_owner(c, sender)
        c["hook"] = args[0].lower()
    elif name == "enrollRemoteRouter":
        only_owner(c, sender)
        c["routers"][args[0]] = args[1].lower()
    else:
        die(f"stub cast: unexpected send {signature} to a {c['type']}")
    return logs

def receipt(state, sender, to=None, created=None, logs=(), data="0x"):
    state["txs"] = state.get("txs", 0) + 1
    tx = "0x" + hashlib.sha256(f"tx:{state['txs']}".encode()).hexdigest()
    nonces = state.setdefault("account_nonces", {})
    nonce = nonces.get(sender.lower(), 0)
    nonces[sender.lower()] = nonce + 1
    state.setdefault("receipts", {})[tx] = {
        "transactionHash": tx, "status": "0x1", "from": sender.lower(),
        "to": to.lower() if to else None, "contractAddress": created,
        "blockNumber": hex(state["block"]), "blockHash": block_hash(state, state["block"]),
        "logs": list(logs), "nonce": nonce, "input": data}
    state["block"] += 1
    return tx

def block_hash(state, number):
    # A block's hash, which changes each time the blocks from a height at
    # or below it are replaced ("reorg_from").
    replaced = len([start for start in state.get("reorgs", []) if start <= number])
    return "0x" + hashlib.sha256(f"block:{number}:{replaced}".encode()).hexdigest()

def finalized(state):
    # "finality_depth", n: the final block is n below the head.
    return max(0, state["block"] - state.get("finality_depth", 0))

def reorg(state):
    # "reorg_from", b: the blocks from b on are replaced by blocks without
    # the fillers in them (nonce_refusals), and with the held transactions
    # that can then be mined, with their nonces. Only fillers' blocks are
    # replaced: they change nothing but their sender's nonce.
    start = state.pop("reorg_from", None)
    if start is None:
        return
    nonces = state.setdefault("account_nonces", {})
    for tx, r in list(state.get("receipts", {}).items()):
        if int(r["blockNumber"], 16) >= start:
            assert r.get("filler"), "the fake EVM replaces only fillers' blocks"
            del state["receipts"][tx]
            nonces[r["from"]] = min(nonces.get(r["from"], 0), r["nonce"])
    state.setdefault("reorgs", []).append(start)
    state["block"] = start
    state["mine_held"] = True
    mine_held(state)

def fill_held(state):
    # "fill_held": another transaction of each held one's sender, a filler,
    # takes its nonce, and the held one is never mined.
    if not state.pop("fill_held", False):
        return
    for item in sorted(state.get("held", []), key=lambda h: h["nonce"]):
        if item["nonce"] == state.setdefault("account_nonces", {}).get(item["sender"], 0):
            tx = receipt(state, item["sender"], to=ZERO, data="0xf111e4")
            state["receipts"][tx]["filler"] = True
    save(state)

def fail_send(state, kind, sender, payload):
    # The held transaction, mined but failed ("fail_held"): its nonce is
    # spent, and nothing else changes.
    if kind == "forge":
        to, data = None, creation_input(payload["name"], payload["args"])
    elif kind == "create":
        to, data = None, payload["data"]
    else:
        to, data = payload["to"], encode({"sig": payload["sig"], "args": payload["args"]})
    tx = receipt(state, sender, to=to, data=data)
    state["receipts"][tx]["status"] = "0x0"
    return tx

def pending_nonce(state, sender):
    # The sender's next nonce as a node with the held transactions in its
    # pool gives it: past its mined transactions, and past each held one
    # that follows them.
    nonce = state.get("account_nonces", {}).get(sender.lower(), 0)
    held_nonces = {h["nonce"] for h in state.get("held", []) if h["sender"] == sender.lower()}
    while nonce in held_nonces:
        nonce += 1
    return nonce

def check_nonce(state, sender, options):
    # A transaction sent with --nonce: refused below the sender's next one,
    # as a node does.
    if "--nonce" not in options:
        return
    expected = pending_nonce(state, sender)
    given = int(options["--nonce"])
    if given < expected:
        die(f"Error: server returned an error response: error code -32000: nonce too low: next nonce {expected}, tx nonce {given}")
    if given > expected:
        die(f"Error: server returned an error response: error code -32000: nonce too high: next nonce {expected}, tx nonce {given}")

def apply_send(state, kind, sender, payload):
    # Make a transaction happen: a forge creation, a proxy creation or a
    # call, with the sender's next nonce. Returns its hash and, for a forge
    # creation, the address.
    before = json.loads(json.dumps(state["contracts"]))
    address = None
    if kind == "forge":
        name, args = payload["name"], payload["args"]
        address = new_address(state, sender)
        created = CREATED[name](sender, args)
        created["created_at"] = state["block"]
        state["contracts"][address] = created
        logs = ([log(address, OWNERSHIP_TRANSFERRED, word(ZERO), word(sender))]
                if created.get("owner", ZERO) != ZERO else [])
        tx = receipt(state, sender, created=address, logs=logs, data=creation_input(name, args))
        record_history(state, tx, before)
        created_only = json.loads(json.dumps(state["contracts"]))
        attack = front_run(state, sender, address, created)
        if attack:
            record_history(state, attack, created_only)
    elif kind == "create":
        tx = create_proxy(state, sender, payload["data"])
        record_history(state, tx, before)
    else:
        logs = send(state, payload["to"], payload["sig"], payload["args"], sender)
        tx = receipt(state, sender, to=payload["to"], logs=logs,
                     data=encode({"sig": payload["sig"], "args": payload["args"]}))
        record_history(state, tx, before)
    went_stale(state, before)
    return tx, address

def held(state, sender, kind, payload, options):
    # "hold", n: the next n transactions sent are taken but not mined, and
    # the send times out, as one a gateway passed on and lost track of does.
    # Each keeps the nonce it was sent with (cast nonce --block pending
    # counts it), and is mined with it once "mine_held" is set, or never,
    # if another transaction has taken it. "hold_skip", n: the first n
    # sends go through.
    if state.get("hold", 0) <= 0:
        return False
    if state.get("hold_skip", 0) > 0:
        state["hold_skip"] -= 1
        save(state)
        return False
    state["hold"] -= 1
    hold(state, sender, kind, payload, options)
    save(state)
    die("Error: error sending request for url (http://fixture): operation timed out")

def hold(state, sender, kind, payload, options):
    nonce = int(options["--nonce"]) if "--nonce" in options else pending_nonce(state, sender)
    state.setdefault("held", []).append(
        {"sender": sender.lower(), "kind": kind, "payload": payload, "nonce": nonce})

def mine_held(state):
    # Each held transaction is mined when its sender's next nonce is its
    # own, and dropped once another transaction has taken it.
    if not state.pop("mine_held", False):
        return
    waiting = state.pop("held", [])
    while True:
        nonces = state.setdefault("account_nonces", {})
        waiting = [h for h in waiting if h["nonce"] >= nonces.get(h["sender"], 0)]
        ready = [h for h in waiting if h["nonce"] == nonces.get(h["sender"], 0)]
        if not ready:
            break
        item = ready[0]
        waiting.remove(item)
        if state.get("fail_held"):
            tx = fail_send(state, item["kind"], item["sender"], item["payload"])
        else:
            tx, _ = apply_send(state, item["kind"], item["sender"], item["payload"])
        assert state["receipts"][tx]["nonce"] == item["nonce"]
    if waiting:
        state["held"] = waiting
    # "after_mining": balances that grow once the held transactions are
    # mined, as fees paid after them do.
    for address, wei in state.pop("after_mining", {}).items():
        state["balances"][address] = state["balances"].get(address, 0) + wei
    save(state)

def accept_then_refuse(state):
    # "accept_then_refuse", n: the next n transactions sent, after the first
    # "accept_then_refuse_skip", are taken and mined, but answered with
    # "nonce too low", as a gateway that sent one on answers a replay of the
    # request.
    if state.get("accept_then_refuse_skip", 0) > 0:
        state["accept_then_refuse_skip"] -= 1
        return False
    if state.get("accept_then_refuse", 0) > 0:
        state["accept_then_refuse"] -= 1
        return True
    return False

# What `forge inspect` gives as the creation code and constructor of the
# contracts the scripts create with forge create.
CONSTRUCTOR_TYPES = {"HypERC20": "uint8,uint256,uint256,address",
                     "StorageMessageIdMultisigIsm": "address[],uint8"}

def creation_code(name):
    return "0x" + name.encode().hex()

def creation_input(name, args):
    if not args:
        return creation_code(name)
    return creation_code(name) + encode({"sig": f"constructor({CONSTRUCTOR_TYPES[name]})", "args": args})[2:]

def create_address(sender, nonce):
    # The address a sender's creation with that nonce gets, from those two
    # alone, as CREATE's does.
    return derive("create", sender.lower(), nonce)

def new_address(state, sender):
    # The address of the creation the sender sends next. "wrong_address", n:
    # the next n creations land, and their receipts say they landed,
    # somewhere else, as a node misreporting them would show.
    nonce = state.get("account_nonces", {}).get(sender.lower(), 0)
    if state.get("wrong_address", 0) > 0:
        state["wrong_address"] -= 1
        return derive("elsewhere", sender.lower(), nonce)
    return create_address(sender, nonce)

CREATED = {
    "StorageGasOracle": lambda sender, args: {"type": "oracle", "owner": sender, "data": {}},
    "InterchainGasPaymaster": lambda sender, args: {
        "type": "igp", "hook_type": 4, "owner": ZERO, "beneficiary": ZERO, "configs": {},
        "quote_signers": [], "quotes": {}},
    "StaticAggregationHookFactory": lambda sender, args: {"type": "aggregation_factory"},
    # MailboxClient's constructor makes the deployer the owner, but leaves
    # initialize open.
    "HypERC20": lambda sender, args: {
        "type": "token", "mailbox": args[3].lower(), "hook": ZERO, "ism": ZERO, "owner": sender,
        "routers": {}, "supply": 0, "decimals": int(args[0]), "scale": [int(args[1]), int(args[2])]},
    "StorageMessageIdMultisigIsm": lambda sender, args: multisig(args),
}

def front_run(state, deployer, address, c):
    attack = state.get("front_run") or {}
    attacker, mode = attack.get("attacker"), attack.get(c["type"])
    if not mode:
        return
    if c["type"] == "igp" and mode == "owner":
        logs = initialize(c, address, attacker, [attacker, attacker])
    elif c["type"] == "igp" and mode == "beneficiary":
        logs = initialize(c, address, attacker, [deployer, attacker])
    elif c["type"] == "igp" and mode == "standing_quote":
        helper = derive("helper", attacker)
        logs = initialize(c, address, attacker, [helper, deployer])
        # The helper adds a signer, which signs a free standing quote for any
        # sender to Dusk; then the signer goes and the deployer gets the IGP.
        c["quotes"][f"{DUSK_DOMAIN}/{WILDCARD_SENDER}"] = {"rate": 0, "price": 0, "expiry": 2**47}
        c["owner"] = deployer.lower()
    elif c["type"] == "token" and mode == "owner":
        logs = initialize(c, address, attacker, [10**30, "Dusk", "DUSK", ZERO, attacker, attacker])
    else:
        die(f"fake EVM: unknown front_run {mode} for a {c['type']}")
    return receipt(state, attacker, to=address, logs=logs)

def create_proxy(state, sender, data):
    if not data.startswith(PROXY_CODE):
        die("stub cast: --create is only for ERC1967Proxy here")
    implementation, init = decode(data[len(PROXY_CODE):])["args"]
    impl = contract(state, implementation)
    address = new_address(state, sender)
    # The proxy's own storage: nothing set yet but the implementation's
    # immutables.
    proxy = CREATED[{"igp": "InterchainGasPaymaster", "token": "HypERC20"}[impl["type"]]](
        sender, [impl.get("decimals", 18), *impl.get("scale", [1, 1]), impl.get("mailbox", ZERO)])
    proxy.update(owner=ZERO, implementation=implementation.lower(), created_at=state["block"])
    logs = [log(address, UPGRADED, word(implementation))]
    if init != "0x":
        call_data = decode(init)
        logs += initialize(proxy, address, sender, call_data["args"])
    state["contracts"][address] = proxy
    return receipt(state, sender, created=address, logs=logs, data=data)

def went_stale(state, before):
    # After each write:
    # - "lag", n: the next n reads of the head, of code or of receipts answer
    #   from a node a block behind, which hasn't seen the write, and so does a
    #   read at a block (cast --block), which that node doesn't have yet;
    # - "stale_latest", n: the head is current, but the next n reads of code,
    #   storage, a call or a receipt at the latest block answer from before
    #   the write, as a node behind the one that answered the head does;
    # - "pinned_misses", n: the next n reads at a block fail as from a node
    #   that doesn't have it yet;
    # `before` is the contracts as they were before the write.
    if state.get("lag"):
        state["stale_left"] = state["lag"]
        state["stale_head"] = state["block"] - 2
    if state.get("stale_latest"):
        state["latest_stale_left"] = state["stale_latest"]
        state["latest_stale"] = {"contracts": before, "block": state["block"] - 2}
    if state.get("pinned_misses"):
        state["pinned_misses_left"] = state["pinned_misses"]

def stale_read(state):
    if state.get("stale_left", 0) <= 0:
        return None
    state["stale_left"] -= 1
    save(state)
    return state["stale_head"]

HEADER_NOT_FOUND = "Error: server returned an error response: error code -32000: header not found"

def record_history(state, tx, before):
    # What a transaction changed, to read the chain as of a block before it:
    # each contract it changed, as it was before ("history", [block, undo]).
    block = int(state["receipts"][tx]["blockNumber"], 16)
    undo = {address: before.get(address) for address in set(before) | set(state["contracts"])
            if before.get(address) != state["contracts"].get(address)}
    state.setdefault("history", []).append([block, undo])

def as_of(state, block):
    # The contracts as of the end of `block`: the current ones, with every
    # later transaction's changes undone.
    contracts = json.loads(json.dumps(state["contracts"]))
    for changed_at, undo in reversed(state.get("history", [])):
        if changed_at <= block:
            break
        for address, previous in undo.items():
            if previous is None:
                contracts.pop(address, None)
            else:
                contracts[address] = previous
    return contracts

def note_early_read(state, block):
    # A read made at a block before the deployer's last transaction's, which
    # a script that checks its own write must never make ("early_pinned_reads").
    deployer_blocks = [int(r["blockNumber"], 16) for r in state.get("receipts", {}).values()
                       if r["from"] in {k.lower() for k in state["keys"].values()}]
    if deployer_blocks and block < max(deployer_blocks):
        state["early_pinned_reads"] = state.get("early_pinned_reads", 0) + 1
        save(state)

def read_view(state, options):
    # What a read of code, storage, a call or a receipt answers from: the
    # state as of the block it names (cast --block), which the node must
    # have, or as of the latest block, which "stale_latest" can make a
    # node's from before the last write. Returns that state and the newest
    # block it has seen.
    block = options.get("--block")
    if block not in (None, "latest"):
        # The head, as block-number answers it, is state["block"].
        if int(block) > state["block"]:
            die(HEADER_NOT_FOUND)
        if state.get("pinned_misses_left", 0) > 0:
            state["pinned_misses_left"] -= 1
            save(state)
            die(HEADER_NOT_FOUND)
        if stale_read(state) is not None:
            die(HEADER_NOT_FOUND)
        note_early_read(state, int(block))
        # The chain as of that block, not as it is now, if anything changed
        # after it.
        history = state.get("history", [])
        if not history or history[-1][0] <= int(block):
            return state, state["block"]
        return {**state, "contracts": as_of(state, int(block))}, state["block"]
    if state.get("latest_stale_left", 0) > 0:
        state["latest_stale_left"] -= 1
        save(state)
        return {**state, "contracts": state["latest_stale"]["contracts"]}, state["latest_stale"]["block"]
    return state, None

def refuse_stale_nonce(state, sender, kind, payload, options):
    # "nonce_refusals", n: the next n transactions sent, after the first
    # "nonce_refusals_skip", find their nonce taken, by an earlier
    # transaction of the sender's that a node behind showed late (a
    # "filler"), and are refused for it, changing nothing else. With
    # "nonce_refusals_hold", a node holds the refused transaction anyway,
    # with its nonce, as one a gateway passed on before the refusal does:
    # it is mined if another block replaces the filler's (reorg_from).
    if state.get("nonce_refusals_skip", 0) > 0:
        state["nonce_refusals_skip"] -= 1
        save(state)
        return
    if state.get("nonce_refusals", 0) > 0:
        state["nonce_refusals"] -= 1
        tx = receipt(state, sender, to=ZERO, data="0xf111e4")
        state["receipts"][tx]["filler"] = True
        if state.get("nonce_refusals_hold"):
            hold(state, sender, kind, payload, options)
        save(state)
        die("Error: server returned an error response: error code -32000: nonce too low")

def main_cast(argv):
    state = load()
    reorg(state)
    fill_held(state)
    mine_held(state)
    positional, options, flags = split(argv)
    trace("cast " + " ".join(positional + ([f"--create {options['--create'][:40]}..."] if "--create" in options else [])
                             + ([f"--from {options['--from']}"] if "--from" in options else [])
                             + ([f"--block {options['--block']}"] if "--block" in options else [])))
    command = positional[0]
    sender = state["keys"].get(options.get("--private-key", ""), ZERO)
    if command == "chain-id":
        print(state["chain_id"])
    elif command == "nonce":
        # "stale_nonce_reads", n: the next n reads answer one below, as a
        # node that hasn't seen the sender's last transaction does.
        nonce = state.get("account_nonces", {}).get(positional[1].lower(), 0)
        if options.get("--block") == "pending":
            nonce = pending_nonce(state, positional[1])
        if state.get("stale_nonce_reads", 0) > 0:
            state["stale_nonce_reads"] -= 1
            save(state)
            nonce = max(nonce - 1, 0)
        print(nonce)
    elif command == "compute-address":
        print(f"Computed Address: {create_address(positional[1], int(options['--nonce']))}")
    elif command == "block" and "--full" in flags:
        number = int(positional[1])
        found = [{"hash": tx, "from": r["from"], "to": r["to"], "nonce": hex(r.get("nonce", 0)),
                  "input": r.get("input", "0x"), "blockNumber": r["blockNumber"], "blockHash": r["blockHash"]}
                 for tx, r in state.get("receipts", {}).items() if int(r["blockNumber"], 16) == number]
        if number > state["block"]:
            die(f"Error: block {number} not found")
        print(json.dumps({"number": hex(number), "hash": block_hash(state, number), "transactions": found}))
    elif command == "block":
        number = finalized(state) if positional[1] == "finalized" else int(positional[1])
        if number > state["block"]:
            die(f"Error: block {number} not found")
        print(json.dumps({"number": hex(number), "hash": block_hash(state, number)}))
    elif command == "block-number" and positional[1:] == ["finalized"]:
        print(finalized(state))
    elif command == "block-number":
        stale = stale_read(state)
        print(state["block"] if stale is None else stale)
    elif command == "code":
        view, _ = read_view(state, options)
        stale = stale_read(state) if view is state and "--block" not in options else None
        c = view["contracts"].get(positional[1].lower())
        visible = c is not None and (stale is None or c.get("created_at", -1) <= stale)
        print("0x6080" if visible else "0x")
    elif command == "balance":
        wei = state["balances"].get(positional[1].lower(), 0)
        print(f"{wei / 10**18:.18f}" if "--ether" in flags else wei)
    elif command == "wallet":
        print(state["keys"][options["--private-key"]])
    elif command in ("calldata", "abi-encode"):
        print(encode({"sig": positional[1], "args": positional[2:]}))
    elif command == "storage":
        view, _ = read_view(state, options)
        c = contract(view, positional[1])
        if positional[2].lower() == IMPLEMENTATION_SLOT and "implementation" in c:
            print(word(c["implementation"]))
        elif positional[2].lower() == ADMIN_SLOT and "admin" in c:
            print(word(c["admin"]))
        else:
            print("0x" + "00" * 32)
    elif command == "receipt":
        found = state.get("receipts", {}).get(positional[1].lower())
        _, seen = read_view(state, options)
        if found is not None and seen is not None and int(found["blockNumber"], 16) > seen:
            found = None
        stale = stale_read(state)
        if found is not None and stale is not None and int(found["blockNumber"], 16) > stale:
            found = None
        if found is None:
            die(f"Error: transaction {positional[1]} not found")
        print(json.dumps(found) if "--json" in flags else found[positional[2]])
    elif command == "call":
        view, _ = read_view(state, options)
        result = call(view, positional[1], positional[2], positional[3:], options.get("--from", ZERO))
        if isinstance(result, tuple):
            # cast prints a string quoted, and with --json in a list.
            result = json.dumps([result[1]]) if "--json" in flags else json.dumps(result[1])
        print(result)
        if view is state and state.pop("changed", False):
            save(state)
    elif command == "logs":
        start, end = int(options["--from-block"]), int(options["--to-block"])
        state.setdefault("log_queries", []).append([start, end])
        save(state)
        found = []
        for tx, r in state.get("receipts", {}).items():
            block = int(r["blockNumber"], 16)
            for entry in r["logs"]:
                if (start <= block <= end and entry["address"] == options["--address"].lower()
                        and entry["topics"][0] == positional[1].lower()):
                    found.append({**entry, "blockNumber": r["blockNumber"], "transactionHash": tx})
        print(json.dumps(found))
    elif command == "send" and "--create" in options:
        refuse_stale_nonce(state, sender, "create", {"data": options["--create"]}, options)
        check_nonce(state, sender, options)
        held(state, sender, "create", {"data": options["--create"]}, options)
        refuse = accept_then_refuse(state)
        tx, _ = apply_send(state, "create", sender, {"data": options["--create"]})
        save(state)
        if refuse:
            die("Error: server returned an error response: error code -32000: nonce too low")
        print(json.dumps(state["receipts"][tx]))
    elif command == "send":
        payload = {"to": positional[1], "sig": positional[2], "args": positional[3:]}
        refuse_stale_nonce(state, sender, "call", payload, options)
        check_nonce(state, sender, options)
        held(state, sender, "call", payload, options)
        refuse = accept_then_refuse(state)
        tx, _ = apply_send(state, "call", sender, payload)
        save(state)
        if refuse:
            die("Error: server returned an error response: error code -32000: nonce too low")
        if "--json" in flags:
            print(json.dumps(state["receipts"][tx]))
    else:
        die(f"stub cast: unexpected {command}")

def main_forge(argv):
    state = load()
    reorg(state)
    fill_held(state)
    mine_held(state)
    positional, options, flags = split(argv)
    trace("forge " + " ".join(positional + options.get("--constructor-args", [])))
    if positional[0] == "remappings":
        print("@openzeppelin/contracts/=dependencies/@openzeppelin-contracts-4.9.3/contracts/")
        return
    if positional[0] == "inspect":
        name = positional[1].split(":")[-1]
        if positional[1].endswith("ERC1967Proxy.sol:ERC1967Proxy") and positional[2] == "bytecode":
            print(PROXY_CODE)
        elif positional[2] == "bytecode" and name in CREATED:
            print(creation_code(name))
        elif positional[2] == "abi" and name in CREATED:
            types = [t for t in CONSTRUCTOR_TYPES.get(name, "").split(",") if t]
            print(json.dumps([{"type": "constructor", "inputs": [{"type": t} for t in types]}]))
        else:
            die(f"stub forge: unexpected inspect {positional[1:]}")
        return
    name = positional[1].split(":")[-1]
    sender = state["keys"][options["--private-key"]]
    payload = {"name": name, "args": options.get("--constructor-args", [])}
    refuse_stale_nonce(state, sender, "forge", payload, options)
    check_nonce(state, sender, options)
    held(state, sender, "forge", payload, options)
    refuse = accept_then_refuse(state)
    tx, address = apply_send(state, "forge", sender, payload)
    save(state)
    if refuse:
        die("Error: server returned an error response: error code -32000: nonce too low")
    print(json.dumps({"deployer": sender, "deployedTo": address, "transactionHash": tx}))
"""

CAST_STUB = """#!/usr/bin/env python3
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fake_evm
fake_evm.main_cast(sys.argv[1:])
"""

FORGE_STUB = CAST_STUB.replace("main_cast", "main_forge")

# The RUES endpoints preflight reads: node info, a block height, the chain
# ID (one raw byte, FIXTURE_DUSK_CHAIN_ID or 2) and an archive query.
CURL_STUB = r'''#!/bin/sh
url=""
data=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --data) data="$2"; shift 2 ;;
        -m|-X|-H|--max-time|--connect-timeout) shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
case "$url" in
    */on/node/info) echo '{"version_build":"fixture","vm_config":{"features":{"HQ_KECCAK256":0,"HQ_SECP256K1_RECOVER":0}}}' ;;
    */chain_id) printf "\\$(printf '%03o' "${FIXTURE_DUSK_CHAIN_ID:-2}")" ;;
    */on/graphql/query)
        case "$data" in
            *finalizedEvents*) echo '{"finalizedEvents":{"json":{"events":[]}}}' ;;
            *) echo '{"block":{"header":{"height":100}}}' ;;
        esac ;;
    *) exit 22 ;;
esac
'''


def evm_transactions(evm):
    """The transactions on the fake EVM, in order: what each created, or
    the contract it called, by type, as addresses follow the deployer's
    nonce; without the fillers that take a nonce first (nonce_refusals),
    and without the failed ones (fail_held)."""
    kind = lambda address: evm["contracts"].get(address, {}).get("type", address)
    return [("create", kind(r["contractAddress"])) if r["contractAddress"] else ("call", kind(r["to"]))
            for r in evm.get("receipts", {}).values() if not r.get("filler") and r["status"] == "0x1"]


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
            # Hyperlane's Sepolia ProtocolFee predates hookType(); the
            # example settings list it in EVM_TYPELESS_HOOKS.
            EVM_PROTOCOL_FEE: {"type": "protocol_fee", "fee": 1},
            EVM_DEFAULT_HOOK: {"type": "fallback_routing", "hook_type": 5, "routes": {},
                               "fallback": EVM_MERKLE},
            EVM_ISM_FACTORY: {"type": "ism_factory"},
            EVM_HOOK_FACTORY: {"type": "aggregation_factory"},
        },
    }


def dusk_queries():
    """A Dusk deployment as deploy/testnet.sh leaves it: every contract at
    this checkout's state_version, the route with no ISM of its own, the
    Mailbox's default a 1-of-1 multisig ISM, and the IGP priced as the
    example settings say. The hooks are wired as dusk-tx deploy-hyperlane
    wires them: the IGP is the Mailbox's default hook, and its required hook
    is an aggregation hook that calls the Merkle tree hook and the
    ProtocolFee, which take their calls from it, not from the Mailbox. The
    deployer owns every contract that has an owner, and is the fee
    contracts' beneficiary."""
    owned = {f"{contract}/owner": {"value": DEPLOYER_H256}
             for contract in (MAILBOX, MULTISIG, PROTOCOL_FEE, IGP, ROUTE)}
    return {
        **owned,
        f"{MAILBOX}/local_domain": {"value": DUSK_DOMAIN},
        f"{MAILBOX}/default_ism": {"value": MULTISIG},
        f"{MAILBOX}/state_version": {"value": 6},
        f"{MAILBOX}/default_hook": {"value": IGP},
        f"{MAILBOX}/required_hook": {"value": AGGREGATION_HOOK},
        f"{AGGREGATION_HOOK}/mailbox": {"value": MAILBOX},
        f"{AGGREGATION_HOOK}/hooks": {"value": [MERKLE_TREE_HOOK, PROTOCOL_FEE]},
        f"{IGP}/mailbox": {"value": MAILBOX},
        f"{PROTOCOL_FEE}/mailbox": {"value": AGGREGATION_HOOK},
        f"{MERKLE_TREE_HOOK}/mailbox": {"value": AGGREGATION_HOOK},
        f"{MERKLE_TREE_HOOK}/state_version": {"value": 2},
        f"{IGP}/state_version": {"value": 4},
        f"{IGP}/domain_gas_config/{EVM_DOMAIN}": {"value": {
            key: int(value) for key, value in DEFAULT_DUSK_IGP.items()}},
        f"{IGP}/claimable_fees": {"value": 0},
        f"{IGP}/beneficiary": {"value": DEPLOYER_H256},
        f"{PROTOCOL_FEE}/claimable_fees": {"value": 0},
        f"{PROTOCOL_FEE}/beneficiary": {"value": DEPLOYER_H256},
        f"{PROTOCOL_FEE}/protocol_fee": {"value": 1000000},
        f"{PROTOCOL_FEE}/max_protocol_fee": {"value": 100000000},
        f"{AGGREGATION_HOOK}/state_version": {"value": 1},
        f"{PROTOCOL_FEE}/state_version": {"value": 1},
        f"{VALIDATOR_ANNOUNCE}/state_version": {"value": 3},
        f"{VALIDATOR_ANNOUNCE}/mailbox": {"value": MAILBOX},
        f"{VALIDATOR_ANNOUNCE}/local_domain": {"value": DUSK_DOMAIN},
        f"{ROUTE}/state_version": {"value": 5},
        f"{ROUTE}/mailbox": {"value": MAILBOX},
        f"{ROUTE}/hook": {"value": ZERO},
        f"{ROUTE}/interchain_security_module": {"value": ZERO},
        f"{MULTISIG}/state_version": {"value": 1},
        f"{MULTISIG}/module_type": {"value": 5},
        f"{MULTISIG}/validators_and_threshold": {"value": {"validators": [VALIDATOR], "threshold": 1}},
        f"{MULTISIG}/threshold": {"value": 1},
        f"{TEST_MOCK}/state_version": {"value": 1},
        f"{TEST_MOCK}/module_type": {"value": 6},
        f"{TEST_RECIPIENT}/state_version": {"value": 1},
        f"{TEST_RECIPIENT}/interchain_security_module": {"value": ZERO},
        f"{MULTICALL}/version": {"value": 1},
    }


DUSK_CONTRACTS = {
    "mailbox": MAILBOX, "igp": IGP, "validator_announce": VALIDATOR_ANNOUNCE,
    "merkle_tree_hook": MERKLE_TREE_HOOK, "protocol_fee": PROTOCOL_FEE, "warp_native": ROUTE,
    "aggregation_hook": AGGREGATION_HOOK, "ism_multisig": MULTISIG, "test_mock": TEST_MOCK,
    "test_recipient": TEST_RECIPIENT,
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
            # Token routes have tests of their own (TestnetTokenRoutes).
            "TOKEN_ROUTES": "",
        }
        self.out = self.root / "deploy" / "out" / "fixture"
        self.out.mkdir(parents=True)
        self.relayer_config = self.out / "relayer.json"
        self.queries = dusk_queries()
        self.evm = sepolia()
        # Extra environment for the script and its stubs.
        self.extra_env = {}
        # The dusk-tx stub's "interleave" and "on_read", and what became of
        # them.
        self.dusk_interleave = None
        self.dusk_on_read = None
        # The canonical DRC20s on the stubbed Dusk chain, by ID, and the
        # stub's failures to come, by command.
        self.tokens = {}
        self.dusk_failures = {}
        # What the stub's next deploy-token-route leaves tampered with
        # ("tamper"), and the deployer's DUSK.
        self.dusk_tamper = None
        self.dusk_balance = 1000 * 10**9
        # The stub's deploy transactions ("txs", "deploys_sent"), and the
        # faults its deploy-token-route plays ("front_run_deploy",
        # "stop_after_token").
        self.dusk_deploys = {}

    def write_state(self, state):
        (self.out / "deployment.json").write_text(json.dumps(state))

    def forget_deployment(self):
        """Start over: no state, no deploy-hyperlane report, and, as the
        stub deploys at the same IDs every time, the Dusk contracts as
        deploy-hyperlane leaves them."""
        (self.out / "deployment.json").unlink(missing_ok=True)
        (self.out / "dusk-deploy.json").unlink(missing_ok=True)
        for log in self.out.glob("dusk-route-*.txs"):
            log.unlink()
        self.queries = dusk_queries()
        self.tokens = {}
        self.dusk_deploys = {}

    def state(self):
        return json.loads((self.out / "deployment.json").read_text())

    def run_script(self, *command, **settings):
        env_file = self.root / "deploy" / "testnet.env"
        values = {**self.settings, **settings}
        env_file.write_text('source "$DUSK_DIR/deploy/testnet.env.example"\n'
                            + "".join(f"{key}={shlex.quote(str(value))}\n" for key, value in values.items()))
        self.chain.write_text(json.dumps({"queries": self.queries, "deployer_h256": DEPLOYER_H256,
                                          "deployer_key": DEPLOYER_KEY_HEX, "tokens": self.tokens,
                                          "fail": self.dusk_failures, "deploy": DUSK_CONTRACTS,
                                          "interleave": self.dusk_interleave, "on_read": self.dusk_on_read,
                                          "tamper": self.dusk_tamper, "balance": self.dusk_balance,
                                          **self.dusk_deploys}))
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
            **self.extra_env,
        }
        result = subprocess.run(["bash", str(self.root / "deploy" / "testnet.sh"), *command],
                                env=env, capture_output=True, text=True, timeout=120)
        # What the run left on the fake chains, for the next run and the asserts.
        chain = json.loads(self.chain.read_text())
        self.queries, self.dusk_interleave = chain["queries"], chain.get("interleave")
        self.dusk_on_read = chain.get("on_read")
        self.tokens, self.dusk_failures = chain.get("tokens", {}), chain.get("fail") or {}
        self.dusk_tamper = chain.get("tamper")
        self.dusk_deploys = {key: chain[key] for key in ("txs", "deploys_sent", "front_run_deploy", "stop_after_token")
                             if key in chain}
        self.evm = json.loads(self.evm_file.read_text())
        # A read that checks a write is made at the write's block or later.
        self.assertEqual(self.evm.pop("early_pinned_reads", 0), 0,
                         f"{command}: an EVM read pinned to a block before the deployer's last transaction's")
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
        self.igp_impl, self.token_impl = "0x" + "4a" * 20, "0x" + "1a" * 20
        evm[self.ism] = {"type": "ism", "module_type": 5, "validators": [VALIDATOR], "threshold": 1}
        evm[self.oracle] = {"type": "oracle", "owner": DEPLOYER, "data": {
            str(DUSK_DOMAIN): [int(DEFAULT_EVM_IGP["token_exchange_rate"]), int(DEFAULT_EVM_IGP["gas_price"])]}}
        # The bare implementations, never initialized, and the proxies the
        # deployment uses, each initialized in the transaction that created
        # it.
        evm[self.igp_impl] = {"type": "igp", "hook_type": 4, "owner": "0x" + "00" * 20,
                              "beneficiary": "0x" + "00" * 20, "configs": {}, "quote_signers": [], "quotes": {}}
        evm[self.token_impl] = {"type": "token", "mailbox": EVM_MAILBOX, "hook": "0x" + "00" * 20,
                                "ism": "0x" + "00" * 20, "owner": DEPLOYER, "routers": {}, "supply": 0,
                                "decimals": 9, "scale": [1, 1]}
        self.igp_tx = proxy_creation(self.evm, self.evm_igp, self.igp_impl, {
            "type": "igp", "hook_type": 4, "owner": DEPLOYER, "beneficiary": DEPLOYER,
            "configs": {str(DUSK_DOMAIN): [self.oracle, int(DEFAULT_EVM_IGP["gas_overhead"])]},
            "quote_signers": [], "quotes": {}})
        evm[self.hook] = {"type": "aggregation", "hook_type": 2, "hooks": [EVM_MERKLE, self.evm_igp]}
        self.token_tx = proxy_creation(self.evm, self.token, self.token_impl, {
            "type": "token", "mailbox": EVM_MAILBOX, "hook": self.hook, "ism": self.ism, "owner": DEPLOYER,
            "routers": {str(DUSK_DOMAIN): "0x" + ROUTE}, "supply": 0, "name": "Dusk", "symbol": "DUSK",
            "decimals": 9, "scale": [1, 1]})
        self.queries[f"{ROUTE}/enrolled_router/{EVM_DOMAIN}"] = {"value": "00" * 12 + "11" * 20}
        self.write_state({
            "evm_chain": "sepolia",
            "evm_domain": EVM_DOMAIN,
            "evm": {"mailbox": EVM_MAILBOX, "merkle_tree_hook": EVM_MERKLE, "validator_announce": EVM_VA,
                    "ism": self.ism, "storage_gas_oracle": self.oracle, "igp": self.evm_igp,
                    "igp_implementation": self.igp_impl, "igp_creation_tx": self.igp_tx,
                    "igp_beneficiary": DEPLOYER, "route_hook": self.hook, "token": self.token,
                    "token_implementation": self.token_impl, "token_creation_tx": self.token_tx,
                    "owner": DEPLOYER, "index_from": 1},
            "dusk": DUSK_CONTRACTS,
            "dusk_domain": DUSK_DOMAIN,
            "dusk_owner": DEPLOYER_H256,
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

    def test_a_validator_listed_twice_is_refused(self):
        # This checkout's ISM keeps its validators strictly ascending, but
        # the route's can be any contract that answers as one. Listed twice,
        # a validator's signature could count twice; folded into one, the
        # set compared equal to the settings.
        a, b = validators(2)
        self.set_default_multisig([a, a, b], 2)
        self.settings["EVM_ORIGIN_VALIDATORS"] = f"{a},{b}"
        self.settings["EVM_ORIGIN_THRESHOLD"] = 2
        self.settings["EVM_IGP_GAS_OVERHEAD"] = 2390000
        self.evm["contracts"][self.evm_igp]["configs"][str(DUSK_DOMAIN)][1] = 2390000
        self.assert_both_commands(False, f"lists [{a}] more than once among its validators")
        self.set_default_multisig([a, b], 2)
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


class TestnetRouteHookPlan(TestnetScript):
    """`deploy` follows the EVM Mailbox's required hook through
    aggregations and routing hooks to count how often it inserts a message to
    Dusk into the Merkle tree validators sign, so that the route's hook makes
    it exactly once, and refuses graphs it can't follow."""

    NESTED, ROUTING, OTHER = "0x" + "a1" * 20, "0x" + "a2" * 20, "0x" + "a3" * 20

    def require(self, hook, **contracts):
        self.evm["contracts"].update(contracts)
        self.evm["contracts"][EVM_MAILBOX]["requiredHook"] = hook

    def assert_route_hook_is_the_igp(self):
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("inserts a message to Dusk into EVM_MERKLE_TREE_HOOK once", result.stderr)
        state = self.state()["evm"]
        self.assertEqual(state["route_hook"], state["igp"])
        self.assert_ok(self.run_script("agent-configs"))

    def test_a_nested_aggregation_that_inserts(self):
        # Before, only the required hook's own children counted: the route's
        # hook aggregated the Merkle hook again and each message went into
        # the tree twice.
        self.require(self.OTHER, **{
            self.NESTED: {"type": "aggregation", "hook_type": 2, "hooks": [EVM_MERKLE, EVM_PROTOCOL_FEE]},
            self.OTHER: {"type": "aggregation", "hook_type": 2, "hooks": [EVM_PROTOCOL_FEE, self.NESTED]},
        })
        self.assert_route_hook_is_the_igp()

    def test_a_routing_hook_that_inserts_for_dusk(self):
        for routing in (
            {"type": "routing", "hook_type": 1, "routes": {str(DUSK_DOMAIN): EVM_MERKLE}},
            {"type": "fallback_routing", "hook_type": 5, "routes": {}, "fallback": self.NESTED},
        ):
            with self.subTest(routing=routing["type"]):
                self.evm = sepolia()
                self.forget_deployment()
                self.require(self.ROUTING, **{
                    self.ROUTING: routing,
                    self.NESTED: {"type": "aggregation", "hook_type": 2, "hooks": [EVM_PROTOCOL_FEE, EVM_MERKLE]},
                })
                self.assert_route_hook_is_the_igp()

    def test_a_routing_hook_that_inserts_elsewhere_for_dusk(self):
        # Another tree, or none, for Dusk: the route's hook inserts.
        self.require(self.ROUTING, **{
            self.ROUTING: {"type": "fallback_routing", "hook_type": 5,
                           "routes": {str(DUSK_DOMAIN): self.NESTED}, "fallback": EVM_MERKLE},
            self.NESTED: {"type": "merkle", "hook_type": 3, "mailbox": EVM_MAILBOX},
        })
        self.assert_ok(self.run_script("deploy"))
        state = self.state()["evm"]
        self.assertEqual(self.evm["contracts"][state["route_hook"]]["hooks"], [EVM_MERKLE, state["igp"]])

    def test_graphs_it_cant_follow_are_refused(self):
        cases = [
            ("inserts each message to Dusk into EVM_MERKLE_TREE_HOOK 2 times",
             {self.OTHER: {"type": "aggregation", "hook_type": 2, "hooks": [EVM_MERKLE, self.NESTED]},
              self.NESTED: {"type": "routing", "hook_type": 1, "routes": {str(DUSK_DOMAIN): EVM_MERKLE}}}),
            (f"reaches {self.NESTED}, of hook type 14, which deploy/testnet.sh can't follow",
             {self.OTHER: {"type": "aggregation", "hook_type": 2, "hooks": [EVM_PROTOCOL_FEE, self.NESTED]},
              self.NESTED: {"type": "amount_routing", "hook_type": 14}}),
            (f"has no hook for domain {DUSK_DOMAIN}: every dispatch to Dusk would revert",
             {self.OTHER: {"type": "routing", "hook_type": 1, "routes": {"1": EVM_MERKLE}}}),
            ("nests hooks more than 8 deep",
             {self.OTHER: {"type": "routing", "hook_type": 1, "routes": {str(DUSK_DOMAIN): self.OTHER}}}),
        ]
        for expected, contracts in cases:
            with self.subTest(expected=expected):
                self.evm = sepolia()
                self.require(self.OTHER, **contracts)
                for command in ("preflight", "deploy"):
                    result = self.run_script(command)
                    self.assert_fails(result, expected)
                    self.assertEqual(self.traced("cast send") + self.traced("forge"), [])


class TestnetLaggingEvmEndpoint(TestnetScript):
    """EVM_RPC can be a load balancer over nodes a block apart, as the public
    Sepolia endpoints are: a read right after a write can reach a node that
    hasn't seen it. Each write waits until the endpoint answers past its
    block, so the checks that follow see what was written, and a deploy on
    such an endpoint completes with each contract deployed once."""

    def test_a_deploy_on_a_lagging_endpoint_completes(self):
        self.evm["lag"] = 2
        result = self.run_script("deploy", EVM_SETTLE_SECONDS="60")
        self.assert_ok(result)
        state = self.state()["evm"]
        for key in ("ism", "igp", "token", "route_hook"):
            self.assertIn(key, state)
        # Each contract once: one ISM and one hook from the factories, the
        # IGP's and the token's proxies, and no forge create repeated.
        sends = self.traced("cast send")
        self.assertEqual(sum("deploy(address[],uint8)" in line for line in sends), 1)
        self.assertEqual(sum("deploy(address[])" in line for line in sends), 1)
        self.assertEqual(sum("--create" in line for line in sends), 2)
        creates = self.traced("forge create")
        self.assertEqual(len(creates), len(set(creates)))
        self.assert_ok(self.run_script("agent-configs"))

    def deployed_transactions(self):
        return evm_transactions(self.evm)

    def test_a_deploy_whose_latest_reads_lag_completes(self):
        # The live Sepolia deploy: the endpoint's head showed each write, so
        # the writes settled, but the read of the IGP proxy's implementation
        # slot right after its creation reached a node behind it and read
        # zero, and deploy stopped. Once a command has written, its reads
        # are made at a block at or past the write, which a node either
        # answers with the write in it or refuses until it has the block;
        # a refused read is made again.
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        self.evm = sepolia()
        self.forget_deployment()
        self.evm.update(stale_latest=2, pinned_misses=1)
        result = self.run_script("deploy", EVM_SETTLE_SECONDS="60")
        self.assert_ok(result)
        self.assertEqual(self.deployed_transactions(), honest, "each transaction once, as without the lag")
        # Every read after the first write names a block.
        trace = self.trace.read_text().splitlines()
        first_write = next(i for i, line in enumerate(trace) if line.startswith(("cast send", "forge create")))
        reads = [line for line in trace[first_write:] if line.split()[1] in ("call", "code", "storage")]
        self.assertTrue(reads)
        self.assertEqual([line for line in reads if "--block" not in line], [])
        self.evm.update(stale_latest=0, latest_stale_left=0, pinned_misses=0, pinned_misses_left=0)
        self.assert_ok(self.run_script("agent-configs"))

    def test_reads_a_node_never_answers_are_given_up(self):
        self.evm.update(pinned_misses=10**6)
        result = self.run_script("deploy", EVM_SETTLE_SECONDS="3")
        self.assert_fails(result, "header not found")

    def test_a_transaction_refused_for_its_nonce_is_never_sent_again(self):
        # The live Sepolia deploy: a send got its nonce from a node that
        # hadn't seen the deployer's last transaction, and the node answered
        # "nonce too low". That doesn't show the transaction was refused: a
        # gateway that sent it on and then answered a replay of the request
        # says so too. So nothing is sent again: deploy stops, and a re-run,
        # which reads the chain afresh, sends what is left, each once.
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        for skip, expected in ((0, "cast send"), (1, "forge create"), (3, "cast send --create")):
            with self.subTest(refused=expected):
                self.evm = sepolia()
                self.forget_deployment()
                self.evm.update(nonce_refusals=1, nonce_refusals_skip=skip)
                result = self.run_script("deploy", EVM_SETTLE_SECONDS="3")
                self.assert_fails(result, "another transaction of the deployer's took that nonce, in a block "
                                          "that is final, so this one never executes. Nothing was sent again")
                self.assertNotIn("evm_sending", self.state())
                sends = [line for line in self.trace.read_text().splitlines()
                         if line.startswith(("cast send", "forge create"))]
                self.assertTrue(sends[skip].startswith(expected), sends)
                self.assertEqual(len(sends), skip + 1, "nothing is sent again")
                self.assert_ok(self.run_script("deploy"))
                self.assertEqual(self.deployed_transactions(), honest)
        # The nonce is the highest of three reads: two nodes behind don't
        # matter. With all three behind, the node refuses the nonce they
        # give, which another transaction of the deployer's took: nothing
        # is sent again. (Creations' addresses follow the deployer's nonce,
        # so the deploy to compare with starts at the same one.)
        self.evm = sepolia()
        self.forget_deployment()
        self.evm["account_nonces"] = {DEPLOYER.lower(): 5}
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        for stale, fails in ((2, False), (3, True)):
            with self.subTest(stale_reads=stale):
                self.evm = sepolia()
                self.forget_deployment()
                self.evm.update(account_nonces={DEPLOYER.lower(): 5}, stale_nonce_reads=stale)
                result = self.run_script("deploy", EVM_SETTLE_SECONDS="3")
                if fails:
                    self.assert_fails(result, "nothing was sent again")
                    self.assertEqual(self.deployed_transactions(), [])
                else:
                    self.assert_ok(result)
                    self.assertEqual(self.deployed_transactions(), honest)

    def test_a_creation_taken_but_answered_with_nonce_too_low_is_adopted(self):
        # A gateway sends the creation on, a block takes it, and the
        # gateway answers a replay of the request with "nonce too low". The
        # deployer's transaction with the creation's nonce is that very
        # creation, executed: it is taken, and nothing is created twice.
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        for skip, expected in ((1, "forge create"), (3, "cast send --create")):
            with self.subTest(creation=expected):
                self.evm = sepolia()
                self.forget_deployment()
                self.evm.update(accept_then_refuse=1, accept_then_refuse_skip=skip)
                result = self.run_script("deploy")
                self.assert_ok(result)
                self.assertIn("is that very creation, executed: taking it", result.stderr)
                self.assertEqual(self.deployed_transactions(), honest)
        # Any other transaction answered so stops deploy, sending nothing
        # again; the re-run finds it done.
        self.evm = sepolia()
        self.forget_deployment()
        self.evm.update(accept_then_refuse=1, accept_then_refuse_skip=0)
        self.assert_fails(self.run_script("deploy"), "nothing was sent again. Run deploy again")
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.deployed_transactions(), honest)
        # A creation whose receipt says it landed somewhere other than the
        # address its nonce gives isn't taken: deploy stops, sending nothing
        # again.
        self.evm = sepolia()
        self.forget_deployment()
        self.evm.update(accept_then_refuse=1, accept_then_refuse_skip=1, wrong_address=1)
        result = self.run_script("deploy", EVM_SETTLE_SECONDS="3")
        self.assert_fails(result, "nothing was sent again")
        self.assertNotIn("taking it", result.stderr)
        self.assertEqual(len([r for r in self.evm["receipts"].values() if r["contractAddress"]]), 1)

    def test_a_send_a_run_left_unsettled_is_settled_before_its_step_is_sent_again(self):
        # A transaction the node took, but whose send timed out, isn't
        # mined until after the run has stopped. Its nonce and input are in
        # the record from before it was sent, so the re-run settles it first:
        # the step's own transaction, mined, is taken, and nothing is sent
        # twice; one still pending stops the re-run, sending nothing.
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        for skip, step in ((0, "the factory's ISM (a call)"), (1, "the oracle (forge create)"),
                           (3, "the IGP's proxy (cast send --create)")):
            with self.subTest(step=step):
                self.evm = sepolia()
                self.forget_deployment()
                self.evm.update(hold=1, hold_skip=skip)
                self.assert_fails(self.run_script("deploy"), "timed out")
                pending = list(self.state()["evm_sending"].values())
                self.assertEqual([entry["nonce"] for entry in pending], [skip])
                # Still pending: the re-run sends nothing.
                result = self.run_script("deploy", EVM_SETTLE_SECONDS="3")
                self.assert_fails(result, "isn't in a block: it may still be pending, so nothing was sent")
                self.assertEqual(len(self.evm.get("receipts", {})), skip)
                # Mined: the re-run takes it, and sends each transaction once.
                self.evm["mine_held"] = True
                result = self.run_script("deploy")
                self.assert_ok(result)
                self.assertEqual(self.deployed_transactions(), honest)
                self.assertNotIn("evm_sending", self.state())
                if skip:
                    self.assertIn("It is this step's own transaction, executed: taking it", result.stderr)

    def interrupt_the_token_implementation(self):
        """Deploy, stopped at the DUSK token's implementation, whose
        creation the node took but whose send timed out; the creation is
        mined after. Returns the honest deploy's transactions."""
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        sends = [line for line in self.trace.read_text().splitlines()
                 if line.startswith(("cast send", "forge create"))]
        skip = next(i for i, line in enumerate(sends) if line.startswith("forge create") and "HypERC20" in line)
        self.evm = sepolia()
        self.forget_deployment()
        self.evm.update(hold=1, hold_skip=skip)
        self.assert_fails(self.run_script("deploy"), "timed out")
        self.assertEqual(list(self.state()["evm_sending"]), ["create .evm.token_implementation"])
        self.evm["mine_held"] = True
        return honest

    def test_a_creation_left_unsettled_is_left_to_its_step_by_the_others(self):
        # Between the run that sent the creation and the re-run, claim
        # sends the IGP's claim(). Only the creation's step records its
        # address, so claim leaves its record alone, and the re-run takes
        # the creation as its own: nothing is created twice.
        honest = self.interrupt_the_token_implementation()
        igp = self.state()["evm"]["igp"]
        self.evm["balances"][igp] = 7
        self.assert_ok(self.run_script("claim"))
        self.assertEqual(self.evm["balances"].get(igp, 0), 0)
        self.assertEqual(list(self.state()["evm_sending"]), ["create .evm.token_implementation"])
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("It is this step's own transaction, executed: taking it", result.stderr)
        self.assertNotIn("evm_sending", self.state())
        self.assertEqual(self.deployed_transactions(), self.with_claim_after_the_implementation(honest),
                         "each creation once")

    @staticmethod
    def with_claim_after_the_implementation(honest):
        # The deploy's transactions, with claim's claim() right after the
        # DUSK token's implementation, which was mined before claim ran.
        implementation = honest.index(("create", "token"))
        return honest[:implementation + 1] + [("call", "igp")] + honest[implementation + 1:]

    def test_a_send_is_forgotten_only_once_what_took_its_nonce_is_final(self):
        # The oracle's creation is answered with "nonce too low": another
        # transaction of the deployer's took its nonce, in a block that
        # isn't final, and a node holds the creation. Until that block is
        # final, another can replace it, without the other transaction, and
        # the creation can still execute: deploy keeps its record, stops,
        # and the re-run takes the creation if it did.
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        step = "create .evm.storage_gas_oracle"
        for case in ("its block replaced", "its block final"):
            with self.subTest(case=case):
                self.evm = sepolia()
                self.forget_deployment()
                self.evm.update(nonce_refusals=1, nonce_refusals_skip=1, nonce_refusals_hold=True,
                                finality_depth=1000)
                result = self.run_script("deploy", EVM_SETTLE_SECONDS="3", EVM_FINALITY_SECONDS="3")
                self.assert_fails(result, "another transaction of the deployer's took that nonce, in a block "
                                          "that isn't final, or that another block has replaced")
                self.assertEqual(list(self.state()["evm_sending"]), [step])
                if case == "its block replaced":
                    filler = next(r for r in self.evm["receipts"].values() if r.get("filler"))
                    self.evm["reorg_from"] = int(filler["blockNumber"], 16)
                    result = self.run_script("deploy")
                    self.assert_ok(result)
                    self.assertIn("It is this step's own transaction, executed: taking it", result.stderr)
                else:
                    # Still not final: the re-run stops too, sending nothing.
                    result = self.run_script("deploy", EVM_FINALITY_SECONDS="3")
                    self.assert_fails(result, "took its nonce, in a block that isn't final after 3s")
                    self.assertEqual(list(self.state()["evm_sending"]), [step])
                    self.evm["finality_depth"] = 0
                    result = self.run_script("deploy")
                    self.assert_ok(result)
                    self.assertIn("Its block is final: forgetting it, for the step to go on afresh", result.stderr)
                self.assertEqual(self.deployed_transactions(), honest, "the oracle created once")
                self.assertNotIn("evm_sending", self.state())

    def test_a_recorded_send_whose_nonce_is_settled_otherwise_is_forgotten_once_final(self):
        # The oracle's creation timed out, and then failed in a block, or
        # never was mined, its nonce taken by another transaction of the
        # deployer's. Either can still change until that block is final:
        # the re-run keeps the record and stops, and once the block is
        # final, forgets it and creates the oracle afresh, once.
        self.assert_ok(self.run_script("deploy"))
        honest = self.deployed_transactions()
        step = "create .evm.storage_gas_oracle"
        for case, settings, expected in (
            ("failed", {"mine_held": True, "fail_held": True}, "it failed, in a block that isn't final after 3s"),
            ("nonce taken", {"fill_held": True}, "took its nonce, in a block that isn't final after 3s"),
        ):
            with self.subTest(case=case):
                self.evm = sepolia()
                self.forget_deployment()
                self.evm.update(hold=1, hold_skip=1)
                self.assert_fails(self.run_script("deploy"), "timed out")
                self.evm.update(finality_depth=1000, **settings)
                result = self.run_script("deploy", EVM_FINALITY_SECONDS="3")
                self.assert_fails(result, expected)
                self.assertEqual(list(self.state()["evm_sending"]), [step])
                self.evm.update(finality_depth=0, fail_held=False)
                result = self.run_script("deploy")
                self.assert_ok(result)
                self.assertIn("Its block is final: forgetting it, for the step to go on afresh", result.stderr)
                self.assertEqual(self.deployed_transactions(), honest, "the oracle created once")
                self.assertNotIn("evm_sending", self.state())

    def test_a_send_recorded_without_its_step_is_taken_by_the_step_that_sends_it(self):
        # The record an earlier version of this script left: .evm_pending,
        # one send without the step that sent it. The step that sends just
        # that takes it; the others leave it alone.
        honest = self.interrupt_the_token_implementation()
        state = self.state()
        entry = state.pop("evm_sending")["create .evm.token_implementation"]
        del entry["step"]
        state["evm_pending"] = entry
        self.write_state(state)
        igp = state["evm"]["igp"]
        self.evm["balances"][igp] = 7
        self.assert_ok(self.run_script("claim"))
        self.assertEqual(self.state()["evm_pending"], entry)
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("It is this step's own transaction, executed: taking it", result.stderr)
        self.assertNotIn("evm_pending", self.state())
        self.assertNotIn("evm_sending", self.state())
        self.assertEqual(self.deployed_transactions(), self.with_claim_after_the_implementation(honest),
                         "each creation once")


class TestnetTypelessHooks(TestnetScript):
    """Sepolia's Mailbox requires a ProtocolFee deployed before hooks had
    hookType(). `deploy` takes such a hook as a ProtocolFee, which inserts
    nothing into the Merkle tree, only when EVM_TYPELESS_HOOKS lists it and
    it answers protocolFee(); any other typeless hook stops it."""

    TYPED = "0x" + "b1" * 20
    OTHER = "0x" + "b2" * 20

    def assert_refused(self, expected, **env):
        for command in ("preflight", "deploy"):
            with self.subTest(command=command, **env):
                result = self.run_script(command, **env)
                self.assert_fails(result, expected)
                self.assertEqual(self.traced("cast send") + self.traced("forge"), [])

    def test_the_listed_protocol_fee_inserts_nothing(self):
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("doesn't insert a message to Dusk into EVM_MERKLE_TREE_HOOK", result.stderr)
        state = self.state()["evm"]
        self.assertEqual(self.evm["contracts"][state["route_hook"]]["hooks"], [EVM_MERKLE, state["igp"]])

    def test_an_unlisted_typeless_hook_is_refused(self):
        self.assert_refused(
            f"Can't read the hook type of {EVM_PROTOCOL_FEE}",
            EVM_TYPELESS_HOOKS=f"{self.OTHER}=protocol_fee")

    def test_a_listed_hook_that_isnt_a_protocol_fee_is_refused(self):
        self.evm["contracts"][EVM_PROTOCOL_FEE] = {"type": "merkle", "mailbox": EVM_MAILBOX}
        self.assert_refused("doesn't answer protocolFee()")

    def test_malformed_lists_are_refused(self):
        for value, expected in (
            (f"{EVM_PROTOCOL_FEE}=merkle_tree", "the only kind it takes is protocol_fee"),
            (f"{EVM_PROTOCOL_FEE}", "isn't address=kind"),
            ("0x1234=protocol_fee", "isn't address=kind"),
        ):
            self.evm = sepolia()
            self.assert_refused(expected, EVM_TYPELESS_HOOKS=value)

    def test_a_hook_with_a_type_is_taken_by_its_type(self):
        # The list only names hooks without a type: one that reports a type
        # is followed by it, listed or not.
        self.evm["contracts"][EVM_PROTOCOL_FEE] = {"type": "merkle", "hook_type": 3, "mailbox": EVM_MAILBOX}
        self.evm["contracts"][EVM_MAILBOX]["requiredHook"] = self.TYPED
        self.evm["contracts"][self.TYPED] = {"type": "aggregation", "hook_type": 2,
                                             "hooks": [EVM_MERKLE, EVM_PROTOCOL_FEE]}
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("inserts a message to Dusk into EVM_MERKLE_TREE_HOOK once", result.stderr)


class TestnetEvmCreationChecks(DeployedTestnet):
    """Every command that relies on the EVM IGP and token checks, from the
    receipt of the transaction that created each, that the deployer created
    it initialized, and that nothing prices delivery to Dusk below the
    oracle."""

    COMMANDS = ["preflight", "agent-configs", "evm-igp", "claim", "deploy"]

    def assert_refused(self, expected, commands=COMMANDS):
        for command in commands:
            with self.subTest(command=command):
                result = self.run_script(command)
                self.assert_fails(result, expected)
                self.assertEqual(self.traced("cast send"), [])
                self.assertEqual(self.traced("forge"), [])
                self.assertFalse(self.relayer_config.exists())

    def test_the_recorded_deployment_passes(self):
        for command in self.COMMANDS:
            with self.subTest(command=command):
                self.assert_ok(self.run_script(command))

    def test_contracts_without_a_creation_record_are_refused(self):
        # What an older deploy/testnet.sh recorded: it initialized the IGP and
        # the token in a second transaction, which anyone could send first.
        for key, label in (("igp", "IGP"), ("token", "DUSK token")):
            with self.subTest(contract=key):
                state = self.state()
                saved = state["evm"].pop(f"{key}_creation_tx")
                self.write_state(state)
                self.assert_refused(f"records the EVM {label} {state['evm'][key]} without the transaction that "
                                    "created and initialized it")
                state["evm"][f"{key}_creation_tx"] = saved
                self.write_state(state)

    def test_a_contract_initialized_after_its_creation_is_refused(self):
        # Created without its initialization: whoever sent initialize first
        # chose its owner and beneficiary, and could have set quotes.
        proxy_creation(self.evm, self.evm_igp, self.igp_impl, self.evm["contracts"][self.evm_igp],
                       initialized=False)
        self.assert_refused(f"The EVM IGP {self.evm_igp} wasn't initialized in the transaction that created it")

    def test_a_contract_someone_else_created_is_refused(self):
        proxy_creation(self.evm, self.token, self.token_impl, self.evm["contracts"][self.token], creator=ATTACKER)
        self.assert_refused(f"The EVM DUSK token {self.token} wasn't initialized in the transaction that created it")

    def test_a_proxy_of_another_implementation_is_refused(self):
        self.evm["contracts"][self.evm_igp]["implementation"] = "0x" + "4b" * 20
        self.assert_refused(f"runs the implementation 0x{'4b' * 20}, not {self.igp_impl}")

    def test_a_proxy_with_an_admin_is_refused(self):
        # This deployment's ERC1967Proxy has no admin. One whose admin can
        # upgrade it, created and initialized by the deployer all the same,
        # passed the creation and implementation checks.
        for key, label in (("evm_igp", "IGP"), ("token", "DUSK token")):
            with self.subTest(contract=key):
                address = getattr(self, key)
                self.evm["contracts"][address]["admin"] = ATTACKER
                self.assert_refused(f"The EVM {label} {address} has the proxy admin {ATTACKER}")
                del self.evm["contracts"][address]["admin"]

    def test_a_standing_quote_that_undercuts_the_oracle_is_refused(self):
        wildcard_sender, wildcard_dest = "0x" + "ff" * 20, str(2**32 - 1)
        commands = ["preflight", "agent-configs", "evm-igp", "deploy"]
        for key in (f"{DUSK_DOMAIN}/{wildcard_sender}", f"{DUSK_DOMAIN}/{self.token}",
                    f"{wildcard_dest}/{self.token}"):
            with self.subTest(quote=key):
                self.evm["contracts"][self.evm_igp]["quotes"] = {
                    key: {"rate": 0, "price": 0, "expiry": 2**47}}
                self.assert_refused("a signed offchain quote", commands)
        # An expired quote no longer counts, for the IGP or the checks.
        self.evm["contracts"][self.evm_igp]["quotes"] = {
            f"{DUSK_DOMAIN}/{wildcard_sender}": {"rate": 0, "price": 0, "expiry": 1}}
        for command in commands:
            with self.subTest(command=command, quote="expired"):
                self.assert_ok(self.run_script(command))

    def test_quote_signers_are_refused(self):
        self.evm["contracts"][self.evm_igp]["quote_signers"] = [ATTACKER]
        self.assert_refused(f"has quote signers [{ATTACKER}]", ["preflight", "agent-configs", "evm-igp", "deploy"])


class TestnetTokenBinding(DeployedTestnet):
    """The recorded EVM token must be this deployment's: bound to EVM_MAILBOX,
    verifying messages from Dusk with the recorded ISM, a MessageIdMultisig
    ISM of DUSK_ORIGIN_VALIDATORS and DUSK_ORIGIN_THRESHOLD, and enrolled with
    the recorded Dusk route, or with none and holding no tokens, before any
    command adopts it or deploy rewires it."""

    COMMANDS = ["preflight", "agent-configs", "evm-igp", "claim", "deploy"]

    def assert_refused(self, expected):
        for command in self.COMMANDS:
            with self.subTest(command=command):
                result = self.run_script(command)
                self.assert_fails(result, expected)
                self.assertEqual(self.traced("cast send") + self.traced("forge")
                                 + self.traced("dusk-tx enroll-router") + self.traced("dusk-tx set-beneficiary"), [])
                self.assertFalse(self.relayer_config.exists())

    def test_a_token_enrolled_with_another_dusk_route_is_refused(self):
        # Deployments A and B by the same deployer, through the same Mailbox
        # and validators, share the ISM the factory gives, so B's token, put
        # in A's state, passed the binding checks. deploy then enrolled A's
        # route on it in place of B's, and the token on A's route: B's
        # tokens, backed by B's custody, could be redeemed for A's DUSK.
        other = "e2" * 32
        self.evm["contracts"][self.token]["routers"] = {str(DUSK_DOMAIN): "0x" + other}
        self.assert_refused(f"is enrolled with the Dusk route {other}, not the route recorded in")
        self.assertEqual(self.evm["contracts"][self.token]["routers"], {str(DUSK_DOMAIN): "0x" + other})

    def test_a_token_holding_tokens_without_a_route_is_refused(self):
        # No route enrolled, but a supply, minted when it was initialized:
        # nothing on Dusk backs those tokens.
        self.evm["contracts"][self.token].update(routers={}, supply=10**18)
        self.assert_refused(f"has a supply of {10**18} but no Dusk route")

    def test_a_token_without_a_route_or_tokens_gets_this_route(self):
        # deploy stopped after creating the token: it has neither yet, and
        # deploy enrolls this deployment's route on it, and it on the route.
        self.evm["contracts"][self.token].update(routers={}, supply=0)
        self.queries.pop(f"{ROUTE}/enrolled_router/{EVM_DOMAIN}")
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.evm["contracts"][self.token]["routers"], {str(DUSK_DOMAIN): "0x" + ROUTE})
        self.assertEqual(self.queries[f"{ROUTE}/enrolled_router/{EVM_DOMAIN}"], {"value": "00" * 12 + "11" * 20})
        self.assert_ok(self.run_script("agent-configs"))

    def deploy_changed_after_preflight(self, change):
        """Run deploy on the token as a deploy that stopped after creating
        it leaves it, no route and no tokens, with `change` made to it once
        preflight has read its router: as its owner could from elsewhere,
        enrolling another route or minting tokens, since the deployment's
        lock keeps out only this script's commands."""
        self.evm["contracts"][self.token].update(routers={}, supply=0)
        self.evm["interleave"] = {"token": self.token, "after_reads": 1, "change": change}
        self.queries.pop(f"{ROUTE}/enrolled_router/{EVM_DOMAIN}", None)
        result = self.run_script("deploy")
        self.assertGreaterEqual(self.evm.pop("interleave")["reads"], 2, "the change came after preflight's read")
        return result

    def token_writes(self):
        return [line for line in self.traced("cast send") if line.split()[2].lower() == self.token]

    def test_a_route_or_tokens_that_appear_after_preflight_are_refused(self):
        # Preflight passed the token, and enroll_routers read its router
        # again and replaced any but this deployment's: another deployment's
        # route, which the same owner enrolled in between, backing tokens
        # minted since, gave those tokens this route's DUSK. The token is
        # checked again right before the write now, and nothing is replaced.
        other = "e2" * 32
        # The token already has the hook deploy sets, so deploy's first write
        # to it is the enrollment.
        self.assert_ok(self.run_script("deploy"))
        for change, expected in (
                ({"routers": {str(DUSK_DOMAIN): "0x" + other}},
                 f"is enrolled with the Dusk route {other}, not the route recorded in"),
                ({"supply": 10**18}, f"has a supply of {10**18} but no Dusk route")):
            with self.subTest(change=change):
                self.assert_fails(self.deploy_changed_after_preflight(change), expected)
                self.assertEqual(self.token_writes() + self.traced("dusk-tx enroll-router"), [])
                self.assertEqual(self.evm["contracts"][self.token]["routers"], change.get("routers", {}))

    def test_a_route_that_appears_before_the_hook_is_set_is_refused(self):
        # The token dispatches through another hook, so deploy's first write
        # to it sets the route's hook, and the check comes right before that.
        other = "e2" * 32
        result = self.deploy_changed_after_preflight({"routers": {str(DUSK_DOMAIN): "0x" + other}})
        self.assert_fails(result, f"is enrolled with the Dusk route {other}, not the route recorded in")
        self.assertEqual(self.token_writes() + self.traced("dusk-tx enroll-router"), [])
        self.assertEqual(self.evm["contracts"][self.token]["hook"], self.hook)

    def test_a_router_on_the_dusk_route_is_never_replaced(self):
        # deploy enrolled the recorded token on the Dusk route whenever the
        # route's router for the EVM chain wasn't that token, replacing any
        # other, where the EVM side refuses to: run again after the token
        # record changed, it cut the old token's holders off from the DUSK
        # behind their tokens. A router that appears after the read-back,
        # which the owner can enroll from elsewhere (the deployment's lock
        # keeps out only this script's commands), is read right before each
        # write and refused: before the write to the token, and right before
        # the write to the route. deploy reads the route's router once in
        # preflight's read-back, then in enroll_routers before each write.
        # One enrolled between that last read and the write is replaced
        # (docs/security.md, LOW-20): only the reads are tested here.
        key, other = f"{ROUTE}/enrolled_router/{EVM_DOMAIN}", "00" * 12 + "22" * 20
        # The token already has the hook deploy sets, so deploy's only
        # writes to it are enrollments.
        self.assert_ok(self.run_script("deploy"))
        for after_reads in (1, 2):
            with self.subTest(after_reads=after_reads):
                self.evm["contracts"][self.token].update(routers={}, supply=0)
                self.queries.pop(key, None)
                self.dusk_interleave = {"key": key, "after_reads": after_reads, "value": other}
                result = self.run_script("deploy")
                self.assertGreater(self.dusk_interleave["reads"], after_reads, "the change came after the read-back")
                self.assert_fails(result, f"The Dusk route {ROUTE} is enrolled with the router {other} for sepolia "
                                          f"(domain {EVM_DOMAIN}), not the EVM DUSK token {self.token} recorded in")
                self.assertIn("not replacing it", result.stderr)
                self.assertEqual(self.traced("dusk-tx enroll-router"), [])
                self.assertEqual(self.queries[key], {"value": other})
                self.assertEqual(len(self.token_writes()), after_reads - 1)
        # With the router already the recorded token, enrolling it again is
        # nothing to send.
        self.dusk_interleave = None
        self.evm["contracts"][self.token]["routers"] = {str(DUSK_DOMAIN): "0x" + ROUTE}
        self.queries[key] = {"value": "00" * 12 + self.token[2:]}
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.traced("dusk-tx enroll-router") + self.token_writes(), [])
        # The token record changed since, to a new token of this deployment
        # with no route yet: the route is enrolled with the old one, and
        # every command that uses the route refuses it before sending
        # anything.
        new = "0x" + "33" * 20
        tx = proxy_creation(self.evm, new, self.token_impl, {
            "type": "token", "mailbox": EVM_MAILBOX, "hook": self.hook, "ism": self.ism, "owner": DEPLOYER,
            "routers": {}, "supply": 0, "name": "Dusk", "symbol": "DUSK", "decimals": 9, "scale": [1, 1]})
        state = self.state()
        self.write_state({**state, "evm": {**state["evm"], "token": new, "token_creation_tx": tx}})
        for command in ["preflight", "agent-configs", "claim", "deploy"]:
            with self.subTest(command=command):
                self.assert_fails(self.run_script(command),
                                  f"The Dusk route {ROUTE} is enrolled with the router {'00' * 12 + self.token[2:]} "
                                  f"for sepolia (domain {EVM_DOMAIN}), not none or the EVM DUSK token {new}")
                self.assertEqual(self.traced("cast send") + self.traced("dusk-tx enroll-router")
                                 + self.traced("dusk-tx claim-fees"), [])

    def test_a_token_of_another_mailbox_is_refused(self):
        # Mixed or restored state: deployment B's token, created by the same
        # deployer the same way, bound to Mailbox B, with its own hook and no
        # router for this route yet. It passed the creation checks, and
        # deploy set its hook and enrolled this route on it.
        token = self.evm["contracts"][self.token]
        token.update(mailbox="0x" + "bb" * 20, hook="0x" + "00" * 20, routers={})
        self.assert_refused(f"through the Mailbox 0x{'bb' * 20}, not EVM_MAILBOX")

    def test_a_token_with_another_ism_is_refused(self):
        other = "0x" + "56" * 20
        self.evm["contracts"][other] = {"type": "ism", "module_type": 5, "validators": [VALIDATOR], "threshold": 1}
        self.evm["contracts"][self.token]["ism"] = other
        self.assert_refused(f"verifies messages from Dusk with the ISM {other}, not {self.ism}")

    def two_of_three(self):
        """The settings' Dusk-origin validators, and the ISM's, 2 of
        [A, B, C]. Returns A, B and C."""
        a, b, c = validators(3)
        self.settings.update(DUSK_ORIGIN_VALIDATORS=f"{a},{b},{c}", DUSK_ORIGIN_THRESHOLD=2)
        self.evm["contracts"][self.ism].update(validators=[a, b, c], threshold=2)
        return a, b, c

    def test_an_ism_other_than_the_settings_is_refused(self):
        # Each case changes one thing: the validators at the same threshold,
        # or the threshold with the same validators.
        a, b, c = self.two_of_three()
        other = "0x" + "77" * 20
        saved = dict(self.evm["contracts"][self.ism])
        self.assert_ok(self.run_script("agent-configs"))
        settings = f"but DUSK_ORIGIN_THRESHOLD and DUSK_ORIGIN_VALIDATORS say 2 of [{a},{b},{c}]"
        for change, expected in [
            ({"validators": [a, b, other]}, f"needs 2 of [{other},{a},{b}], {settings}"),
            ({"validators": [a, b]}, f"needs 2 of [{a},{b}], {settings}"),
            ({"validators": [a, b, c, other]}, f"needs 2 of [{other},{a},{b},{c}], {settings}"),
            ({"threshold": 3}, f"needs 3 of [{a},{b},{c}], {settings}"),
            ({"threshold": 1}, f"needs 1 of [{a},{b},{c}], {settings}"),
            ({"module_type": 6}, "is of module type 6; it must be a MessageIdMultisig ISM"),
        ]:
            with self.subTest(change=change):
                self.evm["contracts"][self.ism] = {**saved, **change}
                self.assert_refused(expected)

    def test_validator_order_and_case_do_not_matter(self):
        a, b, c = self.two_of_three()
        mixed = c.upper().replace("0X", "0x")
        self.settings["DUSK_ORIGIN_VALIDATORS"] = f" {mixed}, {a},{b} "
        self.evm["contracts"][self.ism]["validators"] = [b, mixed, a]
        self.assert_ok(self.run_script("agent-configs"))

    def test_an_ism_that_lists_a_validator_twice_is_refused(self):
        # Hyperlane's MessageIdMultisig ISMs take a validator listed twice and
        # count its signature for each place: [A, A, B] at threshold 2 accepts
        # A's signature twice, so A alone can sign for a message. Folded into
        # [A, B], it passed as the settings' 2 of [A, B].
        a, b = validators(2)
        self.settings.update(DUSK_ORIGIN_VALIDATORS=f"{a},{b}", DUSK_ORIGIN_THRESHOLD=2)
        self.evm["contracts"][self.ism].update(validators=[a, b], threshold=2)
        self.assert_ok(self.run_script("agent-configs"))
        for listed, repeated in (([a, a, b], a), ([a, b, b.upper().replace("0X", "0x")], b),
                                 ([b, a, b, a], f"{a},{b}")):
            with self.subTest(listed=listed):
                self.evm["contracts"][self.ism]["validators"] = listed
                self.assert_refused(f"lists [{repeated}] more than once among its validators")


class TestnetQuoteSignerHistory(DeployedTestnet):
    """This deployment uses no signed quotes: an EVM IGP that ever had a quote
    signer is refused, from its QuoteSignerAdded events since the block that
    created it, whatever it quotes the senders the checks ask as."""

    COMMANDS = ["preflight", "agent-configs", "evm-igp", "deploy"]

    def assert_refused(self, expected):
        for command in self.COMMANDS:
            with self.subTest(command=command):
                result = self.run_script(command)
                self.assert_fails(result, expected)
                self.assertEqual(self.traced("cast send") + self.traced("forge"), [])
                self.assertFalse(self.relayer_config.exists())

    def test_a_former_signers_quote_for_another_sender_is_refused(self):
        # A signer, since removed, left a free standing quote for one sender,
        # neither the token nor the zero address the IGP is asked as. Its
        # signers are gone and both quote the oracle's price, so the checks
        # passed, and that sender bought delivery to Dusk for nothing.
        sender = "0x" + "a5" * 20
        self.evm["contracts"][self.evm_igp]["quotes"] = {
            f"{DUSK_DOMAIN}/{sender}": {"rate": 0, "price": 0, "expiry": 2**47}}
        quote_signer_history(self.evm, self.evm_igp, ATTACKER, 50)
        self.assert_refused(f"The EVM IGP {self.evm_igp} has had a quote signer: QuoteSignerAdded in")

    def test_a_new_igp_passes_and_later_reads_start_where_the_last_ended(self):
        self.assert_ok(self.run_script("agent-configs"))
        # Read from the block that created the IGP to the head, and recorded
        # up to EVM_REORG_PERIOD (2) below it.
        self.assertEqual(self.evm["log_queries"], [[1, 100]])
        self.assertEqual(self.state()["evm"]["igp_quote_signers_scanned"], {"igp": self.evm_igp, "to": 98})
        self.evm["block"] = 130
        self.assert_ok(self.run_script("preflight"))
        self.assertEqual(self.evm["log_queries"][-1], [99, 130])
        # A signer added since is found.
        quote_signer_history(self.evm, self.evm_igp, ATTACKER, 129)
        self.evm["block"] = 140
        self.assert_refused("has had a quote signer: QuoteSignerAdded in")

    def test_reads_span_at_most_evm_log_block_range_blocks(self):
        self.assert_ok(self.run_script("agent-configs", EVM_LOG_BLOCK_RANGE=40))
        self.assertEqual(self.evm["log_queries"], [[1, 40], [41, 80], [81, 100]])
        self.assert_fails(self.run_script("agent-configs", EVM_LOG_BLOCK_RANGE=0),
                          "EVM_LOG_BLOCK_RANGE must be a whole number of blocks")

    def test_a_new_igp_replaces_one_that_had_a_signer(self):
        # As docs/operations.md has it: take the IGP and the route's hook
        # out of the state, and deploy.
        quote_signer_history(self.evm, self.evm_igp, ATTACKER, 50)
        self.assert_fails(self.run_script("agent-configs"), "has had a quote signer")
        state = self.state()
        for key in ("igp", "igp_creation_tx", "route_hook"):
            del state["evm"][key]
        self.write_state(state)
        self.assert_ok(self.run_script("deploy"))
        evm = self.state()["evm"]
        self.assertNotEqual(evm["igp"], self.evm_igp)
        self.assertEqual(self.evm["contracts"][evm["route_hook"]]["hooks"], [EVM_MERKLE, evm["igp"]])
        self.assertEqual(self.evm["contracts"][self.token]["hook"], evm["route_hook"])
        self.assert_ok(self.run_script("agent-configs"))
        relayer = json.loads(self.relayer_config.read_text())
        self.assertEqual(relayer["chains"]["sepolia"]["interchainGasPaymaster"], evm["igp"])

    def test_a_scan_of_another_igp_is_not_used(self):
        state = self.state()
        state["evm"]["igp_quote_signers_scanned"] = {"igp": "0x" + "99" * 20, "to": 99}
        self.write_state(state)
        quote_signer_history(self.evm, self.evm_igp, ATTACKER, 60)
        self.assert_refused("has had a quote signer")


def expected_fees(ratio="25000", margin=5000, evm_gas_price=10**9, dusk_overhead=405000,
                  evm_overhead=2200000, dusk_gas_price=1):
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
        # What the gas paid for costs, in LUX, at the prices, before the margin.
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
        self.assertEqual(fees["dusk_igp"]["config"], f"{EVM_DOMAIN}:405000:375000:1000000000")
        self.assertEqual(fees["evm_igp"], {"domain": DUSK_DOMAIN, **DEFAULT_EVM_IGP})
        # 455,000 EVM gas at 1 gwei is 0.000455 ETH, 11.375 DUSK at 25,000
        # DUSK per ETH; with the 50% margin, 17.0625 DUSK.
        self.assertEqual(fees["dusk_to_evm"]["quote_lux"], "17062500000")
        self.assertEqual(fees["dusk_to_evm"]["gas_cost_lux"], "11375000000")
        # 2,200,000 Dusk gas at 1 LUX is 0.0022 DUSK, 8.8e10 wei; with the
        # margin, 1.32e11 wei.
        self.assertEqual(fees["evm_to_dusk"]["quote_wei"], "132000000000")
        self.assertEqual(fees["evm_to_dusk"]["gas_cost_wei"], "88000000000")
        self.assertEqual(fees["relayer"], {"dusk_origin_gas_fraction": "2/3",
                                           "evm_origin_gas_fraction": "11/25",
                                           "evm_origin_required_gas": 2200000})
        summary = self.run_script("fees")
        self.assert_ok(summary)
        self.assertIn("quote 17.0625 DUSK", summary.stdout)
        self.assertIn("quote 0.000000132 ETH (132,000,000,000 wei)", summary.stdout)

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
        # 420,000 + 7,500 x 6 + 2,500 x 6 = 480,000 EVM gas, less the 50,000.
        six = ",".join(validators(6))
        self.assert_fails(
            self.run_script("fees", DUSK_ORIGIN_VALIDATORS=six, DUSK_ORIGIN_THRESHOLD=6),
            "Set it to at least 430,000")
        self.assert_ok(self.run_script("fees", DUSK_ORIGIN_VALIDATORS=six, DUSK_ORIGIN_THRESHOLD=6,
                                       DUSK_IGP_GAS_OVERHEAD=430000))
        # The default covers 3 of 5. The 110,000 deploy/testnet.env.example
        # had before, sized on Anvil, is refused even at 1 of 1: Sepolia
        # estimated a token's first mint at 424,972 gas at 1 of 2.
        self.assert_ok(self.run_script("fees", DUSK_ORIGIN_VALIDATORS=",".join(validators(5)),
                                       DUSK_ORIGIN_THRESHOLD=3))
        self.assert_fails(self.run_script("fees", DUSK_IGP_GAS_OVERHEAD=110000), "Set it to at least 380,000")
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
            {"type": "onChainFeeQuoting", "gasFraction": "11/25",
             "matchingList": [{"originDomain": EVM_DOMAIN}]},
        ])
        self.assertTrue(relayer["igpIndexingEnabled"])
        # The relayer reads EVM-origin payments from our IGP, not Hyperlane's.
        self.assertEqual(relayer["chains"]["sepolia"]["interchainGasPaymaster"], self.evm_igp)
        self.assertEqual(relayer["chains"]["dusk"]["interchainGasPaymaster"], "0x" + IGP)

    def test_the_relayer_delivers_to_dusk_one_at_a_time(self):
        # Pipelined deliveries are experimental: the configs carry neither
        # pipelineDeliveries nor maxBatchSize, so every agent build delivers
        # one at a time. An env file from before, which still sets
        # DUSK_RELAYER_MAX_BATCH_SIZE, gets a warning and the same configs.
        for settings in ({}, {"DUSK_RELAYER_MAX_BATCH_SIZE": 16}):
            with self.subTest(settings=settings):
                result = self.run_script("agent-configs", **settings)
                self.assert_ok(result)
                relayer = json.loads(self.relayer_config.read_text())
                validator = json.loads((self.out / "validator-dusk.json").read_text())
                for chain in (relayer["chains"]["dusk"], relayer["chains"]["sepolia"], validator["chains"]["dusk"]):
                    self.assertNotIn("maxBatchSize", chain)
                    self.assertNotIn("pipelineDeliveries", chain)
                self.assertIn("The relayer delivers to Dusk one at a time", result.stderr)
                self.assertEqual("DUSK_RELAYER_MAX_BATCH_SIZE is no longer read" in result.stderr, bool(settings))

    def test_the_agents_batch_contract_reads_through_the_multicall(self):
        self.assert_ok(self.run_script("agent-configs"))
        relayer = json.loads(self.relayer_config.read_text())
        validator = json.loads((self.out / "validator-dusk.json").read_text())
        for config in (relayer, validator):
            self.assertEqual(config["chains"]["dusk"]["batchContractAddress"], "0x" + MULTICALL)
        self.assertNotIn("batchContractAddress", relayer["chains"]["sepolia"])

    def test_a_chain_without_the_multicall_gets_single_queries(self):
        for case, entry, expected in [
            ("absent", None, f"Multicall for Dusk isn't on this chain at {MULTICALL}"),
            ("another version", {"value": 2}, "answers version '2', not 1"),
        ]:
            with self.subTest(case=case):
                self.queries.pop(f"{MULTICALL}/version", None)
                if entry is not None:
                    self.queries[f"{MULTICALL}/version"] = entry
                for command in ("agent-configs", "preflight"):
                    result = self.run_script(command)
                    self.assert_ok(result)
                    self.assertIn(expected, result.stderr)
                relayer = json.loads(self.relayer_config.read_text()) if self.relayer_config.exists() else None
                validator = json.loads((self.out / "validator-dusk.json").read_text())
                self.assertNotIn("batchContractAddress", validator["chains"]["dusk"])
                self.assertTrue(relayer is None or "batchContractAddress" not in relayer["chains"]["dusk"])
        # A failure that doesn't say the contract is missing stops the script.
        self.queries[f"{MULTICALL}/version"] = {"error": "Query failed (500): connection reset"}
        self.assert_fails(self.run_script("agent-configs"), "Can't ask Multicall for Dusk")
        self.assertFalse(self.relayer_config.exists())
        # Empty: left out, and not asked for.
        result = self.run_script("agent-configs", DUSK_MULTICALL="")
        self.assert_ok(result)
        self.assertNotIn("batchContractAddress", json.loads(self.relayer_config.read_text())["chains"]["dusk"])
        self.assertFalse([line for line in self.traced("dusk-tx query") if "--method version" in line])
        self.assert_fails(self.run_script("agent-configs", DUSK_MULTICALL="0x1234"),
                          "DUSK_MULTICALL must be a contract ID of 64 hex characters")

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

    def test_agent_configs_refuse_inserting_twice(self):
        # Should Sepolia's Mailbox come to require the MerkleTreeHook, the
        # route's aggregation would insert each message a second time.
        self.evm["contracts"][EVM_MAILBOX]["requiredHook"] = EVM_MERKLE
        self.assert_fails(self.run_script("agent-configs"), "every message would be inserted twice")

    def test_dusk_igp_dry_run_shows_both_quotes_and_sends_nothing(self):
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"] = {"value": {
            "gas_overhead": 200000, "token_exchange_rate": 1, "gas_price": 1000000000}}
        result = self.run_script("dusk-igp")
        self.assert_ok(result)
        rows = [line.split() for line in result.stderr.splitlines() if line.split()[:1] in (["now"], ["new"])]
        # The pricing deploy/testnet.sh used to deploy charged 25,000 LUX.
        self.assertEqual(rows, [["now", "200000", "1", "1000000000", "25000"],
                                ["new", "405000", "375000", "1000000000", "17062500000"]])
        self.assertIn("Dry run: nothing was sent", result.stderr)
        self.assertEqual(self.traced("dusk-tx set-igp-config"), [])

    def test_dusk_igp_apply_sets_the_settings_pricing(self):
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"] = 1
        self.assert_ok(self.run_script("dusk-igp", "--apply"))
        [sent] = self.traced("dusk-tx set-igp-config")
        self.assertIn(f"--igp {IGP} --igp-domain-config {EVM_DOMAIN}:405000:375000:1000000000", sent)
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
        self.assertEqual(rows, [["now", "2200000", "600000000000000", "1", "132000000000"],
                                ["new", "2200000", "750000000000000", "1", "165000000000"]])
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

    def test_evm_igp_never_points_the_igp_at_an_unpriced_oracle(self):
        # The IGP reads from oracle B, priced as the settings say, but the
        # state records oracle A, which prices nothing. Before, --apply saw
        # B's pricing match and left A as it was, pointed the IGP at A and
        # only then failed: the IGP quoted delivery to Dusk at zero.
        other = "0x" + "67" * 20
        contracts = self.evm["contracts"]
        contracts[other] = {"type": "oracle", "owner": DEPLOYER, "data": dict(contracts[self.oracle]["data"])}
        contracts[self.oracle]["data"] = {}
        contracts[self.evm_igp]["configs"][str(DUSK_DOMAIN)][0] = other
        for command in (["evm-igp"], ["evm-igp", "--apply"]):
            with self.subTest(command=command):
                self.assert_fails(self.run_script(*command), f"reads domain {DUSK_DOMAIN}'s pricing from {other}, "
                                                             f"not the StorageGasOracle {self.oracle}")
                self.assertEqual(self.traced("cast send"), [])
                self.assertEqual(self.evm["contracts"][self.evm_igp]["configs"][str(DUSK_DOMAIN)][0], other)

    def test_evm_igp_apply_prices_the_oracle_before_the_igp_reads_from_it(self):
        # The IGP reads from no oracle yet, and the recorded one has an old
        # pricing.
        self.evm["contracts"][self.evm_igp]["configs"] = {}
        self.evm["contracts"][self.oracle]["data"] = {str(DUSK_DOMAIN): [1, 1]}
        self.assert_ok(self.run_script("evm-igp", "--apply"))
        sent = self.traced("cast send")
        self.assertEqual([line.split()[3].split("(")[0] for line in sent],
                         ["setRemoteGasDataConfigs", "setDestinationGasConfigs"])
        self.assertEqual(sent[0].split()[2], self.oracle)
        self.assertEqual(self.evm["contracts"][self.oracle]["data"][str(DUSK_DOMAIN)], [600000000000000, 1])
        self.assertEqual(self.evm["contracts"][self.evm_igp]["configs"][str(DUSK_DOMAIN)], [self.oracle, 2200000])

    def test_fee_commands_refuse_a_dusk_node_on_another_chain(self):
        # The node is on chain 1, not DUSK_EXPECTED_CHAIN_ID 2. A Dusk
        # contract's ID comes from its bytecode, the deployer key and a
        # nonce, so the same IGP can exist on both networks.
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"] = 1
        self.queries[f"{IGP}/claimable_fees"] = {"value": 5}
        self.evm["balances"][self.evm_igp] = 7
        self.extra_env = {"FIXTURE_DUSK_CHAIN_ID": "1"}
        for command in (["dusk-igp", "--apply"], ["claim"], ["dusk-igp"], ["revenue"], ["agent-configs"]):
            with self.subTest(command=command):
                result = self.run_script(*command)
                self.assert_fails(result, "is on chain ID 1, expected DUSK_EXPECTED_CHAIN_ID 2")
                self.assertEqual(self.traced("dusk-tx set-igp-config") + self.traced("dusk-tx claim-fees")
                                 + self.traced("cast send"), [])
                self.assertFalse(self.relayer_config.exists())

    def test_fee_commands_refuse_an_evm_rpc_on_another_chain(self):
        self.evm["chain_id"] = 1
        self.evm["contracts"][self.oracle]["data"][str(DUSK_DOMAIN)] = [1, 1]
        self.evm["balances"][self.evm_igp] = 7
        self.queries[f"{IGP}/claimable_fees"] = {"value": 5}
        for command in (["evm-igp", "--apply"], ["claim"], ["evm-igp"], ["revenue"], ["agent-configs"]):
            with self.subTest(command=command):
                result = self.run_script(*command)
                self.assert_fails(result, f"reports chain ID 1, expected EVM_CHAIN_ID {EVM_DOMAIN}")
                self.assertEqual(self.traced("cast send") + self.traced("dusk-tx claim-fees"), [])
                self.assertFalse(self.relayer_config.exists())

    def test_fee_commands_sign_only_for_the_expected_dusk_chain(self):
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"] = 1
        self.assert_ok(self.run_script("dusk-igp", "--apply"))
        [sent] = self.traced("dusk-tx set-igp-config")
        self.assertIn("--expected-chain-id 2", sent)
        self.queries[f"{IGP}/claimable_fees"] = {"value": 5}
        self.queries[f"{PROTOCOL_FEE}/claimable_fees"] = {"value": 5}
        self.assert_ok(self.run_script("claim"))
        claims = self.traced("dusk-tx claim-fees")
        self.assertEqual(len(claims), 2)
        self.assertTrue(all("--expected-chain-id 2" in line for line in claims), claims)
        # Another beneficiary is told how to claim, on this chain only.
        self.queries[f"{IGP}/claimable_fees"] = {"value": 5}
        self.queries[f"{IGP}/beneficiary"] = {"value": "be" * 32}
        result = self.run_script("claim")
        self.assert_ok(result)
        self.assertIn(f"--contract {IGP} --expected-chain-id 2", result.stderr)

    def test_fee_commands_check_the_recorded_deployment(self):
        other = "a9" * 32
        cases = [
            # Dispatches pay another IGP than the one dusk-igp would price:
            # the Mailbox doesn't call the recorded one.
            ({f"{MAILBOX}/default_hook": {"value": other}}, ["dusk-igp", "--apply"],
             f"The Dusk igp {IGP} takes its calls from the Mailbox {MAILBOX}, which doesn't call it"),
            # The recorded IGP or ProtocolFee is another deployment's.
            ({f"{IGP}/mailbox": {"value": other}}, ["dusk-igp", "--apply"],
             f"The Dusk igp {IGP} takes its calls from {other}, which is neither the Mailbox {MAILBOX}"),
            ({f"{PROTOCOL_FEE}/mailbox": {"value": other}}, ["claim"],
             f"The Dusk protocol_fee {PROTOCOL_FEE} takes its calls from {other}, which is neither the Mailbox {MAILBOX}"),
            # A ProtocolFee anyone deployed for this Mailbox's aggregation
            # hook, which doesn't call it.
            ({f"{AGGREGATION_HOOK}/hooks": {"value": [MERKLE_TREE_HOOK]}}, ["claim"],
             f"The Dusk protocol_fee {PROTOCOL_FEE} takes its calls from the Mailbox's required_hook "
             f"{AGGREGATION_HOOK}, which doesn't call it"),
            # The aggregation hook takes its calls from another Mailbox.
            ({f"{AGGREGATION_HOOK}/mailbox": {"value": other}}, ["claim"],
             f"{AGGREGATION_HOOK}, which takes its own from {other}, not the Mailbox {MAILBOX}"),
            ({f"{MAILBOX}/local_domain": {"value": 7}}, ["claim"],
             f"The Dusk Mailbox {MAILBOX} recorded in"),
        ]
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]["token_exchange_rate"] = 1
        self.queries[f"{IGP}/claimable_fees"] = {"value": 5}
        for change, command, expected in cases:
            with self.subTest(command=command, expected=expected):
                saved = {key: self.queries[key] for key in change}
                self.queries.update(change)
                result = self.run_script(*command)
                self.queries.update(saved)
                self.assert_fails(result, expected)
                self.assertEqual(self.traced("dusk-tx set-igp-config") + self.traced("dusk-tx claim-fees")
                                 + self.traced("cast send"), [])
        # The state is another deployment's.
        state = self.state()
        for key, value, command, expected in [
            ("dusk_domain", 7, ["dusk-igp", "--apply"], f"was made for Dusk domain 7, not DUSK_DOMAIN {DUSK_DOMAIN}"),
            ("evm_domain", 7, ["evm-igp", "--apply"], f"was made for EVM domain 7, not EVM_DOMAIN {EVM_DOMAIN}"),
        ]:
            with self.subTest(state=key):
                self.write_state({**state, key: value})
                self.assert_fails(self.run_script(*command), expected)
                self.assertEqual(self.traced("dusk-tx set-igp-config") + self.traced("cast send"), [])
        self.write_state({**state, "evm": {**state["evm"], "mailbox": "0x" + "12" * 20}})
        self.assert_fails(self.run_script("evm-igp", "--apply"), f"was made for EVM Mailbox 0x{'12' * 20}")
        self.assertEqual(self.traced("cast send"), [])

    def test_fee_contracts_take_their_calls_as_deploy_hyperlane_wires_them(self):
        # As dusk-tx deploy-hyperlane wires it (dusk_queries), the ProtocolFee
        # takes its calls from the aggregation hook that is the Mailbox's
        # required hook. A ProtocolFee bound to the Mailbox itself, as its
        # required hook, passed the binding check, but deploy-hyperlane never
        # deploys one, and the read-back refuses it.
        self.queries[f"{PROTOCOL_FEE}/claimable_fees"] = {"value": 5}
        self.assert_ok(self.run_script("claim"))
        self.assertIn(f"--contract {PROTOCOL_FEE}", " ".join(self.traced("dusk-tx claim-fees")))
        self.queries[f"{PROTOCOL_FEE}/mailbox"] = {"value": MAILBOX}
        self.queries[f"{MAILBOX}/required_hook"] = {"value": PROTOCOL_FEE}
        self.assert_fails(self.run_script("claim"),
                          f"The Dusk protocol_fee {PROTOCOL_FEE}'s mailbox is {MAILBOX}, not {AGGREGATION_HOOK}")
        self.assertEqual(self.traced("dusk-tx claim-fees"), [])

    def test_a_fee_contract_the_mailbox_doesnt_call_is_refused(self):
        # Bound to the Mailbox, but neither its required nor its default
        # hook: the Mailbox never calls it, called by the aggregation hook or
        # not, so it isn't this deployment's. Before, binding to the Mailbox
        # was enough, and claim signed for it; and deploy, with a beneficiary
        # set, made that the detached contract's beneficiary without checking
        # its binding at all.
        self.queries[f"{PROTOCOL_FEE}/claimable_fees"] = {"value": 5}
        self.queries[f"{PROTOCOL_FEE}/mailbox"] = {"value": MAILBOX}
        for hooks in ([MERKLE_TREE_HOOK], [MERKLE_TREE_HOOK, PROTOCOL_FEE]):
            with self.subTest(aggregated=hooks):
                self.queries[f"{AGGREGATION_HOOK}/hooks"] = {"value": hooks}
                for command, settings in ((["claim"], {}), (["revenue"], {}), (["agent-configs"], {}),
                                          (["deploy"], {"DUSK_FEE_BENEFICIARY": "fixture-treasury"})):
                    result = self.run_script(*command, **settings)
                    if command == ["revenue"]:
                        # revenue only reads, and checks the chains alone.
                        self.assert_ok(result)
                        continue
                    self.assert_fails(result, f"The Dusk protocol_fee {PROTOCOL_FEE} takes its calls from the "
                                              f"Mailbox {MAILBOX}, which doesn't call it")
                    self.assertEqual(self.traced("dusk-tx claim-fees") + self.traced("dusk-tx set-beneficiary")
                                     + self.traced("dusk-tx enroll-router") + self.traced("cast send"), [])
                    self.assertFalse(self.relayer_config.exists())

    def test_deploy_sets_the_beneficiary_of_a_recorded_deployment(self):
        # The recorded deployment's fee contracts are its own, so deploy
        # changes their beneficiary.
        self.assert_ok(self.run_script("deploy", DUSK_FEE_BENEFICIARY="fixture-treasury"))
        sent = self.traced("dusk-tx set-beneficiary")
        self.assertEqual([line.split("--contract ")[1].split()[0] for line in sent], [IGP, PROTOCOL_FEE])

    def test_claim_collects_both_chains(self):
        self.queries[f"{IGP}/claimable_fees"] = {"value": 6000000000}
        self.queries[f"{PROTOCOL_FEE}/claimable_fees"] = {"value": 1000000}
        beneficiary = "0x" + "be" * 20
        self.evm["contracts"][self.evm_igp]["beneficiary"] = beneficiary
        self.evm["balances"][self.evm_igp] = 130200000000
        result = self.run_script("revenue")
        self.assert_ok(result)
        self.assertIn(f"holds 6000000000 LUX for the account with hash {DEPLOYER_H256}", result.stderr)
        self.assertIn(f"holds 130200000000 wei for {beneficiary}", result.stderr)
        self.assert_ok(self.run_script("claim"))
        self.assertEqual(self.traced("cast send"), [f"cast send {self.evm_igp} claim()"])
        self.assertEqual(len(self.traced("dusk-tx claim-fees")), 2)
        self.assertEqual(self.evm["balances"][beneficiary], 130200000000)
        self.assertEqual(self.queries[f"{IGP}/claimable_fees"]["value"], 0)

    def test_a_claim_an_earlier_run_sent_is_settled_before_the_balance_is_read(self):
        # claim()'s send times out, and the transaction is mined after,
        # claiming the IGP's balance then; more fees are paid after it. The
        # next claim settles the one an earlier run sent before it reads the
        # balance, and claims what has been paid since with a claim() of
        # its own, rather than take the earlier one for it.
        beneficiary = "0x" + "be" * 20
        self.evm["contracts"][self.evm_igp]["beneficiary"] = beneficiary
        self.evm["balances"][self.evm_igp] = 7
        self.evm.update(hold=1)
        self.assert_fails(self.run_script("claim"), "timed out")
        self.evm.update(mine_held=True, after_mining={self.evm_igp: 5})
        result = self.run_script("claim")
        self.assert_ok(result)
        self.assertIn("It executed: the step reads the chain as of its block, or later", result.stderr)
        self.assertIn(f"Claimed 5 wei from the EVM IGP {self.evm_igp}", result.stderr)
        self.assertEqual(self.evm["balances"][beneficiary], 12)
        self.assertEqual(self.evm["balances"].get(self.evm_igp, 0), 0)
        self.assertNotIn("evm_sending", self.state())

    def test_an_igp_that_takes_no_top_ups_is_refused(self):
        # IGP state_version 3 has no pay_for_gas: a Dusk message that paid it
        # too little could never be topped up, and the relayer would never
        # deliver it.
        self.queries[f"{IGP}/state_version"] = {"value": 3}
        for command in ["preflight", "agent-configs", "dusk-igp"]:
            with self.subTest(command=command):
                result = self.run_script(command)
                self.assert_fails(result, "takes no top-ups (pay_for_gas)")
                self.assertIn("records a Dusk contract (igp) at state version 3; this checkout's is 4",
                              result.stdout + result.stderr)
                self.assertFalse(self.relayer_config.exists())

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
        self.assertIn(f"--igp-domain-config {EVM_DOMAIN}:405000:375000:1000000000", deploy)
        # Every Dusk transaction is signed for DUSK_EXPECTED_CHAIN_ID only.
        [enroll] = self.traced("dusk-tx enroll-router")
        for sent in (deploy, enroll):
            self.assertIn("--expected-chain-id 2", sent)
        state = self.state()
        evm = self.evm["contracts"]
        oracle, igp, hook, token = (state["evm"][key] for key in
                                    ("storage_gas_oracle", "igp", "route_hook", "token"))
        self.assertEqual(evm[oracle]["data"][str(DUSK_DOMAIN)], [600000000000000, 1])
        self.assertEqual(evm[igp]["owner"], DEPLOYER)
        self.assertEqual(evm[igp]["beneficiary"], DEPLOYER)
        self.assertEqual(evm[igp]["configs"][str(DUSK_DOMAIN)], [oracle, 2200000])
        # The agents index from before the IGP and the token existed.
        self.assertLessEqual(state["evm"]["index_from"], min(evm[igp]["created_at"], evm[token]["created_at"]))
        # Sepolia's Mailbox requires only its ProtocolFee: the route's hook
        # replaces the default one, so it inserts into the Merkle tree first.
        self.assertEqual({k: v for k, v in evm[hook].items() if k != "created_at"},
                         {"type": "aggregation", "hook_type": 2, "hooks": [EVM_MERKLE, igp]})
        self.assertEqual(evm[token]["hook"], hook)
        # A transfer pays the 1 wei protocol fee and the IGP's quote.
        self.assertIn("charges 132000000000 wei for a transfer to Dusk", result.stderr)
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

    def test_an_interrupted_deploy_finishes_configuring_the_igp(self):
        # deploy stopped after deploying the oracle and the IGP.
        oracle, igp, impl = "0x" + "66" * 20, "0x" + "44" * 20, "0x" + "4a" * 20
        self.evm["contracts"][oracle] = {"type": "oracle", "owner": DEPLOYER, "data": {}}
        self.evm["contracts"][impl] = {"type": "igp", "hook_type": 4, "owner": "0x" + "00" * 20,
                                       "beneficiary": "0x" + "00" * 20, "configs": {}}
        tx = proxy_creation(self.evm, igp, impl, {"type": "igp", "hook_type": 4, "owner": DEPLOYER,
                                                  "beneficiary": DEPLOYER, "configs": {},
                                                  "quote_signers": [], "quotes": {}})
        self.write_state({"evm": {"mailbox": EVM_MAILBOX, "storage_gas_oracle": oracle, "igp": igp,
                                  "igp_implementation": impl, "igp_creation_tx": tx, "index_from": 1}})
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("isn't priced for Dusk yet; deploy prices it", result.stderr)
        self.assertEqual(self.evm["contracts"][igp]["owner"], DEPLOYER)
        self.assertEqual(self.evm["contracts"][igp]["configs"][str(DUSK_DOMAIN)], [oracle, 2200000])
        self.assertEqual(self.state()["evm"]["igp"], igp)

    def test_deploy_initializes_the_igp_and_the_token_in_their_creation_transactions(self):
        self.assert_ok(self.run_script("deploy"))
        state = self.state()["evm"]
        for key in ("igp", "token"):
            with self.subTest(contract=key):
                address, tx = state[key], state[f"{key}_creation_tx"]
                receipt = self.evm["receipts"][tx]
                self.assertEqual(receipt["contractAddress"], address)
                self.assertEqual(receipt["from"], DEPLOYER)
                topics = [entry["topics"][0] for entry in receipt["logs"] if entry["address"] == address]
                self.assertEqual(topics[0], UPGRADED)
                self.assertIn(INITIALIZED, topics)
                self.assertEqual(self.evm["contracts"][address]["implementation"],
                                 state[f"{key}_implementation"])
        # Nothing initializes them afterwards: there is no second transaction
        # to send first.
        self.assertEqual([line for line in self.trace.read_text().splitlines()
                          if line.startswith("cast send") and "initialize" in line], [])
        token = self.evm["contracts"][state["token"]]
        self.assertEqual((token["owner"], token["ism"], token["hook"], token["supply"]),
                         (DEPLOYER, state["ism"], state["route_hook"], 0))

    def test_front_runners_get_nothing_from_the_bare_contracts(self):
        # An attacker initializes every bare IGP and HypERC20 the deployer
        # creates before the deployer's next transaction. Before, the IGP was
        # theirs (or paid them, with the deployer as its owner), and the
        # token's initialization reverted.
        for mode in ("owner", "beneficiary"):
            with self.subTest(igp=mode):
                self.evm = sepolia()
                self.forget_deployment()
                self.evm["front_run"] = {"attacker": ATTACKER, "igp": mode, "token": "owner"}
                result = self.run_script("deploy")
                self.assert_ok(result)
                state = self.state()["evm"]
                igp, token = self.evm["contracts"][state["igp"]], self.evm["contracts"][state["token"]]
                self.assertEqual((igp["owner"], igp["beneficiary"]), (DEPLOYER, DEPLOYER))
                self.assertEqual((token["owner"], token["ism"], token["supply"]), (DEPLOYER, state["ism"], 0))
                # The attacker only initialized the unused implementations.
                self.assertEqual(self.evm["contracts"][state["igp_implementation"]]["owner"],
                                 ATTACKER if mode == "owner" else DEPLOYER)

    def test_a_front_run_cannot_plant_a_free_standing_quote(self):
        # A helper owner adds a quote signer, which signs a free standing
        # quote for any sender to Dusk, then hands the IGP to the deployer:
        # the oracle's pricing and the owner check still passed.
        self.evm["front_run"] = {"attacker": ATTACKER, "igp": "standing_quote"}
        self.assert_ok(self.run_script("deploy"))
        igp = self.evm["contracts"][self.state()["evm"]["igp"]]
        self.assertEqual(igp["quotes"], {})
        self.assert_ok(self.run_script("agent-configs"))

    def test_beneficiaries_are_configurable(self):
        beneficiary = "0x" + "be" * 20
        self.assert_ok(self.run_script("deploy", EVM_IGP_BENEFICIARY=beneficiary,
                                       DUSK_FEE_BENEFICIARY="fixture-treasury"))
        self.assertEqual(self.evm["contracts"][self.state()["evm"]["igp"]]["beneficiary"], beneficiary)
        sent = self.traced("dusk-tx set-beneficiary")
        self.assertEqual(len(sent), 2)
        self.assertTrue(all("--beneficiary fixture-treasury" in line and "--expected-chain-id 2" in line
                            for line in sent), sent)

    def test_an_unset_beneficiary_leaves_the_igps_alone(self):
        self.assert_ok(self.run_script("deploy"))
        igp = self.state()["evm"]["igp"]
        self.evm["contracts"][igp]["beneficiary"] = "0x" + "be" * 20
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.evm["contracts"][igp]["beneficiary"], "0x" + "be" * 20)
        self.assertEqual(self.state()["evm"]["igp_beneficiary"], "0x" + "be" * 20)
        self.assertEqual(self.traced("cast send"), [])
        self.assertEqual(self.traced("dusk-tx set-beneficiary"), [])

    def test_an_older_deployment_is_refused_until_it_is_redeployed(self):
        # deploy/testnet.sh before relaying fees: Hyperlane's IGP recorded as
        # .evm.igp, and a token it initialized in a second transaction.
        evm = self.evm["contracts"]
        hyperlane_igp, ism, token = "0x" + "6f" * 20, "0x" + "55" * 20, "0x" + "11" * 20
        evm[hyperlane_igp] = {"type": "igp", "hook_type": 4, "owner": "0x" + "ab" * 20,
                              "beneficiary": "0x" + "ab" * 20, "configs": {}}
        evm[ism] = {"type": "ism"}
        evm[token] = {"type": "token", "mailbox": EVM_MAILBOX, "hook": "0x" + "00" * 20, "ism": ism,
                      "owner": DEPLOYER, "routers": {str(DUSK_DOMAIN): "0x" + ROUTE}, "initialized": True}
        self.queries[f"{ROUTE}/enrolled_router/{EVM_DOMAIN}"] = {"value": "00" * 12 + "11" * 20}
        self.write_state({
            "evm_chain": "sepolia", "evm_domain": EVM_DOMAIN,
            "evm": {"mailbox": EVM_MAILBOX, "merkle_tree_hook": EVM_MERKLE, "validator_announce": EVM_VA,
                    "igp": hyperlane_igp, "ism": ism, "token": token, "owner": DEPLOYER, "index_from": 1},
            "dusk": DUSK_CONTRACTS, "dusk_domain": DUSK_DOMAIN,
        })
        for command in ["deploy", "preflight"]:
            with self.subTest(command=command):
                self.assert_fails(self.run_script(command),
                                  f"records the EVM DUSK token {token} without the transaction that created")
                self.assertEqual(self.traced("cast send"), [])
                self.assertEqual(self.traced("forge"), [])

ATTACKER_H256 = "ee" * 32
OTHER_ID = "a9" * 32


class TestnetDuskReadBack(TestnetScript):
    """deploy records the Dusk contracts only once each reads back as
    dusk-tx deploy-hyperlane deploys it: its owner, the bindings its init
    set, its state version, and what only its owner can change later, as
    init left it. A deploy or a contract's init can be front-run, and the
    contract then has whoever came first as owner, and their bindings."""

    FRESH = [
        ({f"{MAILBOX}/owner": {"value": ATTACKER_H256}},
         f"The Dusk mailbox {MAILBOX}'s owner is {ATTACKER_H256}, not the deployer's account {DEPLOYER_H256}"),
        ({f"{ROUTE}/owner": {"value": None}},
         f"The Dusk warp_native {ROUTE}'s owner is nobody, not the deployer's account"),
        ({f"{MULTISIG}/owner": {"error": "Query failed (500): busy"}},
         f"Can't read the owner of the Dusk ism_multisig {MULTISIG}: Query failed (500): busy"),
        ({f"{PROTOCOL_FEE}/beneficiary": {"value": ATTACKER_H256}},
         f"The Dusk protocol_fee {PROTOCOL_FEE}'s beneficiary is {ATTACKER_H256}, not {DEPLOYER_H256}"),
        ({f"{IGP}/beneficiary": {"value": ATTACKER_H256}},
         f"The Dusk igp {IGP}'s beneficiary is {ATTACKER_H256}, not {DEPLOYER_H256}"),
        ({f"{PROTOCOL_FEE}/protocol_fee": {"value": 100000000}},
         f"The Dusk protocol_fee {PROTOCOL_FEE}'s protocol_fee is 100000000, not 1000000"),
        ({f"{MULTISIG}/validators_and_threshold": {"value": {"validators": ["0x" + "77" * 20], "threshold": 1}}},
         f"The Dusk ISM {MULTISIG} needs 1 of [0x{'77' * 20}], not 1 of [{VALIDATOR}]"),
        ({f"{MAILBOX}/default_ism": {"value": TEST_MOCK}},
         f"The Dusk mailbox {MAILBOX}'s default_ism is {TEST_MOCK}, not {MULTISIG}"),
        ({f"{MAILBOX}/required_hook": {"value": OTHER_ID}},
         f"The Dusk mailbox {MAILBOX}'s required_hook is {OTHER_ID}, not {AGGREGATION_HOOK}"),
        ({f"{MERKLE_TREE_HOOK}/mailbox": {"value": MAILBOX}},
         f"The Dusk merkle_tree_hook {MERKLE_TREE_HOOK}'s mailbox is {MAILBOX}, not {AGGREGATION_HOOK}"),
        ({f"{AGGREGATION_HOOK}/hooks": {"value": [PROTOCOL_FEE, MERKLE_TREE_HOOK]}},
         f"The Dusk aggregation_hook {AGGREGATION_HOOK}'s hooks is"),
        ({f"{VALIDATOR_ANNOUNCE}/mailbox": {"value": OTHER_ID}},
         f"The Dusk validator_announce {VALIDATOR_ANNOUNCE}'s mailbox is {OTHER_ID}, not {MAILBOX}"),
        ({f"{ROUTE}/mailbox": {"value": OTHER_ID}},
         f"The Dusk warp_native {ROUTE}'s mailbox is {OTHER_ID}, not {MAILBOX}"),
        ({f"{ROUTE}/hook": {"value": OTHER_ID}},
         f"The Dusk warp_native {ROUTE}'s hook is {OTHER_ID}, not {ZERO}"),
        ({f"{ROUTE}/interchain_security_module": {"value": OTHER_MULTISIG}},
         f"The Dusk warp_native {ROUTE}'s interchain_security_module is {OTHER_MULTISIG}, not {ZERO}"),
        ({f"{ROUTE}/enrolled_router/{EVM_DOMAIN}": {"value": ATTACKER_H256}},
         f"The Dusk route {ROUTE} is enrolled with the router {ATTACKER_H256} for sepolia "
         f"(domain {EVM_DOMAIN}), not none: deploy-hyperlane enrolls none"),
        ({f"{ROUTE}/enrolled_router/{DUSK_DOMAIN}": {"value": ATTACKER_H256}},
         f"The Dusk warp_native {ROUTE}'s enrolled_router is {ATTACKER_H256}, not {ZERO}"),
        ({f"{TEST_RECIPIENT}/interchain_security_module": {"value": TEST_MOCK}},
         f"The Dusk test_recipient {TEST_RECIPIENT}'s interchain_security_module is {TEST_MOCK}, not {ZERO}"),
        ({f"{AGGREGATION_HOOK}/state_version": {"value": 2}},
         f"The Dusk aggregation_hook {AGGREGATION_HOOK}'s state_version is 2, not 1"),
    ]

    def assert_nothing_recorded_or_sent(self):
        self.assertNotIn("dusk", self.state())
        self.assertNotIn("dusk_owner", self.state())
        self.assertEqual(self.traced("cast send") + self.traced("forge") + self.traced("dusk-tx enroll-router")
                         + self.traced("dusk-tx set-beneficiary"), [])

    def test_deploy_records_the_contracts_and_their_owner_once_they_read_back(self):
        self.assert_ok(self.run_script("deploy"))
        state = self.state()
        self.assertEqual(state["dusk"], DUSK_CONTRACTS)
        self.assertEqual(state["dusk_owner"], DEPLOYER_H256)
        self.assertEqual(len(self.traced("dusk-tx deploy-hyperlane")), 1)

    def test_deploy_records_nothing_that_doesnt_read_back_as_deployed(self):
        for change, expected in self.FRESH:
            with self.subTest(change=change):
                self.forget_deployment()
                self.queries.update(change)
                result = self.run_script("deploy")
                self.assert_fails(result, expected)
                self.assertIn("Nothing was recorded", result.stderr)
                self.assert_nothing_recorded_or_sent()

    def test_deploy_reads_back_what_deploy_hyperlane_reported_before_it_stopped(self):
        # deploy-hyperlane finished, and deploy stopped before recording what
        # it deployed: deploying again would find the IDs taken.
        report = {"success": True, "domain": DUSK_DOMAIN, "chain_id": 2, "contracts": DUSK_CONTRACTS}
        (self.out / "dusk-deploy.json").write_text(json.dumps(report))
        self.queries[f"{MAILBOX}/owner"] = {"value": ATTACKER_H256}
        self.assert_fails(self.run_script("deploy"), f"The Dusk mailbox {MAILBOX}'s owner is {ATTACKER_H256}")
        self.assert_nothing_recorded_or_sent()
        self.assertEqual(self.traced("dusk-tx deploy-hyperlane"), [])
        self.queries[f"{MAILBOX}/owner"] = {"value": DEPLOYER_H256}
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.traced("dusk-tx deploy-hyperlane"), [])
        self.assertEqual((self.state()["dusk"], self.state()["dusk_owner"]), (DUSK_CONTRACTS, DEPLOYER_H256))
        # The stub deploys at the same IDs every time: the next deployment's
        # route has no router yet.
        self.queries.pop(f"{ROUTE}/enrolled_router/{EVM_DOMAIN}")
        # A report from another chain is not read back.
        (self.out / "deployment.json").unlink()
        (self.out / "dusk-deploy.json").write_text(json.dumps({**report, "chain_id": 0}))
        self.assert_fails(self.run_script("deploy"), "records a Dusk deployment on chain 0")
        self.assertNotIn("dusk", self.state())
        # One that failed is not a deployment to read back: deploy deploys.
        (self.out / "dusk-deploy.json").write_text(json.dumps({"success": False, "error": "busy"}))
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(len(self.traced("dusk-tx deploy-hyperlane")), 1)


class TestnetRecordedDuskReadBack(DeployedTestnet):
    """Every command that uses the recorded Dusk deployment reads it back
    first: each contract's owner is the one deploy recorded, and the
    bindings nobody can change, or that only the owner can and
    deploy/testnet.sh never does, are as deploy left them."""

    COMMANDS = [["preflight"], ["deploy"], ["agent-configs"], ["dusk-igp", "--apply"], ["claim"]]

    def assert_refused(self, expected, commands=None, **settings):
        # Something to send, so that "nothing was sent" means something: the
        # read-back comes before the checks these would fail.
        pricing = self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"]["value"]
        self.queries[f"{IGP}/domain_gas_config/{EVM_DOMAIN}"] = {"value": {**pricing, "token_exchange_rate": 1}}
        self.queries[f"{IGP}/claimable_fees"] = {"value": 5}
        for command in commands or self.COMMANDS:
            with self.subTest(command=command):
                result = self.run_script(*command, **settings)
                self.assert_fails(result, expected)
                self.assertEqual(self.traced("dusk-tx set-igp-config") + self.traced("dusk-tx claim-fees")
                                 + self.traced("dusk-tx set-beneficiary") + self.traced("dusk-tx enroll-router")
                                 + self.traced("cast send") + self.traced("forge"), [])
                self.assertFalse(self.relayer_config.exists())

    def test_the_recorded_deployment_reads_back(self):
        for command in self.COMMANDS:
            with self.subTest(command=command):
                self.assert_ok(self.run_script(*command))

    def test_a_contract_that_differs_from_what_deploy_left_is_refused(self):
        # Every command refuses a Mailbox someone else owns.
        self.queries[f"{MAILBOX}/owner"] = {"value": ATTACKER_H256}
        self.assert_refused(f"The Dusk mailbox {MAILBOX}'s owner is {ATTACKER_H256}, "
                            f"not the deployer's account {DEPLOYER_H256}")
        self.queries[f"{MAILBOX}/owner"] = {"value": DEPLOYER_H256}
        # And each other difference, where DUSK_KEYS is read (preflight) and
        # where it isn't (claim).
        for change, expected in [
            ({f"{IGP}/owner": {"value": None}}, f"The Dusk igp {IGP}'s owner is nobody"),
            ({f"{MERKLE_TREE_HOOK}/mailbox": {"value": OTHER_ID}},
             f"The Dusk merkle_tree_hook {MERKLE_TREE_HOOK}'s mailbox is {OTHER_ID}, not {AGGREGATION_HOOK}"),
            ({f"{VALIDATOR_ANNOUNCE}/local_domain": {"value": 7}},
             f"The Dusk validator_announce {VALIDATOR_ANNOUNCE}'s local_domain is 7, not {DUSK_DOMAIN}"),
            ({f"{ROUTE}/hook": {"value": OTHER_ID}},
             f"The Dusk warp_native {ROUTE}'s hook is {OTHER_ID}, not {ZERO}"),
            ({f"{TEST_RECIPIENT}/interchain_security_module": {"value": TEST_MOCK}},
             f"The Dusk test_recipient {TEST_RECIPIENT}'s interchain_security_module is {TEST_MOCK}"),
            ({f"{PROTOCOL_FEE}/state_version": {"value": 2}},
             f"The Dusk protocol_fee {PROTOCOL_FEE}'s state_version is 2, not 1"),
            # The route is enrolled with another token than the recorded one.
            ({f"{ROUTE}/enrolled_router/{EVM_DOMAIN}": {"value": "00" * 12 + "22" * 20}},
             f"The Dusk route {ROUTE} is enrolled with the router {'00' * 12 + '22' * 20} for sepolia "
             f"(domain {EVM_DOMAIN}), not none or the EVM DUSK token {self.token} recorded in"),
        ]:
            with self.subTest(change=change):
                saved = {key: self.queries[key] for key in change}
                self.queries.update(change)
                try:
                    self.assert_refused(expected, commands=[["preflight"], ["claim"]])
                finally:
                    self.queries.update(saved)

    def test_a_deployment_recorded_without_its_owner_is_refused(self):
        # An older deploy/testnet.sh recorded the Dusk contracts without
        # reading them back.
        state = self.state()
        del state["dusk_owner"]
        self.write_state(state)
        self.assert_refused("records Dusk contracts but not the owner deploy read back from them")

    def test_a_deployment_owned_by_another_deployer_is_refused_where_dusk_keys_is_read(self):
        # Another deployer's deployment, consistently recorded: preflight and
        # deploy, which read DUSK_KEYS, refuse it.
        state = self.state()
        self.write_state({**state, "dusk_owner": ATTACKER_H256})
        for key in [key for key in self.queries if key.endswith("/owner")]:
            self.queries[key] = {"value": ATTACKER_H256}
        self.assert_refused(f"records the Dusk contracts as owned by the account with hash {ATTACKER_H256}, "
                            f"not DUSK_KEYS's ({DEPLOYER_H256})", commands=[["preflight"], ["deploy"]])



class TestnetDeploymentRecord(DeployedTestnet):
    """Every command but fees holds the deployment's lock while it runs, so
    commands on one deployment run one at a time, and reads the record
    once: what a command checked is what it records, and what the agent
    configs it writes name, whatever happens to the files meanwhile."""

    COMMANDS = [["preflight"], ["deploy"], ["agent-configs"], ["dusk-igp"], ["evm-igp"], ["revenue"],
                ["claim"]]

    def test_commands_on_a_deployment_run_one_at_a_time(self):
        state = self.state()
        with open(self.out / ".lock", "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            for command in self.COMMANDS:
                with self.subTest(command=command):
                    self.assert_fails(self.run_script(*command), "Another deploy/testnet.sh command is running on")
                    self.assertEqual(self.trace.read_text(), "")
                    self.assertEqual(self.state(), state)
            # fees reads no record.
            self.assert_ok(self.run_script("fees"))
        self.assert_ok(self.run_script("preflight"))

    def test_a_record_that_cant_be_written_stops_the_command(self):
        # state_set wrote the record with `printf ... > tmp && mv tmp
        # record`, and under set -e a failed write in that list didn't stop
        # the script: deploy went on from a step the record lacked, sending
        # transactions, and could report the deployment complete with
        # nothing recorded. Here the disk is full.
        if not os.path.exists("/dev/full"):
            self.skipTest("needs /dev/full")
        write_executable(self.bin / "mktemp", MKTEMP_FULL.replace("@REAL@", shutil.which("mktemp")))
        self.forget_deployment()
        self.assert_fails(self.run_script("deploy"), "so this run stops before the next step")
        self.assertEqual(self.traced("dusk-tx deploy-hyperlane") + self.traced("cast send") + self.traced("forge"),
                         [])
        self.assertFalse((self.out / "deployment.json").exists())
        self.assertEqual(list(self.out.glob(".state.*")), [])

    def test_deploy_records_the_report_it_read_back(self):
        # deploy read the contracts in deploy-hyperlane's report back, then
        # read the report again to record them: a report replaced in
        # between was recorded unchecked, with the owner the first one read
        # back, and no later read-back in that run checked it.
        self.forget_deployment()
        other = {**DUSK_CONTRACTS, "warp_native": OTHER_ID}
        self.dusk_on_read = {"key": f"{MAILBOX}/state_version", "path": str(self.out / "dusk-deploy.json"),
                             "content": json.dumps({"success": True, "domain": DUSK_DOMAIN, "chain_id": 2,
                                                    "contracts": other})}
        result = self.run_script("deploy")
        self.assertTrue(self.dusk_on_read["written"], "the report changed during the read-back")
        self.assertEqual(self.state()["dusk"], DUSK_CONTRACTS)
        self.assert_ok(result)

    def test_agent_configs_name_the_contracts_they_checked(self):
        # agent-configs checked the recorded contracts, then read the record
        # again to write the configs: a record replaced in between put
        # contracts nothing checked in the relayer's and validator's configs.
        # A first run records how far it read the EVM IGP's events, so the
        # second records nothing.
        self.assert_ok(self.run_script("agent-configs"))
        state = self.state()
        other = {**DUSK_CONTRACTS, **{name: OTHER_ID for name in
                                      ("mailbox", "igp", "validator_announce", "merkle_tree_hook")}}
        self.dusk_on_read = {"key": f"{IGP}/domain_gas_config/{EVM_DOMAIN}",
                             "path": str(self.out / "deployment.json"),
                             "content": json.dumps({**state, "dusk": other})}
        self.assert_ok(self.run_script("agent-configs"))
        self.assertTrue(self.dusk_on_read["written"], "the record changed before the configs were written")
        expected = {"mailbox": MAILBOX, "interchainGasPaymaster": IGP, "validatorAnnounce": VALIDATOR_ANNOUNCE,
                    "merkleTreeHook": MERKLE_TREE_HOOK}
        for config in ("relayer.json", "validator-dusk.json"):
            with self.subTest(config=config):
                chain = json.loads((self.out / config).read_text())["chains"]["dusk"]
                self.assertEqual({key: chain[key] for key in expected},
                                 {key: "0x" + value for key, value in expected.items()})

def stub_route_ids(nonce):
    """The token and route IDs the dusk-tx stub plans at a deploy nonce."""
    return (hashlib.sha256(f"token:{nonce}".encode()).hexdigest(),
            hashlib.sha256(f"route:{nonce}".encode()).hexdigest())


TOKEN_ROUTE_SETTINGS = {"EURd": ("Dusk Euro (testnet)", 6, "1000000000000"),
                        "USDd": ("Dusk Dollar (testnet)", 6, "1000000000000")}


class TokenRouteChecks:
    """What a token route looks like once deploy is done with it."""

    def use_token_routes(self, routes=EXAMPLE_TOKEN_ROUTES):
        self.settings["TOKEN_ROUTES"] = routes
        for name in ("canonical_drc20_roles_pausable.wasm", "hyperlane_dusk_warp_drc20_collateral.wasm"):
            (self.root / "wasm" / name).write_bytes(b"\0asm" + bytes(1000))

    def assert_token_routes(self, symbols=("EURd", "USDd"), nonces=(10, 12)):
        state = self.state()
        evm = self.evm["contracts"]
        self.assertEqual(list(state["token_routes"]), list(symbols))
        for symbol, nonce in zip(symbols, nonces):
            with self.subTest(route=symbol):
                name, decimals, supply = TOKEN_ROUTE_SETTINGS[symbol]
                record = state["token_routes"][symbol]
                token, route = stub_route_ids(nonce)
                self.assertEqual({key: record[key] for key in ("name", "symbol", "decimals", "supply")},
                                 {"name": name, "symbol": symbol, "decimals": decimals, "supply": supply})
                dusk = record["dusk"]
                self.assertEqual({key: dusk[key] for key in ("deploy_nonce", "token", "route", "read_back")},
                                 {"deploy_nonce": nonce, "token": token, "route": route, "read_back": True})
                # Each created by the transaction recorded for it.
                for what, contract_id in (("token", token), ("route", route)):
                    self.assertEqual(self.dusk_deploys["txs"][dusk[f"{what}_tx"]],
                                     {"id": contract_id, "executed": True}, what)
                # The Dusk token: the deployer its admin and holder of the supply.
                self.assertEqual({key: self.tokens[token][key] for key in ("name", "symbol", "decimals",
                                                                          "total_supply")},
                                 {"name": name, "symbol": symbol, "decimals": decimals, "total_supply": supply})
                # The EVM token: the route's metadata, no scaling, the
                # deployment's hook and ISM, created and initialized at once.
                address = record["evm"]["token"]
                contract = evm[address]
                self.assertEqual({key: contract[key] for key in ("name", "symbol", "decimals", "scale", "supply",
                                                                 "owner", "ism", "hook", "mailbox")},
                                 {"name": name, "symbol": symbol, "decimals": decimals, "scale": [1, 1],
                                  "supply": 0, "owner": DEPLOYER, "ism": state["evm"]["ism"],
                                  "hook": state["evm"]["route_hook"], "mailbox": EVM_MAILBOX})
                self.assertEqual(contract["implementation"], record["evm"]["token_implementation"])
                receipt = self.evm["receipts"][record["evm"]["token_creation_tx"]]
                self.assertEqual((receipt["contractAddress"], receipt["from"]), (address, DEPLOYER))
                # Each enrolled on the other.
                self.assertEqual(contract["routers"], {str(DUSK_DOMAIN): "0x" + route})
                self.assertEqual(self.queries[f"{route}/enrolled_router/{EVM_DOMAIN}"],
                                 {"value": "00" * 12 + address[2:]})
        self.assertEqual(state["dusk_owner_key"], DEPLOYER_KEY_HEX)

    def sends(self):
        return [line for line in self.trace.read_text().splitlines()
                if line.startswith(("cast send", "forge create")) or line.split()[:2] in (
                    ["dusk-tx", "deploy-hyperlane"], ["dusk-tx", "deploy-token-route"], ["dusk-tx", "enroll-router"],
                    ["dusk-tx", "set-beneficiary"], ["dusk-tx", "set-igp-config"])]

    def route_sends(self):
        """The sends, without deploy-token-route's plans, which send
        nothing."""
        return [line for line in self.sends() if "--plan" not in line.split()]


class TestnetTokenRoutes(TokenRouteChecks, TestnetScript):
    """deploy with TOKEN_ROUTES: each token issued on Dusk, its route and its
    synthetic token on the EVM chain, deployed, read back and enrolled, and
    resumed wherever a run stops."""

    def setUp(self):
        super().setUp()
        self.use_token_routes()

    def test_a_fresh_deploy_deploys_the_core_the_dusk_route_and_the_token_routes(self):
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assert_token_routes()
        state = self.state()
        self.assertEqual(state["dusk"], DUSK_CONTRACTS)
        self.assertIn("token", state["evm"])
        # Planned, then deployed, at nonces 10 and 12, signed for chain 2.
        deploys = self.traced("dusk-tx deploy-token-route")
        self.assertEqual(len(deploys), 4)
        for line, (symbol, nonce) in zip(deploys[1::2], (("EURd", 10), ("USDd", 12))):
            name, decimals, supply = TOKEN_ROUTE_SETTINGS[symbol]
            token, route = stub_route_ids(nonce)
            for part in (f"--token-name {name} --token-symbol {symbol} --token-decimals {decimals}",
                         f"--token-supply {supply} --deploy-nonce {nonce} --remote-domain {EVM_DOMAIN}",
                         f"--expected-token {token} --expected-route {route}", "--expected-chain-id 2",
                         f"--mailbox {MAILBOX}"):
                self.assertIn(part, line)
        # The EVM tokens are HypERC20s of the routes' decimals.
        creates = [line for line in self.traced("forge create") if "HypERC20" in line]
        self.assertEqual([line.split()[-4] for line in creates], ["9", "6", "6"])
        # Running it again sends nothing.
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.route_sends(), [])
        result = self.run_script("agent-configs")
        self.assert_ok(result)
        for symbol in ("route", "EURd route", "USDd route"):
            self.assertIn(f"The Dusk {symbol} uses the Mailbox's default ISM", result.stderr)

    def test_the_agent_configs_dont_change_with_routes(self):
        # The relayer relays any message once it is paid for, and the
        # validators sign the Mailbox's tree: neither names a route.
        self.settings["TOKEN_ROUTES"] = ""
        self.assert_ok(self.run_script("deploy"))
        self.assert_ok(self.run_script("agent-configs"))
        without = {name: (self.out / name).read_text() for name in ("relayer.json", "validator-dusk.json")}
        self.use_token_routes()
        self.assert_ok(self.run_script("deploy"))
        self.assert_ok(self.run_script("agent-configs"))
        for name, text in without.items():
            self.assertEqual((self.out / name).read_text(), text, name)
        for command in (["preflight"], ["revenue"], ["claim"], ["dusk-igp"], ["evm-igp"]):
            with self.subTest(command=command):
                self.assert_ok(self.run_script(*command))

    def honest_run(self):
        """What a deploy that nothing stops leaves: the transactions on the
        fake EVM, and the record."""
        self.assert_ok(self.run_script("deploy"))
        return evm_transactions(self.evm), self.state()

    def start_over(self):
        self.evm = sepolia()
        self.forget_deployment()

    def test_a_deploy_stopped_at_any_step_of_a_route_resumes(self):
        honest_txs, honest_state = self.honest_run()
        sends = [line for line in self.trace.read_text().splitlines() if line.startswith(("cast send", "forge create"))]
        eurd = [i for i, line in enumerate(sends) if "HypERC20" in line][1]
        token = honest_state["token_routes"]["EURd"]["evm"]["token"].lower()
        # The EURd token's implementation, its proxy, and the Dusk EURd
        # route's enrollment on it; every route's steps are the same code.
        stops = [("the EURd token's implementation", eurd), ("the EURd token's proxy", eurd + 1)]
        stops += [("the EURd token's enrollment", i) for i, line in enumerate(sends)
                  if "enrollRemoteRouter" in line and token in line.lower()]
        self.assertEqual(len(stops), 3)
        for what, index in stops:
            with self.subTest(stopped_at=what):
                self.start_over()
                self.evm.update(nonce_refusals=10**6, nonce_refusals_skip=index)
                self.assert_fails(self.run_script("deploy", EVM_SETTLE_SECONDS="2"), "nonce too low")
                self.evm.update(nonce_refusals=0, nonce_refusals_skip=0)
                self.assert_ok(self.run_script("deploy"))
                txs = evm_transactions(self.evm)
                self.assertEqual(txs, honest_txs, "each transaction once, as in a run nothing stopped")
                # The same record, but for the EVM addresses, which follow
                # the deployer's nonce, one of which went to another
                # transaction.
                routes, honest_routes = self.state()["token_routes"], json.loads(json.dumps(
                    honest_state["token_routes"]))
                for record in (routes, honest_routes):
                    for route in record.values():
                        route["evm"] = sorted(route["evm"])
                self.assertEqual(routes, honest_routes)
        # On Dusk: deploy-token-route fails once, after the plan was
        # recorded; enroll-router fails once.
        for command in ("deploy-token-route", "enroll-router"):
            with self.subTest(stopped_at=command):
                self.start_over()
                self.dusk_failures = {command: 1}
                self.assert_fails(self.run_script("deploy"), "failed")
                if command == "deploy-token-route":
                    record = self.state()["token_routes"]["EURd"]
                    self.assertEqual(record["dusk"], {"deploy_nonce": 10, "token": stub_route_ids(10)[0],
                                                      "route": stub_route_ids(10)[1]})
                self.assert_ok(self.run_script("deploy"))
                # The plan recorded before the failure is the one deployed:
                # EURd isn't planned again.
                self.assertEqual([line for line in self.traced("dusk-tx deploy-token-route")
                                  if "--plan" in line and "--token-symbol EURd" in line], [])
                self.assert_token_routes()

    def test_the_plan_skips_ids_someone_has_taken(self):
        # Someone deployed at the IDs nonce 10 gives before the route was
        # planned: the plan takes the next free nonces.
        self.tokens[stub_route_ids(10)[0]] = {"error": "someone else's"}
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("Deploy nonces 10 and 11 give contract IDs someone has taken; trying the next two",
                      result.stderr)
        self.assert_token_routes(nonces=(12, 14))
        # Nothing was sent at the taken nonces.
        self.assertEqual([line for line in self.route_sends() if "--deploy-nonce 10" in line], [])

    def test_a_contract_someone_put_at_a_planned_id_is_never_used(self):
        # The plan is recorded, then someone deploys at the token's planned
        # ID before this deployment does, a token that reads back exactly as
        # its own would, the deployer holding every role: but whoever
        # deployed it chose its init, and could hold roles too, which the
        # token can't list. No transaction of this deployment's created it,
        # so the pair is left and the next planned before anything is sent.
        self.dusk_failures = {"deploy-token-route": 1}
        self.assert_fails(self.run_script("deploy"), "failed")
        token, _ = stub_route_ids(10)
        name, decimals, supply = TOKEN_ROUTE_SETTINGS["EURd"]
        self.tokens[token] = {"name": name, "symbol": "EURd", "decimals": decimals, "total_supply": supply,
                              "cap": "0", "paused": False, "accounts": {DEPLOYER_KEY_HEX: {
                                  "balance": supply, "admin": True, "minter": True, "pauser": True}}}
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn(f"The token's planned ID {token} isn't this deployment's: a contract is at its planned ID "
                      "that no transaction of this deployment's created", result.stderr)
        self.assertIn("Leaving deploy nonces 10 and 11 for the EURd route; planning the next two", result.stderr)
        self.assertEqual(self.state()["token_routes"]["EURd"]["dusk"]["abandoned"], [10])
        self.assert_token_routes(nonces=(12, 14))
        # Nothing was sent to nonces 10 and 11.
        self.assertEqual([sent for sent in self.dusk_deploys["txs"].values() if sent["id"] in stub_route_ids(10)], [])

    def test_a_planned_id_taken_between_the_plan_and_the_send_is_left(self):
        # Someone's contract lands at the token's planned ID right before
        # this deployment's deploy to it, which fails: the pair is left, and
        # the next planned and deployed in the same run.
        token, _ = stub_route_ids(10)
        self.dusk_deploys["front_run_deploy"] = [token]
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assertIn("another contract got to its ID first", result.stderr)
        self.assertIn("Leaving deploy nonces 10 and 11 for the EURd route; planning the next two", result.stderr)
        self.assertEqual(self.state()["token_routes"]["EURd"]["dusk"]["abandoned"], [10])
        self.assert_token_routes(nonces=(12, 14))
        # The failed deploy is in the ledger; nothing else went to nonce 10.
        failed = [tx for tx, sent in self.dusk_deploys["txs"].items() if sent["id"] == token]
        self.assertEqual([self.dusk_deploys["txs"][tx]["executed"] for tx in failed], [False])

    def test_a_resumed_deploy_takes_what_its_recorded_transaction_created(self):
        # A run stops once the token is deployed: its transaction is in the
        # tx log, and so in the record. The next run takes the token, which
        # that transaction created, and deploys only the route.
        self.dusk_deploys["stop_after_token"] = True
        self.assert_fails(self.run_script("deploy"), "stopped after the token")
        token, route = stub_route_ids(10)
        record = self.state()["token_routes"]["EURd"]["dusk"]
        self.assertEqual(self.dusk_deploys["txs"][record["token_tx"]], {"id": token, "executed": True})
        self.assertNotIn("route_tx", record)
        # Had deploy itself stopped before it recorded the transaction, the
        # tx log, written before the transaction was sent, still has it.
        state = self.state()
        for key in ("token_tx", "token_tx_nonce"):
            del state["token_routes"]["EURd"]["dusk"][key]
        self.write_state(state)
        self.assert_ok(self.run_script("deploy"))
        [deploy] = [line for line in self.route_sends() if "--deploy-nonce 10" in line]
        self.assertIn(f"--token-tx {record['token_tx']}", deploy)
        self.assertNotIn("abandoned", self.state()["token_routes"]["EURd"]["dusk"])
        self.assert_token_routes()
        self.assertEqual(len([tx for tx, sent in self.dusk_deploys["txs"].items() if sent["id"] == token]), 1)

    def test_a_route_that_doesnt_read_back_is_never_recorded_or_used(self):
        attacker = "ee" * 32
        cases = [
            ({"route": {"owner": {"value": attacker}}}, f"'s owner is {attacker}, not the deployer's account"),
            ({"route": {"mailbox": {"value": OTHER_ID}}}, f"'s mailbox is {OTHER_ID}, not {MAILBOX}"),
            ({"route": {"wrapped_token": {"value": OTHER_ID}}}, f"'s wrapped_token is {OTHER_ID}"),
            ({"route": {"hook": {"value": OTHER_ID}}}, f"'s hook is {OTHER_ID}, not {ZERO}"),
            ({"route": {"interchain_security_module": {"value": OTHER_ID}}},
             f"'s interchain_security_module is {OTHER_ID}, not {ZERO}"),
            ({"route": {f"enrolled_router/{DUSK_DOMAIN}": {"value": attacker}}}, f"'s enrolled_router is {attacker}"),
            ({"route": {f"enrolled_router/{EVM_DOMAIN}": {"value": attacker}}},
             f"is enrolled with the router {attacker} for sepolia (domain {EVM_DOMAIN}), not none"),
            ({"route": {"state_version": {"value": 6}}}, "'s state_version is 6, not 7"),
            ({"token": {"name": "Dusk Euro"}}, "'s name is Dusk Euro, not Dusk Euro (testnet)"),
            ({"token": {"symbol": "EUR"}}, "'s symbol is EUR, not EURd"),
            ({"token": {"decimals": 18}}, "'s decimals is 18, not 6"),
            ({"token": {"total_supply": "2000000000000"}}, "'s supply is 2000000000000, not 1000000000000"),
            ({"token": {"account.balance": "1"}}, "'s balance of the deployer is 1, not 1000000000000"),
            ({"token": {"account.admin": False}}, "'s admin role held by the deployer is false, not true"),
            ({"token": {"account.minter": False}}, "'s minter role held by the deployer is false, not true"),
            ({"token": {"account.pauser": False}}, "'s pauser role held by the deployer is false, not true"),
            ({"token": {"cap": "5"}}, "'s cap is 5, not 0"),
            ({"token": {"paused": True}}, "'s paused is true, not false"),
        ]
        for tamper, expected in cases:
            with self.subTest(tamper=tamper):
                self.start_over()
                self.dusk_tamper = tamper
                result = self.run_script("deploy")
                self.assert_fails(result, expected)
                self.assertIn("isn't recorded as deployed", result.stderr)
                record = self.state()["token_routes"]["EURd"]
                self.assertNotIn("read_back", record["dusk"])
                self.assertNotIn("evm", record)
                self.assertEqual([line for line in self.traced("forge create") if "HypERC20" in line
                                  and " 6 1 1 " in line], [])

    def test_routers_are_never_replaced(self):
        self.assert_ok(self.run_script("deploy"))
        state = self.state()
        eurd, usdd = state["token_routes"]["EURd"], state["token_routes"]["USDd"]
        # The EURd token enrolled with the USDd route, and the EURd route
        # with the USDd token: another route's custody backs those tokens.
        saved = dict(self.evm["contracts"][eurd["evm"]["token"]]["routers"])
        self.evm["contracts"][eurd["evm"]["token"]]["routers"] = {str(DUSK_DOMAIN): "0x" + usdd["dusk"]["route"]}
        for command in ("preflight", "deploy", "agent-configs"):
            with self.subTest(command=command, side="EVM"):
                self.assert_fails(self.run_script(command),
                                  f"The EVM EURd token {eurd['evm']['token']} is enrolled with the Dusk route "
                                  f"{usdd['dusk']['route']}, not the route recorded in")
                self.assertEqual(self.route_sends(), [])
        self.evm["contracts"][eurd["evm"]["token"]]["routers"] = saved
        key = f"{eurd['dusk']['route']}/enrolled_router/{EVM_DOMAIN}"
        self.queries[key] = {"value": "00" * 12 + usdd["evm"]["token"][2:]}
        for command in ("preflight", "deploy", "claim"):
            with self.subTest(command=command, side="Dusk"):
                self.assert_fails(self.run_script(command),
                                  f"The Dusk EURd route {eurd['dusk']['route']} is enrolled with the router "
                                  f"{'00' * 12 + usdd['evm']['token'][2:]} for sepolia (domain {EVM_DOMAIN}), not none or "
                                  f"the EVM EURd token {eurd['evm']['token']}")
                self.assertEqual(self.route_sends(), [])
        # A router that appears after the read-back, before the write, as
        # the owner could enroll it from elsewhere, is refused too.
        self.start_over()
        self.assert_ok(self.run_script("deploy"))
        state = self.state()
        route = state["token_routes"]["USDd"]["dusk"]["route"]
        key = f"{route}/enrolled_router/{EVM_DOMAIN}"
        self.queries.pop(key)
        self.evm["contracts"][state["token_routes"]["USDd"]["evm"]["token"]]["routers"] = {}
        self.dusk_interleave = {"key": key, "after_reads": 1, "value": "00" * 12 + "22" * 20}
        result = self.run_script("deploy")
        self.assert_fails(result, f"The Dusk USDd route {route} is enrolled with the router {'00' * 12 + '22' * 20}")
        self.assertIn("not replacing it", result.stderr)
        self.assertEqual(self.traced("dusk-tx enroll-router"), [])

    def test_the_evm_tokens_are_read_back(self):
        self.assert_ok(self.run_script("deploy"))
        record = self.state()["token_routes"]["EURd"]
        address = record["evm"]["token"]
        token = self.evm["contracts"][address]
        saved = json.loads(json.dumps(token))
        cases = [
            ({"decimals": 18}, f"The EVM EURd token {address}'s decimals is '18', not '6'"),
            ({"scale": [10**12, 1]}, f"The EVM EURd token {address}'s scaleNumerator is '1000000000000', not '1'"),
            ({"name": "Euro"}, f"The EVM EURd token {address}'s name is 'Euro', not 'Dusk Euro (testnet)'"),
            ({"symbol": "EURx"}, f"The EVM EURd token {address}'s symbol is 'EURx', not 'EURd'"),
            ({"mailbox": "0x" + "bb" * 20}, f"through the Mailbox 0x{'bb' * 20}, not EVM_MAILBOX"),
            ({"ism": "0x" + "56" * 20}, f"verifies messages from Dusk with the ISM 0x{'56' * 20}"),
            ({"routers": {}, "supply": 5}, f"The EVM EURd token {address} has a supply of 5 but no Dusk route"),
            ({"admin": ATTACKER}, f"The EVM EURd token {address} has the proxy admin {ATTACKER}"),
            ({"implementation": "0x" + "4b" * 20}, f"runs the implementation 0x{'4b' * 20}"),
            ({"hook": "0x" + "00" * 20}, f"The EVM EURd route {address} dispatches through hook"),
        ]
        for i, (change, expected) in enumerate(cases):
            with self.subTest(change=change):
                self.evm["contracts"][address] = {**saved, **change}
                # Every command reads the tokens back the same way: each
                # change is tried on preflight, and the first on them all.
                # preflight only warns of another hook, as deploy sets it,
                # and evm-igp doesn't use it; agent-configs refuses it.
                commands = ["preflight", "agent-configs", "evm-igp"] if i == 0 else ["preflight"]
                if change.get("hook"):
                    commands = ["agent-configs"]
                    self.assert_ok(self.run_script("preflight"))
                for command in commands:
                    self.assert_fails(self.run_script(command), expected)
                    self.assertEqual(self.route_sends(), [])
        self.evm["contracts"][address] = saved
        # A token created by someone else, or initialized after its creation.
        tx = record["evm"]["token_creation_tx"]
        receipt = self.evm["receipts"][tx]
        self.evm["receipts"][tx] = {**receipt, "from": ATTACKER}
        self.assert_fails(self.run_script("preflight"),
                          f"The EVM EURd token {address} wasn't initialized in the transaction that created it")

    def test_recorded_routes_are_read_back_by_every_command(self):
        self.assert_ok(self.run_script("deploy"))
        record = self.state()["token_routes"]["USDd"]
        token, route = record["dusk"]["token"], record["dusk"]["route"]
        attacker = "ee" * 32
        for change, expected in [
            (lambda: self.queries.update({f"{route}/owner": {"value": attacker}}),
             f"The Dusk USDd route {route}'s owner is {attacker}"),
            (lambda: self.queries.update({f"{route}/mailbox": {"value": OTHER_ID}}),
             f"The Dusk USDd route {route}'s mailbox is {OTHER_ID}"),
            (lambda: self.queries.update({f"{route}/hook": {"value": OTHER_ID}}),
             f"The Dusk USDd route {route}'s hook is {OTHER_ID}"),
            (lambda: self.queries.update({f"{route}/state_version": {"value": 6}}),
             "records a Dusk route (warp_drc20_collateral, the USDd route)"),
            (lambda: self.tokens[token].update(name="Dusk Dollar"),
             f"The Dusk USDd token {token}'s name is Dusk Dollar, not Dusk Dollar (testnet)"),
            (lambda: self.tokens[token]["accounts"][DEPLOYER_KEY_HEX].update(admin=False),
             f"The Dusk USDd token {token}'s admin role held by the deployer is false"),
            (lambda: self.queries.update({f"{route}/interchain_security_module": {"value": TEST_MOCK}}),
             "The Dusk USDd route uses its own ISM"),
        ]:
            with self.subTest(expected=expected):
                saved_queries, saved_tokens = json.loads(json.dumps(self.queries)), json.loads(json.dumps(self.tokens))
                change()
                # Every command reads the routes back the same way: each
                # change is tried on preflight, and the first on them all.
                # The ISM a route uses is checked where the relayer's limit
                # is, as the DUSK route's is.
                commands = [["preflight"]]
                if "owner is" in expected:
                    commands += [["claim"], ["agent-configs"], ["dusk-igp", "--apply"]]
                elif "ISM" in expected:
                    commands += [["agent-configs"]]
                for command in commands:
                    result = self.run_script(*command)
                    self.assert_fails(result, expected)
                    self.assertEqual(self.route_sends() + self.traced("dusk-tx claim-fees"), [])
                    self.assertFalse(self.relayer_config.exists())
                self.queries, self.tokens = saved_queries, saved_tokens
        # What the owner may change on purpose isn't refused: the supply it
        # mints, and the minter and pauser roles.
        self.tokens[token]["total_supply"] = "5000000000000"
        self.tokens[token]["accounts"][DEPLOYER_KEY_HEX].update(minter=False, balance="7")
        self.assert_ok(self.run_script("preflight"))

    def test_settings_must_be_well_formed(self):
        for routes, expected in [
            ("EURd|Dusk Euro|6", "isn't SYMBOL|NAME|DECIMALS|SUPPLY"),
            ("EURd|Dusk Euro|6|1|extra", "isn't SYMBOL|NAME|DECIMALS|SUPPLY"),
            ("1EUR|Dusk Euro|6|1", "the symbol '1EUR' must be a letter"),
            ("EUR-d|Dusk Euro|6|1", "the symbol 'EUR-d' must be a letter"),
            ("EURdEURdEURd|Dusk Euro|6|1", "the symbol 'EURdEURdEURd' must be a letter"),
            ("dusk|Dusk|9|1", "DUSK is the native route's token"),
            ("EURd||6|1", "the name '' must be 1 to 64"),
            ("EURd|Dusk Euro|19|1", "the decimals '19' must be a whole number from 0 to 18"),
            ("EURd|Dusk Euro|six|1", "the decimals 'six'"),
            ("EURd|Dusk Euro|6|-1", "the supply '-1' must be a whole number"),
            ("EURd|Dusk Euro|6|1e12", "the supply '1e12'"),
            ("EURd|Dusk Euro|6|18446744073709551616", "at most 18446744073709551615"),
            ("EURd|Dusk Euro|6|1\nEURD|Dusk Euro 2|6|1", "lists the symbol EURD twice"),
        ]:
            with self.subTest(routes=routes):
                for command in ("fees", "preflight", "deploy"):
                    result = self.run_script(command, TOKEN_ROUTES=routes)
                    self.assert_fails(result, expected)
                    self.assertEqual(self.route_sends(), [])
        # Blank lines, comments and the largest supply are fine.
        self.assert_ok(self.run_script("fees", TOKEN_ROUTES="\n# routes\n  EURd | Dusk Euro | 6 | 18446744073709551615  \n"))

    def test_a_recorded_routes_settings_cant_change(self):
        self.assert_ok(self.run_script("deploy"))
        for routes, what in [
            ("EURd|Dusk Euro|6|1000000000000\nUSDd|Dusk Dollar (testnet)|6|1000000000000", "Dusk Euro|6|1000000000000"),
            ("EURd|Dusk Euro (testnet)|2|1000000000000", "Dusk Euro (testnet)|2|1000000000000"),
            ("EURd|Dusk Euro (testnet)|6|5", "Dusk Euro (testnet)|6|5"),
        ]:
            with self.subTest(routes=routes):
                for command in ("preflight", "deploy", "claim"):
                    self.assert_fails(self.run_script(command, TOKEN_ROUTES=routes),
                                      f"TOKEN_ROUTES has EURd as {what}, but")
                    self.assertEqual(self.route_sends(), [])
        # Nor its symbol's case: EURD would be another route, a token
        # nobody could tell from EURd.
        for command in ("preflight", "deploy"):
            self.assert_fails(self.run_script(command, TOKEN_ROUTES="EURD|Dusk Euro (testnet)|6|1000000000000"),
                              "TOKEN_ROUTES lists EURD, and")
            self.assertEqual(self.route_sends(), [])
        # A recorded route TOKEN_ROUTES no longer lists is still checked.
        result = self.run_script("preflight", TOKEN_ROUTES="")
        self.assert_ok(result)
        self.assertIn("has the token route EURd, which TOKEN_ROUTES doesn't list", result.stderr)

    def test_deliveries_to_dusk_are_priced_for_the_token_routes(self):
        # A token route's payout costs more than the native route's, so the
        # EVM IGP's overhead must cover it once the deployment has one.
        self.assert_fails(self.run_script("fees", EVM_IGP_GAS_OVERHEAD=2170000),
                          "takes about 2,200,000 Dusk gas (1,980,000 plus 220,000 per signature, for a payout from "
                          "a token route")
        self.assert_ok(self.run_script("fees", EVM_IGP_GAS_OVERHEAD=2170000, TOKEN_ROUTES=""))
        self.assert_ok(self.run_script("fees"))
        # The relayer's gas limit too: 1.5 times 1.98M + 0.22M at threshold 7
        # is 5.28M.
        seven = ",".join(validators(7))
        self.assert_fails(self.run_script("preflight", EVM_ORIGIN_VALIDATORS=seven, EVM_ORIGIN_THRESHOLD=7,
                                          EVM_IGP_GAS_OVERHEAD=3520000),
                          "DUSK_RELAYER_GAS_LIMIT is 5000000; with EVM_ORIGIN_THRESHOLD of 7 it must be at least 5280000")

    def test_the_deployer_needs_dusk_for_the_routes(self):
        self.settings["TOKEN_ROUTES"] = ""
        self.assert_ok(self.run_script("deploy"))
        self.use_token_routes()
        # The pinned token's and the route's Wasms, at the sizes make builds:
        # 438,136 bytes a route at 100 gas a byte, and 2M gas for each
        # contract's init, 95.6 DUSK at 2,000 LUX; and a deploy's gas limit
        # times its price up front, 100 DUSK.
        for name, size in (("canonical_drc20_roles_pausable.wasm", 261_980),
                           ("hyperlane_dusk_warp_drc20_collateral.wasm", 176_156)):
            (self.root / "wasm" / name).write_bytes(b"\0asm" + bytes(size - 4))
        self.dusk_balance = 291 * 10**9
        self.assert_fails(self.run_script("preflight"), "needs at least 291 for the 2 token routes to deploy")
        self.dusk_balance = 292 * 10**9
        self.assert_ok(self.run_script("preflight"))
        (self.root / "wasm" / "canonical_drc20_roles_pausable.wasm").unlink()
        self.dusk_balance = 1000 * 10**9
        self.assert_fails(self.run_script("preflight"), "canonical_drc20_roles_pausable.wasm isn't built")

    def test_deploy_leaves_a_beneficiary_already_set_alone(self):
        # So deploy, run again to add routes, reconfigures nothing: here
        # without routes, which it doesn't depend on.
        self.settings["TOKEN_ROUTES"] = ""
        self.assert_ok(self.run_script("deploy", DUSK_FEE_BENEFICIARY="fixture-treasury"))
        self.assertEqual(len(self.traced("dusk-tx set-beneficiary")), 2)
        self.assert_ok(self.run_script("deploy", DUSK_FEE_BENEFICIARY="fixture-treasury"))
        self.assertEqual(self.traced("dusk-tx set-beneficiary"), [])


class TestnetTokenRoutesOnADeployment(TokenRouteChecks, DeployedTestnet):
    """The recorded deployment has the core and the DUSK route only, as the
    live one does: deploy with TOKEN_ROUTES adds the token routes and leaves
    everything else as it is."""

    def setUp(self):
        super().setUp()
        # As deploy leaves it, the route's hook is the one the factory gives
        # for the Merkle hook and the IGP.
        children = f"[{EVM_MERKLE},{self.evm_igp}]"
        hook = "0x" + hashlib.sha256(f"aggregation:{EVM_HOOK_FACTORY}:{children}".encode()).hexdigest()[:40]
        contracts = self.evm["contracts"]
        contracts[hook] = contracts.pop(self.hook)
        contracts[self.token]["hook"] = hook
        self.hook = hook
        state = self.state()
        state["evm"]["route_hook"] = hook
        self.write_state(state)

    def test_routes_are_added_without_touching_the_rest(self):
        before = self.state()
        evm_before = json.loads(json.dumps(self.evm["contracts"]))
        self.use_token_routes()
        result = self.run_script("deploy")
        self.assert_ok(result)
        self.assert_token_routes()
        after = self.state()
        for key in ("dusk", "dusk_owner", "dusk_domain", "evm_domain"):
            self.assertEqual(after[key], before[key], key)
        for key, value in before["evm"].items():
            if key != "igp_quote_signers_scanned":
                self.assertEqual(str(after["evm"][key]).lower(), str(value).lower(), key)
        # Nothing existing was redeployed or reconfigured: no core deploy, no
        # write to the IGP, its oracle, the hook or the DUSK token, and on
        # Dusk only the new routes' deploys and enrollments.
        self.assertEqual(self.traced("dusk-tx deploy-hyperlane"), [])
        new_tokens = {after["token_routes"][symbol]["evm"]["token"] for symbol in ("EURd", "USDd")}
        for line in self.traced("cast send"):
            target = line.split()[2]
            self.assertTrue(target == "--create" or target.lower() in new_tokens, line)
        self.assertEqual([line for line in self.traced("forge create") if "HypERC20" not in line], [])
        for address in (self.evm_igp, self.oracle, self.hook, self.token, self.ism):
            self.assertEqual(self.evm["contracts"][address], evm_before[address], address)
        new_routes = {after["token_routes"][symbol]["dusk"]["route"] for symbol in ("EURd", "USDd")}
        for line in self.traced("dusk-tx enroll-router"):
            self.assertIn(line.split("--warp-contract ")[1].split()[0], new_routes)
        self.assertEqual(self.traced("dusk-tx set-igp-config") + self.traced("dusk-tx set-beneficiary"), [])
        # The record gets the deployer's key, which the routes' tokens name.
        self.assertEqual(after["dusk_owner_key"], DEPLOYER_KEY_HEX)
        self.assert_ok(self.run_script("agent-configs"))
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.route_sends(), [])

    def test_a_deployment_without_token_routes_is_left_alone(self):
        self.assert_ok(self.run_script("deploy"))
        self.assertEqual(self.route_sends(), [])
        self.assertNotIn("token_routes", self.state())


class StateVersions(unittest.TestCase):
    """The state versions the tooling expects are the contracts' own."""

    CRATES = {"mailbox": "mailbox", "merkle_tree_hook": "merkle-tree-hook", "igp": "igp",
              "validator_announce": "validator-announce", "warp_native": "warp-native",
              "warp_drc20": "warp-drc20", "warp_drc20_collateral": "warp-drc20-collateral"}

    def contract_version(self, crate):
        source = (ROOT / "contracts" / crate / "src" / "lib.rs").read_text()
        [version] = re.findall(r"pub fn state_version\(&self\) -> u32 \{\s*(\d+)\s*\}", source)
        return int(version)

    def test_the_tooling_expects_the_contracts_versions(self):
        script = TESTNET_SH.read_text()
        expected = {}
        for name in ("CORE_STATE_VERSIONS", "ROUTE_STATE_VERSIONS", "OTHER_STATE_VERSIONS"):
            [entries] = re.findall(rf'^{name}="([^"]*)"', script, re.M)
            expected.update(entry.split("=") for entry in entries.split())
        crates = {**self.CRATES, "aggregation_hook": "aggregation-hook", "ism_multisig": "ism-multisig",
                  "protocol_fee": "protocol-fee", "test_mock": "test-mock", "test_recipient": "test-recipient"}
        self.assertEqual(sorted(expected), sorted(crates))
        for key, crate in crates.items():
            self.assertEqual(int(expected[key]), self.contract_version(crate), key)
        # deploy-hyperlane reads every contract it deploys back at these.
        deployment = (ROOT / "dusk-tx" / "src" / "deployment.rs").read_text()
        [table] = re.findall(r"pub const STATE_VERSIONS: \[\(&str, u32\); \d+\] = \[(.*?)\];", deployment, re.S)
        read_back = dict(re.findall(r'\("([a-z0-9-]+)", (\d+)\)', table))
        self.assertEqual(sorted(read_back), sorted(crates.values()))
        for crate, version in read_back.items():
            self.assertEqual(int(version), self.contract_version(crate), crate)
        # transfer-remote --native tells a native route by its version, which
        # no DRC20 route of this checkout reports.
        dusk_tx = (ROOT / "dusk-tx" / "src" / "main.rs").read_text()
        [native] = re.findall(r"const WARP_NATIVE_STATE_VERSION: u32 = (\d+);", dusk_tx)
        self.assertEqual(int(native), self.contract_version("warp-native"))
        self.assertNotIn(int(native), [self.contract_version("warp-drc20"),
                                       self.contract_version("warp-drc20-collateral")])


SOLIDITY_DIR = Path(os.environ.get("HYPERLANE_SOLIDITY_DIR", ROOT.parent / "hyperlane-monorepo" / "solidity"))
# Multicall for Dusk's v0.1.0 Wasm, if one is at hand: the Anvil tests then
# have deploy/local-evm-core.sh deploy it to the stubbed Dusk chain.
MULTICALL_WASM = os.environ.get("HYPERLANE_MULTICALL_WASM", "")
MULTICALL_OWNER = ("r2h82mj2ehNhYPphK3GHVpG98MhSsZNCAF9sdSS4nwQJKqVBwihZn7bZXb9VoWVSqtu6onZ8Tkm11zdnavdCfuCD1kS2We4VPxvLeDf73a1sGiG25"
                   "HUFvCwQJm3Ud8q6KLW")
# Anvil's development keys and accounts: #0 deploys, #2 stands in for
# Hyperlane's Sepolia validator, #9 attacks.
ANVIL_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ANVIL_DEPLOYER = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
ANVIL_VALIDATOR = "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc"
ANVIL_ATTACKER_KEY = "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6"
ANVIL_ATTACKER = "0xa0ee7a142d267c1f36714e4a8f75612f20a79720"

# Runs a real tool after logging the call to FIXTURE_TRACE.
LOGGING_WRAPPER = """#!/bin/sh
printf '%s\\n' "TOOL $*" >> "$FIXTURE_TRACE"
exec REAL "$@"
"""


def anvil_unavailable():
    """Why the Anvil tests can't run here, or None."""
    for tool in ("anvil", "forge", "cast"):
        if shutil.which(tool) is None:
            return f"{tool} is not installed"
    if not (SOLIDITY_DIR / "foundry.toml").exists():
        return f"no Hyperlane Solidity sources at {SOLIDITY_DIR} (HYPERLANE_SOLIDITY_DIR)"
    return None


class AnvilEvmDeploy(unittest.TestCase):
    """deploy/testnet.sh's EVM side on Anvil, with Hyperlane's contracts and
    real cast and forge; the Dusk side is the dusk-tx stub. Shows on chain
    that the IGP and the token are initialized in the transaction that
    creates them, so there is nothing to front-run, and that an IGP deployed
    the old way, which an attacker initialized first, is refused. Skipped
    without Anvil, Foundry and the Hyperlane Solidity sources."""

    @classmethod
    def setUpClass(cls):
        reason = anvil_unavailable()
        if reason:
            raise unittest.SkipTest(reason)
        cls.temp = tempfile.TemporaryDirectory(prefix="hyperlane-anvil-checks-")
        cls.root = Path(cls.temp.name)
        try:
            cls.start_anvil()
            cls.prepare_checkout()
            cls.deploy_core_and_route()
        except BaseException:
            cls.tearDownClass()
            raise

    @classmethod
    def tearDownClass(cls):
        anvil = getattr(cls, "anvil", None)
        if anvil is not None and anvil.poll() is None:
            anvil.terminate()
            try:
                anvil.wait(timeout=10)
            except subprocess.TimeoutExpired:
                anvil.kill()
                anvil.wait()
        cls.temp.cleanup()

    @classmethod
    def start_anvil(cls):
        import socket
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            cls.port = probe.getsockname()[1]
        cls.rpc = f"http://127.0.0.1:{cls.port}"
        cls.anvil = subprocess.Popen(["anvil", "--port", str(cls.port), "--chain-id", "31338", "--silent"],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            if subprocess.run(["cast", "block-number", "--rpc-url", cls.rpc],
                              capture_output=True).returncode == 0:
                return
            if cls.anvil.poll() is not None:
                raise RuntimeError("anvil exited at startup")
            import time
            time.sleep(0.1)
        raise RuntimeError("anvil did not start")

    @classmethod
    def prepare_checkout(cls):
        (cls.root / "deploy").mkdir()
        (cls.root / "demo").mkdir()
        shutil.copy2(TESTNET_SH, cls.root / "deploy" / "testnet.sh")
        for name in ("testnet.env.example", "local-evm-core.sh"):
            shutil.copy2(ROOT / "deploy" / name, cls.root / "deploy" / name)
        cls.bin = cls.root / "bin"
        cls.bin.mkdir()
        write_executable(cls.bin / "dusk-tx", DUSK_TX_STUB)
        write_executable(cls.bin / "curl", CURL_STUB)
        for tool in ("cast", "forge"):
            write_executable(cls.bin / tool, LOGGING_WRAPPER.replace("TOOL", tool)
                             .replace("REAL", shlex.quote(shutil.which(tool))))
        wasm = cls.root / "wasm"
        wasm.mkdir()
        for name in ("hyperlane_dusk_mailbox.wasm", "canonical_drc20_roles_pausable.wasm",
                     "hyperlane_dusk_warp_drc20_collateral.wasm"):
            (wasm / name).write_bytes(b"\0asm")
        keys = cls.root / "consensus.keys"
        keys.write_text("{}")
        (cls.root / "demo" / ".env.bridge").write_text("".join(f"{k}={shlex.quote(str(v))}\n" for k, v in {
            "ANVIL_RPC": cls.rpc, "DUSK_RUES_URL": "http://localhost:8080/", "SOLIDITY_DIR": SOLIDITY_DIR,
            "ANVIL_PRIVATE_KEY": ANVIL_KEY, "ANVIL_DEPLOYER": ANVIL_DEPLOYER,
            "CONSENSUS_KEYS": keys, "CONSENSUS_PASSWORD": "fixture", "DUSK_TX": cls.bin / "dusk-tx",
            # The stubbed Dusk chain's, not the local devnet's 0.
            "DUSK_EXPECTED_CHAIN_ID": 2,
            "DUSK_MULTICALL_WASM": MULTICALL_WASM}.items()))
        cls.chain = cls.root / "chain.json"
        cls.trace = cls.root / "tools.trace"
        queries = dusk_queries()
        queries[f"{MULTISIG}/validators_and_threshold"] = {"value": {"validators": [ANVIL_VALIDATOR],
                                                                     "threshold": 1}}
        cls.chain.write_text(json.dumps({"queries": queries, "deployer_h256": DEPLOYER_H256,
                                         "deployer_key": DEPLOYER_KEY_HEX, "deploy": DUSK_CONTRACTS}))
        cls.trace.write_text("")

    @classmethod
    def run_tool(cls, *command, deploy_name=None, check=False):
        env = {
            "PATH": f"{cls.bin}{os.pathsep}{os.environ['PATH']}",
            "HOME": os.environ.get("HOME", str(cls.root)),
            "DUSK_TX": str(cls.bin / "dusk-tx"),
            "WASM_DIR": str(cls.root / "wasm"),
            "TESTNET_ENV": str(cls.root / "deploy" / "local.env"),
            "FIXTURE_CHAIN": str(cls.chain),
            "FIXTURE_TRACE": str(cls.trace),
        }
        if deploy_name:
            env["DEPLOY_NAME_OVERRIDE"] = deploy_name
        result = subprocess.run(["bash", *command], cwd=cls.root, env=env, capture_output=True, text=True,
                                timeout=600)
        if check and result.returncode != 0:
            raise AssertionError(f"{command} failed:\n{result.stdout}{result.stderr}")
        return result

    @classmethod
    def deploy_core_and_route(cls):
        cls.core_output = cls.run_tool(str(cls.root / "deploy" / "local-evm-core.sh"), check=True)
        cls.core_trace = cls.trace.read_text()
        env_file = cls.root / "deploy" / "local.env"
        # Let a test deploy under another name, with the same chains.
        env_file.write_text(env_file.read_text()
                            + 'DEPLOY_NAME="${DEPLOY_NAME_OVERRIDE:-local-rehearsal}"\n')
        cls.trace.write_text("")
        cls.deploy_output = cls.run_tool(str(cls.root / "deploy" / "testnet.sh"), "deploy", check=True)
        cls.state = json.loads((cls.root / "deploy" / "out" / "local-rehearsal" / "deployment.json").read_text())
        cls.deploy_trace = cls.trace.read_text()

    def cast(self, *args):
        result = subprocess.run(["cast", *args, "--rpc-url", self.rpc], capture_output=True, text=True, timeout=60)
        return result

    def cast_ok(self, *args):
        result = self.cast(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def test_the_igp_and_the_token_are_initialized_in_their_creation_transactions(self):
        evm = self.state["evm"]
        expected = {
            "igp": [UPGRADED, "0x8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e0",
                    "0x8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e0",
                    "0x04d55a8be181fb8d75b76f2d48aa0b2ee40f47e53d6e61763eeeec46feea8a24", INITIALIZED],
        }
        for key in ("igp", "token"):
            with self.subTest(contract=key):
                address = evm[key].lower()
                receipt = json.loads(self.cast_ok("receipt", evm[f"{key}_creation_tx"], "--json"))
                # One transaction, sent by the deployer, created the proxy...
                self.assertEqual(receipt["status"], "0x1")
                self.assertEqual(receipt["from"].lower(), ANVIL_DEPLOYER)
                self.assertIsNone(receipt["to"])
                self.assertEqual(receipt["contractAddress"].lower(), address)
                # ...and, in its constructor, initialized it.
                topics = [entry["topics"][0] for entry in receipt["logs"] if entry["address"].lower() == address]
                self.assertEqual(topics[0], UPGRADED)
                self.assertEqual(topics[-1], INITIALIZED)
                if key in expected:
                    self.assertEqual(topics, expected[key])
                slot = self.cast_ok("storage", evm[key],
                                    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc")
                self.assertEqual("0x" + slot[-40:], evm[f"{key}_implementation"].lower())
                # No admin: nobody can swap the implementation.
                self.assertEqual(int(self.cast_ok("storage", evm[key],
                                                  "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103"),
                                     16), 0)
                self.assertEqual(self.cast_ok("call", evm[key], "owner()(address)").lower(), ANVIL_DEPLOYER)
        self.assertEqual(self.cast_ok("call", evm["igp"], "beneficiary()(address)").lower(), ANVIL_DEPLOYER)
        self.assertEqual(self.cast_ok("call", evm["token"], "interchainSecurityModule()(address)").lower(),
                         evm["ism"].lower())
        self.assertEqual(self.cast_ok("call", evm["token"], "totalSupply()(uint256)"), "0")
        # deploy never sent initialize in a transaction of its own.
        self.assertNotRegex(self.deploy_trace, r"cast send [^\n]*initialize")

    def test_the_token_routes_tokens_are_created_as_the_dusk_token_is(self):
        # deploy/testnet.env.example's EURd and USDd: each a HypERC20 of 6
        # decimals and no scaling, behind an ERC1967Proxy that initialized it
        # in the transaction that created it, with the route's hook and the
        # EVM ISM, no supply, and its Dusk route enrolled.
        evm = self.state["evm"]
        routes = self.state["token_routes"]
        self.assertEqual(list(routes), ["EURd", "USDd"])
        for symbol, name in (("EURd", "Dusk Euro (testnet)"), ("USDd", "Dusk Dollar (testnet)")):
            with self.subTest(route=symbol):
                record = routes[symbol]["evm"]
                token = record["token"]
                receipt = json.loads(self.cast_ok("receipt", record["token_creation_tx"], "--json"))
                self.assertEqual((receipt["status"], receipt["from"].lower(), receipt["contractAddress"].lower()),
                                 ("0x1", ANVIL_DEPLOYER, token.lower()))
                topics = [entry["topics"][0] for entry in receipt["logs"]
                          if entry["address"].lower() == token.lower()]
                self.assertEqual((topics[0], topics[-1]), (UPGRADED, INITIALIZED))
                slot = self.cast_ok("storage", token,
                                    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc")
                self.assertEqual("0x" + slot[-40:], record["token_implementation"].lower())
                self.assertEqual(int(self.cast_ok("storage", token,
                                                  "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103"),
                                     16), 0)
                for signature, expected in (("name()(string)", f'"{name}"'), ("symbol()(string)", f'"{symbol}"'),
                                            ("decimals()(uint8)", "6"), ("scaleNumerator()(uint256)", "1"),
                                            ("scaleDenominator()(uint256)", "1"), ("totalSupply()(uint256)", "0")):
                    self.assertEqual(self.cast_ok("call", token, signature), expected, signature)
                for signature, expected in (("owner()(address)", ANVIL_DEPLOYER), ("hook()(address)", evm["route_hook"]),
                                            ("interchainSecurityModule()(address)", evm["ism"])):
                    self.assertEqual(self.cast_ok("call", token, signature).lower(), expected.lower(), signature)
                self.assertEqual(self.cast_ok("call", token, "routers(uint32)(bytes32)", "1146442572"),
                                 "0x" + routes[symbol]["dusk"]["route"])
                # Nobody can initialize it again.
                result = self.cast("send", token, "initialize(uint256,string,string,address,address,address)",
                                   "1000000", name, symbol, "0x" + "00" * 20, ANVIL_ATTACKER, ANVIL_ATTACKER,
                                   "--private-key", ANVIL_ATTACKER_KEY)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Initializable: contract is already initialized", result.stderr)
        # A transfer to Dusk of either token pays the same as one of DUSK.
        quotes = {self.cast_ok("call", record["evm"]["token"], "quoteGasPayment(uint32)(uint256)", "1146442572")
                  for record in routes.values()}
        quotes.add(self.cast_ok("call", evm["token"], "quoteGasPayment(uint32)(uint256)", "1146442572"))
        self.assertEqual(len(quotes), 1, quotes)

    def test_the_other_evm_contracts_have_nothing_to_initialize(self):
        evm = self.state["evm"]
        settings = dict(line.split("=", 1) for line in (self.root / "deploy" / "local.env").read_text().splitlines()
                        if "=" in line and not line.startswith(("#", "export", "source")))
        attack = ("--private-key", ANVIL_ATTACKER_KEY)
        # The StorageGasOracle is Ownable: its constructor made the deployer
        # its owner, and it has no initialize.
        self.assertEqual(self.cast_ok("call", evm["storage_gas_oracle"], "owner()(address)").lower(), ANVIL_DEPLOYER)
        self.assertNotEqual(self.cast("send", evm["storage_gas_oracle"], "initialize(address)", ANVIL_ATTACKER,
                                      *attack).returncode, 0)
        # The ISM, deployed from source, took its validators in its
        # constructor, which disabled initialize; nobody owns it.
        result = self.cast("send", evm["ism"], "initialize(address,address[],uint8)", ANVIL_ATTACKER,
                           f"[{ANVIL_ATTACKER}]", "1", *attack)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Initializable: contract is already initialized", result.stderr)
        self.assertEqual(int(self.cast_ok("call", evm["ism"], "owner()(address)"), 16), 0)
        # The route's hook is the factory's CREATE2 address for exactly these
        # hooks, which are part of its code; it has no initialize either.
        children = f"[{evm['merkle_tree_hook']},{evm['igp']}]"
        self.assertEqual(self.cast_ok("call", settings["EVM_AGGREGATION_HOOK_FACTORY"], "getAddress(address[])(address)",
                                      children).lower(), evm["route_hook"].lower())
        self.assertEqual(self.cast_ok("call", evm["route_hook"], "hooks(bytes)(address[])", "0x").lower(),
                         children.replace(",", ", ").lower())
        self.assertNotEqual(self.cast("send", evm["route_hook"], "initialize(address)", ANVIL_ATTACKER,
                                      *attack).returncode, 0)

    def test_initializing_them_again_reverts(self):
        evm = self.state["evm"]
        attempts = {
            "igp": ("initialize(address,address)", ANVIL_ATTACKER, ANVIL_ATTACKER),
            "token": ("initialize(uint256,string,string,address,address,address)", "1000000000000", "Dusk",
                      "DUSK", "0x" + "00" * 20, ANVIL_ATTACKER, ANVIL_ATTACKER),
        }
        for key, call in attempts.items():
            with self.subTest(contract=key):
                result = self.cast("send", evm[key], *call, "--private-key", ANVIL_ATTACKER_KEY)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Initializable: contract is already initialized", result.stderr)

    def test_an_igp_initialized_by_someone_else_is_refused(self):
        # The old deployment: a bare IGP, which the attacker initializes
        # before the deployer can, keeping the deployer as owner so that the
        # old owner check passed, and making itself the beneficiary.
        created = json.loads(subprocess.run(
            ["forge", "create", "contracts/hooks/igp/InterchainGasPaymaster.sol:InterchainGasPaymaster",
             "--broadcast", "--json", "--rpc-url", self.rpc, "--private-key", ANVIL_KEY],
            cwd=SOLIDITY_DIR, capture_output=True, text=True, check=True).stdout)
        bare = created["deployedTo"]
        self.cast_ok("send", bare, "initialize(address,address)", ANVIL_DEPLOYER, ANVIL_ATTACKER,
                     "--private-key", ANVIL_ATTACKER_KEY)
        self.assertEqual(self.cast_ok("call", bare, "owner()(address)").lower(), ANVIL_DEPLOYER)
        self.assertEqual(self.cast_ok("call", bare, "beneficiary()(address)").lower(), ANVIL_ATTACKER)
        cases = {
            # As an older deploy/testnet.sh recorded it.
            "without a creation record": ({"igp": bare},
                                          "without the transaction that created and initialized it"),
            # Its creation, which has no initialization in it.
            "with its bare creation": ({"igp": bare, "igp_creation_tx": created["transactionHash"],
                                        "igp_implementation": bare},
                                       "wasn't initialized in the transaction that created it"),
        }
        for name, (recorded, expected) in cases.items():
            with self.subTest(case=name):
                state = json.loads(json.dumps(self.state))
                for key in ("igp_creation_tx", "igp_implementation"):
                    state["evm"].pop(key, None)
                state["evm"].update(recorded)
                out = self.root / "deploy" / "out" / "front-run"
                out.mkdir(parents=True, exist_ok=True)
                (out / "deployment.json").write_text(json.dumps(state))
                for command in ("preflight", "agent-configs", "evm-igp", "claim"):
                    result = self.run_tool(str(self.root / "deploy" / "testnet.sh"), command,
                                           deploy_name="front-run")
                    output = result.stdout + result.stderr
                    self.assertNotEqual(result.returncode, 0, output)
                    self.assertIn(expected, output, command)

    def test_a_proxy_someone_else_created_is_refused(self):
        # A copy of the deployment's IGP, created and initialized in one
        # transaction, but by the attacker: owner the deployer, beneficiary
        # the attacker.
        evm = self.state["evm"]
        remappings = subprocess.run(["forge", "remappings"], cwd=SOLIDITY_DIR, capture_output=True, text=True,
                                    check=True).stdout.splitlines()
        oz = next(line.split("=", 1)[1] for line in remappings if line.startswith("@openzeppelin/contracts/="))
        code = subprocess.run(["forge", "inspect", f"{oz.rstrip('/')}/proxy/ERC1967/ERC1967Proxy.sol:ERC1967Proxy",
                               "bytecode"], cwd=SOLIDITY_DIR, capture_output=True, text=True,
                              check=True).stdout.strip()
        init = self.cast_ok("calldata", "initialize(address,address)", ANVIL_DEPLOYER, ANVIL_ATTACKER)
        args = subprocess.run(["cast", "abi-encode", "constructor(address,bytes)", evm["igp_implementation"], init],
                              capture_output=True, text=True, check=True).stdout.strip()
        receipt = json.loads(subprocess.run(
            ["cast", "send", "--rpc-url", self.rpc, "--private-key", ANVIL_ATTACKER_KEY, "--json",
             "--create", code + args[2:]], capture_output=True, text=True, check=True).stdout)
        state = json.loads(json.dumps(self.state))
        state["evm"].update(igp=receipt["contractAddress"], igp_creation_tx=receipt["transactionHash"])
        out = self.root / "deploy" / "out" / "foreign-proxy"
        out.mkdir(parents=True, exist_ok=True)
        (out / "deployment.json").write_text(json.dumps(state))
        result = self.run_tool(str(self.root / "deploy" / "testnet.sh"), "preflight", deploy_name="foreign-proxy")
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertIn("wasn't initialized in the transaction that created it", result.stderr)

    def test_the_local_chain_gets_the_multicall(self):
        local_env = (self.root / "deploy" / "local.env").read_text()
        deploys = [line for line in self.core_trace.splitlines() if line.startswith("dusk-tx deploy-wasm")]
        configs = self.run_tool(str(self.root / "deploy" / "testnet.sh"), "agent-configs", check=True)
        relayer = json.loads((self.root / "deploy" / "out" / "local-rehearsal" / "relayer.json").read_text())
        if MULTICALL_WASM:
            # At the ID it has on every chain: nonce 0 and the published owner.
            [deploy] = deploys
            for part in (f"--wasm {MULTICALL_WASM}", f"--owner {MULTICALL_OWNER}", "--deploy-nonce 0",
                         f"--expected-id {MULTICALL}", "--expected-chain-id 2", "--check-multicall"):
                self.assertIn(part, deploy)
            self.assertIn(f"DUSK_MULTICALL={MULTICALL}\n", local_env)
            self.assertEqual(relayer["chains"]["dusk"]["batchContractAddress"], "0x" + MULTICALL)
        else:
            self.assertEqual(deploys, [])
            self.assertIn("DUSK_MULTICALL_WASM is not set", self.core_output.stderr)
            self.assertIn("DUSK_MULTICALL=\n", local_env)
            self.assertIn("DUSK_MULTICALL is empty", configs.stderr)
            self.assertNotIn("batchContractAddress", relayer["chains"]["dusk"])

    def test_another_multicall_wasm_is_refused(self):
        wasm = self.root / "other.wasm"
        wasm.write_bytes(b"\0asm\x01\0\0\0")
        bridge = self.root / "demo" / ".env.bridge"
        saved = bridge.read_text()
        bridge.write_text(saved + f"DUSK_MULTICALL_WASM={wasm}\n")
        try:
            result = self.run_tool(str(self.root / "deploy" / "local-evm-core.sh"))
        finally:
            bridge.write_text(saved)
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertIn("isn't Multicall for Dusk v0.1.0", result.stderr)

    def test_a_former_quote_signer_is_refused(self):
        # The IGP's owner adds a quote signer, which signs a free standing
        # quote for one sender's messages to Dusk, and removes it again. The
        # quote outlives the signer and comes before the oracle, for that
        # sender only: quoteSigners() is empty, and asked as the token or the
        # zero address the IGP quotes the oracle's price, which is all the
        # checks looked at before. Its events still show the signer. Anvil
        # puts the chain back afterwards.
        igp, token = self.state["evm"]["igp"], self.state["evm"]["token"]
        domain, gas = 1146442572, "2200000"
        testnet = str(self.root / "deploy" / "testnet.sh")
        snapshot = json.loads(self.cast_ok("rpc", "evm_snapshot"))
        try:
            self.cast_ok("send", igp, "addQuoteSigner(address)", ANVIL_ATTACKER, "--private-key", ANVIL_KEY)
            result = self.run_tool(testnet, "preflight")
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"has quote signers [{self.cast_ok('call', igp, 'quoteSigners()(address[])')[1:-1]}]",
                          result.stderr)
            now = int(self.cast_ok("block", "latest", "--field", "timestamp"))
            quote = {"context": "0x" + "00" * 20 + f"{domain:08x}" + ANVIL_ATTACKER[2:],
                     "data": "0x" + "00" * 32, "issuedAt": now, "expiry": now + 10**6,
                     "salt": "0x" + "00" * 32, "submitter": "0x" + "00" * 20}
            typed = {"types": {"EIP712Domain": [{"name": "name", "type": "string"},
                                                {"name": "version", "type": "string"},
                                                {"name": "chainId", "type": "uint256"},
                                                {"name": "verifyingContract", "type": "address"}],
                               "SignedQuote": [{"name": "context", "type": "bytes"},
                                               {"name": "data", "type": "bytes"},
                                               {"name": "issuedAt", "type": "uint48"},
                                               {"name": "expiry", "type": "uint48"},
                                               {"name": "salt", "type": "bytes32"},
                                               {"name": "submitter", "type": "address"}]},
                     "primaryType": "SignedQuote",
                     "domain": {"name": "OffchainQuoter", "version": "1", "chainId": 31338, "verifyingContract": igp},
                     "message": quote}
            signature = subprocess.run(["cast", "wallet", "sign", "--private-key", ANVIL_ATTACKER_KEY, "--data",
                                        json.dumps(typed)], capture_output=True, text=True, check=True).stdout.strip()
            self.cast_ok("send", igp, "submitQuote((bytes,bytes,uint48,uint48,bytes32,address),bytes)",
                         "({context},{data},{issuedAt},{expiry},{salt},{submitter})".format(**quote), signature,
                         "--private-key", ANVIL_KEY)
            self.cast_ok("send", igp, "removeQuoteSigner(address)", ANVIL_ATTACKER, "--private-key", ANVIL_KEY)

            def quoted(sender):
                return self.cast_ok("call", igp, "quoteGasPayment(uint32,uint256)(uint256)", str(domain), gas,
                                    "--from", sender).split()[0]
            self.assertEqual(self.cast_ok("call", igp, "quoteSigners()(address[])"), "[]")
            self.assertEqual(quoted(ANVIL_ATTACKER), "0")
            for sender in (token, "0x" + "00" * 20):
                self.assertNotEqual(quoted(sender), "0")
            result = self.run_tool(testnet, "preflight")
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"The EVM IGP {igp.lower()} has had a quote signer: QuoteSignerAdded in", result.stderr)
        finally:
            self.assertIs(json.loads(self.cast_ok("rpc", "evm_revert", snapshot)), True)
        self.run_tool(testnet, "preflight", check=True)


# Stands in for the relayer and validator binaries: writes the environment
# it was started with next to its config, and waits to be stopped.
AGENT_STUB = """#!/bin/sh
env > "${CONFIG_FILES%.json}.env"
exec sleep 120
"""

# What an operator's shell might hold. Hyperlane applies HYP_* over the
# config files, so the first sends the relayer to another Dusk node.
OPERATOR_ENV = {
    "HYP_CHAINS_DUSK_CUSTOMRPCURLS": "https://public-node.invalid/",
    "HYP_CHAINS_ANVIL_RPCURLS_0_HTTP": "https://public-rpc.invalid/",
    "HYP_DB": "/somewhere/else",
    "HYP_GASPAYMENTENFORCEMENT": "[]",
    "RUST_LOG": "trace",
    "CONFIG_FILES": "/somewhere/else.json",
    "AWS_ACCESS_KEY_ID": "operator",
    "DUSK_TX_BIN": "/somewhere/dusk-tx",
    "DUSK_CONSENSUS_PASSWORD": "operator-secret",
}

# Anvil's development keys #1-#5 and their addresses, as the agent scripts
# use them.
ANVIL_DEV_KEYS = {
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a": "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc",
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6": "0x90f79bf6eb2c4f870365e785982e1f101e93b906",
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a": "0x15d34aaf54267db7d7c367839aaf71a00a2c6a65",
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba": "0x9965507d1a55bcc2695c58ba16fb37d819b0a4dc",
}


class AgentEnvironment(unittest.TestCase):
    """The bench and the local rehearsal start each agent with only the
    variables it needs, whatever the operator's shell holds."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="hyperlane-agent-env-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "agent-bin"
        self.bin.mkdir()
        for name in ("relayer", "validator"):
            write_executable(self.bin / name, AGENT_STUB)
        self.run_dir = self.root / "run"
        self.run_dir.mkdir()

    def stop_all(self, pid_files):
        for pid_file in pid_files:
            try:
                os.kill(int(pid_file.read_text()), 15)
            except (OSError, ValueError):
                pass

    def environments(self, directory):
        found = {}
        for dump in sorted(directory.glob("*.env")):
            found[dump.stem] = dict(line.split("=", 1) for line in dump.read_text().splitlines() if "=" in line)
        return found

    def assert_clean(self, environment, config, keys):
        self.assertEqual(environment["CONFIG_FILES"], str(config))
        self.assertEqual({name: value for name, value in environment.items() if name.startswith("HYP_")}, keys)
        for name in ("RUST_LOG", "AWS_ACCESS_KEY_ID", "DUSK_CONSENSUS_PASSWORD", "EVM_DEPLOYER_KEY"):
            self.assertNotIn(name, environment)

    def test_bench_agents_get_a_clean_environment(self):
        (self.root / "demo" / "bench").mkdir(parents=True)
        shutil.copy2(ROOT / "demo" / "bench" / "agents.sh", self.root / "demo" / "bench" / "agents.sh")
        (self.root / "demo" / ".env.bridge").write_text("RUSK_HTTP_PORT=8080\nANVIL_PORT=8545\n"
                                                        f"SOLIDITY_DIR={self.root}\n")
        bench = self.root / "bench"
        for directory in ("agents", "logs"):
            (bench / directory).mkdir(parents=True)
        env = {"PATH": os.environ["PATH"], "HOME": str(self.root), "BENCH_DIR": str(bench),
               "AGENT_BIN_DIR": str(self.bin), "AGENT_RUN_DIR": str(self.run_dir),
               "DUSK_TX": "/usr/bin/false", **OPERATOR_ENV}
        self.addCleanup(self.stop_all, [bench / "agents" / "relayer.pid"])
        result = subprocess.run(["bash", str(self.root / "demo" / "bench" / "agents.sh"), "start", "relayer"],
                                env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        environment = self.environments(bench / "agents")["relayer"]
        self.assert_clean(environment, bench / "agents" / "relayer.json",
                          {"HYP_CHAINS_ANVIL_SIGNER_KEY": "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"})
        self.assertEqual(environment["DUSK_TX_BIN"], str(self.root / "demo" / "bench" / "dusk-tx-shim.sh"))
        self.assertEqual((environment["BENCH_AGENT"], environment["BENCH_DUSK_TX_REAL"]), ("relayer", "/usr/bin/false"))

    def test_rehearsal_agents_refuse_urls_that_only_look_local(self):
        # local-agents.sh starts agents with public development keys. A URL
        # with userinfo (http://localhost:8545@node.example/) reaches
        # node.example, and DUSK_RUES_URL was not checked at all.
        (self.root / "deploy").mkdir()
        shutil.copy2(ROOT / "deploy" / "local-agents.sh", self.root / "deploy" / "local-agents.sh")
        for evm, dusk in [("http://localhost:8545@node.example/", "http://localhost:8080/"),
                          ("http://127.0.0.1:8545@node.example", "http://localhost:8080/"),
                          ("http://localhost:8545", "https://node.example/"),
                          ("http://localhost:8545", "http://localhost:8080@node.example/")]:
            with self.subTest(evm=evm, dusk=dusk):
                env_file = self.root / "deploy" / "local.env"
                env_file.write_text(f"DEPLOY_NAME=rehearsal\nEVM_CHAIN_NAME=anvil\n"
                                    f"EVM_RPC={shlex.quote(evm)}\nDUSK_RUES_URL={shlex.quote(dusk)}\n")
                env = {"PATH": os.environ["PATH"], "HOME": str(self.root), "TESTNET_ENV": str(env_file),
                       "AGENT_BIN_DIR": str(self.bin), "AGENT_RUN_DIR": str(self.run_dir)}
                result = subprocess.run(["bash", str(self.root / "deploy" / "local-agents.sh"), "start"],
                                        env=env, capture_output=True, text=True, timeout=60)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("for local rehearsals only", result.stderr)
                self.assertNotIn("node.example", result.stdout + result.stderr)
                self.assertFalse((self.root / "deploy" / "out").exists())

    def rehearsal(self, relayer=None, validator=None):
        """deploy/local-agents.sh in a temporary checkout, with the agent
        config templates agent-configs writes, or `relayer` and `validator`
        in their place, and stand-ins for cast, dusk-tx and the agents.
        Returns the environment to run it in, the deployment's directory
        and the dusk-tx stand-in."""
        (self.root / "deploy").mkdir(exist_ok=True)
        shutil.copy2(ROOT / "deploy" / "local-agents.sh", self.root / "deploy" / "local-agents.sh")
        out = self.root / "deploy" / "out" / "rehearsal"
        out.mkdir(parents=True, exist_ok=True)
        anvil = {"name": "anvil", "rpcUrls": [{"http": "http://localhost:8545"}]}
        dusk = {"name": "dusk", "rpcUrls": [{"http": "http://localhost:8080/"}]}
        relayer = relayer or {"db": "x", "chains": {"anvil": anvil, "dusk": dusk}}
        validator = validator or {"db": "y", "chains": {"dusk": dusk}}
        (out / "relayer.json").write_text(json.dumps(relayer))
        (out / "validator-dusk.json").write_text(json.dumps(validator))
        keys = self.root / "consensus.keys"
        keys.write_text("{}")
        dev = list(ANVIL_DEV_KEYS.values())
        env_file = self.root / "deploy" / "local.env"
        env_file.write_text("".join(f"{key}={shlex.quote(str(value))}\n" for key, value in {
            "DEPLOY_NAME": "rehearsal", "DUSK_RUES_URL": "http://localhost:8080/", "DUSK_KEYS": keys,
            "EVM_CHAIN_NAME": "anvil", "EVM_RPC": "http://localhost:8545",
            "EVM_VALIDATOR_ANNOUNCE": "0x" + "5a" * 20, "EVM_ORIGIN_VALIDATORS": dev[1],
            "DUSK_ORIGIN_VALIDATORS": ",".join(dev[2:]), "AGENT_STATE_DIR": out / "agents" / "state",
            "DUSK_SIGNER_KEY_FILE": out / "agents" / "dusk-signer.key",
        }.items()) + "export DUSK_CONSENSUS_PASSWORD=password\nexport EVM_DEPLOYER_KEY=0x" + "11" * 32 + "\n")
        tools = self.root / "tools"
        tools.mkdir(exist_ok=True)
        write_executable(tools / "cast", "#!/usr/bin/env python3\nimport sys\nkeys = " + repr(ANVIL_DEV_KEYS) + "\n"
                         "args = sys.argv[1:]\n"
                         "if args[0] == 'wallet':\n    print(keys[args[args.index('--private-key') + 1]])\n"
                         "else:\n    print('[' + ', '.join(keys.values()) + ']')\n")
        dusk_tx = tools / "dusk-tx"
        write_executable(dusk_tx, "#!/bin/sh\ncase \"$1\" in\n"
                         "  export-signer-key) shift; while [ \"$1\" != --out ]; do shift; done; echo key > \"$2\" ;;\n"
                         "  query) echo '{\"value\": 3}' ;;\nesac\n")
        env = {"PATH": f"{tools}{os.pathsep}{os.environ['PATH']}", "HOME": str(self.root),
               "TESTNET_ENV": str(env_file), "DUSK_TX": str(dusk_tx), "AGENT_BIN_DIR": str(self.bin),
               "AGENT_RUN_DIR": str(self.run_dir), **OPERATOR_ENV}
        return env, out, dusk_tx

    REHEARSAL_AGENTS = ["validator-evm", "validator-dusk-1", "validator-dusk-2", "validator-dusk-3", "relayer"]

    def test_rehearsal_agents_refuse_configs_that_name_other_nodes(self):
        # local-agents.sh checked EVM_RPC and DUSK_RUES_URL, but the agents
        # run configs made from the templates agent-configs saved, which can
        # name other nodes: a validator started on one with a public
        # development key. Every endpoint in every config the agents get is
        # checked before a key is exported or an agent started.
        anvil = {"name": "anvil", "rpcUrls": [{"http": "http://localhost:8545"}]}
        dusk = {"name": "dusk", "rpcUrls": [{"http": "http://localhost:8080/"}]}
        remote = "https://node.example/"
        cases = {
            "the EVM chain's RPC": ({"chains": {"anvil": {**anvil, "rpcUrls": [{"http": remote}]}, "dusk": dusk}},
                                    None),
            "the relayer's Dusk node": ({"chains": {"anvil": anvil, "dusk": {**dusk, "rpcUrls": [{"http": remote}]}}},
                                        None),
            "the validators' Dusk node": (None, {"chains": {"dusk": {**dusk, "rpcUrls": [{"http": remote}]}}}),
            "a second RPC": ({"chains": {"anvil": {**anvil, "rpcUrls": [{"http": "http://localhost:8545"},
                                                                       {"http": remote}]}, "dusk": dusk}}, None),
            "a list in one string": ({"chains": {"anvil": {**anvil, "customRpcUrls":
                                                           f"http://localhost:8545,{remote}"}, "dusk": dusk}}, None),
            "a URL that only looks local": (None, {"chains": {"dusk": {
                **dusk, "rpcUrls": [{"http": "http://localhost:8080@node.example/"}]}}}),
            "a WebSocket": ({"chains": {"anvil": {**anvil, "ws": "ws://node.example"}, "dusk": dusk}}, None),
            "another chain": ({"chains": {"anvil": anvil, "dusk": dusk,
                                          "other": {"rpcUrls": [{"http": remote}]}}}, None),
            "no endpoint at all": (None, {"chains": {"dusk": {"name": "dusk"}}}),
        }
        for case, (relayer, validator) in cases.items():
            with self.subTest(case=case):
                env, out, _ = self.rehearsal(relayer, validator)
                shutil.rmtree(out / "agents", ignore_errors=True)
                result = subprocess.run(["bash", str(self.root / "deploy" / "local-agents.sh"), "start"],
                                        env=env, capture_output=True, text=True, timeout=60)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("names", result.stderr)
                self.assertNotIn("node.example", result.stdout + result.stderr)
                self.assertFalse((out / "agents" / "dusk-signer.key").exists())
                self.assertEqual(list((out / "agents").glob("*.pid")), [])

    def test_rehearsal_agents_get_a_clean_environment(self):
        env, out, dusk_tx = self.rehearsal()
        agents = out / "agents"
        names = self.REHEARSAL_AGENTS
        self.addCleanup(self.stop_all, [agents / f"{name}.pid" for name in names])
        result = subprocess.run(["bash", str(self.root / "deploy" / "local-agents.sh"), "start"],
                                env=env, capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        environments = self.environments(agents)
        self.assertEqual(sorted(environments), sorted(names))
        validator_keys = list(ANVIL_DEV_KEYS)[2:]
        expected = {
            "validator-evm": {"HYP_VALIDATOR_KEY": list(ANVIL_DEV_KEYS)[1],
                              "HYP_CHAINS_ANVIL_SIGNER_KEY": list(ANVIL_DEV_KEYS)[1]},
            "relayer": {"HYP_CHAINS_ANVIL_SIGNER_KEY": list(ANVIL_DEV_KEYS)[0]},
            **{f"validator-dusk-{i + 1}": {"HYP_VALIDATOR_KEY": key} for i, key in enumerate(validator_keys)},
        }
        for name in names:
            with self.subTest(agent=name):
                self.assert_clean(environments[name], agents / f"{name}.json", expected[name])
                self.assertEqual(environments[name]["DUSK_TX_BIN"], str(dusk_tx))
        # The relayer runs agent-configs' config, which delivers to Dusk one
        # at a time.
        dusk = json.loads((agents / "relayer.json").read_text())["chains"]["dusk"]
        self.assertNotIn("maxBatchSize", dusk)
        self.assertNotIn("pipelineDeliveries", dusk)
        stop = subprocess.run(["bash", str(self.root / "deploy" / "local-agents.sh"), "stop"],
                              env=env, capture_output=True, text=True, timeout=60)
        self.assertEqual(stop.returncode, 0, stop.stderr)


# Stands in for an agent in demo/e2e-*.sh's launches, where AGENT_BIN_DIR
# points: writes the environment and the directory it was started in next
# to its config. The scripts' default binaries, which a launch must not
# run, exit without a word.
E2E_AGENT_STUB = """#!/bin/sh
env > "${CONFIG_FILES%.json}.$(basename "$0").env"
pwd > "${CONFIG_FILES%.json}.$(basename "$0").cwd"
"""
# An agent launch in demo/e2e-*.sh: a subshell that changes to the run
# directory and execs the agent under `env -i`, logging, in the background.
E2E_LAUNCH = re.compile(r'^[ \t]*\(cd "[^"]+" && \\\n[ \t]*exec env[^\n]*\\\n[ \t]*>>?"\$\w+" 2>&1\) &', re.M)


class E2eAgentEnvironment(unittest.TestCase):
    """Every agent the E2E scripts (demo/e2e-*.sh) start runs from
    AGENT_BIN_DIR, in AGENT_RUN_DIR, with only PATH, HOME, DUSK_TX_BIN and
    its CONFIG_FILES, whatever the operator's shell holds."""

    def launch(self, launch, script):
        """Run one launch as its script would, with the script's own agent
        settings; returns the environment and directory the agent got."""
        with tempfile.TemporaryDirectory(prefix="hyperlane-e2e-launch-") as name:
            tmp = Path(name)
            bin_dir, run_dir = tmp / "bin", tmp / "run"
            default = tmp / "hyperlane-monorepo" / "rust" / "main" / "target" / "debug"
            for directory in (bin_dir, run_dir, default, tmp / "hyperlane" / "demo"):
                directory.mkdir(parents=True)
            for agent in ("relayer", "validator"):
                write_executable(bin_dir / agent, E2E_AGENT_STUB)
                write_executable(default / agent, "#!/bin/sh\nexit 3\n")
            config = tmp / "agent.json"
            settings = "\n".join(line for line in script.splitlines() if line.startswith(
                ("AGENT_RUN_DIR=", "BUILD_AGENTS=", '[ -z "${AGENT_BIN_DIR', "AGENT_BIN_DIR=")))
            program = (f"set -eu\nSCRIPT_DIR={tmp}/hyperlane/demo\nDUSK_TX=/usr/bin/false\n"
                       f"cfg={config}; relayer_cfg={config}; log={tmp}/agent.log; relayer_log={tmp}/agent.log\n"
                       f"{settings}\n{launch}\nwait\n")
            env = {"PATH": os.environ["PATH"], "HOME": str(tmp), "AGENT_BIN_DIR": str(bin_dir),
                   "AGENT_RUN_DIR": str(run_dir), **OPERATOR_ENV}
            result = subprocess.run(["bash", "-c", program], env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            dumps = list(tmp.glob("agent.*.env"))
            self.assertEqual(len(dumps), 1, f"the launch ran no agent from AGENT_BIN_DIR: {launch.strip()}")
            environment = dict(line.split("=", 1) for line in dumps[0].read_text().splitlines() if "=" in line)
            cwd = dumps[0].with_suffix(".cwd").read_text().strip()
            return environment, cwd, str(config), str(run_dir)

    def test_every_agent_starts_with_a_clean_environment(self):
        scripts = sorted((ROOT / "demo").glob("e2e-*.sh"))
        checked = 0
        for script in scripts:
            text = script.read_text()
            launches = [match.group(0) for match in E2E_LAUNCH.finditer(text)]
            # Every agent the script starts is in a launch this test knows.
            self.assertEqual(len(launches), text.count('"$AGENT_BIN_DIR/'), script.name)
            if "target/debug" in text:
                self.assertTrue(launches, f"{script.name} starts agents in a form this test doesn't know")
            for launch in launches:
                with self.subTest(script=script.name, launch=launch.strip()[:80]):
                    environment, cwd, config, run_dir = self.launch(launch, text)
                    self.assertEqual(environment["CONFIG_FILES"], config)
                    self.assertEqual(environment["DUSK_TX_BIN"], "/usr/bin/false")
                    self.assertEqual(cwd, run_dir)
                    self.assertEqual(set(environment) - {"PWD", "SHLVL", "_", "OLDPWD"},
                                     {"PATH", "HOME", "DUSK_TX_BIN", "CONFIG_FILES"})
                checked += 1
        # The scripts start eleven agents between them.
        self.assertGreaterEqual(checked, 11)


class BenchConfigs(unittest.TestCase):
    """demo/bench/agents.sh configs: every Dusk chain batches its reads
    through the multicall setup deployed, unless BATCH_READS=0, and a
    multicall never deployed is refused; the relayer delivers to Dusk one at
    a time, as on testnet, unless PIPELINE_DELIVERIES=1; each agent reaches
    Rusk through its own proxy port."""

    AGENTS = ("relayer", "validator-dusk-1", "validator-dusk-2", "validator-dusk-3")

    def configs(self, deployed=True, template=None, **settings):
        with tempfile.TemporaryDirectory(prefix="hyperlane-bench-configs-") as name:
            root = Path(name)
            (root / "demo" / "bench").mkdir(parents=True)
            shutil.copy2(ROOT / "demo" / "bench" / "agents.sh", root / "demo" / "bench" / "agents.sh")
            (root / "demo" / ".env.bridge").write_text("RUSK_HTTP_PORT=8080\nANVIL_PORT=8545\n")
            bench = root / "bench"
            (bench / "deploy").mkdir(parents=True)
            dusk = {"name": "dusk", "rpcUrls": [{"http": "http://elsewhere/"}], **(template or {})}
            (bench / "deploy" / "relayer.json").write_text(json.dumps(
                {"db": "x", "chains": {"anvil": {"name": "anvil"}, "dusk": dusk}}))
            (bench / "deploy" / "validator-dusk.json").write_text(json.dumps({"db": "y", "chains": {"dusk": dusk}}))
            if deployed:
                (bench / "deploy" / "multicall.json").write_text(json.dumps({"contract_id": MULTICALL,
                                                                             "deployed": True}))
            env = {"PATH": os.environ["PATH"], "HOME": str(root), "BENCH_DIR": str(bench),
                   **{key: str(value) for key, value in settings.items()}}
            result = subprocess.run(["bash", str(root / "demo" / "bench" / "agents.sh"), "configs"],
                                    env=env, capture_output=True, text=True, timeout=60)
            configs = {}
            if result.returncode == 0:
                for agent in (*self.AGENTS, "validator-evm"):
                    configs[agent] = json.loads((bench / "agents" / f"{agent}.json").read_text())
            return result, configs

    def dusk(self, configs, agent):
        return configs[agent]["chains"]["dusk"]

    def test_the_agents_batch_their_reads_through_the_multicall(self):
        result, configs = self.configs()
        self.assertEqual(result.returncode, 0, result.stderr)
        for agent in self.AGENTS:
            self.assertEqual(self.dusk(configs, agent)["batchContractAddress"], "0x" + MULTICALL)
        self.assertEqual(self.dusk(configs, "relayer")["rpcUrls"], [{"http": "http://127.0.0.1:18100/"}])
        self.assertEqual(self.dusk(configs, "validator-dusk-2")["rpcUrls"], [{"http": "http://127.0.0.1:18102/"}])
        # BATCH_READS=0: single queries, even if the template names the multicall.
        result, configs = self.configs(template={"batchContractAddress": "0x" + MULTICALL}, BATCH_READS=0)
        self.assertEqual(result.returncode, 0, result.stderr)
        for agent in self.AGENTS:
            self.assertNotIn("batchContractAddress", self.dusk(configs, agent))
        # Batching reads through a multicall setup never deployed is refused.
        result, _ = self.configs(deployed=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No Multicall for Dusk deployed", result.stderr)

    def test_the_relayer_delivers_one_at_a_time_unless_asked_to_pipeline(self):
        result, configs = self.configs()
        self.assertEqual(result.returncode, 0, result.stderr)
        for agent in self.AGENTS:
            self.assertNotIn("maxBatchSize", self.dusk(configs, agent))
            self.assertNotIn("pipelineDeliveries", self.dusk(configs, agent))
        for settings, batch in (({"PIPELINE_DELIVERIES": 1}, 16),
                                ({"PIPELINE_DELIVERIES": 1, "DUSK_RELAYER_MAX_BATCH_SIZE": 8}, 8)):
            with self.subTest(settings=settings):
                result, configs = self.configs(**settings)
                self.assertEqual(result.returncode, 0, result.stderr)
                relayer = self.dusk(configs, "relayer")
                self.assertEqual((relayer["pipelineDeliveries"], relayer["maxBatchSize"]), (True, batch))
                for agent in self.AGENTS[1:]:
                    self.assertNotIn("pipelineDeliveries", self.dusk(configs, agent))
                    self.assertNotIn("maxBatchSize", self.dusk(configs, agent))
        for settings, expected in [
            ({"PIPELINE_DELIVERIES": 1, "DUSK_RELAYER_MAX_BATCH_SIZE": 1}, "from 2 to 64"),
            ({"PIPELINE_DELIVERIES": 1, "DUSK_RELAYER_MAX_BATCH_SIZE": 65}, "from 2 to 64"),
            ({"DUSK_RELAYER_MAX_BATCH_SIZE": 16}, "only counts with PIPELINE_DELIVERIES=1"),
            ({"PIPELINE_DELIVERIES": 2}, "PIPELINE_DELIVERIES must be 0"),
        ]:
            with self.subTest(settings=settings):
                result, _ = self.configs(**settings)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(expected, result.stderr)


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

# Records the password export-signer-key gets, and writes a key file. Also
# answers Multicall for Dusk's `version` with FIXTURE_MULTICALL_VERSION (1, or
# nothing when it is empty).
EXPORT_STUB = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
if args[0] == "query":
    assert args[args.index("--method") + 1] == "version", args
    version = os.environ.get("FIXTURE_MULTICALL_VERSION", "1")
    if not version:
        print(json.dumps({"success": False, "error": "Query failed (500): ContractDoesNotExist"}))
        sys.exit(1)
    print(json.dumps({"success": True, "value": int(version)}))
    sys.exit(0)
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


# The tools DemoSigningChain's scripts run up to their first deployment,
# each logging its call to FIXTURE_TRACE. lsof reports every port in use, so
# no script starts a node; cargo and make build nothing; cast answers the
# block number and the chain ID. forge, dusk-tx and anvil fail, so a script
# that gets past its chain check stops at its first deployment, before it
# writes anything.
DEMO_TOOL_STUB = """#!/bin/sh
printf '%s\\n' "$(basename "$0") $*" >> "$FIXTURE_TRACE"
case "$(basename "$0") $1" in
    "lsof "*|"cargo "*|"make "*) exit 0 ;;
    "cast block-number") echo 100 ;;
    "cast chain-id") echo 31338 ;;
    *) exit 1 ;;
esac
"""


class DemoSigningChain(unittest.TestCase):
    """The demo and local rehearsal scripts sign Dusk transactions for
    DUSK_EXPECTED_CHAIN_ID, the local devnet's chain ID 0 unless set, and
    stop before deploying anything when the node at the RUES URL reports
    another chain. Before, each took the node's answer as the chain to sign
    for, even over an operator's explicit setting, so a testnet node, or a
    local forwarder to one, got the deployment; and start-env.sh took any
    healthy node already on its port."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="hyperlane-demo-chain-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        # The checkouts' layout: rusk-private next to hyperlane/, which holds
        # this repository and the monorepo's Solidity sources.
        self.dusk = self.root / "hyperlane" / "dusk"
        for script in ("demo/deploy.sh", "demo/demo.sh", "demo/start-env.sh", "demo/bench/agents.sh",
                       "deploy/local-evm-core.sh"):
            (self.dusk / script).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / script, self.dusk / script)
        shutil.copy2(ROOT / "demo" / "bridge.env.example", self.dusk / "demo" / ".env.bridge")
        (self.root / "hyperlane" / "hyperlane-monorepo" / "solidity" / "dependencies").mkdir(parents=True)
        rusk = self.root / "rusk-private"
        for directory in (rusk / "core", rusk / "examples", rusk / "target" / "release", self.dusk / "e2e"):
            directory.mkdir(parents=True)
        (self.dusk / "rusk").symlink_to("../../rusk-private")
        for path in (rusk / "target" / "release" / "rusk", rusk / "examples" / "consensus.keys",
                     self.dusk / "e2e" / "consensus.keys", self.root / "example.state"):
            path.write_text("fixture\n")
        (self.root / "wasm").mkdir()
        (self.root / "wasm" / "hyperlane_dusk_mailbox.wasm").write_bytes(b"\0asm")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for tool in ("lsof", "cargo", "make", "cast", "forge", "anvil", "dusk-tx"):
            write_executable(self.bin / tool, DEMO_TOOL_STUB)
        write_executable(self.bin / "curl", CURL_STUB)
        self.trace = self.root / "tools.trace"
        self.state = self.root / "bridge-state.json"
        self.pids = self.root / "env.pids"
        self.bench = self.root / "bench"

    def run_demo(self, script, *args, endpoint=2, **settings):
        """Run `script` with a node on chain `endpoint` at the RUES URL.
        Returns the result and the tools it ran."""
        self.trace.write_text("")
        env = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}", "HOME": str(self.root),
            "FIXTURE_TRACE": str(self.trace), "FIXTURE_DUSK_CHAIN_ID": str(endpoint),
            "DUSK_TX": str(self.bin / "dusk-tx"), "WASM_DIR": str(self.root / "wasm"),
            "BRIDGE_STATE_FILE": str(self.state), "PID_FILE": str(self.pids), "BENCH_DIR": str(self.bench),
            "RUSK_HTTP_PORT": "65531", "ANVIL_PORT": "65532", "RUES_URL": "http://localhost:65531/",
            "SKIP_OTTERSCAN": "true", "SKIP_DUSK_EXPLORER": "true",
            **{key: str(value) for key, value in settings.items()},
        }
        result = subprocess.run(["bash", str(self.dusk / script), *args], env=env, capture_output=True,
                                text=True, timeout=60)
        return result, self.trace.read_text().splitlines()

    def assert_refused(self, result, tools, expected):
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn(expected, output)
        self.assertEqual([line for line in tools if line.split()[0] in ("forge", "dusk-tx", "anvil")
                          or line.startswith("cast send")], [])

    def test_deploy_signs_only_for_the_chain_it_is_set_for(self):
        # A node on testnet (2) at the RUES URL. An operator's explicit 0
        # was replaced with the node's 2 too.
        for settings in ({}, {"DUSK_EXPECTED_CHAIN_ID": 0}):
            with self.subTest(settings=settings):
                result, tools = self.run_demo("demo/deploy.sh", "--dusk-ism", "testMock", **settings)
                self.assert_refused(result, tools, "is on chain ID 2, not DUSK_EXPECTED_CHAIN_ID 0")
                self.assertFalse(self.state.exists())
        for value in ("two", "256", "-1", ""):
            with self.subTest(value=value):
                result, tools = self.run_demo("demo/deploy.sh", "--dusk-ism", "testMock", endpoint=0,
                                              DUSK_EXPECTED_CHAIN_ID=value)
                if value == "":
                    # Empty is unset: the devnet's 0.
                    self.assertIn("on chain ID 0", result.stdout)
                    continue
                self.assert_refused(result, tools, f"must be a Dusk chain ID from 0 to 255, not '{value}'")
        # On the chain it is set for, it goes on to its deployment, where the
        # stubbed forge stops it.
        for endpoint, settings in ((0, {}), (2, {"DUSK_EXPECTED_CHAIN_ID": 2})):
            with self.subTest(endpoint=endpoint):
                result, tools = self.run_demo("demo/deploy.sh", "--dusk-ism", "testMock", endpoint=endpoint,
                                              **settings)
                self.assertIn(f"Dusk RUES reachable, on chain ID {endpoint}", result.stdout)
                self.assertIn("Solidity compilation failed", result.stdout)
                self.assertIn("forge build --quiet", tools)

    def test_demo_checks_the_chain_before_deploying_on_either(self):
        result, tools = self.run_demo("demo/demo.sh")
        self.assert_refused(result, tools, "is on chain ID 2, not DUSK_EXPECTED_CHAIN_ID 0")
        self.assertNotIn("Starting Anvil", result.stdout)
        result, tools = self.run_demo("demo/demo.sh", endpoint=0)
        self.assertIn("Dusk node connected, on chain ID 0", result.stdout)
        self.assertIn("Failed to deploy TestIsm", result.stdout)

    def test_start_env_takes_a_running_node_only_on_its_chain(self):
        result, tools = self.run_demo("demo/start-env.sh")
        self.assert_refused(result, tools, "is on chain ID 2, not DUSK_EXPECTED_CHAIN_ID 0")
        self.assertNotIn("rusk:external", self.pids.read_text())
        self.pids.unlink()
        result, tools = self.run_demo("demo/start-env.sh", endpoint=0)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Using healthy external Rusk service on chain ID 0", result.stdout)
        self.assertEqual(self.pids.read_text(), "rusk:external\nanvil:external\n")

    def test_start_env_checks_the_node_it_starts(self):
        # Nothing on Rusk's port: start-env.sh starts the node (a stub that
        # exits) and waits for its RUES, here one on chain 2. Anvil's port is
        # taken, so even a start-env.sh that went on wouldn't start an Anvil,
        # whose log is a fixed path in /tmp.
        write_executable(self.bin / "lsof", '#!/bin/sh\ncase "$*" in *":65531"*) exit 1 ;; esac\nexit 0\n')
        write_executable(self.root / "rusk-private" / "target" / "release" / "rusk", "#!/bin/sh\nexit 0\n")
        # The node's log goes to RUSK_LOG here, not to the fixed
        # /tmp/rusk-dev.log a running demo's node writes to: don't run a
        # start-env.sh that would truncate it.
        self.assertIn('RUSK_LOG="${RUSK_LOG:-/tmp/rusk-dev.log}"', (self.dusk / "demo" / "start-env.sh").read_text())
        log = self.root / "rusk.log"
        result, tools = self.run_demo("demo/start-env.sh", RUSK_LOG=log)
        self.assert_refused(result, tools, "is on chain ID 2, not DUSK_EXPECTED_CHAIN_ID 0")
        self.assertIn("Rusk is ready", result.stdout)
        self.assertNotIn("Starting Anvil", result.stdout)
        self.assertTrue(log.exists())

    def test_the_local_rehearsal_signs_only_for_the_chain_it_is_set_for(self):
        for script, args, written in (("deploy/local-evm-core.sh", (), self.dusk / "deploy" / "local.env"),
                                      ("demo/bench/agents.sh", ("setup",), self.bench / "rehearsal.env")):
            with self.subTest(script=script):
                result, tools = self.run_demo(script, *args)
                self.assert_refused(result, tools, "is on chain ID 2, not DUSK_EXPECTED_CHAIN_ID 0")
                self.assertFalse(written.exists())
                self.assertNotIn("cast chain-id", " ".join(tools))
                # Set for that chain, it goes on, to its first stubbed
                # deployment.
                result, tools = self.run_demo(script, *args, DUSK_EXPECTED_CHAIN_ID=2)
                self.assertNotIn("not DUSK_EXPECTED_CHAIN_ID", result.stdout + result.stderr)
                self.assertIn("cast chain-id", " ".join(tools))
                self.assertFalse(written.exists())


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

    def generate(self, password, dusk_tx, **extra_env):
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
            **extra_env,
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

    def test_relayer_delivers_to_dusk_one_at_a_time(self):
        stub = self.bin / "dusk-tx"
        write_executable(stub, EXPORT_STUB)
        result = self.generate("password", stub)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        generated = json.loads(result.stdout)
        relayer = json.loads(Path(generated["relayer"]).read_text())
        shutil.rmtree(generated["runDir"])
        for chain in relayer["chains"].values():
            self.assertNotIn("maxBatchSize", chain)
            self.assertNotIn("pipelineDeliveries", chain)

    def test_agents_batch_contract_reads_through_the_multicall(self):
        stub = self.bin / "dusk-tx"
        write_executable(stub, EXPORT_STUB)
        for version, included in (("1", True), ("", False), ("2", False)):
            with self.subTest(version=version):
                result = self.generate("password", stub, FIXTURE_MULTICALL_VERSION=version)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                generated = json.loads(result.stdout)
                relayer = json.loads(Path(generated["relayer"]).read_text())
                shutil.rmtree(generated["runDir"])
                if included:
                    self.assertEqual(relayer["chains"]["dusk"]["batchContractAddress"], "0x" + MULTICALL)
                else:
                    self.assertNotIn("batchContractAddress", relayer["chains"]["dusk"])
                    self.assertIn("leave batchContractAddress out", result.stderr)
                self.assertNotIn("batchContractAddress", relayer["chains"]["anvil"])

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
