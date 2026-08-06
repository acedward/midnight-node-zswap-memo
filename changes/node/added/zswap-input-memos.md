#node #ledger #zswap
# Add authenticated memos to ZSwap inputs

Shielded offers can now carry a message that is authenticated by the same secret that
authorizes the spend, without revealing a public key. This is the primitive MIP-0006 wanted
for P2P atomic swaps: until now an offer file was raw transaction bytes with nowhere to put a
message, and anything appended alongside it could be stripped or replaced by any relay.

The mechanism reuses a slot that already exists. Every Midnight proof carries a
`binding_input` — its first public input — which the circuit leaves unconstrained. ZSwap
*outputs* have always used it to bind their coin ciphertext; ZSwap *inputs* hardcoded it to
zero. Putting a commitment to a memo there instead makes the memo tamper-evident (any change,
removal, or graft invalidates the spend proof) and authenticated (only a party who can produce
the spend proof, i.e. the holder of the coin's spending secret, can attach one).

**No circuit, proving-key, verifying-key, or trusted-setup change is required** — the shipped
`spend.verifier` accepts the new statement unchanged, which is pinned by a test that proves and
verifies a memo-carrying spend against the real key.

Ledger side (`midnight-zswap`, `midnight-ledger-v9`):
- New `Memo(Vec<u8>)` type, tag `zswap-memo[v1]`, 1..=512 bytes, and an optional
  `Input.memo` field.
- `memo_to_field` commits to the memo under its own domain separator
  (`midnight:zswap-memo[v1]`), length-prefixed so the packing is injective. Construction and
  verification both derive the proof statement's first element through one shared function, so
  the two cannot drift — a divergence there would be a chain split, not a local bug.
- New public `State::spend_with_memo`; the existing `spend` is unchanged.
- Rejected: memos on contract-owned inputs (a contract spend proves no user secret, so the
  authorization claim would not hold) and empty memos (so "no memo" has one representation).
- An offer may carry a memo per input, and the ledger does not nominate one of them as "the
  offer's" message. Each memo is bound to its own input's proof and nullifier, so authorship is
  already unambiguous, and merging is the settlement mechanism — batch settlement merges many
  parties' offers into one transaction and each party may have something to say. Requiring
  exactly one memo belongs to layers where a single author is actually implied, such as a
  published offer file.
- Memo bytes are priced through the existing serialized-size accounting; a `TODO` marks the
  verifier-side hashing cost as uncalibrated.

This is a backwards-incompatible wire and validity change, so it requires a coordinated
upgrade: tags bump `zswap-input[v2]→[v3]`, `zswap-offer[v5]→[v6]`,
`standard-transaction[v12]→[v13]`, `transaction[v12]→[v13]`. Transactions without memos verify
identically under both rule sets, so history stays valid. Verification runs natively in the
node via the ledger host functions, not in the runtime wasm, so no runtime API or metadata
change is involved.

During development the `midnight-zswap` and `midnight-ledger-v9` entries in
`[patch.crates-io]` point at a local ledger checkout; every other ledger crate resolves exactly
as before.
