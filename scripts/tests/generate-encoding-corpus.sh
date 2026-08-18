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

# Regenerates the encoding-differential corpus (sub-01 phase 6).
#
# The corpus is one ordered chain of transactions scripted against the `undeployed` genesis:
# every entry is built from the genesis state plus every entry before it, so applying the chain
# in order from a fresh genesis is exactly the situation each transaction was proven for. The
# harness in `pallets/midnight/src/encoding_corpus.rs` replays that chain twice — once with
# every entry in its `transaction[v13]` encoding, once with every entry re-encoded as
# `transaction[v12]` — and compares verdicts and state roots entry by entry.
#
# Nothing here needs a running node: the toolkit reconstructs wallet and ledger state from the
# `--src-file` chain and proves locally. It does need the release toolkit binary:
#
#   cargo build --release -p midnight-node-toolkit
#   ./scripts/tests/generate-encoding-corpus.sh
#
# The generated files are checked in. Regenerating them is not byte-reproducible — the toolkit
# stamps each transaction with the wall-clock block context it was built at, and proving draws
# randomness the `--rng-seed` only partly pins — so treat a regeneration as a new corpus and
# refresh `manifest.json`'s hashes with it (the script does this for you). What *is* pinned is
# the shape of every entry, which the harness asserts.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TOOLKIT_BIN="${TOOLKIT_BIN:-$ROOT/target/release/midnight-node-toolkit}"
OUT="${1:-$ROOT/res/test-encoding-corpus}"
GENESIS="$ROOT/res/genesis/genesis_block_undeployed.mn"

[ -x "$TOOLKIT_BIN" ] || { echo "missing $TOOLKIT_BIN — run: cargo build --release -p midnight-node-toolkit"; exit 1; }
[ -f "$GENESIS" ] || { echo "missing $GENESIS"; exit 1; }

# The built-in contract the `contract-simple` builders deploy and call is loaded from disk by
# the ledger's test resolver, which reads this variable.
export MIDNIGHT_LEDGER_TEST_STATIC_DIR="${MIDNIGHT_LEDGER_TEST_STATIC_DIR:-$ROOT/static/contracts}"

mkdir -p "$OUT"

# Wallet seeds. 1 funds everything from genesis; 2..5 are downstream recipients, so the chain
# has wallets that only hold what an earlier corpus entry gave them.
SEED1=0000000000000000000000000000000000000000000000000000000000000001
SEED2=0000000000000000000000000000000000000000000000000000000000000002
SEED3=0000000000000000000000000000000000000000000000000000000000000003
SEED4=0000000000000000000000000000000000000000000000000000000000000004
# "encoding corpus" in hex — the memo payload for the one v13-only entry.
MEMO_HEX=656e636f64696e6720636f72707573

addr() { "$TOOLKIT_BIN" show-address --network undeployed --seed "$1" "--$2" | tr -d '\n'; }

SH2=$(addr "$SEED2" shielded)
SH3=$(addr "$SEED3" shielded)
SH4=$(addr "$SEED4" shielded)
UN4=$(addr "$SEED4" unshielded)

# The chain of source files, grown one entry at a time. Every entry is built against the genesis
# state plus every entry already in `SRC`, which is what makes the corpus replayable in order.
SRC=(--src-file "$GENESIS")
ENTRIES=()
COMMANDS=()

emit() {
    local name="$1" kind="$2" dual="$3" role="$4" file="$5"; shift 5
    ENTRIES+=("$name|$kind|$dual|$role|$(shasum -a 256 "$file" | cut -d' ' -f1)")
    COMMANDS+=("$name|generate-txs $* (chained on $((${#SRC[@]} / 2 - 1)) prior source file(s))")
}

# gen <name> <kind> <dual-encoded> -- <builder args...>
# Generates an entry and extends the chain with it.
gen() {
    local name="$1" kind="$2" dual="$3"; shift 4
    local file="$OUT/$name.mn"
    echo "▸ $name"
    "$TOOLKIT_BIN" generate-txs "${SRC[@]}" --dust-warp --dest-file "$file" "$@" >/dev/null
    [ -f "$file" ] || { echo "❌ $name produced no file"; exit 1; }
    emit "$name" "$kind" "$dual" chain "$file" "$@"
    SRC+=(--src-file "$file")
}

# companion <name> <kind> <dual-encoded> -- <builder args...>
# Generates an entry against the chain as it stands but does NOT extend it, so several companions
# are all valid against the same parent state rather than against each other.
companion() {
    local name="$1" kind="$2" dual="$3"; shift 4
    local file="$OUT/$name.mn"
    echo "▸ $name (companion)"
    "$TOOLKIT_BIN" generate-txs "${SRC[@]}" --dust-warp --dest-file "$file" "$@" >/dev/null
    [ -f "$file" ] || { echo "❌ $name produced no file"; exit 1; }
    emit "$name" "$kind" "$dual" companion "$file" "$@"
}

gen shielded-fanout "shielded fan-out: one spend split into several shielded outputs" true -- \
    single-tx --source-seed "$SEED1" \
    --output "addr=$SH2,amount=100" --output "addr=$SH2,amount=200" --output "addr=$SH2,amount=300" \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000041

gen multi-input-spend "multi-input shielded spend: every coin the fan-out created, in one offer" true -- \
    single-tx --source-seed "$SEED2" --coin-selection smallest-first \
    --output "addr=$SH3,amount=550" \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000042

