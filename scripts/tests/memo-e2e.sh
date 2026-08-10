#!/usr/bin/env bash

# This file is part of midnight-node.
# Copyright (C) Midnight Foundation
# SPDX-License-Identifier: Apache-2.0
# Licensed under the Apache License, Version 2.0 (the "License");
# You may not use this file except in compliance with the License.
# You may obtain a copy of the License at
# http://www.apache.org/licenses/LICENSE-2.0
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# End-to-end check for authenticated zswap input memos.
#
# Builds a shielded transfer carrying a memo, submits it to a local dev node, and confirms the
# node accepts and applies it. The node verifies the spend proof natively, and that proof is
# bound to the memo, so acceptance here means the memo travelled intact through serialization,
# deserialization, proof verification and application.
#
# Unlike the docker-based e2e scripts this one drives the release binaries directly, so it
# needs no docker daemon:
#
#   cargo build --release -p midnight-node -p midnight-node-toolkit
#   ./scripts/tests/memo-e2e.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
NODE_BIN="${NODE_BIN:-$ROOT/target/release/midnight-node}"
TOOLKIT_BIN="${TOOLKIT_BIN:-$ROOT/target/release/midnight-node-toolkit}"
RPC="${RPC:-127.0.0.1:9944}"
SOURCE_SEED="0000000000000000000000000000000000000000000000000000000000000001"
DEST_SEED="0000000000000000000000000000000000000000000000000000000000000002"
# "midnight offer memo" in hex.
MEMO_HEX="6d69646e69676874206f66666572206d656d6f"

for bin in "$NODE_BIN" "$TOOLKIT_BIN"; do
    [ -x "$bin" ] || { echo "missing $bin — run: cargo build --release -p midnight-node -p midnight-node-toolkit"; exit 1; }
done

workdir=$(mktemp -d 2>/dev/null || mktemp -d -t 'memoe2e')
node_pid=""
cleanup() {
    [ -n "$node_pid" ] && kill "$node_pid" 2>/dev/null || true
    rm -rf "$workdir"
}
trap cleanup EXIT

echo "🚀 starting dev node"
CFG_PRESET=dev "$NODE_BIN" --dev --tmp --rpc-port "${RPC##*:}" >"$workdir/node.log" 2>&1 &
node_pid=$!

# The toolkit reconstructs wallet state from finalized blocks, so genesis alone is not enough.
echo "⏳ waiting for the node to finalize a block beyond genesis"
finalized=""
for _ in $(seq 1 180); do
    kill -0 "$node_pid" 2>/dev/null || { echo "node exited early:"; tail -30 "$workdir/node.log"; exit 1; }
    # `|| true`: before the first such line grep exits non-zero, which under `set -e` would
    # abort the script rather than let the loop poll again.
    finalized=$(grep -oE "finalized #[0-9]+" "$workdir/node.log" 2>/dev/null | tail -1 | tr -dc '0-9' || true)
    if [ -n "$finalized" ] && [ "$finalized" -ge 2 ]; then break; fi
    sleep 2
done
[ -n "$finalized" ] && [ "$finalized" -ge 2 ] || {
    echo "node never finalized past genesis:"; tail -30 "$workdir/node.log"; exit 1
}
echo "   finalized #$finalized"

dest_addr=$("$TOOLKIT_BIN" show-address --network undeployed --seed "$DEST_SEED" --shielded | tr -d '\n')
unshielded_addr=$("$TOOLKIT_BIN" show-address --network undeployed --seed "$DEST_SEED" --unshielded | tr -d '\n')
echo "📮 destination: $dest_addr"

echo "✍️  building a shielded transfer carrying a memo"
"$TOOLKIT_BIN" generate-txs \
    -s "ws://$RPC" \
    --dest-file "$workdir/memo-tx.mn" \
    single-tx \
    --source-seed "$SOURCE_SEED" \
    --output "addr=$dest_addr,amount=100" \
    --memo "$MEMO_HEX" \
    2>&1 | tee "$workdir/build.log"

