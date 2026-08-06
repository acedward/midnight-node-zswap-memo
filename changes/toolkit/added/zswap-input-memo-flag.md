#toolkit #zswap
# Add `--memo` to `generate-txs single-tx`

`generate-txs single-tx` now accepts `--memo <hex>`, attaching an authenticated message to the
shielded spend. The memo is committed to in the spend proof, so it is authorized by the same
secret that authorizes the spend and cannot be altered or removed in transit.

The memo rides on the first selected input, since an offer may carry at most one. Sizes are
validated by the argument parser (1..=512 bytes) so an oversized memo is a CLI error rather
than a failure deep in the builder. Requires ledger 9 or later; the ledger-7 and ledger-8
builders reject a memo rather than silently dropping it.

Plumbing: `InputInfo` gains a `memo` field (and consequently is no longer `Copy`), and each
ledger generation supplies a `shielded_spend` shim so the shared builder code compiles against
all three.
