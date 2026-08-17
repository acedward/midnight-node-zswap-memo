#node #ledger #zswap
# Add authenticated memos to ZSwap inputs (fresh-chain dev prototype)

**Scope: a fresh-chain dev/undeployed-network prototype.** It is not an upgrade path. Explicit
non-goals, each a production blocker rather than an oversight:

- **No `transaction[v12]` decoding or replay.** The updated ledger reads `[v13]` only, and there
  is no prior-version decoder. Start from freshly generated dev/undeployed genesis; do not point
  this build at an existing Ledger9 chain, and do not expect it to cold-replay one.
- **No activation boundary.** Every participating node and toolkit client must run the updated
  ledger. There is no version-gated switchover.
- **No deployed-network support.** Only `dev`/`undeployed` fixtures were regenerated.
- **No wallet, SDK, or `ledger-wasm` surface.** The memo is reachable from Rust and the toolkit
  only, so wallets cannot yet read or set one.
- **No calibrated cost for memo verification.** Memo bytes are charged through serialized size;
  the verifier-side hashing (`1 + ceil(len/31)` field elements per memo) is not in the cost
  model. Sufficient for a bounded prototype, not for production.

Tracked as production blockers: v12→v13 transition support, and memo-hashing cost calibration.

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
- New checked `Memo` type, tag `zswap-memo[v1]`, with private 1..=512-byte storage and checked
  construction/serialization. Optional memo and contract-placement fields on `Input` are also
  private and exposed through checked accessors/mutation, so safe callers cannot construct a
  memo on a contract-owned input.
- `memo_to_field` commits to the memo under its own domain separator
  (`midnight:zswap-memo[v1]`), length-prefixed so the packing is injective. Construction and
  verification both derive the proof statement's first element through one shared function, so
  the two cannot drift — a divergence there would be a chain split, not a local bug.
- New public `State::spend_with_memo`; the existing `spend` is unchanged.
- Raw `Transaction` and `VerifiedTransaction` inspection labels memo bytes `Unverified`.
  `MemoTrust::Authenticated` is emitted only by the state-bound validation-and-application API,
  and only for a carrying segment whose paired application result succeeds; this status does not
  imply chain inclusion or finality.
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

This is a backwards-incompatible wire and validity change. Delivering it to any existing network
would require a coordinated upgrade, which this prototype does not provide: tags bump `zswap-input[v2]→[v3]`, `zswap-offer[v5]→[v6]`,
`standard-transaction[v12]→[v13]`, `transaction[v12]→[v13]`. Memo-less transactions have the
same substantive validity rules, but their old v12 wire encoding is not accepted by this
prototype; historical replay therefore remains a production blocker. Verification runs
natively in the node via the ledger host functions, not in the runtime wasm, so no runtime API
or metadata change is involved.

The production node manifest still pins the pre-correction ledger revision. Joint acceptance
uses a disclosed test-only local Git overlay; shipping requires pushing an immutable ledger
revision, permanently re-pinning `midnight-zswap` and `midnight-ledger-v9`, inspecting any Cargo
lock-graph churn, and rerunning the affected matrix against that remote pin.

The Rust construction helpers also fail closed when a requested source wallet is not registered:
direct shielded input construction and shielded coin selection return typed
`SourceWalletNotFound` errors instead of unwinding and poisoning the builder context mutex.
