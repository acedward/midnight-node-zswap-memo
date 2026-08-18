# v12/v13 encoding-differential corpus

The corpus behind sub-01 phase 6: one ordered chain of scripted transactions against the
`undeployed` genesis, each of which exists in both wire encodings, plus two companions.

`manifest.json` is the authority on what is here — for every file it records the kind, the exact
`midnight-node-toolkit generate-txs` invocation that produced it, and its SHA-256. The harness
reverifies those hashes before it trusts a byte of this, so a fixture that drifts from the one its
successors were proven against fails at the file that changed rather than somewhere downstream.

## Why this corpus exists

The whole v12↔v13 wire difference is one `Option` discriminant per **zswap input**, which is where
the memo lives. Every transaction fixture this repository had before phase 6 carries zero zswap
inputs, so for all of them the two encodings differ only in the header string and are byte-for-byte
the same length — a differential over them cannot fail. The entries here carry one-, three- and
five-output shielded offers in both the guaranteed and the fallible position, so the discriminant
is actually on the wire: `multi-input-spend`'s v12 encoding is exactly three bytes shorter than its
v13 one, one byte per input.

## Layout

- **chain** entries (`role: "chain"`) are ordered. Each was built against the genesis state plus
  every entry before it, so they must be replayed in manifest order from a fresh genesis. They are
  deliberately not all accepted: `claim-rewards` is rejected against this genesis, so the verdicts
  being compared are not all the same answer.
- **companion** entries (`role: "companion"`) were built against the chain but are not part of it.
  `memo-bearing` is the one entry with no v12 encoding at all — a memo has no pre-memo
  representation, and the boundary refuses it rather than stripping it.
- `cross_boundary_block` is a separate three-transaction fixture. No two chain entries can share a
  block (each was proven against a state where its predecessor had already been applied in an
  earlier block, and dust is generated per unit of time), so the block-level activation matrix
  needs transactions built against one another at a single instant. Batch 0 funds two independent
  wallets; batch 1's pair spends from them.

## Regenerating

```bash
cargo build --release -p midnight-node-toolkit
./scripts/tests/generate-encoding-corpus.sh
```

Nothing needs a running node — the toolkit rebuilds wallet and ledger state from the `--src-file`
chain and proves locally. Regeneration is **not** byte-reproducible: each transaction is stamped
with the wall-clock block context it was built at. Treat a regeneration as a new corpus; the script
rewrites `manifest.json` with the new hashes, and adding or removing a file also means editing
`encoding_corpus::ENTRIES` in `res/src/lib.rs`, which embeds the bytes.

## Consumers

- `pallets/midnight/src/encoding_corpus.rs` — the differential harness and its JSON report.
- `ledger/tests/v12_transition.rs` — envelope-level cross-version cases, which borrow
  `multi-input-spend` because they need a transaction whose two bodies actually differ.
