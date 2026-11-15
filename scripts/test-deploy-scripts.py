#!/usr/bin/env python3
"""Run deploy/testnet.sh against stubbed tools.

No network, chain or Rust build is needed: dusk-tx, cast, curl and forge are
replaced by stubs that answer from a fixture.
"""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TESTNET_SH = Path(os.environ.get("HYPERLANE_TESTNET_SH", ROOT / "deploy" / "testnet.sh"))

ZERO = "00" * 32
ROUTE = "e1" * 32
MAILBOX = "a1" * 32
IGP = "a2" * 32
VALIDATOR_ANNOUNCE = "a3" * 32
MERKLE_TREE_HOOK = "a4" * 32
MULTISIG = "b1" * 32
OTHER_MULTISIG = "b2" * 32
TEST_MOCK = "c1" * 32
EVM_MAILBOX = "0xfFAEF09B3cd11D9b20d1a19bECca54EEC2884766"
EVM_TOKEN = "0x1111111111111111111111111111111111111111"
DUSK_DOMAIN = 1146442572
EVM_DOMAIN = 11155111
VALIDATOR = "0x3c659e0fe8d01b80d7828b421630085777346e7c"


def validators(count):
    return [f"0x{'ab' * 19}{index:02x}" for index in range(1, count + 1)]


# Answers the dusk-tx subcommands the scripts use from FIXTURE_CHAIN:
# `query` looks up "<contract>/<method>" (plus "/<arg>" for --arg-u32);
# a missing entry, or one with "error", fails as dusk-tx does.
DUSK_TX_STUB = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
with open(os.environ["FIXTURE_TRACE"], "a") as trace:
    trace.write(" ".join(args) + "\n")
def opt(name):
    return args[args.index(name) + 1] if name in args else None
def out(value, code=0):
    print(json.dumps(value))
    sys.exit(code)
chain = json.load(open(os.environ["FIXTURE_CHAIN"]))
if args[0] == "query":
    key = f"{opt('--contract')}/{opt('--method')}"
    if opt("--arg-u32") is not None:
        key += "/" + opt("--arg-u32")
    entry = chain.get("queries", {}).get(key)
    if entry is None or "error" in entry:
        error = (entry or {}).get("error", f"Query {key} failed (500): ContractDoesNotExist")
        out({"success": False, "error": error}, 1)
    out({"success": True, "value": entry["value"]})
if args[0] == "account":
    out({"success": True, "address": "fixture-deployer", "balance": "500000000000",
         "balance_dusk": "500.000000000"})
out({"success": False, "error": f"stub dusk-tx: unexpected {args[0]}"}, 1)
'''

CAST_STUB = r'''#!/bin/sh
case "$1" in
    chain-id) echo 11155111 ;;
    code) echo 0x6080 ;;
    balance) echo 1.000000000000000000 ;;
    wallet) echo 0x000000000000000000000000000000000000dEaD ;;
    call)
        case "$3" in
            "localDomain()(uint32)") echo 11155111 ;;
            "mailbox()(address)") echo "$FIXTURE_EVM_MAILBOX" ;;
            quoteDispatch*) echo "1 [1e0]" ;;
            *) echo "stub cast: unexpected call $3" >&2; exit 1 ;;
        esac ;;
    *) echo "stub cast: unexpected $1" >&2; exit 1 ;;
