#runtime
# Gate memo-capable transactions on a consensus activation height (spec 2.2.0)

> ⚠️ **BREAKING CHANGE — coordinated upgrade required. Partial deployment is not supported.**

`pallet-midnight` now decodes transactions through an explicit `transaction[v12]`/`[v13]`
envelope and admits the memo-capable `[v13]` encoding only at or after a consensus-authoritative
activation height. `[v12]` remains valid indefinitely.

- New `MemoActivationHeight` storage value (`StorageValue<u64>`, `ValueQuery`, default `0`),
  seeded from the chain spec's `MidnightConfig`. Default `0` keeps `[v13]` live from genesis on
  fresh dev/undeployed chains. A real network must configure a future height with at least the
  finality lag of margin.
- New `Error::TransactionVersionNotActive` at codec index **15**, raised from
  `LedgerApiError::TransactionVersionNotActive` (error code **158**). The same rule runs in
  `validate_unsigned` and `pre_dispatch`, so the pool never admits a transaction the candidate
  block would reject, and nothing is staged.
- Eligibility is branch-relative — the candidate height is `frame_system::block_number()` — so
  reorganizations across the boundary re-evaluate in both directions with no extra machinery.
- The consensus context reaches native verification through **new `#[version(2)]` variants** of
  the four ledger-9 host functions; version 1 is unchanged and reports "memo never active", so
  already-deployed runtimes keep replaying their history correctly.
- No migration is required: the new storage key defaults to `0` on chains that never wrote it,
  no ledger-state tag changes, no existing host-function signature changes, and no cost
  parameter changes.

`spec_version` bumped `002_001_000 → 002_002_000`; runtime metadata rebuild required.
Setting a *future* activation height on an already-running chain is a runtime-upgrade operation
that is deliberately not built here — a chain upgraded in place reads the default `0` and
activates at the upgrade block.
