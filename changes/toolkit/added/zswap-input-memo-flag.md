#toolkit #zswap
# Add `--memo` to `generate-txs single-tx`

> ⚠️ **BREAKING CHANGE — coordinated upgrade required. Partial deployment is not supported.**
> A memo-bearing transaction is `transaction[v13]`, which only a node running the memo build can
> decode, and which that node accepts only at or after the chain's configured
> `MemoActivationHeight` (default `0`, so dev/undeployed chains are unaffected). Submitting one
> earlier is refused by both the pool and block validation with
> `TransactionVersionNotActive` (ledger error code 158, pallet error index 15). Runtime
> `spec_version` moves `002_001_000 → 002_002_000`; see the node change file for the full notice.

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
yields no input. Direct construction and coin selection also return a typed error when the source
wallet is not registered in the builder context, rather than panicking while holding the context
mutex. Unit tests cover the generation-dispatch and selector details;
`scripts/tests/memo-e2e.sh` covers the user-facing invalid encodings, unshielded-only and
no-destination shapes, and a funded-wallet no-selection request against a live dev node. Each of
those refusals is proved four ways — the typed error, no panic, nothing submitted to the node, no
transaction file written, and an unchanged shielded coin set — so a "rejected" case cannot quietly
have produced or sent something. It also
decodes the generated transaction to assert the memo is attached to exactly one input before
submitting—finalization alone would not prove that, since a silently dropped memo still yields a
valid transaction.

Plumbing: `InputInfo` gains a `memo` field (and consequently is no longer `Copy`), `BuildInput`
is fallible, and each ledger generation supplies a `shielded_spend` shim so the shared builder
code compiles against all three. `BuilderContext` gains a backwards-compatible fallible
single-wallet lookup, while `ShieldedSpendError` and `ShieldedCoinSelectionError` gain a
`SourceWalletNotFound` variant. Memo bytes and carrier-placement fields are checked/private in
the ledger API, and inspection only labels a memo authenticated after full validation and the
carrying segment's successful application. These are source- and validity-visible breaking changes
and require the coordinated release warning recorded in the project plan.

The block fetcher recognizes runtime `spec_version` `002_002_000` as 2.2.0 and decodes those
blocks with the 2.1.0 metadata, whose extrinsic envelope this release leaves unchanged. Memo-less
transactions may still be presented in the pre-memo `transaction[v12]` encoding indefinitely; the
toolkit emits the current encoding.

Scope: part of the fresh-chain dev/undeployed-network prototype described in the node change
file. Requires ledger 9 or later, and offers no wallet or SDK surface.