esac
'''

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


class TestnetDeployedIsmChecks(unittest.TestCase):
    """`preflight` and `agent-configs` against a recorded deployment."""

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
        write_executable(self.bin / "cast", CAST_STUB)
        write_executable(self.bin / "curl", CURL_STUB)
        write_executable(self.bin / "forge", "#!/bin/sh\nexit 0\n")
        wasm = self.root / "wasm"
        wasm.mkdir()
        (wasm / "hyperlane_dusk_mailbox.wasm").write_bytes(b"\0asm")
        keys = self.root / "deployer.keys"
        keys.write_text("{}")
        self.chain = self.root / "chain.json"
        self.trace = self.root / "dusk-tx.trace"
        self.settings = {
            "DEPLOY_NAME": "fixture",
            "DUSK_KEYS": str(keys),
            "WASM_DIR": str(wasm),
            "DUSK_ORIGIN_VALIDATORS": VALIDATOR,
            "DUSK_ORIGIN_THRESHOLD": "1",
            "EVM_ORIGIN_VALIDATORS": VALIDATOR,
            "EVM_ORIGIN_THRESHOLD": "1",
        }
        out = self.root / "deploy" / "out" / "fixture"
        out.mkdir(parents=True)
        self.relayer_config = out / "relayer.json"
        (out / "deployment.json").write_text(json.dumps({
            "evm_chain": "sepolia",
            "evm_domain": EVM_DOMAIN,
            "evm": {"mailbox": EVM_MAILBOX, "merkle_tree_hook": "0x2222222222222222222222222222222222222222",
                    "validator_announce": "0x3333333333333333333333333333333333333333",
                    "igp": "0x4444444444444444444444444444444444444444", "ism": "0x5555555555555555555555555555555555555555",
                    "token": EVM_TOKEN, "owner": "0x000000000000000000000000000000000000dEaD", "index_from": 1},
            "dusk": {"mailbox": MAILBOX, "igp": IGP, "validator_announce": VALIDATOR_ANNOUNCE,
                     "merkle_tree_hook": MERKLE_TREE_HOOK, "warp_native": ROUTE, "ism_multisig": MULTISIG,
                     "test_mock": TEST_MOCK},
            "dusk_domain": DUSK_DOMAIN,
        }))
        # A deployment as deploy/testnet.sh leaves it: every contract at this
        # checkout's state_version, the route with no ISM of its own, and the
        # Mailbox's default a 1-of-1 multisig ISM.
        self.queries = {
            f"{MAILBOX}/local_domain": {"value": DUSK_DOMAIN},
            f"{MAILBOX}/default_ism": {"value": MULTISIG},
            f"{MAILBOX}/state_version": {"value": 5},
            f"{MERKLE_TREE_HOOK}/state_version": {"value": 2},
            f"{IGP}/state_version": {"value": 3},
            f"{VALIDATOR_ANNOUNCE}/state_version": {"value": 2},
            f"{ROUTE}/state_version": {"value": 4},
            f"{ROUTE}/interchain_security_module": {"value": ZERO},
            f"{MULTISIG}/module_type": {"value": 5},
            f"{MULTISIG}/validators_and_threshold": {"value": {"validators": [VALIDATOR], "threshold": 1}},
            f"{MULTISIG}/threshold": {"value": 1},
            f"{TEST_MOCK}/module_type": {"value": 6},
        }

    def run_script(self, command, **settings):
        env_file = self.root / "deploy" / "testnet.env"
        values = {**self.settings, **settings}
        env_file.write_text('source "$DUSK_DIR/deploy/testnet.env.example"\n'
                            + "".join(f"{key}={shlex.quote(str(value))}\n" for key, value in values.items()))
        self.chain.write_text(json.dumps({"queries": self.queries}))
        if self.relayer_config.exists():
            self.relayer_config.unlink()
        env = {
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "HOME": str(self.root),
            "DUSK_TX": str(self.bin / "dusk-tx"),
            "DUSK_CONSENSUS_PASSWORD": "fixture",
            "EVM_DEPLOYER_KEY": "0x" + "11" * 32,
            "FIXTURE_CHAIN": str(self.chain),
            "FIXTURE_TRACE": str(self.trace),
            "FIXTURE_EVM_MAILBOX": EVM_MAILBOX,
        }
        return subprocess.run(["bash", str(self.root / "deploy" / "testnet.sh"), command],
                              env=env, capture_output=True, text=True, timeout=60)

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
        self.assert_both_commands(False, "DUSK_RELAYER_GAS_LIMIT is 5000000; with EVM_ORIGIN_THRESHOLD of 20")
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


if __name__ == "__main__":
    unittest.main()
