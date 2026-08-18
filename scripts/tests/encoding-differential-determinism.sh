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

# Runs the encoding-differential corpus twice from clean state and requires the two reports to be
# byte-identical (sub-01 phase 6).
#
#   ./scripts/tests/encoding-differential-determinism.sh
#
# WHAT THIS CHECKS, precisely: that the harness returns the same verdicts, the same per-block
# ledger and zswap state roots, and the same fixture hashes on two independent runs. That is a
# DETERMINISM property.
#
# WHAT IT DOES NOT CHECK: anything about a previously deployed system. There is no oracle in this
# project — no pinned pre-upgrade reference binary and no real-network history (owner decision,
# spec FR-018: this is a prototype targeting fresh chains). Two identical reports do not mean the
# answers are right; they mean the harness is reproducible. The correctness claim the reports
# carry is the differential one — that the two encodings of one transaction are indistinguishable
# in consensus state — and it is the assertions inside the harness, not this script, that make it.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="${TMPDIR:-/tmp}/encoding-differential-$$"
mkdir -p "$WORK"
# `KEEP_WORK=1` leaves the two reports and logs behind, which is what you want when they disagree.
trap '[ -n "${KEEP_WORK:-}" ] || rm -rf "$WORK"' EXIT

export MIDNIGHT_LEDGER_TEST_STATIC_DIR="${MIDNIGHT_LEDGER_TEST_STATIC_DIR:-$ROOT/static/contracts}"

for run in 1 2; do
    echo "▸ run $run"
    ENCODING_DIFFERENTIAL_REPORT="$WORK/report-$run.json" \
        cargo test --locked --manifest-path "$ROOT/Cargo.toml" -p pallet-midnight --lib \
        encoding_corpus -- --nocapture >"$WORK/run-$run.log" 2>&1 || {
            echo "❌ run $run failed:"; tail -40 "$WORK/run-$run.log"; exit 1
        }
    [ -s "$WORK/report-$run.json" ] || { echo "❌ run $run wrote no report"; exit 1; }
done

if ! cmp -s "$WORK/report-1.json" "$WORK/report-2.json"; then
    echo "❌ the two runs disagree:"
    diff "$WORK/report-1.json" "$WORK/report-2.json" | head -40
    exit 1
fi

entries=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['summary']['entries'])" "$WORK/report-1.json")
failures=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['summary']['failures'])" "$WORK/report-1.json")
[ "$failures" -eq 0 ] || { echo "❌ the report records $failures failures"; exit 1; }

cp "$WORK/report-1.json" "$ROOT/target/encoding-differential-report.json"
echo "✅ two clean runs produced byte-identical reports over $entries entries, 0 failures"
echo "   determinism only — this is not verification against an external oracle"
echo "   report: $ROOT/target/encoding-differential-report.json"
