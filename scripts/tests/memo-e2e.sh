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
# The node also binds a P2P and a Prometheus port. They are overridable so a run on a shared host
# can put every listening service on a port it has just verified to be free.
P2P_PORT="${P2P_PORT:-30333}"
PROMETHEUS_PORT="${PROMETHEUS_PORT:-9615}"
SOURCE_SEED="0000000000000000000000000000000000000000000000000000000000000001"
DEST_SEED="0000000000000000000000000000000000000000000000000000000000000002"
# "midnight offer memo" in hex.
MEMO_HEX="6d69646e69676874206f66666572206d656d6f"

for bin in "$NODE_BIN" "$TOOLKIT_BIN"; do
    [ -x "$bin" ] || { echo "missing $bin — run: cargo build --release -p midnight-node -p midnight-node-toolkit"; exit 1; }
done

workdir=$(mktemp -d 2>/dev/null || mktemp -d -t 'memoe2e')
node_pid=""

# Provenance header. Evidence review of the previous run could not tie every log back to the
# invocation that produced it, so the run states its own exact command and environment first.
{
    echo "# memo-e2e.sh"
    echo "# argv: $0 $*"
    echo "# started_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# pwd: $(pwd)"
    echo "# uname: $(uname -a)"
    echo "# RPC: $RPC"
    echo "# P2P_PORT: $P2P_PORT"
    echo "# PROMETHEUS_PORT: $PROMETHEUS_PORT"
    echo "# NODE_BIN: $NODE_BIN"
    echo "# TOOLKIT_BIN: $TOOLKIT_BIN"
    for bin in "$NODE_BIN" "$TOOLKIT_BIN"; do
        if command -v sha256sum >/dev/null 2>&1; then sha256sum "$bin"; else shasum -a 256 "$bin"; fi
    done
    echo "# script sha256:"
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$0"; else shasum -a 256 "$0"; fi
    env | LC_ALL=C sort | sed 's/^/# env: /'
} 2>&1
cleanup() {
    [ -n "$node_pid" ] && kill "$node_pid" 2>/dev/null || true
    rm -rf "$workdir"
}
trap cleanup EXIT