# Acceptance alone would not prove the memo travelled: a memo dropped *before* proving yields a
# perfectly valid memo-less transaction that the node would happily apply. So decode the
# transaction with the updated ledger and look for the memo on an input, rather than grepping the
# bytes -- a grep only shows the memo exists somewhere, not that it is attached where it belongs.
echo "🔎 decoding the transaction and checking the memo is on an input"
"$TOOLKIT_BIN" show-transaction --src-file "$workdir/memo-tx.mn" >"$workdir/decoded.txt" 2>&1 || {
    echo "❌ could not decode the generated transaction:"; tail -20 "$workdir/decoded.txt"; exit 1
}
LC_ALL=C grep -qiE "shielded input .*memo\($((${#MEMO_HEX} / 2)) bytes\): $MEMO_HEX" "$workdir/decoded.txt" || {
    echo "❌ the decoded transaction has no input carrying the memo:"
    grep -io "shielded input[^\"]*" "$workdir/decoded.txt" | head -5
    exit 1
}

echo "✉️  submitting it"
"$TOOLKIT_BIN" generate-txs \
    --src-file "$workdir/memo-tx.mn" \
    -d "ws://$RPC" \
    send \
    2>&1 | tee "$workdir/memo-tx.log"

# A transaction the node rejected never reaches a block, so finalization is the real signal:
# the node deserialized the memo-carrying transaction, verified its spend proof natively
# against the memo commitment, and applied it.
echo "⏳ confirming the memo transaction was finalized"
grep -q "FINALIZED" "$workdir/memo-tx.log" || {
    echo "❌ the memo transaction was not finalized:"; tail -20 "$workdir/memo-tx.log"; exit 1
}
if grep -qiE "rejected|invalid|panicked" "$workdir/memo-tx.log"; then
    echo "❌ toolkit reported a failure:"; tail -20 "$workdir/memo-tx.log"; exit 1
fi

# Every way of asking for a memo that cannot be honoured must fail with its own message, rather
# than panicking or -- worse -- succeeding with the memo quietly missing.
echo "🚫 checking invalid memo requests are refused with the intended error"
check_rejected() {
    local label="$1" expect="$2"; shift 2
    if "$TOOLKIT_BIN" generate-txs -s "ws://$RPC" -d "ws://$RPC" single-tx \
            --source-seed "$SOURCE_SEED" "$@" >"$workdir/neg.log" 2>&1; then
        echo "   ❌ $label was accepted"; return 1
    fi
    if grep -qi "panicked" "$workdir/neg.log"; then
        echo "   ❌ $label panicked instead of erroring:"; tail -5 "$workdir/neg.log"; return 1
    fi
    grep -qiE "$expect" "$workdir/neg.log" || {
        echo "   ❌ $label: expected /$expect/, got:"; tail -5 "$workdir/neg.log"; return 1
    }
    echo "   ✓ $label"
}

check_rejected "oversized memo"   "memo must be"        --output "addr=$dest_addr,amount=1" --memo "$(python3 -c 'print("ab"*513)')"
check_rejected "empty memo"       "memo must be"        --output "addr=$dest_addr,amount=1" --memo ""
check_rejected "odd-length hex"   "invalid hex|memo"    --output "addr=$dest_addr,amount=1" --memo "abc"
check_rejected "non-hex memo"     "invalid hex|memo"    --output "addr=$dest_addr,amount=1" --memo "zzzz"
# A memo rides on a shielded spend; with only an unshielded output there is nothing to carry it,
# and this used to be discarded in silence.
check_rejected "unshielded-only"  "no shielded output"  --unshielded-amount 500 --destination-address "$unshielded_addr" --memo "$MEMO_HEX"

echo "✅ memo decoded on its input, transaction finalized, invalid requests rejected"
