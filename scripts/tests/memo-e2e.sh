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
echo "📮 destination: $dest_addr"

echo "✉️  submitting a shielded transfer carrying a memo"
"$TOOLKIT_BIN" generate-txs \
    -s "ws://$RPC" \
    -d "ws://$RPC" \
    single-tx \
    --source-seed "$SOURCE_SEED" \
    --output "addr=$dest_addr,amount=100" \
    --memo "$MEMO_HEX" \
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

echo "🚫 checking an oversized memo is refused up front"
if "$TOOLKIT_BIN" generate-txs -s "ws://$RPC" -d "ws://$RPC" single-tx \
        --source-seed "$SOURCE_SEED" \
        --output "addr=$dest_addr,amount=1" \
        --memo "$(python3 -c 'print("ab"*513)')" >"$workdir/oversize.log" 2>&1; then
    echo "❌ an over-limit memo was accepted"; exit 1
fi
grep -q "memo must be" "$workdir/oversize.log" || {
    echo "❌ expected a memo size error, got:"; cat "$workdir/oversize.log"; exit 1
}

echo "✅ memo transaction accepted and applied; oversized memo rejected"
