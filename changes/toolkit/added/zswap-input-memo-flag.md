#toolkit #zswap
# Add `--memo` to `generate-txs single-tx`

`generate-txs single-tx` now accepts `--memo <hex>`, attaching an authenticated message to the
shielded spend. The memo is committed to in the spend proof, so it is authorized by the same
secret that authorizes the spend and cannot be altered or removed in transit.

The memo rides on the first selected input: one message per spender, rather than the same text
repeated on every coin the transfer happens to spend. Sizes are validated by the argument parser
(1..=512 bytes), so an oversized memo is a CLI error rather than a failure deep in the builder.
Requires ledger 9 or later; the ledger-7 and ledger-8 builders reject a memo rather than
silently dropping it.

Every way of asking for a memo that cannot be honoured now fails with its own message rather
than dropping it silently: pre-ledger-9 generations, a transaction with no shielded spend to
carry it, a memo outside 1..=512 bytes, invalid hex, and the defensive case where coin selection
yields no input. `scripts/tests/memo-e2e.sh` covers all of these against a live dev node, and
decodes the generated transaction to assert the memo is attached to an input before submitting
— finalization alone would not prove it, since a silently dropped memo still yields a valid
transaction.

Plumbing: `InputInfo` gains a `memo` field (and consequently is no longer `Copy`), and each
ledger generation supplies a `shielded_spend` shim so the shared builder code compiles against
all three, returning a typed `ShieldedSpendError` instead of asserting.

Scope: part of the fresh-chain dev/undeployed-network prototype described in the node change
file. Requires ledger 9 or later, and offers no wallet or SDK surface.
