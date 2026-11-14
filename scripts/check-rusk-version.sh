#!/usr/bin/env bash
# Fail unless the rusk-private checkout the workspace builds against is at the
# commit pinned in RUSK_VERSION. Set ALLOW_RUSK_MISMATCH=1 to build anyway
# (for example while porting to a newer Rusk release).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=/dev/null
source <(grep -E '^RUSK_(TAG|COMMIT)=' "$ROOT/RUSK_VERSION")
RUSK_DIR="$ROOT/../../rusk-private"

if [ ! -d "$RUSK_DIR/core" ]; then
    echo "error: no rusk-private checkout at $RUSK_DIR (see README, Building)" >&2
    exit 1
fi

actual="$(git -C "$RUSK_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
if [ "$actual" = "$RUSK_COMMIT" ]; then
    exit 0
fi

msg="rusk-private at $(cd "$RUSK_DIR" && pwd -P) is at $actual, but RUSK_VERSION pins $RUSK_TAG ($RUSK_COMMIT)"
if [ "${ALLOW_RUSK_MISMATCH:-0}" = "1" ]; then
    echo "warning: $msg" >&2
    exit 0
fi
cat >&2 <<MSG
error: $msg.
Contracts built against another Rusk may not deploy or behave the same on the
network. Check out the pinned commit, e.g. in a separate worktree:

  git -C <rusk-private> worktree add --detach <workspace>/rusk-private $RUSK_COMMIT

or set ALLOW_RUSK_MISMATCH=1 to build anyway.
MSG
exit 1
