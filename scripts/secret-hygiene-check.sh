#!/usr/bin/env bash
# Check source and optional runtime artifacts for secret-handling regressions.
#
# Default mode validates tracked source hygiene. Pass artifact/log paths as
# arguments to additionally scan files that may be uploaded from CI.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

command -v rg >/dev/null 2>&1 || fail "rg is required"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || fail "must run inside a git worktree"

info "Checking tracked source hygiene"

secret_files="$(mktemp -t hyperlane-secret-hygiene-files.XXXXXX)"
password_hits="$(mktemp -t hyperlane-secret-hygiene-password.XXXXXX)"
trap 'rm -f "$secret_files" "$password_hits"' EXIT

if git ls-files | rg '(^|/)(consensus\.keys|.*\.keys|.*\.pem|.*\.key|\.env(\..*)?)$' >"$secret_files"; then
    cat "$secret_files" >&2
    fail "tracked secret-like files found"
fi

if rg -n -- '--password' demo/*.sh >"$password_hits"; then
    cat "$password_hits" >&2
    fail "demo scripts must not pass Dusk consensus passwords through CLI argv"
fi

while IFS= read -r script; do
    if [ "$script" = "demo/gen-agent-configs.sh" ]; then
        continue
    fi
    if ! rg -q 'GENERATED_DUSK_SIGNER_KEY_FILES' "$script"; then
        echo "$script" >&2
        fail "E2E scripts that generate agent configs must clean up Dusk signer key files"
    fi
done < <(git ls-files 'demo/*.sh' | xargs rg -l 'gen-agent-configs\.sh' || true)

if [ "$#" -gt 0 ]; then
    info "Scanning runtime artifact paths"
    for path in "$@"; do
        [ -e "$path" ] || fail "artifact path not found: $path"
        if find "$path" -name '*.key' -o -name '*.keys' -o -name '*.pem' | rg .; then
            fail "runtime artifact path includes secret-like file names"
        fi
    done

    if rg -n \
        -e '"key"[[:space:]]*:[[:space:]]*"0x[0-9a-fA-F]{64}"' \
        -e '"type"[[:space:]]*:[[:space:]]*"hexKey"' \
        -e 'secret_key_bls' \
        -e 'DUSK_CONSENSUS_PASSWORD=' \
        -e 'DUSK_CONSENSUS_KEYS_PASS=' \
        -e 'CONSENSUS_PASSWORD=' \
        -e '--password' \
        -e '--private-key[[:space:]]+0x[0-9a-fA-F]{64}' \
        "$@"; then
        fail "runtime artifact scan found signer material or secret-bearing command text"
    fi
fi

info "Secret hygiene checks passed"