gen guaranteed-offer-batch "guaranteed shielded offer plus an unshielded intent" true -- \
    batches --funding-seed "$SEED1" -n 4 -b 0 --enable-shielded -c 250 \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000043

gen mixed-shielded-unshielded "one transaction paying a shielded and an unshielded destination" true -- \
    single-tx --source-seed "$SEED1" \
    --output "addr=$SH4,amount=75" --output "addr=$UN4,amount=1000" \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000044

gen contract-deploy "contract deploy" true -- \
    contract-simple deploy --funding-seed "$SEED1" \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000045

CONTRACT_ADDR=$("$TOOLKIT_BIN" contract-address --src-file "$OUT/contract-deploy.mn" | tr -d '\n')
echo "  contract address: $CONTRACT_ADDR"

gen contract-call "contract call (store)" true -- \
    contract-simple call --funding-seed "$SEED1" --call-key store --contract-address "$CONTRACT_ADDR" \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000046

gen contract-maintenance "contract maintenance update (replace authority)" true -- \
    contract-simple maintenance --funding-seed "$SEED1" --contract-address "$CONTRACT_ADDR" \
    --new-authority-seed "$SEED2" \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000047

gen claim-rewards "claim-rewards: the non-Standard transaction variant" true -- \
    claim-rewards --funding-seed "$SEED1" --amount 500000 --claim-kind reward \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000048

# The one entry that is deliberately NOT dual-encoded: a memo has no v12 representation, so
# `encode_as_prior_version` must refuse it rather than strip it. The harness asserts the refusal.
# A companion rather than a link in the chain, so the cross-boundary block fixture below is not
# built on top of a transaction the pre-memo encoding cannot express.
companion memo-bearing "memo-bearing shielded spend — v13 only, has no v12 encoding by construction" false -- \
    single-tx --source-seed "$SEED3" \
    --output "addr=$SH4,amount=200" --memo "$MEMO_HEX" \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000049

# ------------------------------------------------------------------------------------------
# The cross-boundary block fixture: three transactions, not one.
#
# Every entry above was proven against a state in which its predecessor had already been applied
# *in an earlier block*, so no two of them can share a block. A block that mixes both encodings
# needs two transactions built against the same parent state, which is exactly what a `batches`
# run produces: batch 0 funds two independent wallets, and batch 1's two transactions spend from
# those wallets against the same tip. The harness applies batch 0 in its own block and then puts
# batch 1's pair — one re-encoded as v12, one left as v13 — into a single block, at each of the
# three positions around the activation height.
# ------------------------------------------------------------------------------------------
echo "▸ cross-boundary-block (companion)"
XBLOCK="$OUT/cross-boundary-block.mn"
"$TOOLKIT_BIN" generate-txs "${SRC[@]}" --dust-warp --dest-file "$XBLOCK" \
    batches --funding-seed "$SEED1" -n 2 -b 1 \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000050 >/dev/null
[ -f "$XBLOCK" ] || { echo "❌ cross-boundary-block produced no file"; exit 1; }
XBLOCK_SHA=$(shasum -a 256 "$XBLOCK" | cut -d' ' -f1)

# ------------------------------------------------------------------------------------------
# Manifest: the ordered corpus with provenance and hashes (spec FR-021). The harness reads it,
# reverifies every hash, and names each entry in its report.
# ------------------------------------------------------------------------------------------
{
    echo '{'
    echo '  "corpus": "v12/v13 encoding-differential corpus (sub-01 phase 6)",'
    echo '  "genesis": "res/genesis/genesis_block_undeployed.mn",'
    echo '  "network": "undeployed",'
    echo '  "generator": "scripts/tests/generate-encoding-corpus.sh",'
    echo "  \"generated_at_node_commit\": \"$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)\","
    echo '  "note": "Entries are ordered: each was built against the genesis state plus every entry before it, so the harness must apply them in this order from a fresh genesis.",'
    echo '  "entries": ['
    local_first=1
    for i in "${!ENTRIES[@]}"; do
        IFS='|' read -r name kind dual role sha <<< "${ENTRIES[$i]}"
        IFS='|' read -r _ cmd <<< "${COMMANDS[$i]}"
        [ $local_first -eq 1 ] && local_first=0 || echo '    },'
        echo '    {'
        echo "      \"name\": \"$name\","
        echo "      \"file\": \"$name.mn\","
        echo "      \"kind\": \"$kind\","
        echo "      \"dual_encoded\": $dual,"
        echo "      \"role\": \"$role\","
        echo "      \"sha256\": \"$sha\","
        echo "      \"provenance\": \"$cmd\""
    done
    echo '    }'
    echo '  ],'
    echo '  "cross_boundary_block": {'
    echo '    "file": "cross-boundary-block.mn",'
    echo "    \"sha256\": \"$XBLOCK_SHA\","
    echo '    "kind": "three transactions: batch 0 funds two independent wallets, batch 1 holds the two transactions meant to share one block",'
    echo '    "provenance": "generate-txs batches --funding-seed <seed1> -n 2 -b 1 --rng-seed ...0050 (companion of the chain above, not part of it)"'
    echo '  }'
    echo '}'
} > "$OUT/manifest.json"

echo "✅ wrote ${#ENTRIES[@]} corpus entries and manifest.json to $OUT"
