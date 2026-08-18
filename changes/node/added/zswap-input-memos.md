#node #ledger #zswap
# Add authenticated memos to ZSwap inputs (with a v12→v13 activation boundary)

> ⚠️ **BREAKING CHANGE — coordinated upgrade required. Partial deployment is not supported.**
> This changes the transaction wire format and validity rules: tags bump
> `zswap-input[v2]→[v3]`, `zswap-offer[v5]→[v6]`, `standard-transaction[v12]→[v13]`,
> `transaction[v12]→[v13]`, and the memo type/inspection API is part of the public ledger
> surface. Runtime `spec_version` moves `002_001_000 → 002_002_000` and the runtime metadata
> must be rebuilt. Every node on a network must run this build before the configured
> activation height; a node without it cannot decode `[v13]` transactions and will diverge.

**Scope: a fresh-chain dev/undeployed-network prototype.** Historical `[v12]` transactions are
now decoded, replayed and reserialized byte-identically, and the memo-capable `[v13]` encoding is
gated on a consensus-authoritative activation height — but this is still not a production upgrade
path. Explicit non-goals, each a production blocker rather than an oversight:

- **No live-chain activation write.** `MemoActivationHeight` is seeded by the genesis build, so a
  fresh chain carries its configured value while a chain upgraded in place reads the default `0`
  and activates at the upgrade block. Setting a *future* activation height on an already-running
  chain is a runtime-upgrade operation that this prototype does not build; it is deferred, intact
  and unstubbed, to a later chain-upgradability effort.
- **No real-network differential oracle.** Encoding equivalence is proven against a designed
  synthetic corpus generated from the `undeployed` genesis, not against a pinned pre-upgrade
  binary or a deployed chain's history, and restart/snapshot recovery equivalence is not covered.
- **No deployed-network support.** Only `dev`/`undeployed` fixtures were regenerated.
- **No wallet, SDK, or `ledger-wasm` surface.** The memo is reachable from Rust and the toolkit
  only, so wallets cannot yet read or set one.
- **No calibrated cost for memo verification.** Memo bytes are charged through serialized size;
  the verifier-side hashing (`1 + ceil(len/31)` field elements per memo) is not in the cost
  model. Sufficient for a bounded prototype, not for production.

Tracked as production blockers: memo-hashing cost calibration, the live-chain activation-height
write, and the wallet/SDK surface.

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
- Prior-version decoding: four mirror types (`zswap::prior::{InputV12, OfferV12}`,
  `ledger::prior_versions::{StandardTransactionV12, TransactionV12}`) read the pre-memo layout,
  and `versioned_deserialize` dispatches on the wire tag into a version-preserving
  `VersionedTransaction::{V12, V13}` envelope. An accepted `[v12]` transaction keeps its era:
  it reserializes to byte-identical `[v12]` bytes, and converting a memo-bearing transaction
  back to `[v12]` refuses (`MemoHasNoPriorEncoding`) rather than stripping the memo.

Node side (activation and coexistence, `00001-sub-01-v12-v13-transition`):
- All four ledger host-api choke points (`apply_transaction`, `validate_transaction`,
  `validate_guaranteed_execution`, `get_transaction_cost`) decode through the envelope, so a
  `[v12]` transaction is accepted, applied under the historical zero memo-statement rule, and
  reserialized unchanged. `[v12]` stays valid **indefinitely** after activation — wallets and
  tooling are not required to migrate.
- Activation is a block height in consensus-authoritative chain state:
  `pallet_midnight::MemoActivationHeight`, a `StorageValue<u64>` seeded from the chain spec's
  `MidnightConfig`, **default `0`** (so fresh dev/undeployed chains behave exactly as before,
  with `[v13]` live from genesis). A real network must configure a future height with at least
  the finality lag of margin, so no reorganization can cross an unfinalized activation.
- Validity is branch-relative: the candidate height comes from `frame_system::block_number()`,
  which is the block being built or imported during execution and `parent + 1` on the pool path,
  so competing branches and reorganizations in both directions re-evaluate activation with no
  extra machinery.
- A `[v13]` transaction submitted before activation is refused at decode, before any state is
  read, with a structured `LedgerApiError::TransactionVersionNotActive` (error code **158**;
  `pallet_midnight::Error::TransactionVersionNotActive`, codec index **15**). The transaction
  pool applies the identical rule in `validate_unsigned` and `pre_dispatch` and does **not**
  stage it, so a block producer can never be handed a transaction its candidate block would
  reject.
- The consensus context reaches native verification through **new `#[version(2)]` variants** of
  the four ledger-9 host functions; version 1 keeps its exact signature and reports "memo never
  active", so an already-deployed runtime still replays its `[v12]` history correctly and still
  refuses the memo-capable encoding.
- **No migration is required at any layer**: no storage layout change (`MemoActivationHeight` is
  a new key whose `ValueQuery` default is `0`), no ledger-state tag change, no host-function
  signature change for existing callers, and no cost-parameter change.
- Runtime `spec_version` `002_001_000 → 002_002_000`; metadata rebuild required. The toolkit
  block fetcher recognizes the new spec version.

Memo-less transactions have the same substantive validity rules in both encodings; that
equivalence is enforced by a nine-entry differential corpus (five entries carrying real shielded
inputs, one deliberately rejected, plus a v13-only memo companion) which applies every entry in
both encodings and compares verdict, ledger state root, ZSwap state root, event count and the
`[v12]` byte round-trip, together with seven malformed/mixed/truncated cases and a
cross-boundary block matrix. Verification runs natively in the node via the ledger host
functions, not in the runtime wasm.

The node manifest pins the memo ledger by immutable `rev` through `[patch.crates-io]`
(`midnight-ledger-v9` and `midnight-zswap`); both entries must move together, and the rest of the
dependency graph resolves from the released per-crate tags.

The Rust construction helpers also fail closed when a requested source wallet is not registered:
direct shielded input construction and shielded coin selection return typed
`SourceWalletNotFound` errors instead of unwinding and poisoning the builder context mutex.