echo "🚀 starting dev node"
CFG_PRESET=dev "$NODE_BIN" --dev --tmp \
    --rpc-port "${RPC##*:}" --port "$P2P_PORT" --prometheus-port "$PROMETHEUS_PORT" \
    >"$workdir/node.log" 2>&1 &
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
memo_carriers=$(LC_ALL=C grep -ciE "shielded input .*memo\($((${#MEMO_HEX} / 2)) bytes\): $MEMO_HEX" "$workdir/decoded.txt" || true)
[ "$memo_carriers" -eq 1 ] || {
    echo "❌ expected exactly one input carrying the memo, found $memo_carriers:"
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
# than panicking or -- worse -- succeeding with the memo quietly missing. Each case is proved four
# ways: the typed error is returned, nothing panics, nothing is submitted to the node, no
# transaction file is produced, and the chain state the request would have moved is unchanged.
echo "🚫 checking invalid memo requests are refused with the intended error"

neg_out="$workdir/neg-out"
mkdir -p "$neg_out"

# Fingerprint of the source wallet's shielded coin set, replayed from the node's finalized blocks.
# Only the coins are hashed: block rewards and their UTXOs move on their own every block, while a
# shielded coin set changes only when a shielded spend is applied. A rejected request that somehow
# reached the chain would therefore show up here, and nothing else would.
shielded_state_fingerprint() {
    local raw
    raw=$("$TOOLKIT_BIN" show-wallet -s "ws://$RPC" --seed "$SOURCE_SEED" 2>/dev/null) || {
        echo "❌ could not read the source wallet state" >&2; return 1
    }
    printf '%s' "$raw" | python3 -c '
import hashlib, json, sys
raw = sys.stdin.read()
start = raw.find("{")
if start < 0:
    sys.exit("no JSON object in show-wallet output")
obj, _ = json.JSONDecoder().raw_decode(raw[start:])
coins = obj.get("coins", {})
print(hashlib.sha256(json.dumps(coins, sort_keys=True).encode()).hexdigest())
' || { echo "❌ could not parse the source wallet state" >&2; return 1; }
}

baseline_state=$(shielded_state_fingerprint) || exit 1
echo "   shielded coin-set fingerprint before the negative cases: $baseline_state"

neg_index=0
check_rejected() {
    local label="$1" expect="$2"; shift 2
    neg_index=$((neg_index + 1))
    local slug
    slug=$(printf '%02d-%s' "$neg_index" "$(printf '%s' "$label" | tr ' ' '-')")
    local submit_log="$workdir/neg-$slug-submit.log"
    local file_log="$workdir/neg-$slug-file.log"
    local out_file="$neg_out/$slug.mn"

    # (1) Submission route. `-d` makes the toolkit send whatever it builds, so a case that fails
    # here provably never handed anything to the node.
    {
        echo "# case: $label"
        echo "# started_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo "# command: $TOOLKIT_BIN generate-txs -s ws://$RPC -d ws://$RPC single-tx --source-seed $SOURCE_SEED $*"
    } >"$submit_log"
    if "$TOOLKIT_BIN" generate-txs -s "ws://$RPC" -d "ws://$RPC" single-tx \
            --source-seed "$SOURCE_SEED" "$@" >>"$submit_log" 2>&1; then
        echo "   ❌ $label was accepted"; return 1
    fi
    if grep -qi "panicked" "$submit_log"; then
        echo "   ❌ $label panicked instead of erroring:"; tail -5 "$submit_log"; return 1
    fi
    grep -qiE "$expect" "$submit_log" || {
        echo "   ❌ $label: expected /$expect/, got:"; tail -5 "$submit_log"; return 1
    }
    # The sender logs these only once a transaction has actually been handed to a node.
    if grep -qE "extrinsic_hash|midnight_tx_hash|BEST_BLOCK|FINALIZED|Broadcasted" "$submit_log"; then
        echo "   ❌ $label reached submission:"; tail -10 "$submit_log"; return 1
    fi

    # (2) File route. `--dest-file` conflicts with `--dest-url`, so proving "no output file" needs
    # its own invocation: the same request asked to write a transaction instead of sending one.
    {
        echo "# case: $label (output-file route)"
        echo "# started_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo "# command: $TOOLKIT_BIN generate-txs -s ws://$RPC --dest-file $out_file single-tx --source-seed $SOURCE_SEED $*"
    } >"$file_log"
    if "$TOOLKIT_BIN" generate-txs -s "ws://$RPC" --dest-file "$out_file" single-tx \
            --source-seed "$SOURCE_SEED" "$@" >>"$file_log" 2>&1; then
        echo "   ❌ $label produced a transaction when asked to write one"; return 1
    fi
    if grep -qi "panicked" "$file_log"; then
        echo "   ❌ $label panicked on the output-file route:"; tail -5 "$file_log"; return 1
    fi
    [ ! -e "$out_file" ] || { echo "   ❌ $label wrote $out_file"; return 1; }
    if [ -n "$(ls -A "$neg_out")" ]; then
        echo "   ❌ $label left output behind:"; ls -l "$neg_out"; return 1
    fi

    # (3) No state advance: the shielded coin set the request would have spent is untouched.
    local now_state
    now_state=$(shielded_state_fingerprint) || return 1
    [ "$now_state" = "$baseline_state" ] || {
        echo "   ❌ $label advanced shielded state: $baseline_state -> $now_state"; return 1
    }

    echo "   ✓ $label (typed error, no panic, no submission, no output file, no state advance)"
}

check_rejected "oversized memo"   "memo must be"        --output "addr=$dest_addr,amount=1" --memo "$(python3 -c 'print("ab"*513)')"
check_rejected "empty memo"       "memo must be"        --output "addr=$dest_addr,amount=1" --memo ""
check_rejected "odd-length hex"   "invalid hex|memo"    --output "addr=$dest_addr,amount=1" --memo "abc"
check_rejected "non-hex memo"     "invalid hex|memo"    --output "addr=$dest_addr,amount=1" --memo "zzzz"
# A memo rides on a shielded spend; with only an unshielded output there is nothing to carry it,
# and this used to be discarded in silence.
check_rejected "unshielded-only"  "no shielded output"  --unshielded-amount 500 --destination-address "$unshielded_addr" --memo "$MEMO_HEX"
# A syntactically valid memo request with no destination used to reach `SingleTxBuilder::new` and
# panic before the memo/carrier preflight could return its typed error.
check_rejected "no destination"   "no shielded output"  --memo "$MEMO_HEX"
# The dev genesis wallet is funded, but not enough to cover this deliberately excessive request.
# This exercises the live no-selected-input path rather than only its unit-level selector.
check_rejected "no selected input" "insufficient shielded coins" --output "addr=$dest_addr,amount=100000000000000000" --memo "$MEMO_HEX"

final_state=$(shielded_state_fingerprint) || exit 1
[ "$final_state" = "$baseline_state" ] || {
    echo "❌ the negative cases moved shielded state: $baseline_state -> $final_state"; exit 1
}

echo "# finished_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "✅ memo decoded on exactly one input, transaction finalized, 7 invalid requests rejected with"
echo "   no panic, no submission, no output file and no shielded-state advance"
